mod adb;
mod app;
mod data;
mod runner;
mod shell;
mod ui;

use anyhow::Result;
use clap::Parser;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures::StreamExt;
use ratatui::prelude::*;
use std::io;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{info, Level};

use app::App;
use runner::{PanelUpdate, RunnerConfig, create_shared_package, spawn_runners};

#[derive(Debug)]
enum KeyEvent {
    Char(char),
    Up,
    Down,
    Enter,
    Backspace,
    Escape,
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(short, long)]
    package: Option<String>,

    #[arg(short, long, default_value = "60")]
    duration: u64,

    #[arg(short, long)]
    output: Option<String>,

    #[arg(short, long, default_value = "1")]
    interval: u64,

    #[arg(long)]
    device: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let log_file = std::fs::File::create("android-perf-monitor.log")
        .expect("Failed to create log file");
    tracing_subscriber::fmt()
        .with_max_level(Level::DEBUG)
        .with_writer(log_file)
        .with_ansi(false)
        .init();

    info!("Starting Android Performance Monitor");
    info!("Args: package={:?}, device={:?}", args.package, args.device);

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(args.package.clone(), args.duration, args.interval, args.output);

    if let Some(device) = args.device.clone() {
        app.device_serial = Some(device);
    }

    let (update_tx, update_rx) = mpsc::unbounded_channel::<PanelUpdate>();

    let shared_package = create_shared_package(args.package.clone());
    app.shared_package = Some(shared_package.clone());

    let runner_config = RunnerConfig {
        device_serial: app.device_serial.clone(),
        shared_package,
        update_tx,
    };
    let runner_handle = spawn_runners(runner_config);

    let (key_tx, key_rx) = mpsc::unbounded_channel::<KeyEvent>();
    tokio::spawn(keyboard_task(key_tx));

    let result = run_app(&mut terminal, &mut app, key_rx, update_rx).await;

    runner_handle.abort_all();

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(err) = result {
        eprintln!("Error: {err:?}");
    }

    Ok(())
}

async fn keyboard_task(key_tx: mpsc::UnboundedSender<KeyEvent>) {
    let mut event_stream = EventStream::new();

    while let Some(event_result) = event_stream.next().await {
        if let Ok(Event::Key(key)) = event_result {
            if key.kind == KeyEventKind::Press {
                let key_event = match key.code {
                    KeyCode::Char('q') => KeyEvent::Escape,
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

async fn run_app<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    mut key_rx: mpsc::UnboundedReceiver<KeyEvent>,
    mut update_rx: mpsc::UnboundedReceiver<PanelUpdate>,
) -> Result<()> {
    let draw_interval = Duration::from_millis(16);
    let mut draw_timer = tokio::time::interval(draw_interval);

    terminal.draw(|frame| ui::draw(frame, app))?;

    loop {
        tokio::select! {
            biased;

            Some(key_event) = key_rx.recv() => {
                if handle_key_event(app, key_event) {
                    return Ok(());
                }
                terminal.draw(|frame| ui::draw(frame, app))?;
            }

            Some(update) = update_rx.recv() => {
                handle_panel_update(app, update);
            }

            _ = draw_timer.tick() => {
                terminal.draw(|frame| ui::draw(frame, app))?;
            }
        }
    }
}

fn handle_key_event(app: &mut App, key_event: KeyEvent) -> bool {
    match key_event {
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
        KeyEvent::Char(c) => {
            app.on_key(c);
            false
        }
    }
}

fn handle_panel_update(app: &mut App, update: PanelUpdate) {
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
            app.metrics.app_cpu_total = data.app_cpu_total;

            let cpu_val = if app.package.is_some() {
                data.app_cpu_total
            } else {
                if data.cores.is_empty() { 0.0 } else { total_usage / data.cores.len() as f32 }
            };
            app.cpu_history.push(cpu_val);
            if app.cpu_history.len() > 60 {
                app.cpu_history.remove(0);
            }
        }
        PanelUpdate::Memory(data) => {
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
            app.temp_history.push(temp);
            if app.temp_history.len() > 60 {
                app.temp_history.remove(0);
            }
        }
        PanelUpdate::Network(data) => {
            let prev = &app.network_data;
            let (prev_rx, prev_tx, new_rx, new_tx) = if app.package.is_some() {
                (prev.app_rx_bytes, prev.app_tx_bytes, data.app_rx_bytes, data.app_tx_bytes)
            } else {
                (
                    prev.wifi_rx_bytes + prev.mobile_rx_bytes,
                    prev.wifi_tx_bytes + prev.mobile_tx_bytes,
                    data.wifi_rx_bytes + data.mobile_rx_bytes,
                    data.wifi_tx_bytes + data.mobile_tx_bytes,
                )
            };
            if prev_rx > 0 || prev_tx > 0 {
                let rx_delta = new_rx.saturating_sub(prev_rx);
                let tx_delta = new_tx.saturating_sub(prev_tx);
                app.net_rx_history.push(rx_delta);
                app.net_tx_history.push(tx_delta);
                if app.net_rx_history.len() > 60 {
                    app.net_rx_history.remove(0);
                }
                if app.net_tx_history.len() > 60 {
                    app.net_tx_history.remove(0);
                }
            }
            app.network_data = data;
        }
        PanelUpdate::Doze(data) => {
            app.doze_state = data.device_idle_state.clone();
            app.doze_data = data;
        }
        PanelUpdate::Threads(data) => {
            app.metrics.threads.clear();
            for thread in data.threads {
                app.metrics.threads.push(crate::data::ThreadInfo {
                    pid: thread.tid,
                    name: thread.name,
                    cpu_percent: thread.cpu_percent,
                    mem_percent: 0.0,
                    core: None,
                    time: String::new(),
                });
            }
        }
        PanelUpdate::Alarms(data) => {
            app.alarms_data = data;
        }
        PanelUpdate::Jobs(data) => {
            app.jobs_data = data;
        }
        PanelUpdate::Processes(data) => {
            app.processes_data = data;
        }
        PanelUpdate::Devices(devices) => {
            app.devices.clear();
            for dev in devices {
                app.devices.push(crate::data::DeviceInfo {
                    serial: dev.serial,
                    model: dev.model,
                    brand: dev.brand,
                });
            }
            if app.device_serial.is_none() && !app.devices.is_empty() {
                app.device_serial = Some(app.devices[0].serial.clone());
            }
        }
        PanelUpdate::Packages(packages) => {
            app.packages = packages.clone();
            app.packages_lower = packages.iter().map(|p| p.to_lowercase()).collect();
            app.filter_packages();
        }
        PanelUpdate::Error { panel, message } => {
            app.last_error = Some(format!("[{}] {}", panel, message));
        }
    }
}
