use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Cpu,
    CpuFreq,
    Threads,
    Processes,
    SystemMemory,
    MemoryPressure,
    AppMemory,
    Frames,
    Battery,
    Thermal,
    Doze,
    Jobs,
    Network,
    Capture,
}

impl Source {
    pub const ALL: [Source; 14] = [
        Source::Cpu,
        Source::CpuFreq,
        Source::Threads,
        Source::Processes,
        Source::SystemMemory,
        Source::MemoryPressure,
        Source::AppMemory,
        Source::Frames,
        Source::Battery,
        Source::Thermal,
        Source::Doze,
        Source::Jobs,
        Source::Network,
        Source::Capture,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn label(self) -> &'static str {
        match self {
            Source::Cpu => "cpu",
            Source::CpuFreq => "cpu frequency",
            Source::Threads => "threads",
            Source::Processes => "processes",
            Source::SystemMemory => "system memory",
            Source::MemoryPressure => "memory pressure",
            Source::AppMemory => "app memory",
            Source::Frames => "frames",
            Source::Battery => "battery",
            Source::Thermal => "thermal",
            Source::Doze => "doze",
            Source::Jobs => "jobs",
            Source::Network => "network",
            Source::Capture => "capture",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[serde(tag = "state", content = "reason", rename_all = "snake_case")]
pub enum Support {
    #[default]
    Unknown,
    Supported,
    Unsupported(String),
}

impl Support {
    pub fn is_unsupported(&self) -> bool {
        matches!(self, Support::Unsupported(_))
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Device {
    pub serial: String,
    pub model: String,
    pub product: String,
    pub state: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Cluster {
    pub name: &'static str,
    pub cores: Vec<u16>,
    pub max_khz: u32,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Capabilities {
    pub sdk: u32,
    pub release: String,
    pub model: String,
    pub manufacturer: String,
    pub clusters: Vec<Cluster>,
    pub cpufreq: bool,
    pub psi: bool,
    pub gfxinfo: bool,
    pub thermal: bool,
    pub battery: bool,
    pub deviceidle: bool,
    pub jobscheduler: bool,
    pub meminfo: bool,
    pub page_size: u32,
}

impl Capabilities {
    pub fn cluster_of(&self, core: u16) -> Option<usize> {
        self.clusters.iter().position(|c| c.cores.contains(&core))
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct CoreSample {
    pub id: u16,
    pub usage: f32,
    pub cur_khz: u32,
    pub max_khz: u32,
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct SysMem {
    pub total_kb: u64,
    pub free_kb: u64,
    pub available_kb: u64,
    pub buffers_kb: u64,
    pub cached_kb: u64,
    pub swap_total_kb: u64,
    pub swap_free_kb: u64,
}

impl SysMem {
    pub fn used_kb(&self) -> u64 {
        self.total_kb.saturating_sub(self.available_kb)
    }

    pub fn swap_used_kb(&self) -> u64 {
        self.swap_total_kb.saturating_sub(self.swap_free_kb)
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct Psi {
    pub some_avg10: f32,
    pub full_avg10: f32,
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct NetRates {
    pub wifi_rx_bps: f64,
    pub wifi_tx_bps: f64,
    pub mobile_rx_bps: f64,
    pub mobile_tx_bps: f64,
    pub wifi_up: bool,
    pub mobile_up: bool,
}

impl NetRates {
    pub fn rx_bps(&self) -> f64 {
        self.wifi_rx_bps + self.mobile_rx_bps
    }

    pub fn tx_bps(&self) -> f64 {
        self.wifi_tx_bps + self.mobile_tx_bps
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct AppProc {
    pub pid: u32,
    pub cpu: f32,
    pub kernel: f32,
    pub threads: u32,
    pub running: u32,
    pub blocked: u32,
    pub ctx_voluntary_ps: f32,
    pub ctx_involuntary_ps: f32,
    pub minflt_ps: f32,
    pub majflt_ps: f32,
    pub rss_kb: u64,
    pub swap_kb: u64,
    pub oom_adj: Option<i32>,
    pub nice: i32,
    pub uptime_s: f64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct TaskSample {
    pub id: u32,
    pub name: String,
    pub state: char,
    pub cpu: f32,
    pub core: u16,
    pub ticks: u64,
    pub rss_pages: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AppState {
    #[default]
    NotSelected,
    NotRunning,
    Running,
}

#[derive(Debug, Clone, Default)]
pub struct FastSample {
    pub cores: Vec<CoreSample>,
    pub sys_cpu: f32,
    pub mem: Option<SysMem>,
    pub psi: Option<Psi>,
    pub net: Option<NetRates>,
    pub app_state: AppState,
    pub app: Option<AppProc>,
    pub tasks: Vec<TaskSample>,
    pub restarted: bool,
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct Objects {
    pub views: u32,
    pub view_roots: u32,
    pub app_contexts: u32,
    pub activities: u32,
    pub assets: u32,
    pub asset_managers: u32,
    pub local_binders: u32,
    pub proxy_binders: u32,
    pub parcel_memory_kb: u32,
    pub parcel_count: u32,
    pub death_recipients: u32,
    pub webviews: u32,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Database {
    pub name: String,
    pub size_kb: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Sqlite {
    pub memory_used_kb: u64,
    pub pagecache_overflow_kb: u64,
    pub malloc_size_kb: u64,
    pub databases: Vec<Database>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct AppMemory {
    pub pid: u32,
    pub pss_kb: u64,
    pub rss_kb: u64,
    pub swap_pss_kb: u64,
    pub java_kb: u64,
    pub native_kb: u64,
    pub code_kb: u64,
    pub stack_kb: u64,
    pub graphics_kb: u64,
    pub other_kb: u64,
    pub system_kb: u64,
    pub java_rss_kb: u64,
    pub native_rss_kb: u64,
    pub code_rss_kb: u64,
    pub stack_rss_kb: u64,
    pub graphics_rss_kb: u64,
    pub java_heap_size_kb: u64,
    pub java_heap_alloc_kb: u64,
    pub native_heap_size_kb: u64,
    pub native_heap_alloc_kb: u64,
    pub objects: Option<Objects>,
    pub sqlite: Option<Sqlite>,
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct Frame {
    pub vsync_ns: u64,
    pub ms: f32,
    pub budget_ms: f32,
    pub janky: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct FrameSummary {
    pub total: u64,
    pub janky: u64,
    pub janky_legacy: u64,
    pub p50_ms: u32,
    pub p90_ms: u32,
    pub p95_ms: u32,
    pub p99_ms: u32,
    pub gpu_p50_ms: u32,
    pub gpu_p90_ms: u32,
    pub gpu_p95_ms: u32,
    pub gpu_p99_ms: u32,
    pub missed_vsync: u64,
    pub high_input_latency: u64,
    pub slow_ui_thread: u64,
    pub slow_bitmap_uploads: u64,
    pub slow_draw_commands: u64,
    pub deadline_missed: u64,
    pub histogram: Vec<(u32, u64)>,
}

#[derive(Debug, Clone, Default)]
pub struct FrameUpdate {
    pub pid: u32,
    pub baseline: bool,
    pub frames: Vec<Frame>,
    pub summary: FrameSummary,
    pub uptime_s: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Battery {
    pub level: u8,
    pub status: String,
    pub plugged: String,
    pub health: String,
    pub technology: String,
    pub voltage_mv: u32,
    pub temperature_c: f32,
    pub current_ma: Option<i32>,
    pub charge_counter_uah: Option<i64>,
}

impl Battery {
    pub fn discharging(&self) -> bool {
        self.plugged == "none" && self.status != "charging"
    }

    pub fn drain_ma(&self) -> Option<f32> {
        if self.discharging() {
            self.current_ma.map(|c| c.unsigned_abs() as f32)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Sensor {
    pub name: String,
    pub celsius: f32,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Thermal {
    pub status: u8,
    pub sensors: Vec<Sensor>,
}

pub const THERMAL_LEVELS: [&str; 7] = [
    "NONE",
    "LIGHT",
    "MODERATE",
    "SEVERE",
    "CRITICAL",
    "EMERGENCY",
    "SHUTDOWN",
];

pub fn thermal_level_name(status: u8) -> &'static str {
    THERMAL_LEVELS
        .get(status as usize)
        .copied()
        .unwrap_or("UNKNOWN")
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Doze {
    pub deep: String,
    pub light: String,
    pub bucket: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct JobInfo {
    pub service: String,
    pub count: u32,
    pub running: u32,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct JobRun {
    pub ago: String,
    pub service: String,
    pub event: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Jobs {
    pub scheduled: u32,
    pub running: u32,
    pub services: Vec<JobInfo>,
    pub recent: Vec<JobRun>,
    pub alarms: Option<u32>,
}

#[derive(Debug, Clone, Default)]
pub struct ProcessName {
    pub pid: u32,
    pub name: String,
}
