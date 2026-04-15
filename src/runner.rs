use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, RwLock};
use tracing::{error, info};

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
    Processes(ProcessesData),
    Devices(Vec<DeviceData>),
    Packages(Vec<String>),
    Error { panel: &'static str, message: String },
}

#[derive(Debug, Clone, Default)]
pub struct CpuData {
    pub cores: Vec<CoreInfo>,
    pub total_usage: f32,
    pub app_cpu_total: f32,
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
    pub app_private_dirty_kb: u64,
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
    pub app_rx_bytes: u64,
    pub app_tx_bytes: u64,
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
pub struct ThreadInfo {
    pub tid: u32,
    pub name: String,
    pub cpu_percent: f32,
    pub state: String,
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

#[derive(Debug, Clone, Default)]
pub struct ProcessesData {
    pub processes: Vec<ProcessInfo>,
}

#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct ProcessInfo {
    pub pid: u32,
    pub user: String,
    pub cpu_percent: f32,
    pub mem_percent: f32,
    pub name: String,
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
    pub update_tx: mpsc::UnboundedSender<PanelUpdate>,
}

pub struct RunnerHandle {
    handles: Vec<tokio::task::JoinHandle<()>>,
}

impl RunnerHandle {
    pub fn abort_all(&self) {
        for handle in &self.handles {
            handle.abort();
        }
    }
}

pub fn spawn_runners(config: RunnerConfig) -> RunnerHandle {
    info!("Starting all panel runners, device={:?}", config.device_serial);

    let update_tx = config.update_tx.clone();
    let device = config.device_serial.clone();
    let shared_pkg = config.shared_package.clone();

    let shell = match PersistentShell::spawn(device.clone()) {
        Ok(s) => {
            info!("Persistent shell created successfully");
            Arc::new(s)
        }
        Err(e) => {
            error!("Failed to create shell: {}", e);
            let _ = update_tx.send(PanelUpdate::Error {
                panel: "shell",
                message: format!("Failed to create shell: {}", e),
            });
            return RunnerHandle { handles: vec![] };
        }
    };

    let mut handles = Vec::new();

    {
        let tx = update_tx.clone();
        let sh = Arc::clone(&shell);
        let pkg = Arc::clone(&shared_pkg);
        handles.push(tokio::spawn(async move {
            cpu_runner(tx, sh, pkg).await;
        }));
    }

    {
        let tx = update_tx.clone();
        let sh = Arc::clone(&shell);
        let pkg = Arc::clone(&shared_pkg);
        handles.push(tokio::spawn(async move {
            memory_runner(tx, sh, pkg).await;
        }));
    }

    {
        let tx = update_tx.clone();
        let sh = Arc::clone(&shell);
        handles.push(tokio::spawn(async move {
            battery_runner(tx, sh).await;
        }));
    }

    {
        let tx = update_tx.clone();
        let sh = Arc::clone(&shell);
        handles.push(tokio::spawn(async move {
            thermal_runner(tx, sh).await;
        }));
    }

    {
        let tx = update_tx.clone();
        let sh = Arc::clone(&shell);
        let pkg = Arc::clone(&shared_pkg);
        handles.push(tokio::spawn(async move {
            network_runner(tx, sh, pkg).await;
        }));
    }

    {
        let tx = update_tx.clone();
        let sh = Arc::clone(&shell);
        let pkg = Arc::clone(&shared_pkg);
        handles.push(tokio::spawn(async move {
            doze_runner(tx, sh, pkg).await;
        }));
    }

    {
        let tx = update_tx.clone();
        let sh = Arc::clone(&shell);
        let pkg = Arc::clone(&shared_pkg);
        handles.push(tokio::spawn(async move {
            threads_runner(tx, sh, pkg).await;
        }));
    }

    {
        let tx = update_tx.clone();
        let sh = Arc::clone(&shell);
        let pkg = Arc::clone(&shared_pkg);
        handles.push(tokio::spawn(async move {
            alarms_runner(tx, sh, pkg).await;
        }));
    }

    {
        let tx = update_tx.clone();
        let sh = Arc::clone(&shell);
        let pkg = Arc::clone(&shared_pkg);
        handles.push(tokio::spawn(async move {
            jobs_runner(tx, sh, pkg).await;
        }));
    }

    {
        let tx = update_tx.clone();
        let sh = Arc::clone(&shell);
        let pkg = Arc::clone(&shared_pkg);
        handles.push(tokio::spawn(async move {
            processes_runner(tx, sh, pkg).await;
        }));
    }

    {
        let tx = update_tx.clone();
        handles.push(tokio::spawn(async move {
            devices_runner(tx).await;
        }));
    }

    {
        let tx = update_tx.clone();
        let sh = Arc::clone(&shell);
        handles.push(tokio::spawn(async move {
            packages_runner(tx, sh).await;
        }));
    }

    RunnerHandle { handles }
}

async fn cpu_runner(tx: mpsc::UnboundedSender<PanelUpdate>, shell: Arc<PersistentShell>, shared_pkg: SharedPackage) {
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    let mut prev_stats: Vec<(u64, u64)> = Vec::new();

    loop {
        interval.tick().await;

        let package = shared_pkg.read().await.clone();

        let cmd = if let Some(ref pkg) = package {
            format!(
                r#"cat /proc/stat && echo '===FREQ===' && for c in $(seq 0 15); do cat /sys/devices/system/cpu/cpu${{c}}/cpufreq/scaling_cur_freq 2>/dev/null || echo 0; done && echo '===APP===' && top -n 1 -b 2>/dev/null | grep '{}' | head -1"#,
                pkg
            )
        } else {
            r#"cat /proc/stat && echo '===FREQ===' && for c in $(seq 0 15); do cat /sys/devices/system/cpu/cpu${c}/cpufreq/scaling_cur_freq 2>/dev/null || echo 0; done"#.to_string()
        };

        if let Ok(output) = shell.exec(&cmd).await {
            let data = parse_cpu_output(&output, &mut prev_stats);
            let _ = tx.send(PanelUpdate::Cpu(data));
        }
    }
}

async fn memory_runner(tx: mpsc::UnboundedSender<PanelUpdate>, shell: Arc<PersistentShell>, shared_pkg: SharedPackage) {
    let mut interval = tokio::time::interval(Duration::from_secs(1));

    loop {
        interval.tick().await;

        let package = shared_pkg.read().await.clone();

        let cmd = if let Some(ref pkg) = package {
            format!("cat /proc/meminfo && echo '===APP===' && dumpsys meminfo {} 2>/dev/null | head -20", pkg)
        } else {
            "cat /proc/meminfo".to_string()
        };

        if let Ok(output) = shell.exec(&cmd).await {
            let data = parse_memory_output(&output);
            let _ = tx.send(PanelUpdate::Memory(data));
        }
    }
}

async fn battery_runner(tx: mpsc::UnboundedSender<PanelUpdate>, shell: Arc<PersistentShell>) {
    let mut interval = tokio::time::interval(Duration::from_secs(1));

    loop {
        interval.tick().await;

        if let Ok(output) = shell.exec("dumpsys battery").await {
            let data = parse_battery_output(&output);
            let _ = tx.send(PanelUpdate::Battery(data));
        }
    }
}

async fn thermal_runner(tx: mpsc::UnboundedSender<PanelUpdate>, shell: Arc<PersistentShell>) {
    let mut interval = tokio::time::interval(Duration::from_secs(1));

    loop {
        interval.tick().await;

        if let Ok(output) = shell.exec("dumpsys thermalservice").await {
            let data = parse_thermal_output(&output);
            let _ = tx.send(PanelUpdate::Thermal(data));
        }
    }
}

async fn network_runner(tx: mpsc::UnboundedSender<PanelUpdate>, shell: Arc<PersistentShell>, shared_pkg: SharedPackage) {
    let mut interval = tokio::time::interval(Duration::from_secs(1));

    loop {
        interval.tick().await;

        let package = shared_pkg.read().await.clone();

        let cmd = if let Some(ref pkg) = package {
            format!(
                "cat /proc/net/dev && echo '===WIFI===' && dumpsys wifi 2>/dev/null | grep 'Wi-Fi is' | head -1 && echo '===UID===' && dumpsys netstats --uid 2>/dev/null | grep -A5 '{}' | head -10",
                pkg
            )
        } else {
            "cat /proc/net/dev && echo '===WIFI===' && dumpsys wifi 2>/dev/null | grep 'Wi-Fi is' | head -1".to_string()
        };

        if let Ok(output) = shell.exec(&cmd).await {
            let data = parse_network_output(&output);
            let _ = tx.send(PanelUpdate::Network(data));
        }
    }
}

async fn doze_runner(tx: mpsc::UnboundedSender<PanelUpdate>, shell: Arc<PersistentShell>, shared_pkg: SharedPackage) {
    let mut interval = tokio::time::interval(Duration::from_secs(1));

    loop {
        interval.tick().await;

        let package = shared_pkg.read().await.clone();

        let cmd = if let Some(ref pkg) = package {
            format!("dumpsys deviceidle 2>/dev/null | grep -E '(mState|mLightState|State:)' | head -5 && echo '===BUCKET===' && am get-standby-bucket {} 2>/dev/null", pkg)
        } else {
            "dumpsys deviceidle 2>/dev/null | grep -E '(mState|mLightState|State:)' | head -5".to_string()
        };

        if let Ok(output) = shell.exec(&cmd).await {
            let data = parse_doze_output(&output);
            let _ = tx.send(PanelUpdate::Doze(data));
        }
    }
}

async fn threads_runner(tx: mpsc::UnboundedSender<PanelUpdate>, shell: Arc<PersistentShell>, shared_pkg: SharedPackage) {
    let mut interval = tokio::time::interval(Duration::from_secs(1));

    loop {
        interval.tick().await;

        let package = shared_pkg.read().await.clone();

        let cmd = if let Some(ref pkg) = package {
            format!(
                r#"PID=$(ps -A -o PID,NAME | grep '{}' | head -1 | awk '{{print $1}}'); if [ -n "$PID" ]; then ps -T -p $PID -o TID,%CPU,CMD 2>/dev/null | tail -n +2 | head -200; else echo "NO_PID"; fi"#,
                pkg
            )
        } else {
            "top -n 1 -b 2>/dev/null | tail -n +5 | head -40".to_string()
        };

        match shell.exec(&cmd).await {
            Ok(output) => {
                let data = parse_threads_output(&output, package.as_deref());
                let _ = tx.send(PanelUpdate::Threads(data));
            }
            Err(e) => {
                error!("Threads runner error: {}", e);
            }
        }
    }
}

async fn alarms_runner(tx: mpsc::UnboundedSender<PanelUpdate>, shell: Arc<PersistentShell>, shared_pkg: SharedPackage) {
    let mut interval = tokio::time::interval(Duration::from_secs(2));

    loop {
        interval.tick().await;

        let package = shared_pkg.read().await.clone();

        let cmd = if let Some(ref pkg) = package {
            format!("dumpsys alarm 2>/dev/null | grep '{}' | head -50", pkg)
        } else {
            "dumpsys alarm 2>/dev/null | head -300".to_string()
        };

        if let Ok(output) = shell.exec(&cmd).await {
            let data = parse_alarms_output(&output, package.as_deref());
            let _ = tx.send(PanelUpdate::Alarms(data));
        }
    }
}

async fn jobs_runner(tx: mpsc::UnboundedSender<PanelUpdate>, shell: Arc<PersistentShell>, shared_pkg: SharedPackage) {
    let mut interval = tokio::time::interval(Duration::from_secs(2));

    loop {
        interval.tick().await;

        let package = shared_pkg.read().await.clone();

        let cmd = if let Some(ref pkg) = package {
            format!("dumpsys jobscheduler 2>/dev/null | grep '{}' | head -50", pkg)
        } else {
            "dumpsys jobscheduler 2>/dev/null | head -500".to_string()
        };

        if let Ok(output) = shell.exec(&cmd).await {
            let data = parse_jobs_output(&output, package.as_deref());
            let _ = tx.send(PanelUpdate::Jobs(data));
        }
    }
}

async fn processes_runner(tx: mpsc::UnboundedSender<PanelUpdate>, shell: Arc<PersistentShell>, shared_pkg: SharedPackage) {
    let mut interval = tokio::time::interval(Duration::from_secs(1));

    loop {
        interval.tick().await;

        let package = shared_pkg.read().await.clone();

        let cmd = if let Some(ref pkg) = package {
            format!("ps -A -o PID,USER,%CPU,%MEM,NAME | grep '{}' | head -20", pkg)
        } else {
            "ps -A -o PID,USER,%CPU,%MEM,NAME | head -50".to_string()
        };

        if let Ok(output) = shell.exec(&cmd).await {
            let data = parse_processes_output(&output);
            let _ = tx.send(PanelUpdate::Processes(data));
        }
    }
}

async fn devices_runner(tx: mpsc::UnboundedSender<PanelUpdate>) {
    use std::process::Stdio;
    use tokio::process::Command;

    let mut interval = tokio::time::interval(Duration::from_secs(5));

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
    let mut interval = tokio::time::interval(Duration::from_secs(5));

    loop {
        interval.tick().await;

        if let Ok(output) = shell.exec("ps -A -o NAME | grep -E '\\.'").await {
            let packages: Vec<String> = output
                .lines()
                .filter(|line| {
                    let l = line.trim();
                    l.contains('.')
                        && !l.starts_with("com.android")
                        && !l.starts_with("com.google")
                        && !l.starts_with("com.samsung")
                })
                .map(|s| s.trim().to_string())
                .collect();
            let _ = tx.send(PanelUpdate::Packages(packages));
        }
    }
}

fn parse_cpu_output(output: &str, prev_stats: &mut Vec<(u64, u64)>) -> CpuData {
    let mut data = CpuData::default();

    let freq_parts: Vec<&str> = output.split("===FREQ===").collect();
    let stat_section = freq_parts.first().unwrap_or(&"");

    let (freq_section, app_section): (&str, Option<&str>) = if let Some(rest) = freq_parts.get(1) {
        let app_parts: Vec<&str> = rest.split("===APP===").collect();
        (*app_parts.first().unwrap_or(&""), app_parts.get(1).copied())
    } else {
        ("", None)
    };

    let mut current_stats: Vec<(u64, u64)> = Vec::new();
    let mut total_usage = 0.0f32;

    for line in stat_section.lines() {
        if line.starts_with("cpu") && line.chars().nth(3).map(|c| c.is_ascii_digit()).unwrap_or(false) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 5 {
                if let Ok(id) = parts[0][3..].parse::<u8>() {
                    let user: u64 = parts[1].parse().unwrap_or(0);
                    let nice: u64 = parts[2].parse().unwrap_or(0);
                    let sys: u64 = parts[3].parse().unwrap_or(0);
                    let idle: u64 = parts[4].parse().unwrap_or(0);
                    let iowait: u64 = parts.get(5).and_then(|s| s.parse().ok()).unwrap_or(0);
                    let irq: u64 = parts.get(6).and_then(|s| s.parse().ok()).unwrap_or(0);
                    let softirq: u64 = parts.get(7).and_then(|s| s.parse().ok()).unwrap_or(0);

                    let total = user + nice + sys + idle + iowait + irq + softirq;
                    current_stats.push((total, idle));

                    let usage = if let Some(&(prev_total, prev_idle)) = prev_stats.get(id as usize) {
                        let dt = total.saturating_sub(prev_total);
                        let di = idle.saturating_sub(prev_idle);
                        if dt > 0 { ((dt - di) as f32 / dt as f32) * 100.0 } else { 0.0 }
                    } else {
                        0.0
                    };

                    total_usage += usage;
                    data.cores.push(CoreInfo { id, usage_percent: usage, frequency_mhz: 0 });
                }
            }
        }
    }

    *prev_stats = current_stats;
    data.total_usage = if data.cores.is_empty() { 0.0 } else { total_usage / data.cores.len() as f32 };

    for (i, line) in freq_section.lines().filter(|l| !l.is_empty()).enumerate() {
        if i < data.cores.len() {
            if let Ok(freq) = line.trim().parse::<u32>() {
                data.cores[i].frequency_mhz = freq / 1000;
            }
        }
    }

    if let Some(app_line) = app_section {
        for line in app_line.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 9 {
                for i in 7..10 {
                    if let Some(part) = parts.get(i) {
                        if let Ok(cpu) = part.parse::<f32>() {
                            if cpu >= 0.0 && cpu <= 100.0 {
                                data.app_cpu_total = cpu;
                                break;
                            }
                        }
                    }
                }
                break;
            }
        }
    }

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
            if line.contains("TOTAL PSS:") || line.contains("TOTAL:") {
                let nums: Vec<u64> = line.split_whitespace()
                    .filter_map(|s| s.parse().ok())
                    .collect();
                if !nums.is_empty() {
                    data.app_pss_kb = nums[0];
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
        if section.contains("type: WIFI") {
            data.wifi_connected = true;
        }
        if section.contains("type: MOBILE") {
            data.mobile_connected = true;
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
    let parts: Vec<&str> = output.split("===BUCKET===").collect();
    let doze_section = parts.first().unwrap_or(&"");

    for line in doze_section.lines() {
        let line = line.trim();
        if line.contains("mState") || line.contains("State:") {
            if let Some(value) = line.split(['=', ':']).last() {
                let state = value.trim().to_string();
                if !state.is_empty() && data.device_idle_state.is_empty() {
                    data.device_idle_state = state;
                }
            }
        }
        if line.contains("mLightState") || line.contains("LightState") {
            if let Some(value) = line.split(['=', ':']).last() {
                let state = value.trim().to_string();
                if !state.is_empty() {
                    data.light_idle_state = state;
                }
            }
        }
    }

    if let Some(bucket_section) = parts.get(1) {
        let bucket = bucket_section.trim().lines().next().unwrap_or("").trim().to_string();
        if !bucket.is_empty() && !bucket.contains("Error") {
            data.app_standby_bucket = bucket;
        }
    }

    if data.device_idle_state.is_empty() { data.device_idle_state = "ACTIVE".to_string(); }
    if data.light_idle_state.is_empty() { data.light_idle_state = "ACTIVE".to_string(); }
    if data.app_standby_bucket.is_empty() { data.app_standby_bucket = "UNKNOWN".to_string(); }

    data
}

fn parse_threads_output(output: &str, package: Option<&str>) -> ThreadsData {
    let mut data = ThreadsData::default();

    if output.trim() == "NO_PID" {
        return data;
    }

    for line in output.lines() {
        let line = line.trim();

        if line.is_empty() ||
           line.starts_with("TID") ||
           line.starts_with("PID") ||
           line.contains("Threads:") ||
           line.contains("Mem:") ||
           line.contains("Swap:") ||
           line.contains("%CPU") ||
           line.contains("Tasks:") ||
           line.contains("USER") {
            continue;
        }

        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.is_empty() {
            continue;
        }

        if let Ok(tid) = parts[0].parse::<u32>() {
            if parts.len() >= 9 {
                let mut cpu: f32 = 0.0;

                for (i, part) in parts.iter().enumerate() {
                    if i >= 7 && i <= 9 {
                        if let Ok(val) = part.parse::<f32>() {
                            if val >= 0.0 && val <= 100.0 {
                                cpu = val;
                                break;
                            }
                        }
                    }
                }

                let name = parts.last().unwrap_or(&"unknown").to_string();

                if let Some(pkg) = package {
                    if !name.contains(pkg) && !line.contains(pkg) {
                        continue;
                    }
                }

                if cpu > 0.0 || data.threads.len() < 30 {
                    data.threads.push(ThreadInfo { tid, name, cpu_percent: cpu, state: String::new() });
                }
            } else if parts.len() >= 3 {
                let cpu = parts[1].parse::<f32>().unwrap_or(0.0);
                let name = parts[2..].join(" ");
                data.threads.push(ThreadInfo {
                    tid,
                    name,
                    cpu_percent: cpu,
                    state: String::new(),
                });
            } else if parts.len() == 2 {
                let name = parts[1].to_string();
                data.threads.push(ThreadInfo {
                    tid,
                    name,
                    cpu_percent: 0.0,
                    state: String::new(),
                });
            }
        }
    }

    data.threads.sort_by(|a, b| b.cpu_percent.partial_cmp(&a.cpu_percent).unwrap_or(std::cmp::Ordering::Equal));
    data.threads.truncate(50);
    data
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

fn parse_processes_output(output: &str) -> ProcessesData {
    let mut data = ProcessesData::default();

    for line in output.lines().skip(1) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 5 {
            if let Ok(pid) = parts[0].parse::<u32>() {
                data.processes.push(ProcessInfo {
                    pid,
                    user: parts[1].to_string(),
                    cpu_percent: parts[2].parse().unwrap_or(0.0),
                    mem_percent: parts[3].parse().unwrap_or(0.0),
                    name: parts[4..].join(" "),
                });
            }
        }
    }

    data.processes.sort_by(|a, b| b.cpu_percent.partial_cmp(&a.cpu_percent).unwrap_or(std::cmp::Ordering::Equal));
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
