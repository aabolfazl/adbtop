use std::io::{self, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use futures::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::{mpsc, watch};

use crate::actions::{self, Act, Console, ConsoleMsg, Tweaks};
use crate::cli::Common;
use crate::collect::{self, Collectors, Groups, Update};
use crate::config::{CaptureMode, Config, CpuUnits, GraphStyle, MemChart, NetScale, TempUnit};
use crate::model::{Device, Source};
use crate::session::Session;
use crate::theme::{self, Theme};

pub const PAGES: [&str; 6] = ["Overview", "CPU", "Memory", "Frames", "Network", "Power"];
pub const WINDOWS: [u32; 5] = [60, 120, 300, 900, 1800];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Popup {
    None,
    Device,
    Package,
    Settings,
    Marker,
    Help,
    Actions,
    Prompt,
    Console,
}

pub struct ActionDone {
    act: Act,
    title: String,
    result: Result<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sort {
    Cpu,
    Name,
    Bytes,
    Recent,
}

pub struct Toast {
    pub text: String,
    pub error: bool,
    pub until: Instant,
}

pub struct App {
    pub page: usize,
    pub popup: Popup,
    pub themes: Vec<Theme>,
    pub theme: usize,
    pub config: Config,
    pub session: Session,
    pub paused: bool,
    pub window_s: u32,
    pub serial: Option<String>,
    pub devices: Vec<Device>,
    pub capture_on: bool,
    pub capture_busy: bool,
    pub cursor: usize,
    pub filter: String,
    pub input: String,
    pub scroll: [usize; 6],
    pub sort: Sort,
    pub group_pools: bool,
    pub table_filter: String,
    pub filtering: bool,
    pub toast: Option<Toast>,
    pub quit: bool,
    pub pcap: Option<PathBuf>,
    pub console: Console,
    pub tweaks: Tweaks,
    package_tx: watch::Sender<Option<String>>,
    pcap_started: bool,
    config_autosave: bool,
    config_file_interval: Option<u64>,
}

impl App {
    pub fn t(&self) -> &Theme {
        &self.themes[self.theme]
    }

    pub fn package(&self) -> Option<&str> {
        self.session.package.as_deref()
    }

    pub fn window_samples(&self) -> usize {
        ((self.window_s as f32 / self.session.interval_s().max(0.05)).round() as usize).max(10)
    }

    pub fn toast(&mut self, text: impl Into<String>, error: bool) {
        self.toast = Some(Toast {
            text: text.into(),
            error,
            until: Instant::now() + Duration::from_secs(if error { 8 } else { 4 }),
        });
    }

    pub fn braille(&self) -> bool {
        self.config.graph == GraphStyle::Braille
    }

    pub fn filtered_packages(&self) -> Vec<&String> {
        let f = self.filter.to_lowercase();
        self.session
            .packages
            .iter()
            .filter(|p| f.is_empty() || p.to_lowercase().contains(&f))
            .collect()
    }

    fn set_package(&mut self, package: Option<String>) {
        if package.as_deref() == self.package() {
            return;
        }
        if let Some(original) = self.tweaks.bucket_original.take() {
            self.tweaks.bucket_set = None;
            let restore = Tweaks {
                bucket_original: Some(original),
                ..Tweaks::default()
            };
            tokio::spawn(restore_tweaks(self.serial.clone(), restore));
        }
        self.session.reset_app(package.clone());
        self.scroll = [0; 6];
        let _ = self.package_tx.send(package.clone());
        if package.is_some() {
            self.config.last_package = package;
        }
    }

    fn cycle_theme(&mut self, forward: bool) {
        let n = self.themes.len();
        self.theme = if forward {
            (self.theme + 1) % n
        } else {
            (self.theme + n - 1) % n
        };
        self.config.theme = self.themes[self.theme].name.clone();
    }

    fn cycle_window(&mut self) {
        let max = self.config.history_min * 60;
        let next = WINDOWS
            .iter()
            .copied()
            .filter(|w| *w <= max.max(60))
            .find(|w| *w > self.window_s);
        self.window_s = next.unwrap_or(WINDOWS[0]);
    }

    fn save_config(&self) -> Result<PathBuf, String> {
        let mut config = self.config.clone();
        if let Some(ms) = self.config_file_interval {
            config.interval_ms = ms;
        }
        config.save()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    Theme,
    Graph,
    KeyHints,
    StartPage,
    CpuUnits,
    Temperature,
    Window,
    MemoryChart,
    NetScale,
    HideIdle,
    Interval,
    MemoryInterval,
    History,
    Frames,
    Capture,
    Power,
    DeviceProcesses,
    ReopenLast,
    ReportDir,
    ExportOnQuit,
}

pub struct SettingRow {
    pub key: Setting,
    pub section: &'static str,
    pub label: &'static str,
    pub value: String,
    pub note: &'static str,
}

pub fn settings_rows(app: &App) -> Vec<SettingRow> {
    let c = &app.config;
    let on = |b: bool| {
        if b {
            "on".to_string()
        } else {
            "off".to_string()
        }
    };
    let secs = |ms: u64| {
        if ms % 1000 == 0 {
            format!("{} s", ms / 1000)
        } else {
            format!("{:.1} s", ms as f64 / 1000.0)
        }
    };
    let window = |s: u32| {
        if s >= 60 {
            format!("{} min", s / 60)
        } else {
            format!("{s} s")
        }
    };
    let row = |key, section, label, value: String, note| SettingRow {
        key,
        section,
        label,
        value,
        note,
    };
    vec![
        row(
            Setting::Theme,
            "APPEARANCE",
            "Theme",
            app.themes[app.theme].name.clone(),
            "t / T anywhere",
        ),
        row(
            Setting::Graph,
            "",
            "Graph style",
            if c.graph == GraphStyle::Braille {
                "braille".into()
            } else {
                "blocks".into()
            },
            "blocks for fonts without braille",
        ),
        row(
            Setting::KeyHints,
            "",
            "Key hints",
            on(c.key_hints),
            "footer shortcuts",
        ),
        row(
            Setting::StartPage,
            "",
            "Start page",
            PAGES[(c.start_page.clamp(1, 6) - 1) as usize].into(),
            "",
        ),
        row(
            Setting::CpuUnits,
            "UNITS",
            "CPU %",
            if c.cpu_units == CpuUnits::AllCores {
                "of all cores".into()
            } else {
                "of one core".into()
            },
            "one core = top-style, up to cores×100%",
        ),
        row(
            Setting::Temperature,
            "",
            "Temperature",
            c.temperature.symbol().into(),
            "",
        ),
        row(
            Setting::Window,
            "VIEW",
            "Chart window",
            window(c.window_s),
            "w cycles it",
        ),
        row(
            Setting::MemoryChart,
            "",
            "Memory chart",
            if c.memory_chart == MemChart::AppOwned {
                "app-owned".into()
            } else {
                "full PSS".into()
            },
            "full adds shared Code and System",
        ),
        row(
            Setting::NetScale,
            "",
            "Network scale",
            if c.net_scale == NetScale::Sqrt {
                "√ (small traffic visible)".into()
            } else {
                "linear".into()
            },
            "",
        ),
        row(
            Setting::HideIdle,
            "",
            "Hide idle threads",
            on(c.hide_idle_threads),
            "threads that never used CPU",
        ),
        row(
            Setting::Interval,
            "SAMPLING",
            "Interval",
            secs(c.interval_ms),
            "restarts the collectors",
        ),
        row(
            Setting::MemoryInterval,
            "",
            "Memory interval",
            secs(c.memory_interval_ms),
            "dumpsys meminfo takes ~0.4 s",
        ),
        row(
            Setting::History,
            "",
            "Keep history",
            format!("{} min", c.history_min),
            "new sessions",
        ),
        row(
            Setting::Frames,
            "COLLECTORS",
            "Frames (gfxinfo)",
            on(c.frames),
            "",
        ),
        row(
            Setting::Capture,
            "",
            "Network capture",
            match c.capture {
                CaptureMode::On => "on".into(),
                CaptureMode::Off => "off".into(),
                CaptureMode::Ask => "ask".into(),
            },
            "on: starts with the monitor · ask: press c · off: disabled",
        ),
        row(Setting::Power, "", "Battery & thermal", on(c.power), ""),
        row(
            Setting::DeviceProcesses,
            "",
            "Process list",
            on(c.device_processes),
            "device mode; reads ~700 files a second",
        ),
        row(
            Setting::ReopenLast,
            "STARTUP & EXPORT",
            "Reopen last app",
            on(c.reopen_last_app),
            c_last(c),
        ),
        row(
            Setting::ReportDir,
            "",
            "Report folder",
            c.report_dir.clone(),
            "edit in config.toml",
        ),
        row(
            Setting::ExportOnQuit,
            "",
            "Export on quit",
            on(c.export_on_quit),
            "JSON report when you quit",
        ),
    ]
}

fn c_last(c: &Config) -> &'static str {
    if c.last_package.is_some() {
        "starts on the app you used last"
    } else {
        ""
    }
}

fn step<T: Copy + PartialEq>(options: &[T], current: T, forward: bool) -> T {
    let i = options.iter().position(|o| *o == current).unwrap_or(0);
    let n = options.len();
    options[if forward {
        (i + 1) % n
    } else {
        (i + n - 1) % n
    }]
}

fn change_setting(app: &mut App, key: Setting, forward: bool) -> bool {
    let c = &mut app.config;
    let mut restart = false;
    match key {
        Setting::Theme => app.cycle_theme(forward),
        Setting::Graph => {
            c.graph = if c.graph == GraphStyle::Braille {
                GraphStyle::Blocks
            } else {
                GraphStyle::Braille
            }
        }
        Setting::KeyHints => c.key_hints = !c.key_hints,
        Setting::StartPage => c.start_page = step(&[1, 2, 3, 4, 5, 6], c.start_page, forward),
        Setting::CpuUnits => {
            c.cpu_units = if c.cpu_units == CpuUnits::AllCores {
                CpuUnits::OneCore
            } else {
                CpuUnits::AllCores
            }
        }
        Setting::Temperature => {
            c.temperature = if c.temperature == TempUnit::C {
                TempUnit::F
            } else {
                TempUnit::C
            }
        }
        Setting::Window => {
            c.window_s = step(&WINDOWS, c.window_s, forward);
            app.window_s = c.window_s;
        }
        Setting::MemoryChart => {
            c.memory_chart = if c.memory_chart == MemChart::AppOwned {
                MemChart::Full
            } else {
                MemChart::AppOwned
            }
        }
        Setting::NetScale => {
            c.net_scale = if c.net_scale == NetScale::Sqrt {
                NetScale::Linear
            } else {
                NetScale::Sqrt
            }
        }
        Setting::HideIdle => c.hide_idle_threads = !c.hide_idle_threads,
        Setting::Interval => {
            c.interval_ms = step(&[500, 1000, 2000, 5000], c.interval_ms, forward);
            app.config_file_interval = None;
            restart = true;
        }
        Setting::MemoryInterval => {
            c.memory_interval_ms = step(&[1000, 2000, 5000, 10000], c.memory_interval_ms, forward);
            restart = true;
        }
        Setting::History => c.history_min = step(&[5, 15, 30, 60, 120], c.history_min, forward),
        Setting::Frames => {
            c.frames = !c.frames;
            restart = true;
        }
        Setting::Capture => {
            c.capture = step(
                &[CaptureMode::Ask, CaptureMode::On, CaptureMode::Off],
                c.capture,
                forward,
            )
        }
        Setting::Power => {
            c.power = !c.power;
            restart = true;
        }
        Setting::DeviceProcesses => {
            c.device_processes = !c.device_processes;
            restart = true;
        }
        Setting::ReopenLast => c.reopen_last_app = !c.reopen_last_app,
        Setting::ReportDir => {}
        Setting::ExportOnQuit => c.export_on_quit = !c.export_on_quit,
    }
    restart
}

struct Runtime {
    collectors: Option<Collectors>,
    devices_rx: mpsc::Receiver<Update>,
    _devices_task: tokio::task::JoinHandle<()>,
    capture_rx: mpsc::Receiver<Result<bool, String>>,
    capture_tx: mpsc::Sender<Result<bool, String>>,
    console_rx: mpsc::Receiver<ConsoleMsg>,
    console_tx: mpsc::Sender<ConsoleMsg>,
    action_rx: mpsc::Receiver<ActionDone>,
    action_tx: mpsc::Sender<ActionDone>,
}

fn run_console(app: &mut App, rt: &Runtime, args: Vec<String>, title: String) {
    let gen = app.console.next_gen();
    let cancel = actions::spawn_adb(app.serial.clone(), args, rt.console_tx.clone(), gen);
    app.console.start(title, cancel);
    app.popup = Popup::Console;
}

fn run_silent(app: &App, rt: &Runtime, act: Act, title: String, script: String) {
    let serial = app.serial.clone();
    let tx = rt.action_tx.clone();
    tokio::spawn(async move {
        let result = crate::adb::shell_once(serial.as_deref(), &script)
            .await
            .map_err(|e| e.to_string());
        let _ = tx.send(ActionDone { act, title, result }).await;
    });
}

fn submit_prompt(app: &mut App, rt: &Runtime) {
    let input = app.console.input.trim().to_string();
    match actions::console_args(&input) {
        Ok(args) => {
            app.console.remember(&input);
            app.console.input.clear();
            let title = format!("adb {}", args.join(" "));
            run_console(app, rt, args, title);
        }
        Err(e) => app.toast(e, true),
    }
}

fn run_act(app: &mut App, rt: &Runtime, act: Act) {
    if app.serial.is_none() {
        app.toast("no device selected", true);
        return;
    }
    let pkg = app.package().map(str::to_string);
    let needs_app = matches!(
        act,
        Act::Launch | Act::Restart | Act::ForceStop | Act::ResetFrames | Act::Bucket
    );
    if needs_app && pkg.is_none() {
        app.toast("pick an app with p first", true);
        return;
    }
    let pkg = pkg.unwrap_or_default();
    match act {
        Act::Console => {
            app.popup = Popup::Prompt;
        }
        Act::Launch | Act::Restart => {
            let restart = act == Act::Restart;
            if restart {
                app.session.add_marker("cold start");
            }
            let title = format!("{} {pkg}", if restart { "cold restart" } else { "launch" });
            run_console(
                app,
                rt,
                vec!["shell".into(), actions::launch_script(&pkg, restart)],
                title,
            );
        }
        Act::ForceStop => {
            app.popup = Popup::None;
            run_silent(
                app,
                rt,
                act,
                format!("force-stopped {pkg}"),
                format!("am force-stop {pkg}"),
            );
        }
        Act::ResetFrames => {
            app.popup = Popup::None;
            run_silent(
                app,
                rt,
                act,
                "frame stats reset".into(),
                format!("dumpsys gfxinfo {pkg} reset >/dev/null; echo ok"),
            );
        }
        Act::Doze => {
            app.popup = Popup::None;
            let on = !app.tweaks.doze;
            app.tweaks.doze = on;
            if !on {
                app.tweaks.unplugged = false;
            }
            let script = if on {
                "dumpsys battery unplug; dumpsys deviceidle force-idle"
            } else {
                "dumpsys deviceidle unforce; dumpsys battery reset; echo left forced Doze"
            };
            run_silent(app, rt, act, "Doze".into(), script.into());
        }
        Act::Unplug => {
            app.popup = Popup::None;
            let on = !app.tweaks.unplugged;
            app.tweaks.unplugged = on;
            let script = if on {
                "dumpsys battery unplug; echo battery reports unplugged"
            } else {
                "dumpsys battery reset; echo real power state restored"
            };
            run_silent(app, rt, act, "power".into(), script.into());
        }
        Act::Bucket => {
            app.popup = Popup::None;
            let now = app.session.doze.as_ref().and_then(|d| d.bucket.clone());
            let next = actions::next_bucket(app.tweaks.bucket_set.as_deref().or(now.as_deref()))
                .to_string();
            if app.tweaks.bucket_original.is_none() {
                let original = now
                    .map(|b| b.to_lowercase())
                    .filter(|b| actions::BUCKETS.contains(&b.as_str()))
                    .unwrap_or_else(|| "active".into());
                app.tweaks.bucket_original = Some((pkg.clone(), original));
            }
            app.tweaks.bucket_set = Some(next.clone());
            run_silent(
                app,
                rt,
                act,
                format!("standby bucket → {next}"),
                format!("am set-standby-bucket {pkg} {next} && echo ok"),
            );
        }
    }
}

fn on_action_done(app: &mut App, done: ActionDone) {
    match done.result {
        Ok(out) => {
            let line = out
                .lines()
                .map(str::trim)
                .rfind(|l| !l.is_empty())
                .unwrap_or("done")
                .to_string();
            let failed =
                line.contains("Error") || line.contains("Exception") || line.contains("Unknown");
            if failed {
                revert(app, done.act);
            }
            app.toast(format!("{}: {line}", done.title), failed);
        }
        Err(e) => {
            revert(app, done.act);
            app.toast(format!("{}: {e}", done.title), true);
        }
    }
}

fn revert(app: &mut App, act: Act) {
    match act {
        Act::Doze => app.tweaks.doze = !app.tweaks.doze,
        Act::Unplug => app.tweaks.unplugged = !app.tweaks.unplugged,
        Act::Bucket => app.tweaks.bucket_set = None,
        _ => {}
    }
}

async fn restore_tweaks(serial: Option<String>, tweaks: Tweaks) {
    if let Some(script) = tweaks.restore_script() {
        let _ = tokio::time::timeout(
            Duration::from_secs(5),
            crate::adb::shell_once(serial.as_deref(), &script),
        )
        .await;
    }
}

fn groups(config: &Config) -> Groups {
    Groups {
        frames: config.frames,
        power: config.power,
        threads: config.device_processes,
        ..Groups::ALL
    }
}

fn start_collectors(app: &mut App, rt: &mut Runtime) {
    rt.collectors = None;
    let interval = Duration::from_millis(app.config.interval_ms);
    let history = app.config.history_samples(interval);
    let package = app.session.package.clone();
    let serial = app.serial.clone();
    app.session = Session::new(serial.clone(), package, interval, history);
    app.session.devices = app.devices.clone();
    if serial.is_none() {
        return;
    }
    rt.collectors = Some(collect::spawn(collect::Config {
        serial,
        package: app.package_tx.subscribe(),
        interval,
        memory_interval: Duration::from_millis(app.config.memory_interval_ms).max(interval),
        groups: groups(&app.config),
        capture_stream: true,
        pcap: app.pcap.clone(),
        pcap_append: app.pcap_started,
        watch_devices: false,
    }));
    app.pcap_started |= app.pcap.is_some();
}

fn toggle_capture(app: &mut App, rt: &Runtime) {
    let Some(serial) = app.serial.clone() else {
        app.toast("no device selected", true);
        return;
    };
    if app.config.capture == CaptureMode::Off && !app.capture_on {
        app.toast("network capture is off in settings (,)", true);
        return;
    }
    if app.capture_busy {
        return;
    }
    if let crate::model::Support::Unsupported(reason) = app.session.support(Source::Capture).clone()
    {
        app.toast(reason, true);
        return;
    }
    app.capture_busy = true;
    let start = !app.capture_on;
    let tx = rt.capture_tx.clone();
    tokio::spawn(async move {
        let result = if start {
            crate::agent::start_capture(Some(&serial), None).await
        } else {
            crate::agent::stop_capture(Some(&serial)).await
        };
        let _ = tx
            .send(result.map(|_| start).map_err(|e| e.to_string()))
            .await;
    });
    if start {
        app.toast(
            "starting capture; the first time, accept the VPN request on the phone",
            false,
        );
    }
}

fn export(app: &mut App) {
    let report = crate::report::build(&app.session, "tui", Groups::ALL, 15.0);
    let dir = crate::config::expand(&app.config.report_dir);
    let name = app.package().unwrap_or("device").replace('.', "_");
    let path = dir.join(format!(
        "{name}-{}.json",
        chrono::Local::now().format("%Y%m%d-%H%M%S")
    ));
    let result = std::fs::create_dir_all(&dir).and_then(|_| {
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&report).unwrap_or_default(),
        )
    });
    match result {
        Ok(()) => app.toast(format!("report saved to {}", path.display()), false),
        Err(e) => app.toast(format!("export failed: {e}"), true),
    }
}

fn select_device(app: &mut App, rt: &mut Runtime, serial: String) {
    if app.serial.as_deref() == Some(serial.as_str()) {
        return;
    }
    if app.tweaks.any() {
        tokio::spawn(restore_tweaks(
            app.serial.clone(),
            std::mem::take(&mut app.tweaks),
        ));
    }
    let capture_was_on = app.capture_on;
    if let (true, Some(old)) = (capture_was_on, app.serial.clone()) {
        tokio::spawn(async move {
            let _ = tokio::time::timeout(
                Duration::from_secs(3),
                crate::agent::stop_capture(Some(&old)),
            )
            .await;
        });
    }
    app.serial = Some(serial.clone());
    app.config.last_device = Some(serial);
    app.capture_on = false;
    start_collectors(app, rt);
    if capture_was_on {
        toggle_capture(app, rt);
    }
}

fn on_devices(app: &mut App, rt: &mut Runtime, devices: Vec<Device>) {
    app.devices = devices;
    app.session.devices = app.devices.clone();
    if app.serial.is_some() {
        return;
    }
    let ready: Vec<&Device> = app.devices.iter().filter(|d| d.state == "device").collect();
    let preferred = app
        .config
        .last_device
        .clone()
        .filter(|s| ready.iter().any(|d| &d.serial == s));
    let pick = preferred.or_else(|| (ready.len() == 1).then(|| ready[0].serial.clone()));
    match pick {
        Some(serial) => select_device(app, rt, serial),
        None if ready.len() > 1 && app.popup == Popup::None && app.serial.is_none() => {
            app.popup = Popup::Device;
            app.cursor = 0;
        }
        None => {}
    }
}

fn handle_key(app: &mut App, rt: &mut Runtime, key: KeyEvent) {
    if key.kind != KeyEventKind::Press {
        return;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    if ctrl && matches!(key.code, KeyCode::Char('c')) {
        app.quit = true;
        return;
    }
    match app.popup {
        Popup::None => {}
        Popup::Help => {
            app.popup = Popup::None;
            return;
        }
        Popup::Actions => {
            let items = actions::menu(
                &app.tweaks,
                app.session.doze.as_ref().and_then(|d| d.bucket.as_deref()),
            );
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') => app.popup = Popup::None,
                KeyCode::Up => app.cursor = app.cursor.saturating_sub(1),
                KeyCode::Down => app.cursor = (app.cursor + 1).min(items.len() - 1),
                KeyCode::Enter => {
                    let act = items[app.cursor.min(items.len() - 1)].act;
                    run_act(app, rt, act);
                }
                KeyCode::Char(c) => {
                    if let Some(item) = items.iter().find(|i| i.key == c) {
                        let act = item.act;
                        run_act(app, rt, act);
                    }
                }
                _ => {}
            }
            return;
        }
        Popup::Prompt => {
            match key.code {
                KeyCode::Esc => {
                    app.popup = if app.console.title.is_empty() {
                        Popup::None
                    } else {
                        Popup::Console
                    }
                }
                KeyCode::Enter => submit_prompt(app, rt),
                KeyCode::Up => app.console.history_step(true),
                KeyCode::Down => app.console.history_step(false),
                KeyCode::Backspace => {
                    app.console.input.pop();
                }
                KeyCode::Char('u') if ctrl => app.console.input.clear(),
                KeyCode::Char(c) => app.console.input.push(c),
                _ => {}
            }
            return;
        }
        Popup::Console => {
            let len = app.console.lines.len();
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') => {
                    if app.console.running {
                        app.console.stop();
                    } else {
                        app.popup = Popup::None;
                    }
                }
                KeyCode::Char(':') => {
                    app.console.input.clear();
                    app.popup = Popup::Prompt;
                }
                KeyCode::Up => app.console.scroll = (app.console.scroll + 1).min(len),
                KeyCode::Down => app.console.scroll = app.console.scroll.saturating_sub(1),
                KeyCode::PageUp => app.console.scroll = (app.console.scroll + 20).min(len),
                KeyCode::PageDown => app.console.scroll = app.console.scroll.saturating_sub(20),
                KeyCode::Home => app.console.scroll = len,
                KeyCode::End => app.console.scroll = 0,
                _ => {}
            }
            return;
        }
        Popup::Device => {
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') => app.popup = Popup::None,
                KeyCode::Up => app.cursor = app.cursor.saturating_sub(1),
                KeyCode::Down => {
                    app.cursor = (app.cursor + 1).min(app.devices.len().saturating_sub(1))
                }
                KeyCode::Enter => {
                    if let Some(d) = app
                        .devices
                        .get(app.cursor)
                        .filter(|d| d.state == "device")
                        .cloned()
                    {
                        app.popup = Popup::None;
                        select_device(app, rt, d.serial);
                    }
                }
                _ => {}
            }
            return;
        }
        Popup::Package => {
            match key.code {
                KeyCode::Esc => {
                    app.popup = Popup::None;
                    app.filter.clear();
                }
                KeyCode::Up => app.cursor = app.cursor.saturating_sub(1),
                KeyCode::Down => app.cursor = (app.cursor + 1).min(app.filtered_packages().len()),
                KeyCode::PageUp => app.cursor = app.cursor.saturating_sub(10),
                KeyCode::PageDown => {
                    app.cursor = (app.cursor + 10).min(app.filtered_packages().len())
                }
                KeyCode::Backspace => {
                    app.filter.pop();
                    app.cursor = 0;
                }
                KeyCode::Enter => {
                    let choice = if app.cursor == 0 {
                        None
                    } else {
                        app.filtered_packages()
                            .get(app.cursor - 1)
                            .map(|s| s.to_string())
                    };
                    app.popup = Popup::None;
                    app.filter.clear();
                    app.set_package(choice);
                }
                KeyCode::Char(c) if c.is_ascii_alphanumeric() || c == '.' || c == '_' => {
                    app.filter.push(c);
                    app.cursor = if app.filter.is_empty() { 0 } else { 1 };
                }
                _ => {}
            }
            return;
        }
        Popup::Marker => {
            match key.code {
                KeyCode::Esc => app.popup = Popup::None,
                KeyCode::Enter => {
                    let label = std::mem::take(&mut app.input);
                    app.session.add_marker(label.trim());
                    app.popup = Popup::None;
                }
                KeyCode::Backspace => {
                    app.input.pop();
                }
                KeyCode::Char(c) if app.input.chars().count() < 40 => app.input.push(c),
                _ => {}
            }
            return;
        }
        Popup::Settings => {
            let rows = settings_rows(app).len();
            match key.code {
                KeyCode::Esc | KeyCode::Char(',') | KeyCode::Char('q') => {
                    app.popup = Popup::None;
                    match app.save_config() {
                        Ok(_) => app.config_autosave = true,
                        Err(e) => app.toast(format!("settings not saved: {e}"), true),
                    }
                }
                KeyCode::Up => app.cursor = app.cursor.saturating_sub(1),
                KeyCode::Down => app.cursor = (app.cursor + 1).min(rows - 1),
                KeyCode::Left | KeyCode::Right | KeyCode::Enter | KeyCode::Char(' ') => {
                    let key_of_row = settings_rows(app)[app.cursor.min(rows - 1)].key;
                    if change_setting(app, key_of_row, key.code != KeyCode::Left) {
                        start_collectors(app, rt);
                    }
                }
                KeyCode::Char('r') => {
                    let keep = (
                        app.config.last_device.clone(),
                        app.config.last_package.clone(),
                        app.config.themes.clone(),
                    );
                    app.config = Config {
                        last_device: keep.0,
                        last_package: keep.1,
                        themes: keep.2,
                        ..Config::default()
                    };
                    app.config_file_interval = None;
                    app.theme = theme::find(&app.themes, &app.config.theme).unwrap_or(0);
                    app.window_s = app.config.window_s;
                    start_collectors(app, rt);
                }
                _ => {}
            }
            return;
        }
    }
    if app.filtering {
        match key.code {
            KeyCode::Esc => {
                app.filtering = false;
                app.table_filter.clear();
            }
            KeyCode::Enter => app.filtering = false,
            KeyCode::Backspace => {
                app.table_filter.pop();
            }
            KeyCode::Char(c) => app.table_filter.push(c),
            _ => {}
        }
        app.scroll[app.page] = 0;
        return;
    }
    match key.code {
        KeyCode::Char('q') => app.quit = true,
        KeyCode::Esc => {
            if !app.table_filter.is_empty() {
                app.table_filter.clear();
            } else {
                app.page = 0;
            }
        }
        KeyCode::Char(c @ '1'..='6') => app.page = c as usize - '1' as usize,
        KeyCode::Tab => app.page = (app.page + 1) % PAGES.len(),
        KeyCode::BackTab => app.page = (app.page + PAGES.len() - 1) % PAGES.len(),
        KeyCode::Char(' ') => app.paused = !app.paused,
        KeyCode::Char('d') => {
            app.popup = Popup::Device;
            app.cursor = app
                .serial
                .as_ref()
                .and_then(|s| app.devices.iter().position(|d| &d.serial == s))
                .unwrap_or(0);
        }
        KeyCode::Char('p') => {
            app.popup = Popup::Package;
            app.filter.clear();
            app.cursor = app
                .package()
                .and_then(|p| app.session.packages.iter().position(|x| x == p))
                .map_or(0, |i| i + 1);
        }
        KeyCode::Char('x') => app.set_package(None),
        KeyCode::Char('c') => toggle_capture(app, rt),
        KeyCode::Char('m') => app.session.add_marker(""),
        KeyCode::Char('M') => {
            app.input.clear();
            app.popup = Popup::Marker;
        }
        KeyCode::Char('w') => app.cycle_window(),
        KeyCode::Char('t') => app.cycle_theme(true),
        KeyCode::Char('T') => app.cycle_theme(false),
        KeyCode::Char(',') => {
            app.popup = Popup::Settings;
            app.cursor = 0;
        }
        KeyCode::Char('e') => export(app),
        KeyCode::Char('a') => {
            app.popup = Popup::Actions;
            app.cursor = 0;
        }
        KeyCode::Char(':') => {
            app.console.input.clear();
            app.console.history_pos = None;
            app.popup = Popup::Prompt;
        }
        KeyCode::Char('z') if app.page == 5 => run_act(app, rt, Act::Doze),
        KeyCode::Char('b') if app.page == 5 => run_act(app, rt, Act::Bucket),
        KeyCode::Char('u') if app.page == 5 => run_act(app, rt, Act::Unplug),
        KeyCode::Char('r') if app.page == 3 => run_act(app, rt, Act::ResetFrames),
        KeyCode::Char('?') | KeyCode::Char('h') => app.popup = Popup::Help,
        KeyCode::Char('s') => {
            app.sort = match (app.page, app.sort) {
                (4, Sort::Recent) => Sort::Bytes,
                (4, Sort::Bytes) => Sort::Name,
                (4, _) => Sort::Recent,
                (_, Sort::Cpu) => Sort::Name,
                _ => Sort::Cpu,
            };
            app.scroll[app.page] = 0;
        }
        KeyCode::Char('g') => app.group_pools = !app.group_pools,
        KeyCode::Char('/') => {
            app.filtering = true;
            app.table_filter.clear();
        }
        KeyCode::Up => app.scroll[app.page] = app.scroll[app.page].saturating_sub(1),
        KeyCode::Down => app.scroll[app.page] += 1,
        KeyCode::PageUp => app.scroll[app.page] = app.scroll[app.page].saturating_sub(15),
        KeyCode::PageDown => app.scroll[app.page] += 15,
        KeyCode::Home => app.scroll[app.page] = 0,
        _ => {}
    }
}

fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), LeaveAlternateScreen, crossterm::cursor::Show);
}

static EMERGENCY_CLEANUP: std::sync::Mutex<Option<(Option<String>, String)>> =
    std::sync::Mutex::new(None);

fn capture_running(app: &App) -> bool {
    app.capture_on || app.capture_busy || app.session.capture.snapshot.is_some()
}

fn cleanup_script(app: &App) -> Option<String> {
    let mut steps = Vec::new();
    if capture_running(app) {
        steps.push(crate::agent::stop_script());
    }
    if let Some(restore) = app.tweaks.restore_script() {
        steps.push(restore);
    }
    (!steps.is_empty()).then(|| steps.join("; "))
}

fn arm_emergency_cleanup(app: &App) {
    let next = cleanup_script(app).map(|script| (app.serial.clone(), script));
    if let Ok(mut slot) = EMERGENCY_CLEANUP.try_lock() {
        if *slot != next {
            *slot = next;
        }
    }
}

fn run_emergency_cleanup() {
    let Some((serial, script)) = EMERGENCY_CLEANUP.try_lock().ok().and_then(|mut s| s.take())
    else {
        return;
    };
    let mut cmd = std::process::Command::new("adb");
    if let Some(serial) = serial {
        cmd.arg("-s").arg(serial);
    }
    let _ = cmd
        .arg("shell")
        .arg(script)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

#[cfg(unix)]
struct ExitSignals {
    hangup: Option<tokio::signal::unix::Signal>,
    terminate: Option<tokio::signal::unix::Signal>,
    interrupt: Option<tokio::signal::unix::Signal>,
}

#[cfg(unix)]
impl ExitSignals {
    fn new() -> Self {
        use tokio::signal::unix::{signal, SignalKind};
        Self {
            hangup: signal(SignalKind::hangup()).ok(),
            terminate: signal(SignalKind::terminate()).ok(),
            interrupt: signal(SignalKind::interrupt()).ok(),
        }
    }

    async fn recv(&mut self) {
        async fn one(s: &mut Option<tokio::signal::unix::Signal>) {
            match s {
                Some(s) => {
                    s.recv().await;
                }
                None => std::future::pending::<()>().await,
            }
        }
        tokio::select! {
            _ = one(&mut self.hangup) => {}
            _ = one(&mut self.terminate) => {}
            _ = one(&mut self.interrupt) => {}
        }
    }
}

#[cfg(not(unix))]
struct ExitSignals;

#[cfg(not(unix))]
impl ExitSignals {
    fn new() -> Self {
        ExitSignals
    }

    async fn recv(&mut self) {
        let _ = tokio::signal::ctrl_c().await;
    }
}

pub async fn run(common: Common, theme_arg: Option<String>) -> i32 {
    let (mut config, config_error) = Config::load();
    let themes = theme::all(&config.themes);
    let mut theme = theme::find(&themes, &config.theme).unwrap_or(0);
    if let Some(name) = &theme_arg {
        match theme::find(&themes, name) {
            Some(i) => theme = i,
            None => {
                let names: Vec<&str> = themes.iter().map(|t| t.name.as_str()).collect();
                eprintln!("unknown theme '{name}'; available: {}", names.join(", "));
                return 2;
            }
        }
    }
    let mut config_file_interval = None;
    if let Some(i) = common.interval {
        config_file_interval = Some(config.interval_ms);
        config.interval_ms = i.as_millis().clamp(200, 60_000) as u64;
    }
    if let Err(e) = enable_raw_mode() {
        eprintln!("this needs an interactive terminal ({e}); for scripts use `android-perf-monitor record`");
        return 1;
    }
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        default_hook(info);
        run_emergency_cleanup();
    }));
    let _ = execute!(io::stdout(), EnterAlternateScreen, crossterm::cursor::Hide);
    let mut terminal = match Terminal::new(CrosstermBackend::new(io::stdout())) {
        Ok(t) => t,
        Err(e) => {
            restore_terminal();
            eprintln!("{e}");
            return 1;
        }
    };

    let interval = Duration::from_millis(config.interval_ms);
    let package = common.package.clone().or_else(|| {
        config
            .reopen_last_app
            .then(|| config.last_package.clone())
            .flatten()
            .filter(|p| crate::parse::is_valid_package_name(p))
    });
    let (package_tx, _) = watch::channel(package.clone());
    let (devices_tx, devices_rx) = mpsc::channel(8);
    let (capture_tx, capture_rx) = mpsc::channel(4);
    let (console_tx, console_rx) = mpsc::channel(1024);
    let (action_tx, action_rx) = mpsc::channel(16);
    let mut rt = Runtime {
        collectors: None,
        devices_rx,
        _devices_task: tokio::spawn(collect::devices_task(devices_tx)),
        capture_rx,
        capture_tx,
        console_rx,
        console_tx,
        action_rx,
        action_tx,
    };
    let window_s = config.window_s;
    let mut app = App {
        page: (config.start_page.clamp(1, 6) - 1) as usize,
        popup: Popup::None,
        themes,
        theme,
        session: Session::new(None, package, interval, config.history_samples(interval)),
        config,
        paused: false,
        window_s,
        serial: common.device.clone(),
        devices: Vec::new(),
        capture_on: false,
        capture_busy: false,
        cursor: 0,
        filter: String::new(),
        input: String::new(),
        scroll: [0; 6],
        sort: Sort::Cpu,
        group_pools: false,
        table_filter: String::new(),
        filtering: false,
        toast: None,
        quit: false,
        pcap: common.pcap.clone(),
        console: Console::default(),
        tweaks: Tweaks::default(),
        package_tx,
        pcap_started: false,
        config_autosave: config_error.is_none(),
        config_file_interval,
    };
    if let Some(e) = config_error {
        app.toast(format!("config ignored: {e}"), true);
    }
    if app.serial.is_some() {
        start_collectors(&mut app, &mut rt);
    }
    let auto_capture =
        common.capture || common.pcap.is_some() || app.config.capture == CaptureMode::On;
    let mut capture_requested = false;

    let mut events = EventStream::new();
    let mut exit_signals = ExitSignals::new();
    let mut clock = tokio::time::interval(Duration::from_millis(500));
    let mut dirty = true;
    let mut last_draw = Instant::now() - Duration::from_secs(1);
    let min_frame = Duration::from_millis(33);

    loop {
        if dirty && last_draw.elapsed() >= min_frame {
            if let Some(t) = &app.toast {
                if Instant::now() > t.until {
                    app.toast = None;
                }
            }
            if terminal.draw(|f| crate::ui::draw(f, &app)).is_err() {
                break;
            }
            last_draw = Instant::now();
            dirty = false;
        }
        let redraw_at = tokio::time::Instant::from_std(last_draw + min_frame);
        tokio::select! {
            event = events.next() => {
                match event {
                    Some(Ok(Event::Key(key))) => {
                        handle_key(&mut app, &mut rt, key);
                        dirty = true;
                    }
                    Some(Ok(Event::Resize(_, _))) => dirty = true,
                    Some(Ok(_)) => {}
                    Some(Err(_)) | None => break,
                }
            }
            update = async {
                match rt.collectors.as_mut() {
                    Some(c) => c.rx.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                match update {
                    Some(update) => {
                        let data = matches!(update, Update::Fast(_) | Update::AppMemory(_) | Update::Frames(_) | Update::Battery(_) | Update::Thermal(_) | Update::Doze(_) | Update::Jobs(_) | Update::Capture(_));
                        if !(app.paused && data) {
                            if let Update::CaptureError(e) = &update {
                                app.toast(format!("capture: {e}"), true);
                            }
                            app.session.apply(update);
                            dirty = true;
                        }
                        if !capture_requested && auto_capture && app.session.caps.is_some() {
                            capture_requested = true;
                            toggle_capture(&mut app, &rt);
                        }
                    }
                    None => rt.collectors = None,
                }
            }
            Some(update) = rt.devices_rx.recv() => {
                match update {
                    Update::Devices(devices) => on_devices(&mut app, &mut rt, devices),
                    Update::Error(_, e) => app.toast(e, true),
                    _ => {}
                }
                dirty = true;
            }
            Some(msg) = rt.console_rx.recv() => {
                app.console.on_event(msg);
                dirty = true;
            }
            Some(done) = rt.action_rx.recv() => {
                on_action_done(&mut app, done);
                dirty = true;
            }
            Some(result) = rt.capture_rx.recv() => {
                app.capture_busy = false;
                match result {
                    Ok(on) => app.capture_on = on,
                    Err(e) => {
                        if e == crate::agent::NOT_INSTALLED {
                            app.session.support[Source::Capture.index()] = crate::model::Support::Unsupported(e.clone());
                        }
                        app.toast(format!("capture: {e}"), true);
                    }
                }
                dirty = true;
            }
            _ = clock.tick() => dirty = true,
            _ = exit_signals.recv() => app.quit = true,
            _ = tokio::time::sleep_until(redraw_at), if dirty => {}
        }
        arm_emergency_cleanup(&app);
        if app.quit {
            break;
        }
    }

    restore_terminal();
    app.console.stop();
    rt.collectors = None;
    if app.config.export_on_quit && app.session.ticks > 0 {
        export(&mut app);
    }
    let stop_capture = capture_running(&app);
    if let Some(script) = cleanup_script(&app) {
        let serial = app.serial.clone();
        let done = tokio::time::timeout(
            Duration::from_secs(5),
            crate::adb::shell_once(serial.as_deref(), &script),
        )
        .await;
        if stop_capture {
            let mut err = io::stderr();
            let _ = match done {
                Ok(Ok(_)) => writeln!(err, "stopped the network capture on {}", serial.as_deref().unwrap_or("the device")),
                _ => writeln!(err, "could not stop the network capture; the agent stops by itself {} s after the monitor is gone", crate::agent::IDLE_STOP_SECS),
            };
        }
        if let Ok(mut slot) = EMERGENCY_CLEANUP.lock() {
            *slot = None;
        }
    }
    if app.config_autosave {
        let _ = app.save_config();
    }
    0
}
