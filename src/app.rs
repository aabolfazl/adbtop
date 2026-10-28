use std::collections::HashMap;
use std::time::{Duration, Instant};
use tracing::info;

use crate::agent::proto;
use crate::data::{DeviceInfo, Metrics};
use crate::recorder::CsvRecorder;

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
    Connections,
}

/// The latest snapshot from the capture agent, plus rates computed from the one before.
pub struct CaptureState {
    pub snapshot: proto::Snapshot,
    /// Bytes per second by uid since the previous snapshot: (received, sent).
    pub rates: HashMap<i32, (u64, u64)>,
}

/// Captured traffic of the selected package, or of all apps in device mode.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct CaptureTraffic {
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_rate: u64,
    pub tx_rate: u64,
    pub open_connections: u32,
}

pub const HISTORY_LEN: usize = 60;

pub struct App {
    pub current_tab: Tab,
    pub popup: Popup,
    pub focused_panel: FocusedPanel,

    pub package: Option<String>,

    pub devices: Vec<DeviceInfo>,
    pub selected_device: usize,
    pub device_serial: Option<String>,

    pub packages: Vec<String>,
    pub packages_lower: Vec<String>,
    pub filtered_packages: Vec<usize>,
    pub selected_package: usize,
    pub package_search: String,

    pub paused: bool,
    pub start_time: Instant,
    pub sample_count: u32,
    /// Samples that had a real app CPU reading (what AVG is averaged over).
    pub app_cpu_samples: u32,

    pub metrics: Metrics,

    pub temp_history: Vec<f32>,
    pub cpu_history: Vec<f32>,
    pub mem_history: Vec<u64>,
    pub net_rx_history: Vec<u64>,
    pub net_tx_history: Vec<u64>,
    /// Set when the previous network counters can't be used for a delta (resume, package change).
    pub net_resync: bool,

    pub last_error: Option<String>,

    pub memory_data: crate::runner::MemoryData,
    pub battery_data: crate::runner::BatteryData,
    pub network_data: crate::runner::NetworkData,
    pub doze_data: crate::runner::DozeData,
    pub alarms_data: crate::runner::AlarmsData,
    pub jobs_data: crate::runner::JobsData,

    pub tracking_mode: crate::runner::TrackingMode,

    pub shared_package: Option<crate::runner::SharedPackage>,

    pub recorder: Option<CsvRecorder>,
    /// Kept until exit, unlike `last_error`, which clears on the next good sample.
    pub recorder_error: Option<String>,

    pub capture: Option<CaptureState>,
    /// The user wants the on-device capture running (`c` or `--capture`).
    pub capture_wanted: bool,
    pub capture_error: Option<String>,
    pub capture_rx_history: Vec<u64>,
    pub capture_tx_history: Vec<u64>,
}

impl App {
    pub fn new(package: Option<String>, recorder: Option<CsvRecorder>) -> Self {
        let tracking_mode = if package.is_some() {
            crate::runner::TrackingMode::Package
        } else {
            crate::runner::TrackingMode::Device
        };

        Self {
            current_tab: Tab::Monitor,
            popup: Popup::None,
            focused_panel: FocusedPanel::None,

            package,

            devices: Vec::new(),
            selected_device: 0,
            device_serial: None,

            packages: Vec::new(),
            packages_lower: Vec::new(),
            filtered_packages: Vec::new(),
            selected_package: 0,
            package_search: String::new(),

            paused: false,
            start_time: Instant::now(),
            sample_count: 0,
            app_cpu_samples: 0,

            metrics: Metrics::default(),

            temp_history: Vec::with_capacity(HISTORY_LEN),
            cpu_history: Vec::with_capacity(HISTORY_LEN),
            mem_history: Vec::with_capacity(HISTORY_LEN),
            net_rx_history: Vec::with_capacity(HISTORY_LEN),
            net_tx_history: Vec::with_capacity(HISTORY_LEN),
            net_resync: false,

            last_error: None,

            memory_data: Default::default(),
            battery_data: Default::default(),
            network_data: Default::default(),
            doze_data: Default::default(),
            alarms_data: Default::default(),
            jobs_data: Default::default(),
            tracking_mode,
            shared_package: None,
            recorder,
            recorder_error: None,

            capture: None,
            capture_wanted: false,
            capture_error: None,
            capture_rx_history: Vec::with_capacity(HISTORY_LEN),
            capture_tx_history: Vec::with_capacity(HISTORY_LEN),
        }
    }

    /// Returns true when the app should quit.
    pub fn on_key(&mut self, key: char) -> bool {
        match self.popup {
            Popup::None => return self.handle_main_key(key),
            Popup::SelectDevice => {
                if key == 'q' {
                    self.close_popup();
                }
            }
            Popup::SelectPackage => self.handle_package_search_key(key),
        }
        false
    }

    fn handle_main_key(&mut self, key: char) -> bool {
        match key {
            'q' => return true,
            'f' => self.toggle_focus(FocusedPanel::Threads),
            'n' => self.toggle_focus(FocusedPanel::Connections),
            'c' => {
                self.capture_wanted = !self.capture_wanted;
                self.capture_error = None;
            }
            'd' => self.popup = Popup::SelectDevice,
            'p' => self.popup = Popup::SelectPackage,
            'x' => self.clear_package(),
            ' ' => self.toggle_pause(),
            _ => {}
        }
        false
    }

    fn toggle_focus(&mut self, panel: FocusedPanel) {
        self.focused_panel = if self.focused_panel == panel { FocusedPanel::None } else { panel };
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
                if let Some(device) = self.devices.get(self.selected_device) {
                    let serial = device.serial.clone();
                    self.close_popup();
                    if self.device_serial.as_ref() != Some(&serial) {
                        info!("Device selected: {}", serial);
                        // The main loop restarts the collectors when the serial changes.
                        self.device_serial = Some(serial);
                        self.reset_device_data();
                    }
                }
            }
            Popup::SelectPackage => {
                if let Some(&idx) = self.filtered_packages.get(self.selected_package) {
                    let selected_pkg = self.packages[idx].clone();
                    info!("Package selected: {}", selected_pkg);

                    self.close_popup();
                    self.set_package(Some(selected_pkg));
                }
            }
            Popup::None => {}
        }
    }

    pub fn on_escape(&mut self) -> bool {
        if self.popup != Popup::None {
            self.close_popup();
            false
        } else if self.focused_panel != FocusedPanel::None {
            self.focused_panel = FocusedPanel::None;
            false
        } else {
            true
        }
    }

    fn close_popup(&mut self) {
        self.popup = Popup::None;
        self.package_search.clear();
        self.filter_packages();
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

    /// Replaces the package list without moving the cursor off the highlighted package.
    pub fn set_packages(&mut self, packages: Vec<String>) {
        let selected = self.filtered_packages
            .get(self.selected_package)
            .and_then(|&idx| self.packages.get(idx))
            .cloned();

        self.packages_lower = packages.iter().map(|p| p.to_lowercase()).collect();
        self.packages = packages;
        self.filter_packages();

        if let Some(selected) = selected {
            if let Some(pos) = self.filtered_packages.iter().position(|&idx| self.packages[idx] == selected) {
                self.selected_package = pos;
            }
        }
    }

    pub fn set_devices(&mut self, devices: Vec<DeviceInfo>) {
        self.devices = devices;
        self.selected_device = self.selected_device.min(self.devices.len().saturating_sub(1));
        if self.device_serial.is_none() {
            if let Some(first) = self.devices.first() {
                self.device_serial = Some(first.serial.clone());
            }
        }
    }

    pub fn toggle_pause(&mut self) {
        self.paused = !self.paused;
        if !self.paused {
            self.net_resync = true;
        }
    }

    pub fn clear_package(&mut self) {
        info!("Clearing package selection, switching to DEVICE mode");
        self.set_package(None);
    }

    fn set_package(&mut self, package: Option<String>) {
        self.tracking_mode = if package.is_some() {
            crate::runner::TrackingMode::Package
        } else {
            crate::runner::TrackingMode::Device
        };
        self.package = package.clone();
        self.metrics.threads.clear();
        self.metrics.app_cpu_total = 0.0;
        self.reset_history();

        if let Some(ref shared_pkg) = self.shared_package {
            let shared = shared_pkg.clone();
            tokio::spawn(async move {
                *shared.write().await = package;
            });
        }
    }

    fn reset_history(&mut self) {
        self.start_time = Instant::now();
        self.sample_count = 0;
        self.app_cpu_samples = 0;
        self.metrics.app_cpu_avg = 0.0;
        self.metrics.app_cpu_peak = 0.0;
        self.temp_history.clear();
        self.cpu_history.clear();
        self.mem_history.clear();
        self.net_rx_history.clear();
        self.net_tx_history.clear();
        self.net_resync = true;
        self.capture_rx_history.clear();
        self.capture_tx_history.clear();
    }

    fn reset_device_data(&mut self) {
        self.metrics = Metrics::default();
        self.memory_data = Default::default();
        self.battery_data = Default::default();
        self.network_data = Default::default();
        self.doze_data = Default::default();
        self.alarms_data = Default::default();
        self.jobs_data = Default::default();
        self.last_error = None;
        self.capture = None;
        self.set_packages(Vec::new());
        self.reset_history();
    }

    pub fn on_capture(&mut self, snapshot: proto::Snapshot) {
        let mut rates = HashMap::new();
        if let Some(prev) = &self.capture {
            let dt = snapshot.time_ms.saturating_sub(prev.snapshot.time_ms);
            // A restarted capture starts counting from zero again.
            if dt > 0 && snapshot.started_ms == prev.snapshot.started_ms {
                for app in &snapshot.apps {
                    let (prev_rx, prev_tx) = prev.snapshot.apps.iter()
                        .find(|p| p.uid == app.uid)
                        .map(|p| (p.rx_bytes, p.tx_bytes))
                        .unwrap_or((0, 0));
                    rates.insert(app.uid, (
                        app.rx_bytes.saturating_sub(prev_rx) * 1000 / dt,
                        app.tx_bytes.saturating_sub(prev_tx) * 1000 / dt,
                    ));
                }
            }
        }
        let had_rates = !rates.is_empty();
        self.capture = Some(CaptureState { snapshot, rates });
        self.capture_error = None;
        if had_rates {
            let traffic = self.capture_traffic().unwrap_or_default();
            push_history(&mut self.capture_rx_history, traffic.rx_rate);
            push_history(&mut self.capture_tx_history, traffic.tx_rate);
        }
    }

    /// The uids of the selected package (several apps can share one uid, listed as "a,b").
    fn package_uids(&self, snapshot: &proto::Snapshot) -> Option<Vec<i32>> {
        let package = self.package.as_deref()?;
        Some(snapshot.apps.iter()
            .filter(|a| a.name.split(',').any(|name| name == package))
            .map(|a| a.uid)
            .collect())
    }

    pub fn capture_traffic(&self) -> Option<CaptureTraffic> {
        let capture = self.capture.as_ref()?;
        let uids = self.package_uids(&capture.snapshot);
        let mut traffic = CaptureTraffic::default();
        for app in &capture.snapshot.apps {
            if uids.as_ref().is_some_and(|u| !u.contains(&app.uid)) {
                continue;
            }
            let (rx_rate, tx_rate) = capture.rates.get(&app.uid).copied().unwrap_or((0, 0));
            traffic.rx_bytes += app.rx_bytes;
            traffic.tx_bytes += app.tx_bytes;
            traffic.rx_rate += rx_rate;
            traffic.tx_rate += tx_rate;
            traffic.open_connections += app.open_connections;
        }
        Some(traffic)
    }

    /// Connections of the selected package (or all), open ones first, most recent first.
    pub fn capture_connections(&self) -> Vec<&proto::Connection> {
        let Some(capture) = &self.capture else { return Vec::new() };
        let uids = self.package_uids(&capture.snapshot);
        let mut connections: Vec<&proto::Connection> = capture.snapshot.connections.iter()
            .filter(|c| uids.as_ref().map_or(true, |u| u.contains(&c.uid)))
            .collect();
        let is_open = |c: &proto::Connection| {
            c.state == proto::ConnectionState::Open as i32 || c.state == proto::ConnectionState::Connecting as i32
        };
        connections.sort_by(|a, b| is_open(b).cmp(&is_open(a)).then(b.last_ms.cmp(&a.last_ms)));
        connections
    }

    pub fn capture_app_name(&self, uid: i32) -> &str {
        self.capture.as_ref()
            .and_then(|c| c.snapshot.apps.iter().find(|a| a.uid == uid))
            .map(|a| a.name.as_str())
            .unwrap_or("?")
    }

    pub fn elapsed(&self) -> Duration {
        self.start_time.elapsed()
    }

    pub fn current_device(&self) -> Option<&DeviceInfo> {
        self.device_serial.as_ref().and_then(|serial| {
            self.devices.iter().find(|d| &d.serial == serial)
        })
    }
}

/// Appends a sample and keeps the last `HISTORY_LEN` values.
pub fn push_history<T>(history: &mut Vec<T>, value: T) {
    history.push(value);
    if history.len() > HISTORY_LEN {
        history.remove(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_with_packages(packages: &[&str]) -> App {
        let mut app = App::new(None, None);
        app.set_packages(packages.iter().map(|p| p.to_string()).collect());
        app
    }

    #[test]
    fn q_is_typed_into_the_package_filter() {
        let mut app = app_with_packages(&["com.qualcomm.app", "com.example.app"]);
        app.on_key('p');
        assert!(!app.on_key('q'));
        assert_eq!(app.popup, Popup::SelectPackage);
        assert_eq!(app.package_search, "q");
        assert_eq!(app.filtered_packages, vec![0]);
    }

    #[test]
    fn q_quits_from_the_main_screen() {
        let mut app = App::new(None, None);
        assert!(app.on_key('q'));
    }

    #[test]
    fn package_refresh_keeps_the_cursor_on_the_same_package() {
        let mut app = app_with_packages(&["a.a", "b.b", "c.c"]);
        app.on_key('p');
        app.on_down();
        app.on_down();
        app.set_packages(vec!["0.0".into(), "a.a".into(), "b.b".into(), "c.c".into()]);
        app.on_enter();
        assert_eq!(app.package.as_deref(), Some("c.c"));
    }

    #[test]
    fn switching_device_clears_the_package_list_safely() {
        let mut app = app_with_packages(&["a.a", "b.b"]);
        app.set_devices(vec![
            DeviceInfo { serial: "A".into(), model: "a".into(), brand: String::new() },
            DeviceInfo { serial: "B".into(), model: "b".into(), brand: String::new() },
        ]);
        assert_eq!(app.device_serial.as_deref(), Some("A"));

        app.on_key('d');
        app.on_down();
        app.on_enter();
        assert_eq!(app.device_serial.as_deref(), Some("B"));
        assert!(app.packages.is_empty());
        assert!(app.filtered_packages.is_empty());

        app.on_key('p');
        app.on_enter();
        assert_eq!(app.package, None);
    }

    #[test]
    fn device_cursor_is_clamped_when_a_device_disappears() {
        let mut app = App::new(None, None);
        let device = |s: &str| DeviceInfo { serial: s.into(), model: s.into(), brand: String::new() };
        app.set_devices(vec![device("A"), device("B")]);
        app.on_key('d');
        app.on_down();
        app.set_devices(vec![device("A")]);
        app.on_enter();
        assert_eq!(app.device_serial.as_deref(), Some("A"));
    }

    #[tokio::test]
    async fn capture_rates_and_connections_follow_the_selected_package() {
        let snapshots = crate::agent::tests::golden_snapshots().await;
        let mut app = App::new(Some("com.example.app".into()), None);
        for snapshot in snapshots.clone() {
            app.on_capture(snapshot);
        }

        let traffic = app.capture_traffic().unwrap();
        assert_eq!(traffic.rx_rate, 15_000);
        assert_eq!(traffic.tx_rate, 500);
        assert_eq!(traffic.rx_bytes, 75_000);
        assert_eq!(app.capture_rx_history, vec![15_000; 4]);

        let hosts: Vec<&str> = app.capture_connections().iter().map(|c| c.host.as_str()).collect();
        assert_eq!(hosts, vec!["www.example.com", "api.example.com"]);

        // Device mode shows every app.
        let mut device = App::new(None, None);
        for snapshot in snapshots {
            device.on_capture(snapshot);
        }
        assert_eq!(device.capture_connections().len(), 3);
        assert_eq!(device.capture_traffic().unwrap().rx_rate, 15_100);
        assert_eq!(device.capture_app_name(10200), "com.other.app");
    }

    #[test]
    fn capture_and_connections_keys() {
        let mut app = App::new(None, None);
        assert!(!app.on_key('c'));
        assert!(app.capture_wanted);
        app.on_key('n');
        assert_eq!(app.focused_panel, FocusedPanel::Connections);
        app.on_key('f');
        assert_eq!(app.focused_panel, FocusedPanel::Threads);
        app.on_key('f');
        assert_eq!(app.focused_panel, FocusedPanel::None);
    }

    #[test]
    fn package_from_command_line_starts_in_package_mode() {
        let app = App::new(Some("com.example.app".into()), None);
        assert_eq!(app.tracking_mode, crate::runner::TrackingMode::Package);
    }
}
