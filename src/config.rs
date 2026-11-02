use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GraphStyle {
    Braille,
    Blocks,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CaptureMode {
    Off,
    On,
    Ask,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CpuUnits {
    AllCores,
    OneCore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TempUnit {
    C,
    F,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NetScale {
    Sqrt,
    Linear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemChart {
    AppOwned,
    Full,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub theme: String,
    pub graph: GraphStyle,
    pub key_hints: bool,
    pub start_page: u8,
    pub cpu_units: CpuUnits,
    pub temperature: TempUnit,
    pub window_s: u32,
    pub memory_chart: MemChart,
    pub net_scale: NetScale,
    pub hide_idle_threads: bool,
    pub interval_ms: u64,
    pub memory_interval_ms: u64,
    pub history_min: u32,
    pub frames: bool,
    pub capture: CaptureMode,
    pub power: bool,
    pub device_processes: bool,
    pub reopen_last_app: bool,
    pub report_dir: String,
    pub export_on_quit: bool,
    pub last_device: Option<String>,
    pub last_package: Option<String>,
    pub themes: BTreeMap<String, BTreeMap<String, String>>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: "Gruvbox Dark".into(),
            graph: GraphStyle::Braille,
            key_hints: true,
            start_page: 1,
            cpu_units: CpuUnits::AllCores,
            temperature: TempUnit::C,
            window_s: 120,
            memory_chart: MemChart::AppOwned,
            net_scale: NetScale::Sqrt,
            hide_idle_threads: false,
            interval_ms: 1000,
            memory_interval_ms: 2000,
            history_min: 15,
            frames: true,
            capture: CaptureMode::Ask,
            power: true,
            device_processes: true,
            reopen_last_app: false,
            report_dir: "~/perf/reports".into(),
            export_on_quit: false,
            last_device: None,
            last_package: None,
            themes: BTreeMap::new(),
        }
    }
}

impl TempUnit {
    pub fn convert(self, celsius: f32) -> f32 {
        match self {
            TempUnit::C => celsius,
            TempUnit::F => celsius * 9.0 / 5.0 + 32.0,
        }
    }

    pub fn format(self, celsius: f32) -> String {
        match self {
            TempUnit::C => format!("{celsius:.1}°C"),
            TempUnit::F => format!("{:.1}°F", self.convert(celsius)),
        }
    }

    pub fn symbol(self) -> &'static str {
        match self {
            TempUnit::C => "°C",
            TempUnit::F => "°F",
        }
    }
}

pub fn path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| home().map(|h| h.join(".config")))?;
    Some(base.join("android-perf-monitor").join("config.toml"))
}

pub fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

pub fn expand(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => home()
            .map(|h| h.join(rest))
            .unwrap_or_else(|| PathBuf::from(p)),
        None => PathBuf::from(p),
    }
}

impl Config {
    pub fn load() -> (Config, Option<String>) {
        let Some(path) = path() else {
            return (Config::default(), None);
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => match toml::from_str::<Config>(&text) {
                Ok(c) => (c.sanitized(), None),
                Err(e) => (
                    Config::default(),
                    Some(format!("{}: {}", path.display(), e.message())),
                ),
            },
            Err(_) => (Config::default(), None),
        }
    }

    fn sanitized(mut self) -> Config {
        self.interval_ms = self.interval_ms.clamp(200, 60_000);
        self.memory_interval_ms = self.memory_interval_ms.clamp(500, 60_000);
        self.history_min = self.history_min.clamp(1, 360);
        self.window_s = self.window_s.clamp(30, 3600);
        self.start_page = self.start_page.clamp(1, 6);
        self
    }

    pub fn save(&self) -> Result<PathBuf, String> {
        let path = path().ok_or("no home directory")?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let text = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&path, text).map_err(|e| e.to_string())?;
        Ok(path)
    }

    pub fn history_samples(&self, interval: std::time::Duration) -> usize {
        ((self.history_min as f64 * 60.0) / interval.as_secs_f64().max(0.05)).ceil() as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_config_keeps_defaults() {
        let c: Config =
            toml::from_str("theme = \"nord\"\ninterval_ms = 50\n[themes.mine]\nbg = \"#000000\"\n")
                .unwrap();
        let c = c.sanitized();
        assert_eq!(c.theme, "nord");
        assert_eq!(c.interval_ms, 200);
        assert_eq!(c.memory_interval_ms, 2000);
        assert_eq!(c.themes["mine"]["bg"], "#000000");
        let text = toml::to_string_pretty(&c).unwrap();
        assert!(text.contains("theme = \"nord\""));
    }
}
