use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::time::Instant;

use crate::cli::{groups, Command, Common, RecordArgs, SnapshotArgs};
use crate::collect::{self, Groups};
use crate::config::Config;
use crate::model::{AppState, Source, Support};
use crate::report::{self, Report};
use crate::session::Session;

pub const EXIT_OK: i32 = 0;
pub const EXIT_ERROR: i32 = 1;
pub const EXIT_ASSERT: i32 = 3;
pub const EXIT_NOT_FOUND: i32 = 4;
pub const EXIT_CAPTURE: i32 = 5;

#[derive(Serialize)]
struct ErrorBody<'a> {
    code: &'a str,
    message: String,
}

#[derive(Serialize)]
struct ErrorOut<'a> {
    error: ErrorBody<'a>,
}

pub fn fail(code: &str, message: impl Into<String>, exit: i32) -> i32 {
    let out = ErrorOut {
        error: ErrorBody {
            code,
            message: message.into(),
        },
    };
    println!("{}", serde_json::to_string(&out).unwrap_or_default());
    exit
}

pub async fn run(command: Command, common: Common) -> i32 {
    match command {
        Command::Record(args) => record(args, common).await,
        Command::Snapshot(args) => snapshot(args, common).await,
        Command::Mark { label, session } => {
            control(session, format!("mark {}", label.join(" "))).await
        }
        Command::Stop { session } => control(session, "stop".into()).await,
        Command::Devices => devices().await,
        Command::Packages => packages(common).await,
        Command::Themes => themes(),
    }
}

pub async fn resolve_device(requested: Option<&str>) -> Result<String, i32> {
    let devices = crate::adb::devices()
        .await
        .map_err(|e| fail("adb_unavailable", e.to_string(), EXIT_ERROR))?;
    if let Some(serial) = requested {
        return match devices.iter().find(|d| d.serial == serial) {
            Some(d) if d.state == "device" => Ok(serial.to_string()),
            Some(d) => Err(fail(
                "device_not_ready",
                format!(
                    "device {serial} is {}; unlock it and accept the USB debugging prompt",
                    d.state
                ),
                EXIT_NOT_FOUND,
            )),
            None => Err(fail(
                "device_not_found",
                format!("no device with serial {serial}"),
                EXIT_NOT_FOUND,
            )),
        };
    }
    let ready: Vec<_> = devices.iter().filter(|d| d.state == "device").collect();
    match ready.len() {
        0 if devices.is_empty() => Err(fail(
            "no_device",
            "no Android device connected (check `adb devices`)",
            EXIT_NOT_FOUND,
        )),
        0 => Err(fail(
            "device_not_ready",
            format!(
                "{} is {}; unlock it and accept the USB debugging prompt",
                devices[0].serial, devices[0].state
            ),
            EXIT_NOT_FOUND,
        )),
        1 => Ok(ready[0].serial.clone()),
        _ => {
            let list: Vec<&str> = ready.iter().map(|d| d.serial.as_str()).collect();
            Err(fail(
                "multiple_devices",
                format!(
                    "{} devices connected ({}); pass -d SERIAL",
                    ready.len(),
                    list.join(", ")
                ),
                EXIT_NOT_FOUND,
            ))
        }
    }
}

async fn check_package(serial: &str, package: &str) -> Result<(), i32> {
    match crate::adb::shell_once(Some(serial), &format!("pm path {package}")).await {
        Ok(out) if out.contains("package:") => Ok(()),
        Ok(_) => Err(fail(
            "package_not_installed",
            format!("{package} is not installed on {serial}"),
            EXIT_NOT_FOUND,
        )),
        Err(e) => Err(fail("adb_error", e.to_string(), EXIT_ERROR)),
    }
}

#[derive(Serialize, Deserialize)]
struct SessionFile {
    id: String,
    pid: u32,
    port: u16,
    serial: String,
    package: Option<String>,
}

fn sessions_dir() -> PathBuf {
    std::env::temp_dir().join("android-perf-monitor")
}

fn new_session_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!(
        "{:04x}",
        (nanos ^ std::process::id().rotate_left(7)) & 0xffff
    )
}

struct Registration {
    file: PathBuf,
    current: PathBuf,
    id: String,
}

impl Drop for Registration {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.file);
        if std::fs::read_to_string(&self.current)
            .map(|c| c.trim() == self.id)
            .unwrap_or(false)
        {
            let _ = std::fs::remove_file(&self.current);
        }
    }
}

fn register(info: &SessionFile) -> std::io::Result<Registration> {
    let dir = sessions_dir();
    std::fs::create_dir_all(&dir)?;
    let file = dir.join(format!("{}.json", info.id));
    std::fs::write(&file, serde_json::to_vec(info).unwrap_or_default())?;
    let current = dir.join("current");
    std::fs::write(&current, &info.id)?;
    Ok(Registration {
        file,
        current,
        id: info.id.clone(),
    })
}

enum Sink {
    Stdout(std::io::Stdout),
    File(BufWriter<File>),
}

impl Sink {
    fn open(path: &Path) -> std::io::Result<Sink> {
        if path == Path::new("-") {
            Ok(Sink::Stdout(std::io::stdout()))
        } else {
            if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
                std::fs::create_dir_all(dir)?;
            }
            Ok(Sink::File(BufWriter::new(File::create(path)?)))
        }
    }

    fn line(&mut self, text: &str) {
        let _ = match self {
            Sink::Stdout(s) => {
                let mut lock = s.lock();
                writeln!(lock, "{text}").and_then(|_| lock.flush())
            }
            Sink::File(f) => writeln!(f, "{text}").and_then(|_| f.flush()),
        };
    }
}

struct SampleWriter {
    sink: Sink,
    csv: bool,
}

impl SampleWriter {
    fn write(&mut self, session: &Session, marker: Option<&str>) {
        let line = report::sample_line(session, marker);
        if self.csv {
            self.sink.line(&report::csv_line(&line));
        } else if let Ok(text) = serde_json::to_string(&line) {
            self.sink.line(&text);
        }
    }
}

fn is_stdout(p: &Option<PathBuf>) -> bool {
    p.as_deref().map_or(true, |p| p == Path::new("-"))
}

async fn terminated() {
    #[cfg(unix)]
    {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            s.recv().await;
            return;
        }
    }
    std::future::pending::<()>().await
}

struct Prepared {
    serial: String,
    session: Session,
    collectors: collect::Collectors,
    _package_tx: watch::Sender<Option<String>>,
    capture_started: bool,
}

async fn prepare(
    common: &Common,
    groups: Groups,
    history: impl FnOnce(Duration) -> usize,
    quiet: bool,
) -> Result<Prepared, i32> {
    let serial = resolve_device(common.device.as_deref()).await?;
    if let Some(p) = &common.package {
        check_package(&serial, p).await?;
    }
    let (config, _) = Config::load();
    let interval = common
        .interval
        .unwrap_or(Duration::from_millis(config.interval_ms));
    let memory_interval = Duration::from_millis(config.memory_interval_ms).max(interval);
    let capture = common.capture || common.pcap.is_some();
    if capture {
        if let Err(e) = crate::agent::start_capture(Some(&serial), None).await {
            let code = if e.to_string() == crate::agent::NOT_INSTALLED {
                "capture_agent_missing"
            } else {
                "capture_failed"
            };
            return Err(fail(code, e.to_string(), EXIT_CAPTURE));
        }
        if !quiet {
            eprintln!("capture: started the agent on the phone; the first time, accept the VPN request there");
        }
    }
    let (package_tx, package_rx) = watch::channel(common.package.clone());
    let collectors = collect::spawn(collect::Config {
        serial: Some(serial.clone()),
        package: package_rx,
        interval,
        memory_interval,
        groups,
        capture_stream: capture,
        pcap: common.pcap.clone(),
        pcap_append: false,
        watch_devices: false,
    });
    let mut session = Session::new(
        Some(serial.clone()),
        common.package.clone(),
        interval,
        history(interval),
    );
    session.app_state = if common.package.is_some() {
        AppState::NotRunning
    } else {
        AppState::NotSelected
    };
    Ok(Prepared {
        serial,
        session,
        collectors,
        _package_tx: package_tx,
        capture_started: capture,
    })
}

async fn finish_capture(p: &Prepared) {
    if p.capture_started {
        let _ = tokio::time::timeout(
            Duration::from_secs(3),
            crate::agent::stop_capture(Some(&p.serial)),
        )
        .await;
    }
}

fn capture_warning(p: &Prepared, report: &mut Report) {
    if p.capture_started
        && p.session.capture.snapshot.is_none()
        && !p.session.support(Source::Capture).is_unsupported()
    {
        report.warnings.push("capture: no data arrived from the agent; accept the VPN request on the phone and record again".into());
    }
}

fn write_report(report: &Report, out: &Option<PathBuf>, compact: bool) -> Result<(), i32> {
    let text = if compact {
        serde_json::to_string(report)
    } else {
        serde_json::to_string_pretty(report)
    }
    .map_err(|e| fail("internal", e.to_string(), EXIT_ERROR))?;
    match out.as_deref().filter(|p| *p != Path::new("-")) {
        Some(path) => {
            if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
                std::fs::create_dir_all(dir).map_err(|e| {
                    fail(
                        "write_failed",
                        format!("{}: {e}", dir.display()),
                        EXIT_ERROR,
                    )
                })?;
            }
            std::fs::write(path, text + "\n").map_err(|e| {
                fail(
                    "write_failed",
                    format!("{}: {e}", path.display()),
                    EXIT_ERROR,
                )
            })
        }
        None => {
            println!("{text}");
            Ok(())
        }
    }
}

async fn record(args: RecordArgs, common: Common) -> i32 {
    let groups = groups(&args.metrics);
    let shared_stdout = is_stdout(&args.out) && args.samples.as_deref() == Some(Path::new("-"));
    let mut samples = match &args.samples {
        Some(path) => match Sink::open(path) {
            Ok(sink) => Some(SampleWriter {
                sink,
                csv: path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("csv")),
            }),
            Err(e) => {
                return fail(
                    "write_failed",
                    format!("{}: {e}", path.display()),
                    EXIT_ERROR,
                )
            }
        },
        None => None,
    };
    let duration = args.duration;
    let history = move |interval: Duration| match duration {
        Some(d) => (d.as_secs_f64() / interval.as_secs_f64()).ceil() as usize + 16,
        None => (6.0 * 3600.0 / interval.as_secs_f64()) as usize,
    };
    let mut p = match prepare(&common, groups, history, args.quiet).await {
        Ok(p) => p,
        Err(code) => return code,
    };
    let listener = match TcpListener::bind(("127.0.0.1", 0)).await {
        Ok(l) => l,
        Err(e) => return fail("control_failed", e.to_string(), EXIT_ERROR),
    };
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    let id = new_session_id();
    let registration = register(&SessionFile {
        id: id.clone(),
        pid: std::process::id(),
        port,
        serial: p.serial.clone(),
        package: common.package.clone(),
    });
    if let Err(e) = &registration {
        if !args.quiet {
            eprintln!("warning: `mark` and `stop` won't find this session: {e}");
        }
    }
    if samples.as_ref().is_some_and(|s| s.csv) {
        if let Some(s) = samples.as_mut() {
            s.sink.line(report::CSV_HEADER);
        }
    }
    if !args.quiet {
        let target = common.package.as_deref().unwrap_or("the whole device");
        let until = args.duration.map_or(
            "until `android-perf-monitor stop` or Ctrl+C".to_string(),
            |d| format!("for {:.0} s", d.as_secs_f64()),
        );
        eprintln!("recording {target} on {} (session {id}) {until}", p.serial);
    }

    let deadline = args.duration.map(|d| Instant::now() + d);
    let mut pending_marker: Option<String> = None;
    let started = Instant::now();
    let mut warned_capture = false;
    let interrupted = tokio::signal::ctrl_c();
    tokio::pin!(interrupted);
    let term = terminated();
    tokio::pin!(term);
    loop {
        tokio::select! {
            update = p.collectors.rx.recv() => {
                let Some(update) = update else { break };
                if p.session.apply(update) {
                    if let Some(w) = samples.as_mut() {
                        w.write(&p.session, pending_marker.as_deref());
                    }
                    pending_marker = None;
                }
                if !warned_capture && !args.quiet && p.capture_started && p.session.capture.snapshot.is_none() && started.elapsed() > Duration::from_secs(12) {
                    warned_capture = true;
                    eprintln!("capture: no data from the agent yet; accept the VPN request on the phone");
                }
            }
            _ = async { match deadline { Some(d) => tokio::time::sleep_until(d).await, None => std::future::pending().await } } => break,
            conn = listener.accept() => {
                if let Ok((stream, _)) = conn {
                    match handle_control(stream, &mut p.session).await {
                        Some(ControlAction::Mark(label)) => pending_marker = Some(label),
                        Some(ControlAction::Stop) => break,
                        None => {}
                    }
                }
            }
            _ = &mut interrupted => break,
            _ = &mut term => break,
        }
    }
    finish_capture(&p).await;
    let mut report = report::build(
        &p.session,
        "record",
        groups,
        args.after_marker.as_secs_f32(),
    );
    capture_warning(&p, &mut report);
    let passed = report::evaluate_assertions(&mut report, &args.asserts);
    if let Err(code) = write_report(&report, &args.out, args.compact || shared_stdout) {
        return code;
    }
    drop(registration);
    if !args.quiet {
        let failed = report.assertions.iter().filter(|a| !a.pass).count();
        let dest = args
            .out
            .as_deref()
            .filter(|p| *p != Path::new("-"))
            .map_or("stdout".to_string(), |p| p.display().to_string());
        let n = |count: usize, word: &str| {
            format!("{count} {word}{}", if count == 1 { "" } else { "s" })
        };
        eprintln!(
            "report: {dest} · {} · {} · {}{}",
            n(report.session.samples as usize, "sample"),
            n(report.markers.len(), "marker"),
            n(report.warnings.len(), "warning"),
            if args.asserts.is_empty() {
                String::new()
            } else {
                format!(" · {failed} of {} checks failed", args.asserts.len())
            }
        );
    }
    if passed {
        EXIT_OK
    } else {
        EXIT_ASSERT
    }
}

enum ControlAction {
    Mark(String),
    Stop,
}

#[derive(Serialize)]
struct ControlReply<'a> {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    marker: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    t_s: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stopping: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

async fn handle_control(stream: TcpStream, session: &mut Session) -> Option<ControlAction> {
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    let line = tokio::time::timeout(Duration::from_secs(2), lines.next_line())
        .await
        .ok()?
        .ok()??;
    let (reply, action) = if let Some(label) = line.strip_prefix("mark ") {
        session.add_marker(label.trim());
        let m = session.markers.last()?;
        let reply = ControlReply {
            ok: true,
            marker: Some(&m.label),
            t_s: Some((m.secs * 10.0).round() / 10.0),
            stopping: None,
            error: None,
        };
        (
            serde_json::to_string(&reply).ok()?,
            Some(ControlAction::Mark(m.label.clone())),
        )
    } else if line.trim() == "stop" {
        let reply = ControlReply {
            ok: true,
            marker: None,
            t_s: None,
            stopping: Some(true),
            error: None,
        };
        (
            serde_json::to_string(&reply).ok()?,
            Some(ControlAction::Stop),
        )
    } else {
        let reply = ControlReply {
            ok: false,
            marker: None,
            t_s: None,
            stopping: None,
            error: Some(format!("unknown command: {line}")),
        };
        (serde_json::to_string(&reply).ok()?, None)
    };
    let _ = write.write_all(format!("{reply}\n").as_bytes()).await;
    action
}

async fn control(session: Option<String>, command: String) -> i32 {
    let dir = sessions_dir();
    let id = match session {
        Some(id) => id,
        None => match std::fs::read_to_string(dir.join("current")) {
            Ok(id) => id.trim().to_string(),
            Err(_) => {
                return fail(
                    "no_session",
                    "no recording is running (start one with `android-perf-monitor record`)",
                    EXIT_ERROR,
                )
            }
        },
    };
    let info: SessionFile = match std::fs::read(dir.join(format!("{id}.json")))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
    {
        Some(info) => info,
        None => {
            return fail(
                "no_session",
                format!("no recording with session id {id}"),
                EXIT_ERROR,
            )
        }
    };
    let mut stream = match TcpStream::connect(("127.0.0.1", info.port)).await {
        Ok(s) => s,
        Err(_) => {
            return fail(
                "no_session",
                format!("recording {id} is not running anymore"),
                EXIT_ERROR,
            )
        }
    };
    if stream
        .write_all(format!("{command}\n").as_bytes())
        .await
        .is_err()
    {
        return fail(
            "control_failed",
            "could not talk to the recording",
            EXIT_ERROR,
        );
    }
    let mut lines = BufReader::new(stream).lines();
    match tokio::time::timeout(Duration::from_secs(3), lines.next_line()).await {
        Ok(Ok(Some(reply))) => {
            println!("{reply}");
            if reply.contains("\"ok\":true") {
                EXIT_OK
            } else {
                EXIT_ERROR
            }
        }
        _ => fail("control_failed", "the recording did not answer", EXIT_ERROR),
    }
}

fn snapshot_ready(s: &Session, groups: Groups) -> bool {
    if s.ticks < 1 {
        return false;
    }
    let running = s.app_state == AppState::Running;
    let pending = |source: Source, has: bool| {
        !has && !s.support(source).is_unsupported() && *s.support(source) != Support::Unknown
    };
    if s.package.is_some() && running {
        if groups.memory && pending(Source::AppMemory, s.app_mem.is_some()) {
            return false;
        }
        if groups.frames && pending(Source::Frames, s.frame_summary.is_some()) {
            return false;
        }
    }
    if groups.power
        && (pending(Source::Battery, s.battery.is_some())
            || pending(Source::Thermal, s.thermal.is_some()))
    {
        return false;
    }
    true
}

async fn snapshot(args: SnapshotArgs, common: Common) -> i32 {
    let groups = groups(&args.metrics);
    let mut p = match prepare(&common, groups, |_| 64, true).await {
        Ok(p) => p,
        Err(code) => return code,
    };
    let limit = Instant::now() + Duration::from_secs(8).max(p.session.interval * 4);
    let interrupted = tokio::signal::ctrl_c();
    tokio::pin!(interrupted);
    loop {
        tokio::select! {
            update = p.collectors.rx.recv() => {
                let Some(update) = update else { break };
                p.session.apply(update);
                if snapshot_ready(&p.session, groups) {
                    break;
                }
            }
            _ = tokio::time::sleep_until(limit) => break,
            _ = &mut interrupted => break,
        }
    }
    finish_capture(&p).await;
    let mut report = report::build(&p.session, "snapshot", groups, 0.0);
    capture_warning(&p, &mut report);
    match write_report(&report, &args.out, args.compact) {
        Ok(()) => EXIT_OK,
        Err(code) => code,
    }
}

async fn devices() -> i32 {
    match crate::adb::devices().await {
        Ok(devices) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&devices).unwrap_or_default()
            );
            EXIT_OK
        }
        Err(e) => fail("adb_unavailable", e.to_string(), EXIT_ERROR),
    }
}

async fn packages(common: Common) -> i32 {
    let serial = match resolve_device(common.device.as_deref()).await {
        Ok(s) => s,
        Err(code) => return code,
    };
    match crate::adb::shell_once(Some(&serial), "ps -A -o PID,USER,NAME").await {
        Ok(out) => {
            let (packages, _) = crate::parse::parse_ps(&out);
            println!(
                "{}",
                serde_json::to_string_pretty(&packages).unwrap_or_default()
            );
            EXIT_OK
        }
        Err(e) => fail("adb_error", e.to_string(), EXIT_ERROR),
    }
}

fn themes() -> i32 {
    let (config, _) = Config::load();
    let names: Vec<String> = crate::theme::all(&config.themes)
        .into_iter()
        .map(|t| t.name)
        .collect();
    println!(
        "{}",
        serde_json::to_string_pretty(&names).unwrap_or_default()
    );
    EXIT_OK
}
