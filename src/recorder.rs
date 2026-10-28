use anyhow::{Context, Result};
use std::fs::File;
use std::io::{BufWriter, Write};

use crate::app::App;

const HEADER: &str = "timestamp,device,package,sys_cpu_pct,app_cpu_pct,mem_used_mb,app_pss_mb,\
battery_pct,battery_temp_c,cpu_temp_c,skin_temp_c,net_rx_bytes,net_tx_bytes";

/// Writes one CSV row per CPU sample (`--output`).
pub struct CsvRecorder {
    out: BufWriter<File>,
}

impl CsvRecorder {
    pub fn create(path: &str) -> Result<Self> {
        let file = File::create(path).with_context(|| format!("Cannot create CSV file {}", path))?;
        let mut out = BufWriter::new(file);
        writeln!(out, "{}", HEADER)?;
        out.flush()?;
        Ok(Self { out })
    }

    /// `app_cpu` is left empty when there was no reading for this sample.
    pub fn record(&mut self, app: &App, app_cpu: Option<f32>) -> Result<()> {
        let cores = &app.metrics.cpu_cores;
        let sys_cpu = if cores.is_empty() {
            0.0
        } else {
            cores.iter().map(|c| c.usage_percent).sum::<f32>() / cores.len() as f32
        };
        let mem = &app.memory_data;
        let mem_used_mb = mem.total_kb.saturating_sub(mem.available_kb) / 1024;
        let app_cpu = match (&app.package, app_cpu) {
            (Some(_), Some(cpu)) => format!("{:.1}", cpu),
            _ => String::new(),
        };
        let app_pss = if app.package.is_some() { (mem.app_pss_kb / 1024).to_string() } else { String::new() };

        writeln!(
            self.out,
            "{},{},{},{:.1},{},{},{},{},{:.1},{:.1},{:.1},{},{}",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
            app.device_serial.as_deref().unwrap_or(""),
            app.package.as_deref().unwrap_or(""),
            sys_cpu,
            app_cpu,
            mem_used_mb,
            app_pss,
            app.battery_data.level,
            app.battery_data.temperature,
            app.metrics.thermal.ap,
            app.metrics.thermal.skin,
            app.net_rx_history.last().copied().unwrap_or(0),
            app.net_tx_history.last().copied().unwrap_or(0),
        )?;
        // Flush every row so a crash or Ctrl+C doesn't lose samples.
        self.out.flush()?;
        Ok(())
    }
}
