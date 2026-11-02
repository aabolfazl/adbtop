use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;

use crate::collect::Groups;
use crate::model::*;
use crate::session::{Series, Session};

pub const SCHEMA: &str = "android-perf-monitor/report@1";

fn finite(v: f32) -> Option<f32> {
    v.is_finite().then_some(v)
}

fn r1(v: f32) -> f32 {
    (v * 10.0).round() / 10.0
}

fn r2(v: f32) -> f32 {
    (v * 100.0).round() / 100.0
}

#[derive(Debug, Clone, Serialize)]
pub struct Stat {
    pub avg: f32,
    pub min: f32,
    pub max: f32,
    pub p95: f32,
    pub last: f32,
}

impl Stat {
    pub fn of(s: &Series) -> Option<Stat> {
        Some(Stat {
            avg: r2(s.avg()?),
            min: r2(s.min()?),
            max: r2(s.max()?),
            p95: r2(s.percentile(0.95)?),
            last: r2(s.last()?),
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Trend {
    pub start: f32,
    pub end: f32,
    pub min: f32,
    pub max: f32,
    pub avg: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub growth_per_min: Option<f32>,
}

impl Trend {
    fn of(s: &Series, interval_s: f32) -> Option<Trend> {
        Some(Trend {
            start: r1(s.first()?),
            end: r1(s.last()?),
            min: r1(s.min()?),
            max: r1(s.max()?),
            avg: r1(s.avg()?),
            growth_per_min: s.slope_per_min(interval_s).map(r2),
        })
    }
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub schema: &'static str,
    pub tool: String,
    pub mode: &'static str,
    pub device: DeviceOut,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<AppOut>,
    pub session: SessionOut,
    pub unsupported: BTreeMap<&'static str, String>,
    pub markers: Vec<MarkerOut>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu: Option<CpuOut>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory: Option<MemoryOut>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frames: Option<FramesOut>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<NetworkOut>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub power: Option<PowerOut>,
    pub after_markers: Vec<AfterMarker>,
    pub warnings: Vec<String>,
    pub assertions: Vec<AssertionOut>,
}

#[derive(Debug, Serialize)]
pub struct DeviceOut {
    pub serial: Option<String>,
    pub model: String,
    pub manufacturer: String,
    pub android: String,
    pub sdk: u32,
    pub cpu_clusters: Vec<ClusterOut>,
}

#[derive(Debug, Serialize)]
pub struct ClusterOut {
    pub name: &'static str,
    pub cores: Vec<u16>,
    pub max_ghz: f32,
}

#[derive(Debug, Serialize)]
pub struct AppOut {
    pub package: String,
    pub state: AppState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    pub restarts: u32,
}

#[derive(Debug, Serialize)]
pub struct SessionOut {
    pub started: String,
    pub duration_s: f32,
    pub interval_s: f32,
    pub samples: u64,
}

#[derive(Debug, Serialize)]
pub struct MarkerOut {
    pub t_s: f32,
    pub label: String,
}

#[derive(Debug, Serialize)]
pub struct ThreadOut {
    pub tid: u32,
    pub name: String,
    pub main: bool,
    pub state: char,
    pub now: f32,
    pub avg: f32,
    pub max: f32,
}

#[derive(Debug, Serialize)]
pub struct ClusterNow {
    pub name: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ghz: Option<Stat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cap_min_ghz: Option<f32>,
    pub max_ghz: f32,
}

#[derive(Debug, Serialize)]
pub struct CpuOut {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_pct: Option<Stat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_kernel_pct: Option<Stat>,
    pub system_pct: Option<Stat>,
    pub cores_pct: Vec<Option<f32>>,
    pub clusters: Vec<ClusterNow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threads: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ctx_switches_ps: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_faults_ps: Option<f32>,
    pub top_threads: Vec<ThreadOut>,
    pub unit_note: &'static str,
}

#[derive(Debug, Serialize)]
pub struct CategoriesOut {
    pub java: f32,
    pub native: f32,
    pub code: f32,
    pub stack: f32,
    pub graphics: f32,
    pub other: f32,
    pub system: f32,
}

#[derive(Debug, Serialize)]
pub struct HeapOut {
    pub size_mb: Option<Trend>,
    pub alloc_mb: Option<Trend>,
    pub gc_estimate: u32,
}

#[derive(Debug, Serialize)]
pub struct DeviceMemOut {
    pub total_gb: f32,
    pub available_gb: Option<Stat>,
    pub swap_used_gb: Option<Stat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub psi_some_avg10: Option<Stat>,
}

#[derive(Debug, Serialize)]
pub struct MemoryOut {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pss_mb: Option<Trend>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rss_mb: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub swap_pss_mb: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub by_category_mb: Option<CategoriesOut>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub java_heap: Option<HeapOut>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub objects: BTreeMap<&'static str, [u32; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sqlite: Option<Sqlite>,
    pub device: DeviceMemOut,
}

#[derive(Debug, Serialize)]
pub struct FramesOut {
    pub rendered: u64,
    pub janky: u64,
    pub janky_pct: f32,
    pub p50_ms: u32,
    pub p90_ms: u32,
    pub p95_ms: u32,
    pub p99_ms: u32,
    pub gpu_p50_ms: u32,
    pub gpu_p99_ms: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_hz: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fps: Option<Stat>,
    pub causes: BTreeMap<&'static str, u64>,
}

#[derive(Debug, Serialize)]
pub struct HostOut {
    pub host: String,
    pub connections: u32,
    pub rx: u64,
    pub tx: u64,
}

#[derive(Debug, Serialize)]
pub struct FailureOut {
    pub host: String,
    pub remote: String,
    pub error: String,
}

#[derive(Debug, Serialize)]
pub struct NetworkOut {
    pub source: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rx_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tx_bytes: Option<u64>,
    pub rx_bps: Option<Stat>,
    pub tx_bps: Option<Stat>,
    pub connections: u32,
    pub open: u32,
    pub failed: u32,
    pub top_hosts: Vec<HostOut>,
    pub failures: Vec<FailureOut>,
    pub dns_lookups: u32,
    pub dns_failures: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct BatteryOut {
    pub level_start: u8,
    pub level_end: u8,
    pub status: String,
    pub plugged: String,
    pub temperature_c: f32,
    pub voltage_mv: u32,
}

#[derive(Debug, Serialize)]
pub struct ThermalOut {
    pub status: &'static str,
    pub worst: &'static str,
    pub max_c: BTreeMap<String, f32>,
}

#[derive(Debug, Serialize)]
pub struct PowerOut {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub battery: Option<BatteryOut>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drain_ma: Option<Stat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used_mah: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thermal: Option<ThermalOut>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doze: Option<Doze>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jobs: Option<Jobs>,
}

#[derive(Debug, Serialize)]
pub struct AfterMarker {
    pub label: String,
    pub window_s: [f32; 2],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_app_max: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_system_max: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pss_mb_delta: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rx_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub janky_frames: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drain_ma_avg: Option<f32>,
}

#[derive(Debug, Serialize)]
pub struct AssertionOut {
    pub expr: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    pub pass: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn net_series(s: &Session) -> (&'static str, &Series, &Series) {
    if s.history.cap_rx.has_data() {
        ("capture", &s.history.cap_rx, &s.history.cap_tx)
    } else {
        ("device", &s.history.dev_rx, &s.history.dev_tx)
    }
}

pub fn build(s: &Session, mode: &'static str, groups: Groups, after_window_s: f32) -> Report {
    let caps = s.caps.clone().unwrap_or_default();
    let interval = s.interval_s();
    let h = &s.history;
    let device = DeviceOut {
        serial: s.serial.clone(),
        model: caps.model.clone(),
        manufacturer: caps.manufacturer.clone(),
        android: caps.release.clone(),
        sdk: caps.sdk,
        cpu_clusters: caps
            .clusters
            .iter()
            .map(|c| ClusterOut {
                name: c.name,
                cores: c.cores.clone(),
                max_ghz: r2(c.max_khz as f32 / 1e6),
            })
            .collect(),
    };
    let app = s.package.as_ref().map(|p| AppOut {
        package: p.clone(),
        state: s.app_state,
        pid: s.fast.as_ref().and_then(|f| f.app.as_ref()).map(|a| a.pid),
        restarts: s.app_restarts,
    });
    let unsupported: BTreeMap<&'static str, String> = Source::ALL
        .iter()
        .filter_map(|src| match s.support(*src) {
            Support::Unsupported(reason) => Some((src.label(), reason.clone())),
            _ => None,
        })
        .collect();

    let fast = s.fast.as_ref();
    let cpu = groups.cpu.then(|| {
        let mut top: Vec<ThreadOut> = s
            .tasks_sorted()
            .into_iter()
            .map(|(t, hist)| ThreadOut {
                tid: t.id,
                name: s.task_display_name(t).to_string(),
                main: fast.and_then(|f| f.app.as_ref()).is_some_and(|a| a.pid == t.id),
                state: t.state,
                now: r1(t.cpu),
                avg: r1(hist.map_or(0.0, |h| h.avg())),
                max: r1(hist.map_or(0.0, |h| h.peak)),
            })
            .collect();
        top.sort_by(|a, b| b.main.cmp(&a.main).then(b.avg.partial_cmp(&a.avg).unwrap_or(std::cmp::Ordering::Equal)));
        top.truncate(15);
        let app_proc = fast.and_then(|f| f.app.as_ref());
        CpuOut {
            app_pct: Stat::of(&h.app_cpu),
            app_kernel_pct: Stat::of(&h.app_kernel),
            system_pct: Stat::of(&h.sys_cpu),
            cores_pct: h.cores.iter().map(|c| c.avg().map(r1)).collect(),
            clusters: caps
                .clusters
                .iter()
                .enumerate()
                .map(|(i, c)| ClusterNow {
                    name: c.name,
                    ghz: h.cluster_ghz.get(i).and_then(Stat::of),
                    cap_min_ghz: h.cluster_cap_ghz.get(i).and_then(|x| x.min()).map(r2),
                    max_ghz: r2(c.max_khz as f32 / 1e6),
                })
                .collect(),
            threads: app_proc.map(|a| a.threads),
            ctx_switches_ps: app_proc.map(|a| r1(a.ctx_voluntary_ps + a.ctx_involuntary_ps)),
            page_faults_ps: app_proc.map(|a| r1(a.minflt_ps + a.majflt_ps)),
            top_threads: top,
            unit_note: "app_pct and system_pct are shares of all cores (100 = every core busy); thread values are % of one core",
        }
    });

    let memory = groups.memory.then(|| {
        let m = s.app_mem.as_ref();
        let first = s.app_mem_first.as_ref();
        let mut objects = BTreeMap::new();
        if let (Some(a), Some(b)) = (first.and_then(|f| f.objects), m.and_then(|m| m.objects)) {
            for (name, x, y) in [
                ("activities", a.activities, b.activities),
                ("app_contexts", a.app_contexts, b.app_contexts),
                ("views", a.views, b.views),
                ("view_roots", a.view_roots, b.view_roots),
                ("webviews", a.webviews, b.webviews),
                ("local_binders", a.local_binders, b.local_binders),
                ("proxy_binders", a.proxy_binders, b.proxy_binders),
                ("death_recipients", a.death_recipients, b.death_recipients),
                ("assets", a.assets, b.assets),
                ("parcel_count", a.parcel_count, b.parcel_count),
            ] {
                objects.insert(name, [x, y]);
            }
        }
        let total_gb = fast
            .and_then(|f| f.mem)
            .map_or(0.0, |m| r2(m.total_kb as f32 / 1048576.0));
        MemoryOut {
            pss_mb: Trend::of(&h.pss_mb, interval),
            rss_mb: m.map(|m| r1(m.rss_kb as f32 / 1024.0)),
            swap_pss_mb: m.map(|m| r1(m.swap_pss_kb as f32 / 1024.0)),
            by_category_mb: m.map(|m| CategoriesOut {
                java: r1(m.java_kb as f32 / 1024.0),
                native: r1(m.native_kb as f32 / 1024.0),
                code: r1(m.code_kb as f32 / 1024.0),
                stack: r1(m.stack_kb as f32 / 1024.0),
                graphics: r1(m.graphics_kb as f32 / 1024.0),
                other: r1(m.other_kb as f32 / 1024.0),
                system: r1(m.system_kb as f32 / 1024.0),
            }),
            java_heap: m.map(|_| HeapOut {
                size_mb: Trend::of(&h.heap_size_mb, interval),
                alloc_mb: Trend::of(&h.heap_alloc_mb, interval),
                gc_estimate: h.heap_alloc_mb.drops(1.0),
            }),
            objects,
            sqlite: m.and_then(|m| m.sqlite.clone()),
            device: DeviceMemOut {
                total_gb,
                available_gb: Stat::of(&h.mem_avail_gb),
                swap_used_gb: Stat::of(&h.swap_used_gb),
                psi_some_avg10: Stat::of(&h.psi_some),
            },
        }
    });

    let frames = if groups.frames {
        s.frames_since_start()
    } else {
        None
    }
    .map(|f| {
        let refresh = s.frames.back().map(|f| r1(1000.0 / f.budget_ms));
        FramesOut {
            rendered: f.total,
            janky: f.janky,
            janky_pct: if f.total > 0 {
                r2(f.janky as f32 * 100.0 / f.total as f32)
            } else {
                0.0
            },
            p50_ms: f.p50_ms,
            p90_ms: f.p90_ms,
            p95_ms: f.p95_ms,
            p99_ms: f.p99_ms,
            gpu_p50_ms: f.gpu_p50_ms,
            gpu_p99_ms: f.gpu_p99_ms,
            refresh_hz: refresh,
            fps: Stat::of(&h.fps),
            causes: BTreeMap::from([
                ("high_input_latency", f.high_input_latency),
                ("slow_ui_thread", f.slow_ui_thread),
                ("missed_vsync", f.missed_vsync),
                ("deadline_missed", f.deadline_missed),
                ("slow_draw_commands", f.slow_draw_commands),
                ("slow_bitmap_uploads", f.slow_bitmap_uploads),
            ]),
        }
    });

    let network = groups.network.then(|| {
        let (source, rx, tx) = net_series(s);
        let conns = s.connections();
        let traffic = (source == "capture").then(|| s.capture.traffic(s.app_uids().as_deref()));
        NetworkOut {
            source,
            rx_bytes: traffic.map(|t| t.0),
            tx_bytes: traffic.map(|t| t.1),
            rx_bps: Stat::of(rx),
            tx_bps: Stat::of(tx),
            connections: conns.len() as u32,
            open: conns
                .iter()
                .filter(|c| c.state == crate::agent::proto::ConnectionState::Open as i32)
                .count() as u32,
            failed: conns
                .iter()
                .filter(|c| c.state == crate::agent::proto::ConnectionState::Failed as i32)
                .count() as u32,
            top_hosts: s
                .hosts()
                .into_iter()
                .take(10)
                .map(|h| HostOut {
                    host: h.host,
                    connections: h.connections,
                    rx: h.rx,
                    tx: h.tx,
                })
                .collect(),
            failures: conns
                .iter()
                .filter(|c| !c.error.is_empty())
                .take(20)
                .map(|c| FailureOut {
                    host: c.host.clone(),
                    remote: c.remote.clone(),
                    error: c.error.clone(),
                })
                .collect(),
            dns_lookups: s.dns().len() as u32,
            dns_failures: s
                .dns()
                .into_iter()
                .filter(|d| d.failed)
                .map(|d| d.name.clone())
                .take(20)
                .collect(),
        }
    });

    let power = groups.power.then(|| PowerOut {
        battery: s.battery.as_ref().map(|b| BatteryOut {
            level_start: s.battery_first.as_ref().map_or(b.level, |f| f.level),
            level_end: b.level,
            status: b.status.clone(),
            plugged: b.plugged.clone(),
            temperature_c: r1(b.temperature_c),
            voltage_mv: b.voltage_mv,
        }),
        drain_ma: Stat::of(&h.drain_ma),
        used_mah: s.used_mah().map(r1),
        thermal: s.thermal.as_ref().map(|t| ThermalOut {
            status: thermal_level_name(t.status),
            worst: thermal_level_name(s.thermal_worst),
            max_c: h
                .temps
                .iter()
                .filter_map(|(n, x)| x.max().map(|m| (n.clone(), r1(m))))
                .collect(),
        }),
        doze: s.doze.clone(),
        jobs: s.jobs.clone(),
    });

    let after_markers = s
        .markers
        .iter()
        .map(|m| {
            let (a, b) = s.marker_window(m, after_window_s);
            let max = |x: &Series| {
                x.range_values(a, b)
                    .fold(None, |acc: Option<f32>, v| {
                        Some(acc.map_or(v, |p| p.max(v)))
                    })
                    .map(r1)
            };
            let pss: Vec<f32> = h.pss_mb.range_values(a, b).collect();
            let rx: f32 = net_series(s).1.range_values(a, b).sum::<f32>() * interval;
            let janky: f32 = h
                .fps
                .range_values(a, b)
                .zip(h.jank_pct.range_values(a, b))
                .map(|(f, j)| f * j / 100.0 * interval)
                .sum();
            let drain: Vec<f32> = h.drain_ma.range_values(a, b).collect();
            AfterMarker {
                label: m.label.clone(),
                window_s: [0.0, r1((b - a) as f32 * interval)],
                cpu_app_max: max(&h.app_cpu),
                cpu_system_max: max(&h.sys_cpu),
                pss_mb_delta: (pss.len() >= 2).then(|| r1(pss[pss.len() - 1] - pss[0])),
                rx_bytes: (rx > 0.0).then_some(rx as u64),
                janky_frames: h.fps.has_data().then_some(janky.round() as u32),
                drain_ma_avg: (!drain.is_empty())
                    .then(|| r1(drain.iter().sum::<f32>() / drain.len() as f32)),
            }
        })
        .collect();

    let mut report = Report {
        schema: SCHEMA,
        tool: format!("android-perf-monitor {}", env!("CARGO_PKG_VERSION")),
        mode,
        device,
        app,
        session: SessionOut {
            started: s
                .started_at
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
            duration_s: r1(s.elapsed().as_secs_f32()),
            interval_s: interval,
            samples: s.ticks,
        },
        unsupported,
        markers: s
            .markers
            .iter()
            .map(|m| MarkerOut {
                t_s: r1(m.secs as f32),
                label: m.label.clone(),
            })
            .collect(),
        cpu,
        memory,
        frames,
        network,
        power,
        after_markers,
        warnings: Vec::new(),
        assertions: Vec::new(),
    };
    report.warnings = warnings(s, &report);
    report
}

fn warnings(s: &Session, r: &Report) -> Vec<String> {
    let mut w = Vec::new();
    if let Some(app) = &r.app {
        match app.state {
            AppState::NotRunning => w.push(format!(
                "app: {} was not running at the end of the session",
                app.package
            )),
            _ if app.restarts > 0 => w.push(format!(
                "app: {} restarted {} time(s) during the session",
                app.package, app.restarts
            )),
            _ => {}
        }
    }
    if let Some(cpu) = &r.cpu {
        if let Some(app) = &cpu.app_pct {
            if app.p95 > 50.0 {
                w.push(format!(
                    "cpu: app used {:.0}% of all cores at p95 (avg {:.1}%)",
                    app.p95, app.avg
                ));
            }
        }
        for c in &cpu.clusters {
            if let Some(cap) = c.cap_min_ghz {
                if cap + 0.05 < c.max_ghz {
                    w.push(format!(
                        "cpu: {} cores were capped at {:.2} of {:.2} GHz (thermal or power limit)",
                        c.name, cap, c.max_ghz
                    ));
                }
            }
        }
    }
    if let Some(m) = &r.memory {
        if let Some(g) = m.pss_mb.as_ref().and_then(|p| p.growth_per_min) {
            let minutes = s.elapsed().as_secs_f32() / 60.0;
            if g > 0.5 && minutes >= 1.0 {
                w.push(format!(
                    "memory: PSS grew {g:.2} MB/min over {minutes:.1} min"
                ));
            }
        }
        if let (Some(a), Some(c)) = (m.objects.get("activities"), m.objects.get("app_contexts")) {
            if c[1] > c[0] && a[1] <= a[0] {
                w.push(format!("memory: AppContexts grew {} → {} while Activities stayed at {} (possible Context leak)", c[0], c[1], a[1]));
            }
            if a[1] >= a[0] + 2 {
                w.push(format!(
                    "memory: Activities grew {} → {} (check that closed screens are released)",
                    a[0], a[1]
                ));
            }
        }
        let alloc = &s.history.heap_alloc_mb;
        let n = alloc.len();
        if n >= 60 {
            let third = n / 3;
            let early = alloc.range_values(0, third).fold(f32::INFINITY, f32::min);
            let late = alloc
                .range_values(n - third, n)
                .fold(f32::INFINITY, f32::min);
            if early.is_finite() && late.is_finite() && late - early > 2.0 {
                w.push(format!(
                    "memory: Java heap floor after GC rose {early:.1} → {late:.1} MB"
                ));
            }
        }
        if let Some(sql) = &m.sqlite {
            for db in &sql.databases {
                let total = db.cache_hits + db.cache_misses;
                if total > 500 && (db.cache_hits as f32 / total as f32) < 0.1 {
                    w.push(format!("sqlite: {} statement cache hit rate is {:.1}% (many distinct SQL strings?)", db.name, db.cache_hits as f32 * 100.0 / total as f32));
                }
            }
        }
    }
    if let Some(f) = &r.frames {
        if f.rendered >= 30 && f.janky_pct > 5.0 {
            w.push(format!(
                "frames: {:.1}% of {} frames were janky (p99 {} ms)",
                f.janky_pct, f.rendered, f.p99_ms
            ));
        }
        if let Some(slow) = f.causes.get("slow_ui_thread") {
            if f.janky > 0 && *slow * 2 >= f.janky {
                w.push(format!(
                    "frames: {slow} frames had a slow UI thread (main thread work)"
                ));
            }
        }
    }
    if let Some(n) = &r.network {
        for f in n.failures.iter().take(5) {
            let host = if f.host.is_empty() {
                &f.remote
            } else {
                &f.host
            };
            w.push(format!("network: {host} failed: {}", f.error));
        }
        for d in n.dns_failures.iter().take(5) {
            w.push(format!("network: DNS lookup for {d} failed"));
        }
    }
    if let Some(p) = &r.power {
        if let Some(t) = &p.thermal {
            if t.worst != "NONE" {
                w.push(format!("thermal: reached {} throttling", t.worst));
            }
        }
        if let Some(b) = &p.battery {
            if b.plugged != "none" {
                w.push(format!("power: the phone was plugged in ({}), so battery drain does not reflect the app; use adb over Wi-Fi", b.plugged));
            }
        }
    }
    w
}

pub fn evaluate_assertions(report: &mut Report, exprs: &[String]) -> bool {
    let value = serde_json::to_value(&*report).unwrap_or(Value::Null);
    let mut all = true;
    report.assertions = exprs
        .iter()
        .map(|expr| {
            let out = evaluate(&value, expr);
            all &= out.pass;
            out
        })
        .collect();
    all
}

fn evaluate(root: &Value, expr: &str) -> AssertionOut {
    let fail = |error: String| AssertionOut {
        expr: expr.to_string(),
        value: None,
        pass: false,
        error: Some(error),
    };
    let ops = ["<=", ">=", "==", "!=", "<", ">"];
    let Some((op, pos)) = ops
        .iter()
        .filter_map(|op| expr.find(op).map(|p| (*op, p)))
        .min_by_key(|(_, p)| *p)
    else {
        return fail("expected PATH OP NUMBER, e.g. frames.janky_pct < 5".into());
    };
    let path = expr[..pos].trim();
    let Ok(limit) = expr[pos + op.len()..].trim().parse::<f64>() else {
        return fail("the right side must be a number".into());
    };
    let mut node = root;
    for part in path.split('.') {
        node = match node {
            Value::Object(map) => match map.get(part) {
                Some(v) => v,
                None => return fail(format!("{path} is not in the report")),
            },
            Value::Array(items) => match part.parse::<usize>().ok().and_then(|i| items.get(i)) {
                Some(v) => v,
                None => return fail(format!("{path}: no item {part}")),
            },
            _ => return fail(format!("{path} is not in the report")),
        };
    }
    let actual = match node {
        Value::Number(n) => n.as_f64(),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    };
    let Some(actual) = actual else {
        return fail(format!("{path} is not a number"));
    };
    let pass = match op {
        "<" => actual < limit,
        "<=" => actual <= limit,
        ">" => actual > limit,
        ">=" => actual >= limit,
        "==" => (actual - limit).abs() < 1e-9,
        _ => (actual - limit).abs() >= 1e-9,
    };
    AssertionOut {
        expr: expr.to_string(),
        value: Some(actual),
        pass,
        error: None,
    }
}

#[derive(Serialize)]
pub struct SampleLine<'a> {
    pub t: f32,
    pub tick: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub marker: Option<&'a str>,
    pub cpu: SampleCpu,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mem: Option<SampleMem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frames: Option<SampleFrames>,
    pub net: SampleNet,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub power: Option<SamplePower>,
}

#[derive(Serialize)]
pub struct SampleCpu {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_kernel: Option<f32>,
    pub system: f32,
    pub cores: Vec<f32>,
}

#[derive(Serialize)]
pub struct SampleMem {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pss_mb: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub java_mb: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_mb: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub graphics_mb: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available_gb: Option<f32>,
}

#[derive(Serialize)]
pub struct SampleFrames {
    pub fps: f32,
    pub jank_pct: f32,
}

#[derive(Serialize)]
pub struct SampleNet {
    pub source: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rx_bps: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tx_bps: Option<f32>,
}

#[derive(Serialize)]
pub struct SamplePower {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drain_ma: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thermal: Option<u8>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub temps_c: BTreeMap<String, f32>,
}

fn last_value(s: &Series) -> Option<f32> {
    finite(s.get(s.len().wrapping_sub(1))).map(r2)
}

pub fn sample_line<'a>(s: &'a Session, marker: Option<&'a str>) -> SampleLine<'a> {
    let h = &s.history;
    let (source, rx, tx) = net_series(s);
    let mem = SampleMem {
        pss_mb: last_value(&h.pss_mb),
        java_mb: last_value(&h.java_mb),
        native_mb: last_value(&h.native_mb),
        graphics_mb: last_value(&h.graphics_mb),
        available_gb: last_value(&h.mem_avail_gb),
    };
    let has_mem = mem.pss_mb.is_some() || mem.available_gb.is_some();
    SampleLine {
        t: r2(s.elapsed().as_secs_f32()),
        tick: s.ticks,
        marker,
        cpu: SampleCpu {
            app: last_value(&h.app_cpu),
            app_kernel: last_value(&h.app_kernel),
            system: last_value(&h.sys_cpu).unwrap_or(0.0),
            cores: s
                .fast
                .as_ref()
                .map_or_else(Vec::new, |f| f.cores.iter().map(|c| r1(c.usage)).collect()),
        },
        mem: has_mem.then_some(mem),
        frames: last_value(&h.fps).map(|fps| SampleFrames {
            fps: r1(fps),
            jank_pct: last_value(&h.jank_pct).unwrap_or(0.0),
        }),
        net: SampleNet {
            source,
            rx_bps: last_value(rx).map(r1),
            tx_bps: last_value(tx).map(r1),
        },
        power: Some(SamplePower {
            drain_ma: last_value(&h.drain_ma),
            thermal: s.thermal.as_ref().map(|t| t.status),
            temps_c: s.thermal.as_ref().map_or_else(BTreeMap::new, |t| {
                t.sensors
                    .iter()
                    .map(|x| (x.name.clone(), r1(x.celsius)))
                    .collect()
            }),
        })
        .filter(|p| p.drain_ma.is_some() || p.thermal.is_some()),
    }
}

pub const CSV_HEADER: &str = "t,tick,marker,app_cpu,app_kernel,system_cpu,pss_mb,java_mb,native_mb,graphics_mb,available_gb,fps,jank_pct,net_source,rx_bps,tx_bps,drain_ma,thermal";

pub fn csv_line(line: &SampleLine) -> String {
    let o = |v: Option<f32>| v.map_or(String::new(), |v| v.to_string());
    let mem = line.mem.as_ref();
    let power = line.power.as_ref();
    format!(
        "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
        line.t,
        line.tick,
        line.marker
            .map_or(String::new(), |m| format!("\"{}\"", m.replace('"', "'"))),
        o(line.cpu.app),
        o(line.cpu.app_kernel),
        line.cpu.system,
        o(mem.and_then(|m| m.pss_mb)),
        o(mem.and_then(|m| m.java_mb)),
        o(mem.and_then(|m| m.native_mb)),
        o(mem.and_then(|m| m.graphics_mb)),
        o(mem.and_then(|m| m.available_gb)),
        o(line.frames.as_ref().map(|f| f.fps)),
        o(line.frames.as_ref().map(|f| f.jank_pct)),
        line.net.source,
        o(line.net.rx_bps),
        o(line.net.tx_bps),
        o(power.and_then(|p| p.drain_ma)),
        power
            .and_then(|p| p.thermal)
            .map_or(String::new(), |t| t.to_string()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn assertions_follow_paths_and_operators() {
        let v = json!({"frames": {"janky_pct": 2.7}, "network": {"failed": 0}, "list": [{"x": 5}], "s": "text"});
        assert!(evaluate(&v, "frames.janky_pct < 5").pass);
        assert!(!evaluate(&v, "frames.janky_pct >= 5").pass);
        assert!(evaluate(&v, "network.failed == 0").pass);
        assert!(evaluate(&v, "list.0.x <= 5").pass);
        assert!(evaluate(&v, "nope.x < 1").error.is_some());
        assert!(evaluate(&v, "s < 1").error.is_some());
        assert!(evaluate(&v, "frames.janky_pct").error.is_some());
    }
}
