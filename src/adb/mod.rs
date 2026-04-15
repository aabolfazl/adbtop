use anyhow::Result;
use std::process::Stdio;
use tokio::process::Command;

use crate::data::{DeviceInfo, Metrics, ThreadInfo};

#[allow(dead_code)]
pub struct Adb {
    device: Option<String>,
    num_cores: usize,
    prev_total: Vec<u64>,
    prev_idle: Vec<u64>,
}

#[allow(dead_code)]
impl Adb {
    pub fn new() -> Self {
        Self {
            device: None,
            num_cores: 0,
            prev_total: Vec::new(),
            prev_idle: Vec::new(),
        }
    }

    pub fn set_device(&mut self, serial: Option<String>) {
        self.device = serial;
    }

    pub fn adb_cmd(&self) -> Command {
        let mut cmd = Command::new("adb");
        if let Some(ref serial) = self.device {
            cmd.arg("-s").arg(serial);
        }
        cmd
    }

    pub async fn shell(&self, command: &str) -> Result<String> {
        let output = self
            .adb_cmd()
            .arg("shell")
            .arg(command)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await?;

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    pub async fn list_devices(&self) -> Result<Vec<DeviceInfo>> {
        let output = Command::new("adb")
            .arg("devices")
            .stdout(Stdio::piped())
            .output()
            .await?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut devices = Vec::new();

        for line in stdout.lines().skip(1) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 && parts[1] == "device" {
                let serial = parts[0].to_string();

                let model = self.get_prop(&serial, "ro.product.model").await?;
                let brand = self.get_prop(&serial, "ro.product.brand").await?;

                devices.push(DeviceInfo {
                    serial,
                    model,
                    brand,
                });
            }
        }

        Ok(devices)
    }

    async fn get_prop(&self, serial: &str, prop: &str) -> Result<String> {
        let output = Command::new("adb")
            .arg("-s")
            .arg(serial)
            .arg("shell")
            .arg("getprop")
            .arg(prop)
            .stdout(Stdio::piped())
            .output()
            .await?;

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    pub async fn list_packages(&self) -> Result<Vec<String>> {
        let output = self.shell("ps -A -o NAME").await?;

        let packages: Vec<String> = output
            .lines()
            .filter(|line| {
                line.contains('.')
                    && !line.starts_with("com.android")
                    && !line.starts_with("com.google")
                    && !line.starts_with("com.samsung")
            })
            .map(|s| s.trim().to_string())
            .collect();

        Ok(packages)
    }

    pub async fn collect_metrics(&mut self, package: &str) -> Result<Metrics> {
        let batch_cmd = r#"
            echo '===BATTERY==='
            dumpsys battery | grep 'level:'
            echo '===STAT==='
            cat /proc/stat
            echo '===FREQ==='
            for c in $(seq 0 31); do
                cat /sys/devices/system/cpu/cpu${c}/cpufreq/scaling_cur_freq 2>/dev/null || echo 0
            done
            echo '===THERMAL==='
            dumpsys thermalservice
            "#;

        let batch_output = self.shell(batch_cmd).await?;
        let top_output = self.shell("top -n 1 -b -H").await?;

        let mut metrics = Metrics::default();

        if let Some(battery_section) = Self::extract_section(&batch_output, "===BATTERY===", "===STAT===") {
            if let Some(level) = battery_section
                .chars()
                .filter(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse::<u8>()
                .ok()
            {
                metrics.battery_level = level;
            }
        }

        if let Some(stat_section) = Self::extract_section(&batch_output, "===STAT===", "===FREQ===") {
            self.parse_cpu_stats(&stat_section, &mut metrics);
        }

        if let Some(freq_section) = Self::extract_section(&batch_output, "===FREQ===", "===THERMAL===") {
            self.parse_frequencies(&freq_section, &mut metrics);
        }

        if let Some(thermal_section) = Self::extract_section(&batch_output, "===THERMAL===", "") {
            self.parse_thermal(&thermal_section, &mut metrics);
        }

        self.parse_top_output(&top_output, package, &mut metrics);

        metrics.timestamp = chrono::Local::now().format("%H:%M:%S").to_string();

        Ok(metrics)
    }

    fn extract_section(output: &str, start: &str, end: &str) -> Option<String> {
        let start_idx = output.find(start)?;
        let content = &output[start_idx + start.len()..];

        if end.is_empty() {
            Some(content.to_string())
        } else {
            let end_idx = content.find(end).unwrap_or(content.len());
            Some(content[..end_idx].to_string())
        }
    }

    fn parse_cpu_stats(&mut self, stat: &str, metrics: &mut Metrics) {
        let actual_cores = stat.lines()
            .filter(|l| {
                if !l.starts_with("cpu") { return false; }
                let rest = &l[3..];
                rest.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false)
            })
            .count();

        if actual_cores > self.num_cores {
            self.num_cores = actual_cores;
        }

        if self.prev_total.len() < self.num_cores {
            self.prev_total.resize(self.num_cores, 0);
            self.prev_idle.resize(self.num_cores, 0);
        }

        while metrics.cpu_cores.len() < self.num_cores {
            let id = metrics.cpu_cores.len() as u8;
            metrics.cpu_cores.push(crate::data::CpuCore {
                id,
                ..Default::default()
            });
        }

        for line in stat.lines() {
            if line.starts_with("cpu") && line.chars().nth(3).map(|c| c.is_ascii_digit()).unwrap_or(false) {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 8 {
                    if let Ok(core_id) = parts[0][3..].parse::<usize>() {
                        if core_id < self.num_cores {
                            let user: u64 = parts[1].parse().unwrap_or(0);
                            let nice: u64 = parts[2].parse().unwrap_or(0);
                            let sys: u64 = parts[3].parse().unwrap_or(0);
                            let idle: u64 = parts[4].parse().unwrap_or(0);
                            let iow: u64 = parts[5].parse().unwrap_or(0);
                            let irq: u64 = parts[6].parse().unwrap_or(0);
                            let sirq: u64 = parts[7].parse().unwrap_or(0);

                            let total = user + nice + sys + idle + iow + irq + sirq;

                            let prev_total = self.prev_total[core_id];
                            let prev_idle = self.prev_idle[core_id];

                            if prev_total > 0 {
                                let dt = total.saturating_sub(prev_total);
                                let di = idle.saturating_sub(prev_idle);
                                if dt > 0 {
                                    let usage = ((dt - di) as f32 / dt as f32) * 100.0;
                                    metrics.cpu_cores[core_id].usage_percent = usage;
                                }
                            }

                            metrics.cpu_cores[core_id].id = core_id as u8;
                            self.prev_total[core_id] = total;
                            self.prev_idle[core_id] = idle;
                        }
                    }
                }
            }
        }
    }

    fn parse_frequencies(&self, freq_data: &str, metrics: &mut Metrics) {
        for (i, line) in freq_data.lines().filter(|l| !l.is_empty()).enumerate() {
            if i < metrics.cpu_cores.len() {
                if let Ok(freq) = line.trim().parse::<u32>() {
                    metrics.cpu_cores[i].frequency_mhz = freq / 1000;
                }
            }
        }
    }

    fn parse_thermal(&self, thermal_data: &str, metrics: &mut Metrics) {
        let extract_temp = |name: &str| -> f32 {
            thermal_data
                .lines()
                .find(|l| l.contains(&format!("mName={},", name)))
                .and_then(|l| {
                    l.split("mValue=")
                        .nth(1)
                        .and_then(|v| v.split(',').next())
                        .and_then(|v| v.parse::<f32>().ok())
                })
                .unwrap_or(0.0)
        };

        metrics.thermal.ap = extract_temp("AP");
        metrics.thermal.cp = extract_temp("CP");
        metrics.thermal.pa = extract_temp("PA");
        metrics.thermal.skin = extract_temp("SKIN");
        metrics.thermal.battery = extract_temp("BAT");
        metrics.thermal.usb = extract_temp("USB");
    }

    fn parse_top_output(&self, top_output: &str, package: &str, metrics: &mut Metrics) {
        let mut total_cpu = 0.0;
        let app_core_cpu = vec![0.0f32; self.num_cores];
        let app_core_threads = vec![0u32; self.num_cores];

        for line in top_output.lines().filter(|l| l.contains(package)) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 12 {
                let pid: u32 = parts[0].parse().unwrap_or(0);
                let cpu: f32 = parts[8].parse().unwrap_or(0.0);
                let mem: f32 = parts[9].parse().unwrap_or(0.0);
                let time = parts[10].to_string();
                let name = parts[11].to_string();

                total_cpu += cpu;

                if metrics.threads.len() < 10 {
                    metrics.threads.push(ThreadInfo {
                        pid,
                        name,
                        cpu_percent: cpu,
                        mem_percent: mem,
                        core: None,
                        time,
                    });
                }
            }
        }

        metrics.app_cpu_total = total_cpu;

        for (i, usage) in metrics.app_core_usage.iter_mut().enumerate() {
            usage.cpu_percent = app_core_cpu.get(i).copied().unwrap_or(0.0);
            usage.thread_count = app_core_threads.get(i).copied().unwrap_or(0);
        }
    }

    pub async fn detect_cores(&mut self) -> Result<()> {
        let output = self.shell("grep -c '^cpu[0-9]' /proc/stat").await?;
        let detected = output.trim().parse().unwrap_or(0);

        if detected == 0 {
            let output = self.shell("grep -c '^processor' /proc/cpuinfo").await?;
            self.num_cores = output.trim().parse().unwrap_or(8);
        } else {
            self.num_cores = detected;
        }

        self.prev_total = vec![0; self.num_cores];
        self.prev_idle = vec![0; self.num_cores];
        Ok(())
    }
}

impl Default for Adb {
    fn default() -> Self {
        Self::new()
    }
}
