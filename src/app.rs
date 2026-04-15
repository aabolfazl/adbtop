use std::time::{Duration, Instant};
use tracing::info;

use crate::adb::Adb;
use crate::data::{DeviceInfo, Metrics};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Monitor,
}

impl Tab {
    pub fn all() -> &'static [Tab] {
        &[Tab::Monitor]
    }

    pub fn name(&self) -> &'static str {
        match self {
            Tab::Monitor => "Monitor",
        }
    }

    pub fn key(&self) -> char {
        match self {
            Tab::Monitor => '1',
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Popup {
    None,
    SelectDevice,
    SelectPackage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusedPanel {
    None,
    Threads,
}

pub struct App {
    pub current_tab: Tab,
    pub popup: Popup,
    pub focused_panel: FocusedPanel,

    pub package: Option<String>,
    pub _output_file: Option<String>,
    pub _interval_secs: u64,

    pub devices: Vec<DeviceInfo>,
    pub selected_device: usize,
    pub device_serial: Option<String>,

    pub packages: Vec<String>,
    pub packages_lower: Vec<String>,
    pub filtered_packages: Vec<usize>,
    pub selected_package: usize,
    pub package_search: String,

    pub paused: bool,
    pub start_time: Option<Instant>,
    pub _last_update: Option<Instant>,
    pub sample_count: u32,

    pub metrics: Metrics,

    pub temp_history: Vec<f32>,
    pub cpu_history: Vec<f32>,
    pub net_rx_history: Vec<u64>,
    pub net_tx_history: Vec<u64>,

    pub adb: Adb,

    pub _initialized: bool,

    pub status_message: Option<String>,
    pub last_error: Option<String>,

    pub doze_state: String,

    pub memory_data: crate::runner::MemoryData,
    pub battery_data: crate::runner::BatteryData,
    pub network_data: crate::runner::NetworkData,
    pub doze_data: crate::runner::DozeData,
    pub alarms_data: crate::runner::AlarmsData,
    pub jobs_data: crate::runner::JobsData,
    pub processes_data: crate::runner::ProcessesData,

    pub tracking_mode: crate::runner::TrackingMode,

    pub shared_package: Option<crate::runner::SharedPackage>,
}

impl App {
    pub fn new(
        package: Option<String>,
        _duration: u64,
        interval: u64,
        output: Option<String>,
    ) -> Self {
        Self {
            current_tab: Tab::Monitor,
            popup: Popup::None,
            focused_panel: FocusedPanel::None,

            package,
            _output_file: output,
            _interval_secs: interval,

            devices: Vec::new(),
            selected_device: 0,
            device_serial: None,

            packages: Vec::new(),
            packages_lower: Vec::new(),
            filtered_packages: Vec::new(),
            selected_package: 0,
            package_search: String::new(),

            paused: false,
            start_time: None,
            _last_update: None,
            sample_count: 0,

            metrics: Metrics::default(),

            temp_history: Vec::with_capacity(60),
            cpu_history: Vec::with_capacity(60),
            net_rx_history: Vec::with_capacity(60),
            net_tx_history: Vec::with_capacity(60),

            adb: Adb::new(),

            _initialized: false,
            status_message: None,
            last_error: None,
            doze_state: String::new(),

            memory_data: Default::default(),
            battery_data: Default::default(),
            network_data: Default::default(),
            doze_data: Default::default(),
            alarms_data: Default::default(),
            jobs_data: Default::default(),
            processes_data: Default::default(),
            tracking_mode: crate::runner::TrackingMode::Device,
            shared_package: None,
        }
    }

    pub fn on_key(&mut self, key: char) {
        if key == 'f' && self.popup == Popup::None {
            self.focused_panel = if self.focused_panel != FocusedPanel::None {
                FocusedPanel::None
            } else {
                FocusedPanel::Threads
            };
            return;
        }

        match self.popup {
            Popup::None => self.handle_main_key(key),
            Popup::SelectDevice => {}
            Popup::SelectPackage => self.handle_package_search_key(key),
        }
    }

    fn handle_main_key(&mut self, key: char) {
        match key {
            'd' => self.popup = Popup::SelectDevice,
            'p' => self.popup = Popup::SelectPackage,
            'x' => self.clear_package(),
            ' ' => self.toggle_pause(),
            _ => {}
        }
    }

    fn handle_package_search_key(&mut self, key: char) {
        if key.is_alphanumeric() || key == '.' || key == '_' {
            self.package_search.push(key);
            self.filter_packages();
        }
    }

    pub fn on_backspace(&mut self) {
        if self.popup == Popup::SelectPackage {
            self.package_search.pop();
            self.filter_packages();
        }
    }

    pub fn on_up(&mut self) {
        match self.popup {
            Popup::SelectDevice => {
                if self.selected_device > 0 {
                    self.selected_device -= 1;
                }
            }
            Popup::SelectPackage => {
                if self.selected_package > 0 {
                    self.selected_package -= 1;
                }
            }
            Popup::None => {}
        }
    }

    pub fn on_down(&mut self) {
        match self.popup {
            Popup::SelectDevice => {
                if self.selected_device < self.devices.len().saturating_sub(1) {
                    self.selected_device += 1;
                }
            }
            Popup::SelectPackage => {
                let max = self.filtered_packages.len().saturating_sub(1);
                if self.selected_package < max {
                    self.selected_package += 1;
                }
            }
            Popup::None => {}
        }
    }

    pub fn on_enter(&mut self) {
        match self.popup {
            Popup::SelectDevice => {
                if !self.devices.is_empty() {
                    self.device_serial = Some(self.devices[self.selected_device].serial.clone());
                    self.adb.set_device(self.device_serial.clone());
                    self.popup = Popup::None;
                    self.metrics = Metrics::default();
                    self.packages.clear();
                }
            }
            Popup::SelectPackage => {
                if !self.filtered_packages.is_empty() {
                    let idx = self.filtered_packages[self.selected_package];
                    let selected_pkg = self.packages[idx].clone();
                    info!("Package selected: {}", selected_pkg);

                    self.package = Some(selected_pkg.clone());
                    self.popup = Popup::None;
                    self.start_time = Some(Instant::now());
                    self.sample_count = 0;
                    self.temp_history.clear();
                    self.cpu_history.clear();
                    self.package_search.clear();
                    self.selected_package = 0;

                    self.tracking_mode = crate::runner::TrackingMode::Package;
                    if let Some(ref shared_pkg) = self.shared_package {
                        let pkg_clone = selected_pkg.clone();
                        let shared = shared_pkg.clone();
                        tokio::spawn(async move {
                            *shared.write().await = Some(pkg_clone);
                        });
                    }
                }
            }
            Popup::None => {}
        }
    }

    pub fn on_escape(&mut self) -> bool {
        if self.popup != Popup::None {
            self.popup = Popup::None;
            self.package_search.clear();
            self.selected_package = 0;
            false
        } else if self.focused_panel != FocusedPanel::None {
            self.focused_panel = FocusedPanel::None;
            false
        } else {
            true
        }
    }

    pub fn filter_packages(&mut self) {
        if self.package_search.is_empty() {
            self.filtered_packages = (0..self.packages.len()).collect();
        } else {
            let search = self.package_search.to_lowercase();
            self.filtered_packages = self.packages_lower
                .iter()
                .enumerate()
                .filter(|(_, p)| p.contains(&search))
                .map(|(i, _)| i)
                .collect();
        }
        self.selected_package = 0;
    }

    pub fn toggle_pause(&mut self) {
        self.paused = !self.paused;
    }

    pub fn clear_package(&mut self) {
        info!("Clearing package selection, switching to DEVICE mode");

        self.package = None;
        self.tracking_mode = crate::runner::TrackingMode::Device;
        self.temp_history.clear();
        self.cpu_history.clear();
        self.net_rx_history.clear();
        self.net_tx_history.clear();

        if let Some(ref shared_pkg) = self.shared_package {
            let shared = shared_pkg.clone();
            tokio::spawn(async move {
                *shared.write().await = None;
            });
        }
    }

    pub fn elapsed(&self) -> Duration {
        self.start_time
            .map(|t| t.elapsed())
            .unwrap_or(Duration::ZERO)
    }

    pub fn current_device(&self) -> Option<&DeviceInfo> {
        self.device_serial.as_ref().and_then(|serial| {
            self.devices.iter().find(|d| &d.serial == serial)
        })
    }
}
