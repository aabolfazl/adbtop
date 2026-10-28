use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, RwLock};
use tokio::time::MissedTickBehavior;
use tracing::info;

use crate::data::ThreadInfo;
use crate::shell::PersistentShell;

pub type SharedPackage = Arc<RwLock<Option<String>>>;

pub fn create_shared_package(initial: Option<String>) -> SharedPackage {
    Arc::new(RwLock::new(initial))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TrackingMode {
    #[default]
    Device,
    Package,
}

#[derive(Debug, Clone)]
pub enum PanelUpdate {
    Cpu(CpuData),
    Memory(MemoryData),
    Battery(BatteryData),
    Thermal(ThermalData),
    Network(NetworkData),
    Doze(DozeData),
    Threads(ThreadsData),
    Alarms(AlarmsData),
    Jobs(JobsData),
    Devices(Vec<DeviceData>),
    Packages(Vec<String>),
    /// Latest numbers from the on-device capture agent.
    Capture(Box<crate::agent::proto::Snapshot>),
    /// The agent stopped or went away.
    CaptureStopped,
    /// Starting or stopping the capture failed.
    CaptureError(String),
    Error { panel: &'static str, message: String },
}

#[derive(Debug, Clone, Default)]
pub struct CpuData {
    pub cores: Vec<CoreInfo>,
    pub total_usage: f32,
    /// Share of the whole CPU (all cores together = 100%) used by the tracked app.
    /// `None` until there are two samples of the same running process to compare.
    pub app_cpu_total: Option<f32>,
}

#[derive(Debug, Clone, Default)]
pub struct CoreInfo {
    pub id: u8,
    pub usage_percent: f32,
    pub frequency_mhz: u32,
}

#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct MemoryData {
    pub total_kb: u64,
    pub free_kb: u64,
    pub available_kb: u64,
    pub buffers_kb: u64,
    pub cached_kb: u64,
    pub swap_total_kb: u64,
    pub swap_free_kb: u64,
    pub app_pss_kb: u64,
    pub app_heap_size_kb: u64,
    pub app_heap_alloc_kb: u64,
}

#[derive(Debug, Clone, Default)]
pub struct BatteryData {
    pub level: u8,
    pub temperature: f32,
    pub voltage: u32,
    pub current: i32,
    pub health: String,
    pub status: String,
    pub technology: String,
    pub plugged: String,
}

#[derive(Debug, Clone, Default)]
pub struct ThermalData {
    pub cpu_temp: f32,
    pub battery_temp: f32,
    pub skin_temp: f32,
    pub usb_temp: f32,
}

#[derive(Debug, Clone, Default)]
pub struct NetworkData {
    pub wifi_rx_bytes: u64,
    pub wifi_tx_bytes: u64,
    pub mobile_rx_bytes: u64,
    pub mobile_tx_bytes: u64,
    pub wifi_connected: bool,
    pub mobile_connected: bool,
}

#[derive(Debug, Clone, Default)]
pub struct DozeData {
    pub device_idle_state: String,
    pub light_idle_state: String,
    pub app_standby_bucket: String,
}

#[derive(Debug, Clone, Default)]
pub struct ThreadsData {
    pub threads: Vec<ThreadInfo>,
}

#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct AlarmsData {
    pub total_pending: u32,
    pub app_pending: u32,
    pub next_alarm_time: String,
    pub next_alarm_package: String,
    pub top_alarm_packages: Vec<(String, u32)>,
}

#[derive(Debug, Clone, Default)]
pub struct JobsData {
    pub total_pending: u32,
    pub total_running: u32,
    pub app_pending: u32,
    pub app_running: u32,
    pub top_job_packages: Vec<(String, u32)>,
    pub running_packages: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct DeviceData {
    pub serial: String,
    pub model: String,
    pub brand: String,
}

pub struct RunnerConfig {
    pub device_serial: Option<String>,
    pub shared_package: SharedPackage,
    pub interval_secs: u64,
    /// Where to save the agent's packet capture, if anywhere.
    pub pcap_path: Option<PathBuf>,
}

/// The collectors for one device. Dropping the handle stops them and their adb shell.
pub struct RunnerHandle {
    pub device_serial: Option<String>,
    pub updates: mpsc::UnboundedReceiver<PanelUpdate>,
    /// For reporting results of one-off commands (such as starting the capture) to the UI.
    pub sender: mpsc::UnboundedSender<PanelUpdate>,
    handles: Vec<tokio::task::JoinHandle<()>>,
}

impl Drop for RunnerHandle {
    fn drop(&mut self) {
        for handle in &self.handles {
            handle.abort();
        }
    }
}

pub fn spawn_runners(config: RunnerConfig) -> RunnerHandle {
    info!("Starting all panel runners, device={:?}", config.device_serial);

    let (tx, updates) = mpsc::unbounded_channel();
    let shell = Arc::new(PersistentShell::spawn(config.device_serial.clone()));
    let pkg = config.shared_package;
    let period = Duration::from_secs(config.interval_secs.max(1));
    let slow_period = period * 2;

    let mut handles = vec![
        tokio::spawn(cpu_runner(tx.clone(), Arc::clone(&shell), Arc::clone(&pkg), period)),
        tokio::spawn(memory_runner(tx.clone(), Arc::clone(&shell), Arc::clone(&pkg), period)),
        tokio::spawn(battery_runner(tx.clone(), Arc::clone(&shell), period)),
        tokio::spawn(thermal_runner(tx.clone(), Arc::clone(&shell), period)),
        tokio::spawn(network_runner(tx.clone(), Arc::clone(&shell), period)),
        tokio::spawn(doze_runner(tx.clone(), Arc::clone(&shell), Arc::clone(&pkg), period)),
        tokio::spawn(threads_runner(tx.clone(), Arc::clone(&shell), Arc::clone(&pkg), period)),
        tokio::spawn(alarms_runner(tx.clone(), Arc::clone(&shell), Arc::clone(&pkg), slow_period)),
        tokio::spawn(jobs_runner(tx.clone(), Arc::clone(&shell), Arc::clone(&pkg), slow_period)),
        tokio::spawn(devices_runner(tx.clone())),
        tokio::spawn(packages_runner(tx.clone(), shell)),
        tokio::spawn(crate::agent::snapshot_runner(tx.clone(), config.device_serial.clone())),
    ];
    if let Some(path) = config.pcap_path {
        handles.push(tokio::spawn(crate::agent::pcap_runner(tx.clone(), config.device_serial.clone(), path)));
    }

    RunnerHandle {
        device_serial: config.device_serial,
        updates,
        sender: tx,
        handles,
    }
}

fn ticker(period: Duration) -> tokio::time::Interval {
    let mut interval = tokio::time::interval(period);
    // A slow adb round-trip should delay the next sample, not trigger a burst of catch-up samples.
    interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
    interval
}

async fn exec_or_report(
    shell: &PersistentShell,
    tx: &mpsc::UnboundedSender<PanelUpdate>,
    panel: &'static str,
    cmd: &str,
) -> Option<String> {
    match shell.exec(cmd).await {
        Ok(output) => Some(output),
        Err(e) => {
            let _ = tx.send(PanelUpdate::Error { panel, message: e.to_string() });
            None
        }
    }
}

async fn cpu_runner(tx: mpsc::UnboundedSender<PanelUpdate>, shell: Arc<PersistentShell>, shared_pkg: SharedPackage, period: Duration) {
    let mut interval = ticker(period);
    let mut state = CpuState::default();

    loop {
        interval.tick().await;

        let package = shared_pkg.read().await.clone();

        let mut cmd = String::from(
            r#"cat /proc/stat; echo '===FREQ==='; for c in /sys/devices/system/cpu/cpu[0-9]*; do echo "${c##*cpu} $(cat $c/cpufreq/scaling_cur_freq 2>/dev/null)"; done"#,
        );
        if let Some(ref pkg) = package {
            cmd.push_str(&format!("; echo '===APP==='; PID=$(pidof -s {}) && cat /proc/$PID/stat 2>/dev/null", pkg));
        }

        if let Some(output) = exec_or_report(&shell, &tx, "cpu", &cmd).await {
            // The first sample only sets the baseline; every usage in it would read 0%.
            let is_baseline = state.total.is_none();
            let data = parse_cpu_output(&output, &mut state);
            if !is_baseline {
                let _ = tx.send(PanelUpdate::Cpu(data));
            }
        }
    }
}

async fn memory_runner(tx: mpsc::UnboundedSender<PanelUpdate>, shell: Arc<PersistentShell>, shared_pkg: SharedPackage, period: Duration) {
    let mut interval = ticker(period);

    loop {
        interval.tick().await;

        let package = shared_pkg.read().await.clone();

        let cmd = if let Some(ref pkg) = package {
            format!("cat /proc/meminfo && echo '===APP===' && dumpsys meminfo {} 2>/dev/null", pkg)
        } else {
            "cat /proc/meminfo".to_string()
        };

        if let Some(output) = exec_or_report(&shell, &tx, "memory", &cmd).await {
            let data = parse_memory_output(&output);
            let _ = tx.send(PanelUpdate::Memory(data));
        }
    }
}

async fn battery_runner(tx: mpsc::UnboundedSender<PanelUpdate>, shell: Arc<PersistentShell>, period: Duration) {
    let mut interval = ticker(period);

    loop {
        interval.tick().await;

        if let Some(output) = exec_or_report(&shell, &tx, "battery", "dumpsys battery").await {
            let data = parse_battery_output(&output);
            let _ = tx.send(PanelUpdate::Battery(data));
        }
    }
}

async fn thermal_runner(tx: mpsc::UnboundedSender<PanelUpdate>, shell: Arc<PersistentShell>, period: Duration) {
    let mut interval = ticker(period);

    loop {
        interval.tick().await;

        if let Some(output) = exec_or_report(&shell, &tx, "thermal", "dumpsys thermalservice").await {
            let data = parse_thermal_output(&output);
            let _ = tx.send(PanelUpdate::Thermal(data));
        }
    }
}

async fn network_runner(tx: mpsc::UnboundedSender<PanelUpdate>, shell: Arc<PersistentShell>, period: Duration) {
    let mut interval = ticker(period);
    let cmd = "cat /proc/net/dev && echo '===WIFI===' && dumpsys wifi 2>/dev/null | grep 'Wi-Fi is' | head -1";

    loop {
        interval.tick().await;

        if let Some(output) = exec_or_report(&shell, &tx, "network", cmd).await {
            let data = parse_network_output(&output);
            let _ = tx.send(PanelUpdate::Network(data));
        }
    }
}

async fn doze_runner(tx: mpsc::UnboundedSender<PanelUpdate>, shell: Arc<PersistentShell>, shared_pkg: SharedPackage, period: Duration) {
    let mut interval = ticker(period);

    loop {
        interval.tick().await;

        let package = shared_pkg.read().await.clone();

        let cmd = if let Some(ref pkg) = package {
            format!("dumpsys deviceidle 2>/dev/null | grep -E '(mState|mLightState|State:)' | head -5 && echo '===BUCKET===' && am get-standby-bucket {} 2>/dev/null", pkg)
        } else {
            "dumpsys deviceidle 2>/dev/null | grep -E '(mState|mLightState|State:)' | head -5".to_string()
        };

        if let Some(output) = exec_or_report(&shell, &tx, "power", &cmd).await {
            let data = parse_doze_output(&output);
            let _ = tx.send(PanelUpdate::Doze(data));
        }
    }
}

async fn threads_runner(tx: mpsc::UnboundedSender<PanelUpdate>, shell: Arc<PersistentShell>, shared_pkg: SharedPackage, period: Duration) {
    let mut interval = ticker(period);
    let mut state = ThreadsState::default();

    loop {
        interval.tick().await;

        let package = shared_pkg.read().await.clone();

        // For an app, read every thread's /proc stat so CPU% is measured over the last
        // interval (ps %CPU is an average over the whole lifetime of the thread).
        let cmd = if let Some(ref pkg) = package {
            format!(
                "grep '^cpu' /proc/stat; PID=$(pidof -s {}) && cat /proc/$PID/task/*/stat 2>/dev/null || echo NO_PID",
                pkg
            )
        } else {
            "top -n 1 -b 2>/dev/null | tail -n +5 | head -40".to_string()
        };

        if let Some(output) = exec_or_report(&shell, &tx, "threads", &cmd).await {
            let data = if package.is_some() {
                parse_thread_stats(&output, &mut state)
            } else {
                state = ThreadsState::default();
                parse_top_processes(&output)
            };
            let _ = tx.send(PanelUpdate::Threads(data));
        }
    }
}

async fn alarms_runner(tx: mpsc::UnboundedSender<PanelUpdate>, shell: Arc<PersistentShell>, shared_pkg: SharedPackage, period: Duration) {
    let mut interval = ticker(period);

    loop {
        interval.tick().await;

        let package = shared_pkg.read().await.clone();

        let cmd = if let Some(ref pkg) = package {
            format!("dumpsys alarm 2>/dev/null | grep '{}' | head -50", pkg)
        } else {
            "dumpsys alarm 2>/dev/null | head -300".to_string()
        };

        if let Some(output) = exec_or_report(&shell, &tx, "alarms", &cmd).await {
            let data = parse_alarms_output(&output, package.as_deref());
            let _ = tx.send(PanelUpdate::Alarms(data));
        }
    }
}

async fn jobs_runner(tx: mpsc::UnboundedSender<PanelUpdate>, shell: Arc<PersistentShell>, shared_pkg: SharedPackage, period: Duration) {
    let mut interval = ticker(period);

    loop {
        interval.tick().await;

        let package = shared_pkg.read().await.clone();

        let cmd = if let Some(ref pkg) = package {
            format!("dumpsys jobscheduler 2>/dev/null | grep '{}' | head -50", pkg)
        } else {
            "dumpsys jobscheduler 2>/dev/null | head -500".to_string()
        };

        if let Some(output) = exec_or_report(&shell, &tx, "jobs", &cmd).await {
            let data = parse_jobs_output(&output, package.as_deref());
            let _ = tx.send(PanelUpdate::Jobs(data));
        }
    }
}

async fn devices_runner(tx: mpsc::UnboundedSender<PanelUpdate>) {
    use std::process::Stdio;
    use tokio::process::Command;

    let mut interval = ticker(Duration::from_secs(5));

    loop {
        interval.tick().await;

        if let Ok(output) = Command::new("adb")
            .arg("devices")
            .arg("-l")
            .stdout(Stdio::piped())
            .output()
            .await
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let devices = parse_devices_output(&stdout);
            let _ = tx.send(PanelUpdate::Devices(devices));
        }
    }
}

async fn packages_runner(tx: mpsc::UnboundedSender<PanelUpdate>, shell: Arc<PersistentShell>) {
    let mut interval = ticker(Duration::from_secs(5));

    loop {
        interval.tick().await;

        if let Some(output) = exec_or_report(&shell, &tx, "packages", "ps -A -o USER,NAME").await {
            let _ = tx.send(PanelUpdate::Packages(parse_packages_output(&output)));
        }
    }
}

/// Sums the time fields of a `/proc/stat` cpu line, returning `(total, idle)`.
fn stat_times(parts: &[&str]) -> Option<(u64, u64)> {
    if parts.len() < 5 {
        return None;
    }
    let field = |i: usize| -> u64 { parts.get(i).and_then(|s| s.parse().ok()).unwrap_or(0) };
    let total = (1..=7).map(field).sum();
    Some((total, field(4)))
}

#[derive(Default)]
struct CpuState {
    cores: HashMap<u8, (u64, u64)>,
    total: Option<u64>,
    app: Option<(u32, u64)>,
}

fn parse_cpu_output(output: &str, state: &mut CpuState) -> CpuData {
    let mut data = CpuData::default();

    let (stat_section, rest) = output.split_once("===FREQ===").unwrap_or((output, ""));
    let (freq_section, app_section) = match rest.split_once("===APP===") {
        Some((freq, app)) => (freq, Some(app)),
        None => (rest, None),
    };

    let mut cores = HashMap::new();
    let mut total_now = None;
    let mut total_usage = 0.0f32;

    for line in stat_section.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        let Some(name) = parts.first() else { continue };
        let Some((total, idle)) = stat_times(&parts) else { continue };

        if *name == "cpu" {
            total_now = Some(total);
            continue;
        }
        let Some(id) = name.strip_prefix("cpu").and_then(|s| s.parse::<u8>().ok()) else { continue };

        // Offline cores are missing from /proc/stat, so samples are matched by core id, not position.
        let usage = match state.cores.get(&id) {
            Some(&(prev_total, prev_idle)) => {
                let dt = total.saturating_sub(prev_total);
                let di = idle.saturating_sub(prev_idle);
                if dt > 0 { (dt.saturating_sub(di)) as f32 / dt as f32 * 100.0 } else { 0.0 }
            }
            None => 0.0,
        };

        cores.insert(id, (total, idle));
        total_usage += usage;
        data.cores.push(CoreInfo { id, usage_percent: usage, frequency_mhz: 0 });
    }

    data.total_usage = if data.cores.is_empty() { 0.0 } else { total_usage / data.cores.len() as f32 };

    for line in freq_section.lines() {
        let mut parts = line.split_whitespace();
        let id = parts.next().and_then(|s| s.parse::<u8>().ok());
        let freq = parts.next().and_then(|s| s.parse::<u32>().ok());
        if let (Some(id), Some(freq)) = (id, freq) {
            if let Some(core) = data.cores.iter_mut().find(|c| c.id == id) {
                core.frequency_mhz = freq / 1000;
            }
        }
    }

    let app_now = app_section.and_then(|s| s.lines().find_map(parse_task_stat));
    if let (Some(app), Some(now), Some(prev_total), Some((prev_pid, prev_ticks))) = (&app_now, total_now, state.total, state.app) {
        let dt = now.saturating_sub(prev_total);
        if app.id == prev_pid && dt > 0 {
            data.app_cpu_total = Some(app.ticks.saturating_sub(prev_ticks) as f32 / dt as f32 * 100.0);
        }
    }

    state.cores = cores;
    state.total = total_now;
    state.app = app_now.map(|app| (app.id, app.ticks));

    data
}

fn parse_memory_output(output: &str) -> MemoryData {
    let mut data = MemoryData::default();
    let parts: Vec<&str> = output.split("===APP===").collect();
    let meminfo = parts.first().unwrap_or(&"");

    for line in meminfo.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 {
            let value: u64 = parts[1].parse().unwrap_or(0);
            match parts[0] {
                "MemTotal:" => data.total_kb = value,
                "MemFree:" => data.free_kb = value,
                "MemAvailable:" => data.available_kb = value,
                "Buffers:" => data.buffers_kb = value,
                "Cached:" => data.cached_kb = value,
                "SwapTotal:" => data.swap_total_kb = value,
                "SwapFree:" => data.swap_free_kb = value,
                _ => {}
            }
        }
    }

    if let Some(app_section) = parts.get(1) {
        for line in app_section.lines() {
            let Some(rest) = line.trim().strip_prefix("TOTAL") else { continue };
            let nums: Vec<u64> = rest.split_whitespace().filter_map(|s| s.parse().ok()).collect();

            // Table row: "TOTAL  <Pss> <Private Dirty> ... <Heap Size> <Heap Alloc> <Heap Free>"
            if rest.starts_with(char::is_whitespace) && nums.len() >= 7 {
                data.app_pss_kb = nums[0];
                data.app_heap_size_kb = nums[nums.len() - 3];
                data.app_heap_alloc_kb = nums[nums.len() - 2];
                break;
            }

            // App summary: "TOTAL PSS:  56254  TOTAL RSS: ..." (older releases print "TOTAL:")
            let rest = rest.trim_start();
            if let Some(value) = rest.strip_prefix("PSS:").or_else(|| rest.strip_prefix(':')) {
                if let Some(pss) = value.split_whitespace().next().and_then(|s| s.parse().ok()) {
                    data.app_pss_kb = pss;
                }
            }
        }
    }

    data
}

fn parse_battery_output(output: &str) -> BatteryData {
    let mut data = BatteryData::default();

    for line in output.lines() {
        let line = line.trim();
        if let Some((key, value)) = line.split_once(':') {
            let value = value.trim();
            match key.trim() {
                "level" => data.level = value.parse().unwrap_or(0),
                "temperature" => data.temperature = value.parse::<i32>().unwrap_or(0) as f32 / 10.0,
                "voltage" => data.voltage = value.parse().unwrap_or(0),
                "current now" => data.current = value.parse().unwrap_or(0),
                "health" => data.health = value.to_string(),
                "status" => data.status = match value {
                    "1" => "Unknown".to_string(),
                    "2" => "Charging".to_string(),
                    "3" => "Discharging".to_string(),
                    "4" => "Not charging".to_string(),
                    "5" => "Full".to_string(),
                    _ => value.to_string(),
                },
                "technology" => data.technology = value.to_string(),
                "AC powered" | "USB powered" | "Wireless powered" => {
                    if value == "true" {
                        data.plugged = key.replace(" powered", "");
                    }
                }
                _ => {}
            }
        }
    }

    if data.plugged.is_empty() {
        data.plugged = "None".to_string();
    }

    data
}

fn parse_thermal_output(output: &str) -> ThermalData {
    let mut data = ThermalData::default();

    let extract_temp = |name: &str| -> f32 {
        output
            .lines()
            .find(|l| l.contains(&format!("mName={}", name)))
            .and_then(|l| {
                if let Some(start) = l.find("mValue=") {
                    let rest = &l[start + 7..];
                    rest.split(|c: char| !c.is_ascii_digit() && c != '.' && c != '-')
                        .next()
                        .and_then(|v| v.parse::<f32>().ok())
                } else {
                    None
                }
            })
            .unwrap_or(0.0)
    };

    data.cpu_temp = extract_temp("CPU");
    if data.cpu_temp == 0.0 { data.cpu_temp = extract_temp("AP"); }
    data.battery_temp = extract_temp("BAT");
    data.skin_temp = extract_temp("SKIN");
    data.usb_temp = extract_temp("USB");

    data
}


fn parse_network_output(output: &str) -> NetworkData {
    let mut data = NetworkData::default();
    let sections: Vec<&str> = output.split("===").collect();

    let net_dev = sections.first().unwrap_or(&"");
    for line in net_dev.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 10 {
            let iface = parts[0].trim_end_matches(':');
            let rx_bytes: u64 = parts[1].parse().unwrap_or(0);
            let tx_bytes: u64 = parts[9].parse().unwrap_or(0);

            if iface.starts_with("wlan") {
                data.wifi_rx_bytes += rx_bytes;
                data.wifi_tx_bytes += tx_bytes;
            } else if iface.starts_with("rmnet") || iface.starts_with("ccmni") || iface.starts_with("swlan") {
                data.mobile_rx_bytes += rx_bytes;
                data.mobile_tx_bytes += tx_bytes;
            }
        }
    }

    for section in &sections {
        if section.contains("Wi-Fi is") {
            data.wifi_connected = section.contains("enabled") || section.contains("connected");
        }
    }

    if data.wifi_rx_bytes > 0 || data.wifi_tx_bytes > 0 {
        data.wifi_connected = true;
    }
    if data.mobile_rx_bytes > 0 || data.mobile_tx_bytes > 0 {
        data.mobile_connected = true;
    }

    data
}

fn parse_doze_output(output: &str) -> DozeData {
    let mut data = DozeData::default();
    let (doze_section, bucket_section) = match output.split_once("===BUCKET===") {
        Some((doze, bucket)) => (doze, Some(bucket)),
        None => (output, None),
    };

    // Both states are usually printed on one line: "mState=ACTIVE mLightState=ACTIVE"
    for token in doze_section.split_whitespace() {
        if let Some(state) = token.strip_prefix("mState=") {
            if data.device_idle_state.is_empty() {
                data.device_idle_state = state.to_string();
            }
        } else if let Some(state) = token.strip_prefix("mLightState=") {
            if data.light_idle_state.is_empty() {
                data.light_idle_state = state.to_string();
            }
        }
    }

    if let Some(bucket_section) = bucket_section {
        let bucket = bucket_section.trim().lines().next().unwrap_or("").trim();
        if !bucket.is_empty() && !bucket.contains("Error") {
            data.app_standby_bucket = standby_bucket_name(bucket);
        }
    }

    if data.device_idle_state.is_empty() { data.device_idle_state = "ACTIVE".to_string(); }
    if data.light_idle_state.is_empty() { data.light_idle_state = "ACTIVE".to_string(); }
    if data.app_standby_bucket.is_empty() { data.app_standby_bucket = "UNKNOWN".to_string(); }

    data
}

/// `am get-standby-bucket` prints the numeric bucket from UsageStatsManager.
fn standby_bucket_name(bucket: &str) -> String {
    match bucket {
        "5" => "EXEMPTED",
        "10" => "ACTIVE",
        "20" => "WORKING_SET",
        "30" => "FREQUENT",
        "40" => "RARE",
        "45" => "RESTRICTED",
        "50" => "NEVER",
        other => other,
    }
    .to_string()
}

struct TaskStat {
    id: u32,
    name: String,
    ticks: u64,
    processor: Option<u8>,
}

/// Parses one `/proc/<pid>/stat` or `/proc/<pid>/task/<tid>/stat` line.
fn parse_task_stat(line: &str) -> Option<TaskStat> {
    // The name is wrapped in parentheses and may itself contain spaces or ')'.
    let open = line.find('(')?;
    let close = line.rfind(')')?;
    if close < open {
        return None;
    }
    let id = line[..open].trim().parse().ok()?;
    let name = line[open + 1..close].to_string();
    // Fields after the name start at field 3 (state) of proc(5).
    let fields: Vec<&str> = line[close + 1..].split_whitespace().collect();
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    let processor = fields.get(36).and_then(|s| s.parse().ok());
    Some(TaskStat { id, name, ticks: utime + stime, processor })
}

/// Formats clock ticks (USER_HZ = 100) like top's TIME+ column.
fn format_cpu_time(ticks: u64) -> String {
    let secs = ticks / 100;
    format!("{}:{:02}.{:02}", secs / 60, secs % 60, ticks % 100)
}

#[derive(Default)]
struct ThreadsState {
    total: Option<u64>,
    ticks: HashMap<u32, u64>,
}

fn parse_thread_stats(output: &str, state: &mut ThreadsState) -> ThreadsData {
    let mut data = ThreadsData::default();
    let mut total_now = None;
    let mut cores = 0usize;
    let mut tasks = Vec::new();

    for line in output.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        match parts.first() {
            Some(&"cpu") => total_now = stat_times(&parts).map(|(total, _)| total),
            Some(name) if name.starts_with("cpu") => cores += 1,
            _ => tasks.extend(parse_task_stat(line)),
        }
    }

    // Ticks that elapsed on a single core, so a thread that keeps one core busy shows 100%.
    let elapsed_per_core = match (total_now, state.total) {
        (Some(now), Some(prev)) if cores > 0 => now.saturating_sub(prev) as f32 / cores as f32,
        _ => 0.0,
    };

    let mut ticks_now = HashMap::new();
    for task in tasks {
        let cpu = match state.ticks.get(&task.id) {
            Some(&prev) if elapsed_per_core > 0.0 => {
                (task.ticks.saturating_sub(prev) as f32 / elapsed_per_core * 100.0).min(100.0)
            }
            _ => 0.0,
        };
        ticks_now.insert(task.id, task.ticks);
        data.threads.push(ThreadInfo {
            pid: task.id,
            name: task.name,
            cpu_percent: cpu,
            mem_percent: None,
            core: task.processor,
            time: format_cpu_time(task.ticks),
        });
    }

    state.total = total_now;
    state.ticks = ticks_now;

    sort_by_cpu(&mut data.threads);
    data
}

/// Parses `top -b` rows: PID USER PR NI VIRT RES SHR S %CPU %MEM TIME+ ARGS
fn parse_top_processes(output: &str) -> ThreadsData {
    let mut data = ThreadsData::default();

    for line in output.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 12 {
            continue;
        }
        let Ok(pid) = parts[0].parse::<u32>() else { continue };
        data.threads.push(ThreadInfo {
            pid,
            name: parts[11..].join(" "),
            cpu_percent: parts[8].parse().unwrap_or(0.0),
            mem_percent: parts[9].parse().ok(),
            core: None,
            time: parts[10].to_string(),
        });
    }

    sort_by_cpu(&mut data.threads);
    data
}

fn sort_by_cpu(threads: &mut Vec<ThreadInfo>) {
    threads.sort_by(|a, b| b.cpu_percent.partial_cmp(&a.cpu_percent).unwrap_or(std::cmp::Ordering::Equal));
    threads.truncate(50);
}

fn extract_package_name(line: &str) -> Option<String> {
    for (i, _) in line.match_indices('/') {
        let rest = &line[i + 1..];
        let candidate: String = rest.chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '_')
            .collect();
        let candidate = candidate.trim_end_matches('.');
        if candidate.chars().filter(|c| *c == '.').count() >= 2 && candidate.len() > 5 {
            return Some(candidate.to_string());
        }
    }
    for prefix in &["pkg=", "pkg "] {
        if let Some(pos) = line.find(prefix) {
            let rest = &line[pos + prefix.len()..];
            let candidate: String = rest.chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '_')
                .collect();
            let candidate = candidate.trim_end_matches('.');
            if candidate.contains('.') && candidate.len() > 3 {
                return Some(candidate.to_string());
            }
        }
    }
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_lowercase() {
            let start = i;
            let mut dots = 0;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '.' || chars[i] == '_') {
                if chars[i] == '.' { dots += 1; }
                i += 1;
            }
            if dots >= 2 {
                let candidate: String = chars[start..i].iter().collect();
                let candidate = candidate.trim_end_matches('.');
                if candidate.split('.').all(|s| !s.is_empty()) && candidate.len() > 5 {
                    return Some(candidate.to_string());
                }
            }
        }
        i += 1;
    }
    None
}

fn parse_alarms_output(output: &str, package: Option<&str>) -> AlarmsData {
    let mut data = AlarmsData::default();
    let mut pkg_counts: std::collections::HashMap<String, u32> = std::collections::HashMap::new();

    for line in output.lines() {
        let line = line.trim();

        if line.contains("Pending alarm batches") {
            if let Some(num_str) = line.split(':').last() {
                if let Ok(num) = num_str.trim().parse::<u32>() {
                    data.total_pending = num;
                }
            }
        }

        if line.contains("alarms") {
            for word in line.split_whitespace() {
                if let Ok(num) = word.parse::<u32>() {
                    if num > data.total_pending {
                        data.total_pending = num;
                    }
                    break;
                }
            }
        }

        if line.contains("Batch") || line.contains("batch") {
            for word in line.split_whitespace() {
                if let Ok(num) = word.parse::<u32>() {
                    if data.total_pending == 0 {
                        data.total_pending = num;
                    }
                    break;
                }
            }
        }

        if let Some(pkg) = extract_package_name(line) {
            *pkg_counts.entry(pkg).or_insert(0) += 1;
        }

        if let Some(filter_pkg) = package {
            if line.contains(filter_pkg) {
                data.app_pending += 1;
            }
        }
    }

    let mut sorted: Vec<(String, u32)> = pkg_counts.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1));
    sorted.truncate(8);
    data.top_alarm_packages = sorted;

    data
}

fn parse_jobs_output(output: &str, package: Option<&str>) -> JobsData {
    let mut data = JobsData::default();
    let mut pkg_counts: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    let mut in_running = false;
    let mut running_set: std::collections::HashSet<String> = std::collections::HashSet::new();

    for line in output.lines() {
        let trimmed = line.trim();

        if trimmed.contains("Registered") && trimmed.contains("jobs") {
            for word in trimmed.split_whitespace() {
                if let Ok(num) = word.parse::<u32>() {
                    data.total_pending = num;
                    break;
                }
            }
            in_running = false;
            continue;
        }

        if trimmed.contains("Currently running") {
            in_running = true;
            if let Some(rest) = trimmed.split(':').last() {
                let rest = rest.trim();
                if rest == "none" || rest == "0" || rest.is_empty() {
                    data.total_running = 0;
                } else if let Ok(num) = rest.parse::<u32>() {
                    data.total_running = num;
                } else {
                    data.total_running = rest.split(',').count() as u32;
                }
            }
            if let Some(pkg) = extract_package_name(trimmed) {
                running_set.insert(pkg.clone());
                *pkg_counts.entry(pkg).or_insert(0) += 1;
            }
            continue;
        }

        if trimmed.contains("pending") || trimmed.contains("Pending") {
            for word in trimmed.split_whitespace() {
                if let Ok(num) = word.parse::<u32>() {
                    if data.total_pending == 0 {
                        data.total_pending = num;
                    }
                    break;
                }
            }
        }

        if let Some(pkg) = extract_package_name(trimmed) {
            *pkg_counts.entry(pkg.clone()).or_insert(0) += 1;
            if in_running {
                running_set.insert(pkg);
            }
        } else if in_running && trimmed.is_empty() {
            in_running = false;
        }

        if let Some(filter_pkg) = package {
            if trimmed.contains(filter_pkg) {
                data.app_pending += 1;
            }
        }
    }

    if !running_set.is_empty() && data.total_running == 0 {
        data.total_running = running_set.len() as u32;
    }

    let mut sorted: Vec<(String, u32)> = pkg_counts.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1));
    sorted.truncate(8);
    data.top_job_packages = sorted;
    data.running_packages = running_set.into_iter().collect();

    data
}


fn parse_devices_output(output: &str) -> Vec<DeviceData> {
    let mut devices = Vec::new();

    for line in output.lines().skip(1) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 && parts[1] == "device" {
            let serial = parts[0].to_string();
            let mut model = String::new();
            let mut brand = String::new();

            for part in &parts[2..] {
                if part.starts_with("model:") {
                    model = part.strip_prefix("model:").unwrap_or("").to_string();
                } else if part.starts_with("device:") {
                    brand = part.strip_prefix("device:").unwrap_or("").to_string();
                }
            }

            devices.push(DeviceData { serial, model, brand });
        }
    }

    devices
}

/// Running app processes (users like `u0_a123`), reduced to their package names.
fn parse_packages_output(output: &str) -> Vec<String> {
    let mut packages = BTreeSet::new();

    for line in output.lines() {
        let mut parts = line.split_whitespace();
        let (Some(user), Some(name)) = (parts.next(), parts.next()) else { continue };
        if !is_app_user(user) {
            continue;
        }
        // "com.example.app:remote" is a secondary process of com.example.app.
        let package = name.split(':').next().unwrap_or(name);
        if is_valid_package_name(package) {
            packages.insert(package.to_string());
        }
    }

    packages.into_iter().collect()
}

fn is_app_user(user: &str) -> bool {
    let is_number = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    user.strip_prefix('u')
        .and_then(|rest| rest.split_once("_a"))
        .is_some_and(|(user_id, app_id)| is_number(user_id) && is_number(app_id))
}

/// Package names end up inside adb shell commands, so only allow what Android allows.
pub fn is_valid_package_name(name: &str) -> bool {
    name.contains('.')
        && name
            .split('.')
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A /proc/<pid>/stat line with utime (field 14) and processor (field 39) set.
    fn stat_line(id: u32, name: &str, utime: u64, processor: u8) -> String {
        let mut fields = vec!["0".to_string(); 37];
        fields[0] = "S".to_string();
        fields[11] = utime.to_string();
        fields[36] = processor.to_string();
        format!("{id} ({name}) {}", fields.join(" "))
    }

    const STAT_1: &str = "cpu  100 0 100 800 0 0 0 0 0 0\n\
cpu0 50 0 50 400 0 0 0 0 0 0\n\
cpu2 50 0 50 400 0 0 0 0 0 0\n\
intr 1 2 3\n";
    const STAT_2: &str = "cpu  200 0 200 1000 0 0 0 0 0 0\n\
cpu0 100 0 100 500 0 0 0 0 0 0\n\
cpu2 100 0 100 500 0 0 0 0 0 0\n";

    #[test]
    fn cpu_usage_is_matched_by_core_id_when_cores_are_offline() {
        let mut state = CpuState::default();
        parse_cpu_output(STAT_1, &mut state);
        let data = parse_cpu_output(STAT_2, &mut state);

        let ids: Vec<u8> = data.cores.iter().map(|c| c.id).collect();
        assert_eq!(ids, vec![0, 2]);
        // Each core: 100 busy ticks out of 200.
        assert!((data.cores[1].usage_percent - 50.0).abs() < 0.01);
    }

    #[test]
    fn cpu_frequencies_are_mapped_by_core_id() {
        let mut state = CpuState::default();
        let output = format!("{STAT_1}===FREQ===\n0 1804800\n10 \n2 2400000\n");
        let data = parse_cpu_output(&output, &mut state);
        assert_eq!(data.cores[0].frequency_mhz, 1804);
        assert_eq!(data.cores[1].frequency_mhz, 2400);
    }

    #[test]
    fn app_cpu_can_use_more_than_one_core() {
        let app = |ticks: u64| format!("===APP===\n{}\n", stat_line(4321, "com.example.app", ticks, 3));
        let mut state = CpuState::default();
        parse_cpu_output(&format!("{STAT_1}===FREQ===\n{}", app(1000)), &mut state);
        // 300 of the 400 ticks across both cores (1.5 cores busy).
        let data = parse_cpu_output(&format!("{STAT_2}===FREQ===\n{}", app(1300)), &mut state);
        let app_cpu = data.app_cpu_total.unwrap();
        assert!((app_cpu - 75.0).abs() < 0.01, "{}", app_cpu);
    }

    #[test]
    fn app_cpu_resets_when_the_process_restarts() {
        let app = |pid: u32, ticks: u64| format!("===APP===\n{}\n", stat_line(pid, "com.example.app", ticks, 3));
        let mut state = CpuState::default();
        parse_cpu_output(&format!("{STAT_1}===FREQ===\n{}", app(1, 1000)), &mut state);
        let data = parse_cpu_output(&format!("{STAT_2}===FREQ===\n{}", app(2, 10)), &mut state);
        assert_eq!(data.app_cpu_total, None);
    }

    #[test]
    fn meminfo_reads_totals_and_heap_from_the_table() {
        let output = "MemTotal: 8000000 kB\nMemAvailable: 2000000 kB\n===APP===\n\
Applications Memory Usage (in Kilobytes):\n\
** MEMINFO in pid 4321 [com.example.app] **\n\
                   Pss  Private  Private  SwapPss      Rss     Heap     Heap     Heap\n\
                 Total    Dirty    Clean    Dirty    Total     Size    Alloc     Free\n\
                ------   ------   ------   ------   ------   ------   ------   ------\n\
  Native Heap    10520    10456        0       77    12868    18432    13730     4701\n\
  Dalvik Heap     3148     3044        0       45     8028     9432     4716     4716\n\
        TOTAL    56254    25228    16728      191   121744    27864    18446     9417\n\
 App Summary\n\
           TOTAL PSS:    56254            TOTAL RSS:   121744       TOTAL SWAP PSS:      191\n";
        let data = parse_memory_output(output);
        assert_eq!(data.total_kb, 8000000);
        assert_eq!(data.app_pss_kb, 56254);
        assert_eq!(data.app_heap_size_kb, 27864);
        assert_eq!(data.app_heap_alloc_kb, 18446);
    }

    #[test]
    fn meminfo_without_rss_column() {
        let output = "===APP===\n        TOTAL    40000    20000    10000      100    27864    18446     9417\n";
        let data = parse_memory_output(output);
        assert_eq!(data.app_pss_kb, 40000);
        assert_eq!(data.app_heap_alloc_kb, 18446);
    }

    #[test]
    fn meminfo_for_a_stopped_app() {
        let data = parse_memory_output("MemTotal: 1 kB\n===APP===\nNo process found for: com.example.app\n");
        assert_eq!(data.app_pss_kb, 0);
    }

    #[test]
    fn doze_states_on_one_line_are_parsed_separately() {
        let data = parse_doze_output("  mState=IDLE mLightState=OVERRIDE\n===BUCKET===\n10\n");
        assert_eq!(data.device_idle_state, "IDLE");
        assert_eq!(data.light_idle_state, "OVERRIDE");
        assert_eq!(data.app_standby_bucket, "ACTIVE");
    }

    #[test]
    fn thread_cpu_is_measured_between_samples() {
        let sample = |total: u64, t1: u64, t2: u64| {
            format!(
                "cpu  0 0 0 {total} 0 0 0\ncpu0 1 1 1 1\ncpu1 1 1 1 1\n{}\n{}\n",
                stat_line(100, "main", t1, 1),
                stat_line(101, "Render (Thread)", t2, 0)
            )
        };
        let mut state = ThreadsState::default();
        parse_thread_stats(&sample(1000, 500, 0), &mut state);
        // 200 ticks on 2 cores = 100 ticks per core.
        let data = parse_thread_stats(&sample(1200, 550, 100), &mut state);

        assert_eq!(data.threads[0].name, "Render (Thread)");
        assert!((data.threads[0].cpu_percent - 100.0).abs() < 0.01);
        assert_eq!(data.threads[0].core, Some(0));
        assert!((data.threads[1].cpu_percent - 50.0).abs() < 0.01);
        assert_eq!(data.threads[1].time, "0:05.50");
    }

    #[test]
    fn top_rows_keep_cpu_above_100_percent() {
        let output = "  PID USER         PR  NI VIRT  RES  SHR S[%CPU] %MEM     TIME+ ARGS\n\
 4125 u0_a181      10 -10  15G 241M 149M S 150.0   2.1   1:23.45 com.example.app\n\
  812 system       18  -2  14G 200M 150M S  4.0   1.9  10:01.00 system_server\n";
        let data = parse_top_processes(output);
        assert_eq!(data.threads.len(), 2);
        assert_eq!(data.threads[0].pid, 4125);
        assert_eq!(data.threads[0].cpu_percent, 150.0);
        assert_eq!(data.threads[0].mem_percent, Some(2.1));
        assert_eq!(data.threads[0].time, "1:23.45");
    }

    #[test]
    fn packages_are_app_processes_only() {
        let output = "USER NAME\nroot init\nu0_a101 com.example.app\nu0_a101 com.example.app:remote\n\
u10_a150 com.whatsapp\nu0_a90 com.google.android.gms\naudioserver android.hardware.audio.service\n\
u0_i5 com.android.chrome:sandboxed_process0\n";
        assert_eq!(
            parse_packages_output(output),
            vec!["com.example.app", "com.google.android.gms", "com.whatsapp"]
        );
    }

    #[test]
    fn package_name_validation() {
        assert!(is_valid_package_name("com.example.app_2"));
        assert!(!is_valid_package_name("app"));
        assert!(!is_valid_package_name("com.example'; reboot; '"));
        assert!(!is_valid_package_name("com..app"));
    }
}
