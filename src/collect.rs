use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::{interval, MissedTickBehavior};

use crate::adb::Shell;
use crate::agent::proto;
use crate::model::*;
use crate::parse::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Groups {
    pub cpu: bool,
    pub threads: bool,
    pub memory: bool,
    pub frames: bool,
    pub network: bool,
    pub power: bool,
}

impl Groups {
    pub const ALL: Groups = Groups {
        cpu: true,
        threads: true,
        memory: true,
        frames: true,
        network: true,
        power: true,
    };
}

pub struct Config {
    pub serial: Option<String>,
    pub package: watch::Receiver<Option<String>>,
    pub interval: Duration,
    pub memory_interval: Duration,
    pub groups: Groups,
    pub capture_stream: bool,
    pub pcap: Option<PathBuf>,
    pub pcap_append: bool,
    pub watch_devices: bool,
}

#[derive(Debug)]
pub enum Update {
    Probe(Capabilities),
    Fast(FastSample),
    AppMemory(Option<AppMemory>),
    Frames(Option<FrameUpdate>),
    Battery(Battery),
    Thermal(Thermal),
    Doze(Doze),
    Jobs(Jobs),
    Devices(Vec<Device>),
    Processes {
        packages: Vec<String>,
        names: Vec<ProcessName>,
    },
    Capture(Box<proto::Snapshot>),
    CaptureStopped,
    CaptureError(String),
    Support(Source, Support),
    Error(Source, String),
}

pub struct Collectors {
    pub rx: mpsc::Receiver<Update>,
    tasks: Vec<JoinHandle<()>>,
}

impl Drop for Collectors {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

fn ticker(every: Duration) -> tokio::time::Interval {
    let mut t = interval(every);
    t.set_missed_tick_behavior(MissedTickBehavior::Delay);
    t
}

pub fn spawn(config: Config) -> Collectors {
    let (tx, rx) = mpsc::channel(256);
    let (caps_tx, caps_rx) = watch::channel::<Option<Capabilities>>(None);
    let serial = config.serial.clone();
    let mut tasks = vec![tokio::spawn(fast_task(
        tx.clone(),
        serial.clone(),
        config.package.clone(),
        config.interval,
        config.groups,
        caps_tx,
    ))];
    if config.groups.memory {
        tasks.push(tokio::spawn(memory_task(
            tx.clone(),
            serial.clone(),
            config.package.clone(),
            config.memory_interval,
            caps_rx.clone(),
        )));
    }
    if config.groups.frames {
        tasks.push(tokio::spawn(frames_task(
            tx.clone(),
            serial.clone(),
            config.package.clone(),
            config.interval,
            caps_rx.clone(),
        )));
    }
    tasks.push(tokio::spawn(slow_task(
        tx.clone(),
        serial.clone(),
        config.package.clone(),
        config.groups,
        caps_rx,
    )));
    if config.watch_devices {
        tasks.push(tokio::spawn(devices_task(tx.clone())));
    }
    if config.capture_stream {
        tasks.push(tokio::spawn(crate::agent::snapshot_runner(
            tx.clone(),
            serial.clone(),
        )));
    }
    if let Some(path) = config.pcap {
        tasks.push(tokio::spawn(crate::agent::pcap_runner(
            tx.clone(),
            serial.clone(),
            path,
            config.pcap_append,
        )));
    }
    Collectors { rx, tasks }
}

fn supports(caps: &Capabilities) -> Vec<(Source, Support)> {
    let yes_no = |ok: bool, reason: &str| {
        if ok {
            Support::Supported
        } else {
            Support::Unsupported(reason.to_string())
        }
    };
    vec![
        (Source::Cpu, Support::Supported),
        (Source::Threads, Support::Supported),
        (Source::Processes, Support::Supported),
        (Source::SystemMemory, Support::Supported),
        (Source::Network, Support::Supported),
        (
            Source::CpuFreq,
            yes_no(
                caps.cpufreq,
                "cpufreq files are not readable on this device",
            ),
        ),
        (
            Source::MemoryPressure,
            yes_no(caps.psi, "the kernel has no /proc/pressure (PSI)"),
        ),
        (
            Source::AppMemory,
            yes_no(caps.meminfo, "dumpsys meminfo is not available"),
        ),
        (
            Source::Frames,
            if !caps.gfxinfo {
                Support::Unsupported("the gfxinfo service is not available".into())
            } else if caps.sdk < 23 {
                Support::Unsupported(format!(
                    "frame stats need Android 6 or newer (this is API {})",
                    caps.sdk
                ))
            } else {
                Support::Supported
            },
        ),
        (
            Source::Battery,
            yes_no(caps.battery, "the battery service is not available"),
        ),
        (
            Source::Thermal,
            yes_no(caps.thermal, "the thermal service is not available"),
        ),
        (
            Source::Doze,
            yes_no(caps.deviceidle, "the deviceidle service is not available"),
        ),
        (
            Source::Jobs,
            yes_no(
                caps.jobscheduler,
                "the jobscheduler service is not available",
            ),
        ),
    ]
}

async fn fast_task(
    tx: mpsc::Sender<Update>,
    serial: Option<String>,
    mut package: watch::Receiver<Option<String>>,
    every: Duration,
    groups: Groups,
    caps_tx: watch::Sender<Option<Capabilities>>,
) {
    let shell = Shell::spawn(serial);
    let caps = loop {
        match shell.exec(probe_command()).await {
            Ok(out) => break parse_probe(&out),
            Err(e) => {
                if tx
                    .send(Update::Error(Source::Cpu, e.to_string()))
                    .await
                    .is_err()
                {
                    return;
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    };
    for (source, support) in supports(&caps) {
        if tx.send(Update::Support(source, support)).await.is_err() {
            return;
        }
    }
    if tx.send(Update::Probe(caps.clone())).await.is_err() {
        return;
    }
    let _ = caps_tx.send(Some(caps.clone()));
    let mut parser = FastParser::default();
    let mut pid: Option<u32> = None;
    let mut current = package.borrow_and_update().clone();
    let mut tick = ticker(every);
    loop {
        tick.tick().await;
        if package.has_changed().unwrap_or(false) {
            current = package.borrow_and_update().clone();
            parser.reset_app();
            pid = None;
        }
        let app = current.as_deref().map(|p| (p, pid));
        let cmd = fast_command(
            app,
            caps.cpufreq,
            caps.psi,
            current.is_none() && groups.threads,
        );
        match shell.exec(cmd).await {
            Ok(out) => {
                let result = parser.parse(&out, current.is_some(), pid);
                pid = result.pid;
                if let Some(sample) = result.sample {
                    if tx.send(Update::Fast(sample)).await.is_err() {
                        return;
                    }
                }
            }
            Err(e) => {
                if tx
                    .send(Update::Error(Source::Cpu, e.to_string()))
                    .await
                    .is_err()
                {
                    return;
                }
            }
        }
    }
}

async fn wait_caps(caps: &mut watch::Receiver<Option<Capabilities>>) -> Option<Capabilities> {
    caps.wait_for(|c| c.is_some())
        .await
        .ok()
        .and_then(|c| c.clone())
}

async fn memory_task(
    tx: mpsc::Sender<Update>,
    serial: Option<String>,
    mut package: watch::Receiver<Option<String>>,
    every: Duration,
    mut caps: watch::Receiver<Option<Capabilities>>,
) {
    let Some(caps) = wait_caps(&mut caps).await else {
        return;
    };
    if !caps.meminfo {
        return;
    }
    let shell = Shell::spawn(serial);
    let mut tick = ticker(every);
    loop {
        tick.tick().await;
        let Some(pkg) = package.borrow_and_update().clone() else {
            continue;
        };
        match shell.exec(format!("dumpsys meminfo {pkg}")).await {
            Ok(out) => {
                if tx
                    .send(Update::AppMemory(parse_app_meminfo(&out)))
                    .await
                    .is_err()
                {
                    return;
                }
            }
            Err(e) => {
                if tx
                    .send(Update::Error(Source::AppMemory, e.to_string()))
                    .await
                    .is_err()
                {
                    return;
                }
            }
        }
    }
}

async fn frames_task(
    tx: mpsc::Sender<Update>,
    serial: Option<String>,
    mut package: watch::Receiver<Option<String>>,
    every: Duration,
    mut caps: watch::Receiver<Option<Capabilities>>,
) {
    let Some(caps) = wait_caps(&mut caps).await else {
        return;
    };
    if !caps.gfxinfo || caps.sdk < 23 {
        return;
    }
    let shell = Shell::spawn(serial);
    let mut parser = FrameParser::default();
    let mut tick = ticker(every);
    let mut current: Option<String> = None;
    loop {
        tick.tick().await;
        let pkg = package.borrow_and_update().clone();
        if pkg != current {
            parser.reset();
            current = pkg.clone();
        }
        let Some(pkg) = pkg else { continue };
        match shell
            .exec(format!("dumpsys gfxinfo {pkg} framestats"))
            .await
        {
            Ok(out) => {
                if tx.send(Update::Frames(parser.parse(&out))).await.is_err() {
                    return;
                }
            }
            Err(e) => {
                if tx
                    .send(Update::Error(Source::Frames, e.to_string()))
                    .await
                    .is_err()
                {
                    return;
                }
            }
        }
    }
}

async fn slow_task(
    tx: mpsc::Sender<Update>,
    serial: Option<String>,
    mut package: watch::Receiver<Option<String>>,
    groups: Groups,
    mut caps: watch::Receiver<Option<Capabilities>>,
) {
    let Some(caps) = wait_caps(&mut caps).await else {
        return;
    };
    let shell = Shell::spawn(serial);
    let mut tick = ticker(Duration::from_secs(1));
    let mut n: u64 = 0;
    let mut thermal_ok = caps.thermal;
    loop {
        tick.tick().await;
        let pkg = package.borrow_and_update().clone();
        if n % 5 == 0 {
            if let Ok(out) = shell.exec("ps -A -o PID,USER,NAME").await {
                let (packages, names) = parse_ps(&out);
                if tx
                    .send(Update::Processes { packages, names })
                    .await
                    .is_err()
                {
                    return;
                }
            }
        }
        if groups.power {
            if caps.battery && n % 2 == 0 {
                match shell.exec(battery_command()).await {
                    Ok(out) => {
                        if let Some(b) = parse_battery(&out) {
                            if tx.send(Update::Battery(b)).await.is_err() {
                                return;
                            }
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Update::Error(Source::Battery, e.to_string())).await;
                    }
                }
            }
            if thermal_ok && n % 2 == 1 {
                if let Ok(out) = shell.exec("dumpsys thermalservice").await {
                    match parse_thermal(&out) {
                        Some(t) => {
                            if tx.send(Update::Thermal(t)).await.is_err() {
                                return;
                            }
                        }
                        None => {
                            thermal_ok = false;
                            let reason = "dumpsys thermalservice reports no thermal status (needs Android 10+)".to_string();
                            let _ = tx
                                .send(Update::Support(
                                    Source::Thermal,
                                    Support::Unsupported(reason),
                                ))
                                .await;
                        }
                    }
                }
            }
            if caps.deviceidle && n % 5 == 2 {
                let cmd = match &pkg {
                    Some(p) => format!("dumpsys deviceidle | grep -E 'mState|mLightState'; echo ==B; am get-standby-bucket {p}"),
                    None => "dumpsys deviceidle | grep -E 'mState|mLightState'; echo ==B".to_string(),
                };
                if let Ok(out) = shell.exec(cmd).await {
                    if tx.send(Update::Doze(parse_doze(&out))).await.is_err() {
                        return;
                    }
                }
            }
            if caps.jobscheduler && n % 10 == 3 {
                if let Some(p) = &pkg {
                    if let Ok(out) = shell.exec(jobs_command(p)).await {
                        if tx.send(Update::Jobs(parse_jobs(&out, p))).await.is_err() {
                            return;
                        }
                    }
                }
            }
        }
        n += 1;
    }
}

pub async fn devices_task(tx: mpsc::Sender<Update>) {
    let mut tick = ticker(Duration::from_secs(3));
    let mut last: Option<Vec<String>> = None;
    loop {
        tick.tick().await;
        match crate::adb::devices().await {
            Ok(devices) => {
                let key: Vec<String> = devices
                    .iter()
                    .map(|d| format!("{}:{}", d.serial, d.state))
                    .collect();
                if last.as_ref() != Some(&key) {
                    last = Some(key);
                    if tx.send(Update::Devices(devices)).await.is_err() {
                        return;
                    }
                }
            }
            Err(e) => {
                if tx
                    .send(Update::Error(Source::Cpu, e.to_string()))
                    .await
                    .is_err()
                {
                    return;
                }
            }
        }
    }
}
