use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub serial: String,
    pub model: String,
    pub brand: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CpuCore {
    pub id: u8,
    pub usage_percent: f32,
    pub frequency_mhz: u32,
    pub temperature: Option<f32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppCoreUsage {
    pub core_id: u8,
    pub cpu_percent: f32,
    pub thread_count: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ThermalInfo {
    pub ap: f32,
    pub cp: f32,
    pub pa: f32,
    pub skin: f32,
    pub battery: f32,
    pub usb: f32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ThreadInfo {
    pub pid: u32,
    pub name: String,
    pub cpu_percent: f32,
    pub mem_percent: f32,
    pub core: Option<u8>,
    pub time: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Metrics {
    pub timestamp: String,
    pub battery_level: u8,
    pub cpu_cores: Vec<CpuCore>,
    pub app_core_usage: Vec<AppCoreUsage>,
    pub app_cpu_total: f32,
    pub app_cpu_peak: f32,
    pub app_cpu_avg: f32,
    pub thermal: ThermalInfo,
    pub threads: Vec<ThreadInfo>,
}

#[allow(dead_code)]
impl Metrics {
    pub fn new(num_cores: usize) -> Self {
        Self {
            cpu_cores: (0..num_cores)
                .map(|i| CpuCore {
                    id: i as u8,
                    ..Default::default()
                })
                .collect(),
            app_core_usage: (0..num_cores)
                .map(|i| AppCoreUsage {
                    core_id: i as u8,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }
}
