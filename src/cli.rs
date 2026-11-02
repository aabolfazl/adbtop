use std::path::PathBuf;
use std::time::Duration;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::collect::Groups;

#[derive(Parser, Debug)]
#[command(
    name = "android-perf-monitor",
    version,
    about = "Android performance monitor: an interactive TUI, or headless recording that writes JSON for scripts, CI and AI agents",
    after_help = "Without a subcommand the interactive TUI starts.\n\nExit codes: 0 ok, 1 error, 2 bad arguments, 3 an --assert failed, 4 device or app not found, 5 capture agent missing or not allowed."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
    #[command(flatten)]
    pub common: Common,
    #[arg(
        long,
        help = "TUI color theme, e.g. \"tokyo night\" (list: android-perf-monitor themes)"
    )]
    pub theme: Option<String>,
}

#[derive(Args, Debug, Clone, Default)]
pub struct Common {
    #[arg(
        short = 'd',
        long,
        global = true,
        value_name = "SERIAL",
        help = "ADB device serial (default: the only connected device)"
    )]
    pub device: Option<String>,
    #[arg(short = 'p', long, global = true, value_name = "PACKAGE", value_parser = parse_package, help = "App package to measure (default: whole device)")]
    pub package: Option<String>,
    #[arg(short = 'i', long, global = true, value_name = "DURATION", value_parser = parse_duration, help = "Sample interval, e.g. 1s or 500ms [default: 1s]")]
    pub interval: Option<Duration>,
    #[arg(
        long,
        global = true,
        help = "Start the on-device network capture agent (android-agent/)"
    )]
    pub capture: bool,
    #[arg(
        long,
        global = true,
        value_name = "FILE",
        help = "Save every captured packet to a pcap file (implies --capture)"
    )]
    pub pcap: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    #[command(about = "Record without the UI and write a JSON report (stdout by default)")]
    Record(RecordArgs),
    #[command(about = "Take one sample of everything and print it as JSON")]
    Snapshot(SnapshotArgs),
    #[command(about = "Add a marker to the running recording, e.g. mark \"tapped login\"")]
    Mark {
        #[arg(required = true)]
        label: Vec<String>,
        #[arg(
            long,
            value_name = "ID",
            help = "Session id (default: the most recent recording)"
        )]
        session: Option<String>,
    },
    #[command(about = "Stop the running recording; it writes its report and exits")]
    Stop {
        #[arg(long, value_name = "ID")]
        session: Option<String>,
    },
    #[command(about = "List connected devices as JSON")]
    Devices,
    #[command(about = "List running app packages on the device as JSON")]
    Packages,
    #[command(about = "List the TUI color themes as JSON")]
    Themes,
}

#[derive(Args, Debug, Clone)]
pub struct RecordArgs {
    #[arg(long, value_name = "DURATION", value_parser = parse_duration, help = "Stop after this long, e.g. 60s or 5m (default: until `stop` or Ctrl+C)")]
    pub duration: Option<Duration>,
    #[arg(
        short = 'm',
        long,
        value_enum,
        value_delimiter = ',',
        value_name = "LIST",
        help = "Metrics to collect [default: all]"
    )]
    pub metrics: Vec<Metric>,
    #[arg(
        short = 'o',
        long,
        value_name = "FILE",
        help = "Write the JSON report to FILE ('-' = stdout) [default: stdout]"
    )]
    pub out: Option<PathBuf>,
    #[arg(
        long,
        value_name = "FILE",
        help = "Stream one sample per interval: JSON lines, or CSV when FILE ends in .csv ('-' = stdout)"
    )]
    pub samples: Option<PathBuf>,
    #[arg(
        long = "assert",
        value_name = "EXPR",
        help = "Check the report, e.g. 'frames.janky_pct < 5'; exit code 3 if any check fails"
    )]
    pub asserts: Vec<String>,
    #[arg(long, value_name = "DURATION", value_parser = parse_duration, default_value = "15s", help = "How long after each marker the report summarises")]
    pub after_marker: Duration,
    #[arg(long, help = "Write the report as one line of JSON")]
    pub compact: bool,
    #[arg(short, long, help = "No progress messages on stderr")]
    pub quiet: bool,
}

#[derive(Args, Debug, Clone)]
pub struct SnapshotArgs {
    #[arg(
        short = 'm',
        long,
        value_enum,
        value_delimiter = ',',
        value_name = "LIST"
    )]
    pub metrics: Vec<Metric>,
    #[arg(short = 'o', long, value_name = "FILE")]
    pub out: Option<PathBuf>,
    #[arg(long)]
    pub compact: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Metric {
    All,
    Cpu,
    Threads,
    Memory,
    Frames,
    Network,
    Power,
}

pub fn groups(metrics: &[Metric]) -> Groups {
    if metrics.is_empty() || metrics.contains(&Metric::All) {
        return Groups::ALL;
    }
    let has = |m: Metric| metrics.contains(&m);
    Groups {
        cpu: has(Metric::Cpu) || has(Metric::Threads),
        threads: has(Metric::Threads) || has(Metric::Cpu),
        memory: has(Metric::Memory),
        frames: has(Metric::Frames),
        network: has(Metric::Network),
        power: has(Metric::Power),
    }
}

fn parse_package(value: &str) -> Result<String, String> {
    if crate::parse::is_valid_package_name(value) {
        Ok(value.to_string())
    } else {
        Err(format!("'{value}' is not a valid Android package name"))
    }
}

pub fn parse_duration(value: &str) -> Result<Duration, String> {
    let v = value.trim();
    let (number, unit) = match v.find(|c: char| c.is_ascii_alphabetic()) {
        Some(i) => (&v[..i], &v[i..]),
        None => (v, "s"),
    };
    let n: f64 = number
        .trim()
        .parse()
        .map_err(|_| format!("'{value}' is not a duration (try 500ms, 2s, 5m)"))?;
    let secs = match unit {
        "ms" => n / 1000.0,
        "s" | "sec" => n,
        "m" | "min" => n * 60.0,
        "h" => n * 3600.0,
        _ => return Err(format!("unknown unit in '{value}' (use ms, s, m or h)")),
    };
    if !(secs > 0.0 && secs.is_finite()) {
        return Err("duration must be positive".into());
    }
    Duration::try_from_secs_f64(secs).map_err(|_| format!("'{value}' is too long"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(parse_duration("500ms").unwrap(), Duration::from_millis(500));
        assert_eq!(parse_duration("2").unwrap(), Duration::from_secs(2));
        assert_eq!(parse_duration("1.5s").unwrap(), Duration::from_millis(1500));
        assert_eq!(parse_duration("5m").unwrap(), Duration::from_secs(300));
        assert!(parse_duration("0s").is_err());
        assert!(parse_duration("3d").is_err());
    }

    #[test]
    fn metric_groups() {
        assert_eq!(groups(&[]), Groups::ALL);
        let g = groups(&[Metric::Memory, Metric::Network]);
        assert!(g.memory && g.network && !g.frames && !g.cpu);
    }

    #[test]
    fn cli_parses_record() {
        let cli = Cli::try_parse_from([
            "apm",
            "record",
            "-p",
            "com.example.app",
            "--duration",
            "30s",
            "-m",
            "cpu,memory",
            "--assert",
            "frames.janky_pct < 5",
        ])
        .unwrap();
        assert_eq!(cli.common.package.as_deref(), Some("com.example.app"));
        match cli.command {
            Some(Command::Record(r)) => {
                assert_eq!(r.duration, Some(Duration::from_secs(30)));
                assert_eq!(r.metrics, [Metric::Cpu, Metric::Memory]);
                assert_eq!(r.asserts.len(), 1);
            }
            _ => panic!("expected record"),
        }
    }
}
