mod agent;
mod app;
mod data;
mod recorder;
mod runner;
mod shell;
mod ui;

use anyhow::Result;
use clap::Parser;
use crossterm::{
    cursor::Show,
    event::{DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures::StreamExt;
use ratatui::prelude::*;
use std::io;
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{info, Level};

use app::{push_history, App};
use recorder::CsvRecorder;
use runner::{create_shared_package, spawn_runners, PanelUpdate, RunnerConfig, RunnerHandle, SharedPackage};

#[derive(Debug)]
enum KeyEvent {
    Char(char),
    Up,
    Down,
    Enter,
    Backspace,
    Escape,
    Quit,
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// ADB device serial (default: first connected device)
    #[arg(short, long)]
    device: Option<String>,

    /// Package to track (default: device-wide)
    #[arg(short, long, value_parser = parse_package)]
    package: Option<String>,

    /// Sample interval in seconds
    #[arg(short, long, default_value = "1", value_parser = clap::value_parser!(u64).range(1..))]
    interval: u64,

    /// Write one CSV row per sample to this file
    #[arg(short, long)]
    output: Option<String>,

    /// Start the on-device capture agent: per-app traffic, connections and domains
    #[arg(long)]
    capture: bool,

    /// Save the agent's packet capture to this file for Wireshark (implies --capture)
    #[arg(long, value_name = "FILE")]
    pcap: Option<PathBuf>,
}

fn parse_package(value: &str) -> Result<String, String> {
    if runner::is_valid_package_name(value) {
        Ok(value.to_string())
    } else {
        Err(format!("'{}' is not a valid Android package name", value))
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Logging is optional; a read-only working directory shouldn't stop the monitor.
    if let Ok(log_file) = std::fs::File::create("android-perf-monitor.log") {
        tracing_subscriber::fmt()
            .with_max_level(Level::DEBUG)
            .with_writer(log_file)
            .with_ansi(false)
            .init();
    }

    info!("Starting Android Performance Monitor");
    info!("Args: package={:?}, device={:?}", args.package, args.device);

    // Open the CSV file before taking over the terminal so errors are readable.
    let recorder = args.output.as_deref().map(CsvRecorder::create).transpose()?;

    install_panic_hook();
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(args.package.clone(), recorder);
    app.device_serial = args.device.clone();
    app.capture_wanted = args.capture || args.pcap.is_some();

    let shared_package = create_shared_package(args.package.clone());
    app.shared_package = Some(shared_package.clone());

    let (key_tx, key_rx) = mpsc::unbounded_channel::<KeyEvent>();
    tokio::spawn(keyboard_task(key_tx));

    let result = run_app(&mut terminal, &mut app, key_rx, shared_package, args.interval, args.pcap).await;

    restore_terminal()?;

    if let Err(err) = result {
        eprintln!("Error: {err:?}");
    }

    Ok(())
}

fn restore_terminal() -> io::Result<()> {
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture, Show)
}

/// Puts the terminal back before printing a panic, then exits: a half-dead TUI is worse than none.
fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = restore_terminal();
        default_hook(panic_info);
        std::process::exit(101);
    }));
}

async fn keyboard_task(key_tx: mpsc::UnboundedSender<KeyEvent>) {
    let mut event_stream = EventStream::new();

    while let Some(event_result) = event_stream.next().await {
        if let Ok(Event::Key(key)) = event_result {
            if key.kind == KeyEventKind::Press {
                let key_event = match key.code {
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => KeyEvent::Quit,
                    KeyCode::Esc => KeyEvent::Escape,
                    KeyCode::Up => KeyEvent::Up,
                    KeyCode::Down => KeyEvent::Down,
                    KeyCode::Enter => KeyEvent::Enter,
                    KeyCode::Backspace => KeyEvent::Backspace,
                    KeyCode::Char(c) => KeyEvent::Char(c),
                    _ => continue,
                };

                if key_tx.send(key_event).is_err() {
                    break;
                }
            }
        }
    }
}

fn start_collectors(app: &App, shared_package: &SharedPackage, interval_secs: u64, pcap: &Option<PathBuf>) -> RunnerHandle {
    spawn_runners(RunnerConfig {
        device_serial: app.device_serial.clone(),
        shared_package: shared_package.clone(),
        interval_secs,
        pcap_path: pcap.clone(),
    })
}

/// Starts or stops the agent on the phone in the background; failures show in the footer.
fn spawn_capture_command(sender: &mpsc::UnboundedSender<PanelUpdate>, serial: String, start: bool) {
    let sender = sender.clone();
    tokio::spawn(async move {
        let result = if start {
            agent::start_capture(Some(&serial), None).await
        } else {
            agent::stop_capture(Some(&serial)).await
        };
        if let Err(e) = result {
            let _ = sender.send(PanelUpdate::CaptureError(e.to_string()));
        }
    });
}

async fn run_app<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    mut key_rx: mpsc::UnboundedReceiver<KeyEvent>,
    shared_package: SharedPackage,
    interval_secs: u64,
    pcap: Option<PathBuf>,
) -> Result<()> {
    let draw_interval = Duration::from_millis(16);
    let mut draw_timer = tokio::time::interval(draw_interval);
    let mut collectors = start_collectors(app, &shared_package, interval_secs, &pcap);
    // Device the capture agent was started on by us.
    let mut capture_on: Option<String> = None;

    terminal.draw(|frame| ui::draw(frame, app))?;

    loop {
        tokio::select! {
            biased;

            Some(key_event) = key_rx.recv() => {
                if handle_key_event(app, key_event) {
                    break;
                }
                terminal.draw(|frame| ui::draw(frame, app))?;
            }

            Some(update) = collectors.updates.recv() => {
                handle_panel_update(app, update);
            }

            _ = draw_timer.tick() => {
                terminal.draw(|frame| ui::draw(frame, app))?;
            }
        }

        // Picking a device (or the first device showing up) needs a new adb shell for that serial.
        if app.device_serial != collectors.device_serial {
            info!("Device changed to {:?}, restarting collectors", app.device_serial);
            collectors = start_collectors(app, &shared_package, interval_secs, &pcap);
        }

        // Keep the agent running on the current device exactly while the user wants it.
        let capture_target = if app.capture_wanted { app.device_serial.clone() } else { None };
        if capture_target != capture_on {
            if let Some(old) = capture_on.take() {
                spawn_capture_command(&collectors.sender, old, false);
            }
            if let Some(new) = capture_target.clone() {
                spawn_capture_command(&collectors.sender, new, true);
            }
            capture_on = capture_target;
        }
    }

    // Don't leave the phone's VPN running after we quit.
    if let Some(serial) = capture_on {
        let _ = tokio::time::timeout(Duration::from_secs(3), agent::stop_capture(Some(&serial))).await;
    }
    Ok(())
}

fn handle_key_event(app: &mut App, key_event: KeyEvent) -> bool {
    match key_event {
        KeyEvent::Quit => true,
        KeyEvent::Escape => app.on_escape(),
        KeyEvent::Up => {
            app.on_up();
            false
        }
        KeyEvent::Down => {
            app.on_down();
            false
        }
        KeyEvent::Enter => {
            app.on_enter();
            false
        }
        KeyEvent::Backspace => {
            app.on_backspace();
            false
        }
        KeyEvent::Char(c) => app.on_key(c),
    }
}

fn handle_panel_update(app: &mut App, update: PanelUpdate) {
    match update {
        PanelUpdate::Devices(devices) => {
            app.set_devices(
                devices
                    .into_iter()
                    .map(|dev| crate::data::DeviceInfo {
                        serial: dev.serial,
                        model: dev.model,
                        brand: dev.brand,
                    })
                    .collect(),
            );
            return;
        }
        PanelUpdate::Packages(packages) => {
            app.set_packages(packages);
            app.last_error = None;
            return;
        }
        PanelUpdate::Error { panel, message } => {
            app.last_error = Some(format!("[{}] {}", panel, message));
            return;
        }
        PanelUpdate::CaptureError(message) => {
            app.capture_error = Some(format!("[capture] {}", message));
            return;
        }
        PanelUpdate::CaptureStopped => {
            app.capture = None;
            return;
        }
        PanelUpdate::Capture(_) => {}
        _ => {
            // Any sample from the adb shell means it works again.
            app.last_error = None;
        }
    }

    if app.paused {
        return;
    }

    match update {
        PanelUpdate::Cpu(data) => {
            app.metrics.cpu_cores.clear();
            let mut total_usage = 0.0f32;
            for core in &data.cores {
                app.metrics.cpu_cores.push(crate::data::CpuCore {
                    id: core.id,
                    usage_percent: core.usage_percent,
                    frequency_mhz: core.frequency_mhz,
                    temperature: None,
                });
                total_usage += core.usage_percent;
            }
            app.metrics.app_cpu_total = data.app_cpu_total.unwrap_or(0.0);
            app.sample_count += 1;

            let cpu_val = if app.package.is_some() {
                if let Some(app_cpu) = data.app_cpu_total {
                    app.app_cpu_samples += 1;
                    let n = app.app_cpu_samples as f32;
                    app.metrics.app_cpu_avg += (app_cpu - app.metrics.app_cpu_avg) / n;
                    app.metrics.app_cpu_peak = app.metrics.app_cpu_peak.max(app_cpu);
                }
                app.metrics.app_cpu_total
            } else if data.cores.is_empty() {
                0.0
            } else {
                total_usage / data.cores.len() as f32
            };
            push_history(&mut app.cpu_history, cpu_val);

            if let Some(mut recorder) = app.recorder.take() {
                match recorder.record(app, data.app_cpu_total) {
                    Ok(()) => app.recorder = Some(recorder),
                    Err(e) => app.recorder_error = Some(format!("[csv] recording stopped: {}", e)),
                }
            }
        }
        PanelUpdate::Memory(data) => {
            if data.total_kb > 0 {
                let used = data.total_kb.saturating_sub(data.available_kb);
                push_history(&mut app.mem_history, used * 100 / data.total_kb);
            }
            app.memory_data = data;
        }
        PanelUpdate::Battery(data) => {
            app.metrics.battery_level = data.level;
            app.battery_data = data;
        }
        PanelUpdate::Thermal(data) => {
            app.metrics.thermal.ap = data.cpu_temp;
            app.metrics.thermal.battery = data.battery_temp;
            app.metrics.thermal.skin = data.skin_temp;

            let temp = if data.cpu_temp > 0.0 { data.cpu_temp } else { data.skin_temp };
            push_history(&mut app.temp_history, temp);
        }
        PanelUpdate::Network(data) => {
            let prev = &app.network_data;
            let prev_rx = prev.wifi_rx_bytes + prev.mobile_rx_bytes;
            let prev_tx = prev.wifi_tx_bytes + prev.mobile_tx_bytes;
            let new_rx = data.wifi_rx_bytes + data.mobile_rx_bytes;
            let new_tx = data.wifi_tx_bytes + data.mobile_tx_bytes;
            if !app.net_resync && (prev_rx > 0 || prev_tx > 0) {
                push_history(&mut app.net_rx_history, new_rx.saturating_sub(prev_rx));
                push_history(&mut app.net_tx_history, new_tx.saturating_sub(prev_tx));
            }
            app.net_resync = false;
            app.network_data = data;
        }
        PanelUpdate::Doze(data) => {
            app.doze_data = data;
        }
        PanelUpdate::Threads(data) => {
            app.metrics.threads = data.threads;
        }
        PanelUpdate::Alarms(data) => {
            app.alarms_data = data;
        }
        PanelUpdate::Jobs(data) => {
            app.jobs_data = data;
        }
        PanelUpdate::Capture(snapshot) => {
            app.on_capture(*snapshot);
        }
        PanelUpdate::Devices(_)
        | PanelUpdate::Packages(_)
        | PanelUpdate::Error { .. }
        | PanelUpdate::CaptureError(_)
        | PanelUpdate::CaptureStopped => {}
    }
}
