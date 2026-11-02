use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use chrono::{DateTime, Local};

use crate::agent::proto;
use crate::collect::Update;
use crate::model::*;

#[derive(Debug, Clone)]
pub struct Series {
    buf: VecDeque<f32>,
    cap: usize,
    n: u64,
    sum: f64,
    min: f32,
    max: f32,
    first: f32,
}

impl Series {
    pub fn new(cap: usize) -> Self {
        Self {
            buf: VecDeque::with_capacity(cap.min(4096)),
            cap,
            n: 0,
            sum: 0.0,
            min: f32::INFINITY,
            max: f32::NEG_INFINITY,
            first: f32::NAN,
        }
    }

    pub fn push(&mut self, v: f32) {
        if self.buf.len() == self.cap {
            self.buf.pop_front();
        }
        self.buf.push_back(v);
        if v.is_finite() {
            if self.n == 0 {
                self.first = v;
            }
            self.n += 1;
            self.sum += v as f64;
            self.min = self.min.min(v);
            self.max = self.max.max(v);
        }
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn get(&self, i: usize) -> f32 {
        self.buf.get(i).copied().unwrap_or(f32::NAN)
    }

    pub fn last(&self) -> Option<f32> {
        self.buf.iter().rev().copied().find(|v| v.is_finite())
    }

    pub fn has_data(&self) -> bool {
        self.n > 0
    }

    pub fn avg(&self) -> Option<f32> {
        (self.n > 0).then(|| (self.sum / self.n as f64) as f32)
    }

    pub fn max(&self) -> Option<f32> {
        (self.n > 0).then_some(self.max)
    }

    pub fn min(&self) -> Option<f32> {
        (self.n > 0).then_some(self.min)
    }

    pub fn first(&self) -> Option<f32> {
        (self.n > 0).then_some(self.first)
    }

    pub fn percentile(&self, q: f32) -> Option<f32> {
        let mut v: Vec<f32> = self.buf.iter().copied().filter(|v| v.is_finite()).collect();
        if v.is_empty() {
            return None;
        }
        v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let idx = ((v.len() - 1) as f32 * q).round() as usize;
        Some(v[idx.min(v.len() - 1)])
    }

    pub fn slope_per_min(&self, interval_s: f32) -> Option<f32> {
        let points: Vec<(f32, f32)> = self
            .buf
            .iter()
            .enumerate()
            .filter(|(_, v)| v.is_finite())
            .map(|(i, v)| (i as f32 * interval_s / 60.0, *v))
            .collect();
        if points.len() < 10 || points.last().map_or(0.0, |p| p.0) - points[0].0 < 1.0 {
            return None;
        }
        let n = points.len() as f32;
        let mx = points.iter().map(|p| p.0).sum::<f32>() / n;
        let my = points.iter().map(|p| p.1).sum::<f32>() / n;
        let sxx: f32 = points.iter().map(|p| (p.0 - mx).powi(2)).sum();
        let sxy: f32 = points.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum();
        (sxx > 0.0).then(|| sxy / sxx)
    }

    pub fn drops(&self, threshold: f32) -> u32 {
        let mut prev = f32::NAN;
        let mut count = 0;
        for &v in &self.buf {
            if v.is_finite() {
                if prev.is_finite() && prev - v > threshold {
                    count += 1;
                }
                prev = v;
            }
        }
        count
    }

    pub fn range_values(&self, from: usize, to: usize) -> impl Iterator<Item = f32> + '_ {
        self.buf
            .iter()
            .skip(from)
            .take(to.saturating_sub(from))
            .copied()
            .filter(|v| v.is_finite())
    }
}

#[derive(Debug, Clone)]
pub struct History {
    pub cap: usize,
    pub app_cpu: Series,
    pub app_kernel: Series,
    pub sys_cpu: Series,
    pub cores: Vec<Series>,
    pub cluster_ghz: Vec<Series>,
    pub cluster_cap_ghz: Vec<Series>,
    pub app_threads: Series,
    pub mem_avail_gb: Series,
    pub swap_used_gb: Series,
    pub psi_some: Series,
    pub dev_rx: Series,
    pub dev_tx: Series,
    pub cap_rx: Series,
    pub cap_tx: Series,
    pub pss_mb: Series,
    pub java_mb: Series,
    pub native_mb: Series,
    pub code_mb: Series,
    pub stack_mb: Series,
    pub graphics_mb: Series,
    pub other_mb: Series,
    pub system_mb: Series,
    pub heap_size_mb: Series,
    pub heap_alloc_mb: Series,
    pub fps: Series,
    pub jank_pct: Series,
    pub drain_ma: Series,
    pub thermal_status: Series,
    pub temps: Vec<(String, Series)>,
    pub deep_idle: Series,
    pub light_idle: Series,
    pub bucket: Series,
}

impl History {
    fn new(cap: usize) -> Self {
        let s = || Series::new(cap);
        Self {
            cap,
            app_cpu: s(),
            app_kernel: s(),
            sys_cpu: s(),
            cores: Vec::new(),
            cluster_ghz: Vec::new(),
            cluster_cap_ghz: Vec::new(),
            app_threads: s(),
            mem_avail_gb: s(),
            swap_used_gb: s(),
            psi_some: s(),
            dev_rx: s(),
            dev_tx: s(),
            cap_rx: s(),
            cap_tx: s(),
            pss_mb: s(),
            java_mb: s(),
            native_mb: s(),
            code_mb: s(),
            stack_mb: s(),
            graphics_mb: s(),
            other_mb: s(),
            system_mb: s(),
            heap_size_mb: s(),
            heap_alloc_mb: s(),
            fps: s(),
            jank_pct: s(),
            drain_ma: s(),
            thermal_status: s(),
            temps: Vec::new(),
            deep_idle: s(),
            light_idle: s(),
            bucket: s(),
        }
    }

    fn aligned(&self, len: usize) -> Series {
        let mut s = Series::new(self.cap);
        for _ in 0..len {
            s.push(f32::NAN);
        }
        s
    }
}

#[derive(Debug, Clone)]
pub struct TaskHist {
    pub name: String,
    pub hist: VecDeque<f32>,
    pub sum: f64,
    pub n: u32,
    pub peak: f32,
    pub last_tick: u64,
}

pub const TASK_HISTORY: usize = 120;

impl TaskHist {
    pub fn avg(&self) -> f32 {
        if self.n == 0 {
            0.0
        } else {
            (self.sum / self.n as f64) as f32
        }
    }
}

#[derive(Debug, Clone)]
pub struct Marker {
    pub tick: u64,
    pub secs: f64,
    pub label: String,
}

#[derive(Debug, Clone, Default)]
pub struct ConnRecord {
    pub id: u64,
    pub uid: i32,
    pub udp: bool,
    pub host: String,
    pub source: i32,
    pub remote: String,
    pub rx: u64,
    pub tx: u64,
    pub packets: u64,
    pub start_ms: u64,
    pub last_ms: u64,
    pub state: i32,
    pub error: String,
}

#[derive(Debug, Clone, Default)]
pub struct DnsRecord {
    pub time_ms: u64,
    pub uid: i32,
    pub name: String,
    pub answers: Vec<String>,
    pub failed: bool,
}

#[derive(Debug, Default)]
pub struct Capture {
    pub snapshot: Option<proto::Snapshot>,
    pub rates: HashMap<i32, (f64, f64)>,
    pub conns: HashMap<u64, ConnRecord>,
    pub dns: VecDeque<DnsRecord>,
    dns_seen: HashSet<(u64, String)>,
    known_apps: HashMap<i32, String>,
    pub error: Option<String>,
}

pub fn is_open(c: &ConnRecord) -> bool {
    c.state == proto::ConnectionState::Open as i32
        || c.state == proto::ConnectionState::Connecting as i32
}

pub const MAX_CONNS: usize = 20_000;

impl Capture {
    fn apply(&mut self, snapshot: proto::Snapshot) {
        self.rates.clear();
        if let Some(prev) = &self.snapshot {
            let dt = snapshot.time_ms.saturating_sub(prev.time_ms);
            if dt > 0 && snapshot.started_ms == prev.started_ms {
                for app in &snapshot.apps {
                    let (pr, pt) = prev
                        .apps
                        .iter()
                        .find(|p| p.uid == app.uid)
                        .map(|p| (p.rx_bytes, p.tx_bytes))
                        .unwrap_or((0, 0));
                    let secs = dt as f64 / 1000.0;
                    self.rates.insert(
                        app.uid,
                        (
                            app.rx_bytes.saturating_sub(pr) as f64 / secs,
                            app.tx_bytes.saturating_sub(pt) as f64 / secs,
                        ),
                    );
                }
            } else if snapshot.started_ms != prev.started_ms {
                self.conns.clear();
            }
        }
        for app in &snapshot.apps {
            if self.known_apps.get(&app.uid) != Some(&app.name) {
                self.known_apps.insert(app.uid, app.name.clone());
            }
        }
        for c in &snapshot.connections {
            let r = self.conns.entry(c.id).or_default();
            r.id = c.id;
            r.uid = c.uid;
            r.udp = c.protocol == proto::Protocol::Udp as i32;
            if !c.host.is_empty() {
                r.host = c.host.clone();
                r.source = c.host_source;
            }
            r.remote = format!("{}:{}", format_ip(&c.remote_ip), c.remote_port);
            r.rx = c.rx_bytes;
            r.tx = c.tx_bytes;
            r.packets = c.rx_packets + c.tx_packets;
            r.start_ms = c.start_ms;
            r.last_ms = c.last_ms;
            r.state = c.state;
            r.error = c.error.clone();
        }
        if self.conns.len() > MAX_CONNS {
            let mut closed: Vec<(u64, u64)> = self
                .conns
                .values()
                .filter(|c| !is_open(c))
                .map(|c| (c.last_ms, c.id))
                .collect();
            closed.sort_unstable();
            let excess = self.conns.len() - MAX_CONNS * 9 / 10;
            for (_, id) in closed.into_iter().take(excess) {
                self.conns.remove(&id);
            }
        }
        for d in &snapshot.dns {
            let key = (d.time_ms, d.name.clone());
            if self.dns_seen.insert(key) {
                self.dns.push_front(DnsRecord {
                    time_ms: d.time_ms,
                    uid: d.uid,
                    name: d.name.clone(),
                    answers: d.addresses.iter().map(|a| format_ip(a)).collect(),
                    failed: d.failed,
                });
                if self.dns.len() > 500 {
                    if let Some(old) = self.dns.pop_back() {
                        self.dns_seen.remove(&(old.time_ms, old.name));
                    }
                }
            }
        }
        self.snapshot = Some(snapshot);
        self.error = None;
    }

    pub fn uids_for(&self, package: Option<&str>) -> Option<Vec<i32>> {
        let package = package?;
        Some(
            self.known_apps
                .iter()
                .filter(|(_, name)| name.split(',').any(|n| n == package))
                .map(|(uid, _)| *uid)
                .collect(),
        )
    }

    pub fn traffic(&self, uids: Option<&[i32]>) -> (u64, u64, f64, f64, u32) {
        let Some(snapshot) = &self.snapshot else {
            return (0, 0, 0.0, 0.0, 0);
        };
        let mut t = (0u64, 0u64, 0f64, 0f64, 0u32);
        for app in &snapshot.apps {
            if uids.is_some_and(|u| !u.contains(&app.uid)) {
                continue;
            }
            let (rr, tr) = self.rates.get(&app.uid).copied().unwrap_or((0.0, 0.0));
            t.0 += app.rx_bytes;
            t.1 += app.tx_bytes;
            t.2 += rr;
            t.3 += tr;
            t.4 += app.open_connections;
        }
        t
    }

    pub fn app_name(&self, uid: i32) -> &str {
        self.known_apps.get(&uid).map_or("?", String::as_str)
    }
}

pub fn format_ip(bytes: &[u8]) -> String {
    match bytes.len() {
        4 => std::net::Ipv4Addr::new(bytes[0], bytes[1], bytes[2], bytes[3]).to_string(),
        16 => {
            let mut o = [0u8; 16];
            o.copy_from_slice(bytes);
            std::net::Ipv6Addr::from(o).to_string()
        }
        _ => "?".into(),
    }
}

pub struct Session {
    pub started: Instant,
    pub started_at: DateTime<Local>,
    pub interval: Duration,
    pub serial: Option<String>,
    pub caps: Option<Capabilities>,
    pub package: Option<String>,
    pub support: Vec<Support>,
    pub last_error: Option<(Source, String)>,
    pub ticks: u64,
    pub markers: Vec<Marker>,
    pub fast: Option<FastSample>,
    pub app_state: AppState,
    pub app_restarts: u32,
    pub app_mem: Option<AppMemory>,
    pub app_mem_first: Option<AppMemory>,
    pub frames: VecDeque<Frame>,
    pub frame_summary: Option<FrameSummary>,
    pub frame_base: Option<FrameSummary>,
    frame_pid: u32,
    pending_frames: u32,
    pending_janky: u32,
    pub battery: Option<Battery>,
    pub battery_first: Option<Battery>,
    pub thermal: Option<Thermal>,
    pub thermal_worst: u8,
    pub doze: Option<Doze>,
    pub jobs: Option<Jobs>,
    pub devices: Vec<Device>,
    pub packages: Vec<String>,
    pub process_names: HashMap<u32, String>,
    pub capture: Capture,
    pub history: History,
    pub tasks: HashMap<u32, TaskHist>,
}

pub const FRAME_HISTORY: usize = 2000;

fn mb(kb: u64) -> f32 {
    kb as f32 / 1024.0
}

fn idle_code(state: &str) -> f32 {
    match state {
        "ACTIVE" => 0.0,
        s if s.starts_with("IDLE") => 2.0,
        _ => 1.0,
    }
}

fn bucket_code(bucket: &str) -> f32 {
    match bucket {
        "EXEMPTED" | "ACTIVE" => 0.0,
        "WORKING_SET" => 1.0,
        "FREQUENT" => 2.0,
        "RARE" => 3.0,
        "RESTRICTED" | "NEVER" => 4.0,
        _ => f32::NAN,
    }
}

impl Session {
    pub fn new(
        serial: Option<String>,
        package: Option<String>,
        interval: Duration,
        history: usize,
    ) -> Self {
        Self {
            started: Instant::now(),
            started_at: Local::now(),
            interval,
            serial,
            caps: None,
            package,
            support: vec![Support::Unknown; Source::ALL.len()],
            last_error: None,
            ticks: 0,
            markers: Vec::new(),
            fast: None,
            app_state: AppState::NotSelected,
            app_restarts: 0,
            app_mem: None,
            app_mem_first: None,
            frames: VecDeque::with_capacity(FRAME_HISTORY),
            frame_summary: None,
            frame_base: None,
            frame_pid: 0,
            pending_frames: 0,
            pending_janky: 0,
            battery: None,
            battery_first: None,
            thermal: None,
            thermal_worst: 0,
            doze: None,
            jobs: None,
            devices: Vec::new(),
            packages: Vec::new(),
            process_names: HashMap::new(),
            capture: Capture::default(),
            history: History::new(history.max(2)),
            tasks: HashMap::new(),
        }
    }

    pub fn support(&self, source: Source) -> &Support {
        &self.support[source.index()]
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    pub fn interval_s(&self) -> f32 {
        self.interval.as_secs_f32()
    }

    pub fn reset_app(&mut self, package: Option<String>) {
        let cap = self.history.cap;
        let caps = self.caps.clone();
        let support = self.support.clone();
        let devices = std::mem::take(&mut self.devices);
        let packages = std::mem::take(&mut self.packages);
        let names = std::mem::take(&mut self.process_names);
        let capture = std::mem::take(&mut self.capture);
        let serial = self.serial.clone();
        *self = Session::new(serial, package, self.interval, cap);
        self.caps = caps;
        self.support = support;
        self.devices = devices;
        self.packages = packages;
        self.process_names = names;
        self.capture = capture;
    }

    pub fn add_marker(&mut self, label: impl Into<String>) {
        let label = label.into();
        let label = if label.trim().is_empty() {
            format!("m{}", self.markers.len() + 1)
        } else {
            label
        };
        self.markers.push(Marker {
            tick: self.ticks,
            secs: self.elapsed().as_secs_f64(),
            label,
        });
    }

    pub fn apply(&mut self, update: Update) -> bool {
        match update {
            Update::Probe(caps) => self.caps = Some(caps),
            Update::Support(source, support) => self.support[source.index()] = support,
            Update::Error(source, message) => self.last_error = Some((source, message)),
            Update::Fast(sample) => {
                self.last_error = None;
                self.on_fast(sample);
                return true;
            }
            Update::AppMemory(m) => {
                if let Some(m) = &m {
                    let restart = self.app_mem_first.as_ref().is_some_and(|f| f.pid != m.pid);
                    if self.app_mem_first.is_none() || restart {
                        self.app_mem_first = Some(m.clone());
                    }
                }
                self.app_mem = m;
            }
            Update::Frames(update) => match update {
                Some(u) => {
                    let restart = self.frame_base.is_none()
                        || u.pid != self.frame_pid
                        || self
                            .frame_summary
                            .as_ref()
                            .is_some_and(|prev| u.summary.total < prev.total);
                    if restart {
                        self.frame_base = Some(u.summary.clone());
                    }
                    self.frame_pid = u.pid;
                    for f in &u.frames {
                        if !u.baseline {
                            self.pending_frames += 1;
                            if f.janky {
                                self.pending_janky += 1;
                            }
                        }
                        if self.frames.len() == FRAME_HISTORY {
                            self.frames.pop_front();
                        }
                        self.frames.push_back(*f);
                    }
                    self.frame_summary = Some(u.summary);
                }
                None => self.frame_summary = None,
            },
            Update::Battery(b) => {
                if self.battery_first.is_none() {
                    self.battery_first = Some(b.clone());
                }
                self.battery = Some(b);
            }
            Update::Thermal(t) => {
                self.thermal_worst = self.thermal_worst.max(t.status);
                self.thermal = Some(t);
            }
            Update::Doze(d) => self.doze = Some(d),
            Update::Jobs(j) => self.jobs = Some(j),
            Update::Devices(d) => self.devices = d,
            Update::Processes { packages, names } => {
                self.packages = packages;
                self.process_names = names.into_iter().map(|p| (p.pid, p.name)).collect();
            }
            Update::Capture(snapshot) => {
                self.support[Source::Capture.index()] = Support::Supported;
                self.capture.apply(*snapshot);
            }
            Update::CaptureStopped => self.capture.snapshot = None,
            Update::CaptureError(e) => {
                if e == crate::agent::NOT_INSTALLED {
                    self.support[Source::Capture.index()] = Support::Unsupported(e.clone());
                }
                self.capture.error = Some(e);
            }
        }
        false
    }

    fn on_fast(&mut self, s: FastSample) {
        let nan = f32::NAN;
        self.app_state = s.app_state;
        if s.restarted {
            self.app_restarts += 1;
        }
        let h = &mut self.history;
        let ticks = self.ticks as usize;
        let app = s.app.as_ref();
        h.app_cpu.push(app.map_or(nan, |a| a.cpu));
        h.app_kernel.push(app.map_or(nan, |a| a.kernel));
        h.app_threads.push(app.map_or(nan, |a| a.threads as f32));
        h.sys_cpu.push(s.sys_cpu);

        let max_id = s
            .cores
            .iter()
            .map(|c| c.id as usize)
            .max()
            .map_or(0, |m| m + 1);
        while h.cores.len() < max_id {
            let aligned = h.aligned(ticks);
            h.cores.push(aligned);
        }
        let mut seen = vec![false; h.cores.len()];
        for c in &s.cores {
            h.cores[c.id as usize].push(c.usage);
            seen[c.id as usize] = true;
        }
        for (i, series) in h.cores.iter_mut().enumerate() {
            if !seen[i] {
                series.push(nan);
            }
        }

        if let Some(caps) = &self.caps {
            while h.cluster_ghz.len() < caps.clusters.len() {
                let aligned = h.aligned(ticks);
                h.cluster_ghz.push(aligned);
            }
            while h.cluster_cap_ghz.len() < caps.clusters.len() {
                let aligned = h.aligned(ticks);
                h.cluster_cap_ghz.push(aligned);
            }
            for (ci, cluster) in caps.clusters.iter().enumerate() {
                let cores: Vec<&CoreSample> = s
                    .cores
                    .iter()
                    .filter(|c| cluster.cores.contains(&c.id) && c.cur_khz > 0)
                    .collect();
                let (cur, cap) = if cores.is_empty() {
                    (nan, nan)
                } else {
                    let cur = cores.iter().map(|c| c.cur_khz).max().unwrap_or(0) as f32 / 1e6;
                    let cap = cores
                        .iter()
                        .map(|c| c.max_khz)
                        .filter(|k| *k > 0)
                        .min()
                        .unwrap_or(0) as f32
                        / 1e6;
                    (cur, if cap > 0.0 { cap } else { nan })
                };
                if let Some(series) = h.cluster_ghz.get_mut(ci) {
                    series.push(cur);
                }
                if let Some(series) = h.cluster_cap_ghz.get_mut(ci) {
                    series.push(cap);
                }
            }
        }

        match &s.mem {
            Some(m) => {
                h.mem_avail_gb.push(m.available_kb as f32 / 1048576.0);
                h.swap_used_gb.push(m.swap_used_kb() as f32 / 1048576.0);
            }
            None => {
                h.mem_avail_gb.push(nan);
                h.swap_used_gb.push(nan);
            }
        }
        h.psi_some.push(s.psi.map_or(nan, |p| p.some_avg10));
        h.dev_rx.push(s.net.map_or(nan, |n| n.rx_bps() as f32));
        h.dev_tx.push(s.net.map_or(nan, |n| n.tx_bps() as f32));

        if self.capture.snapshot.is_some() {
            let uids = self.capture.uids_for(self.package.as_deref());
            let t = self.capture.traffic(uids.as_deref());
            h.cap_rx.push(t.2 as f32);
            h.cap_tx.push(t.3 as f32);
        } else {
            h.cap_rx.push(nan);
            h.cap_tx.push(nan);
        }

        let running = s.app_state == AppState::Running;
        match self.app_mem.as_ref().filter(|_| running) {
            Some(m) => {
                h.pss_mb.push(mb(m.pss_kb));
                h.java_mb.push(mb(m.java_kb));
                h.native_mb.push(mb(m.native_kb));
                h.code_mb.push(mb(m.code_kb));
                h.stack_mb.push(mb(m.stack_kb));
                h.graphics_mb.push(mb(m.graphics_kb));
                h.other_mb.push(mb(m.other_kb));
                h.system_mb.push(mb(m.system_kb));
                h.heap_size_mb.push(mb(m.java_heap_size_kb));
                h.heap_alloc_mb.push(mb(m.java_heap_alloc_kb));
            }
            None => {
                for series in [
                    &mut h.pss_mb,
                    &mut h.java_mb,
                    &mut h.native_mb,
                    &mut h.code_mb,
                    &mut h.stack_mb,
                    &mut h.graphics_mb,
                    &mut h.other_mb,
                    &mut h.system_mb,
                    &mut h.heap_size_mb,
                    &mut h.heap_alloc_mb,
                ] {
                    series.push(nan);
                }
            }
        }

        if self.frame_summary.is_some() && running {
            let dt = s.dt.max(0.05);
            h.fps.push(self.pending_frames as f32 / dt);
            h.jank_pct.push(if self.pending_frames > 0 {
                self.pending_janky as f32 * 100.0 / self.pending_frames as f32
            } else {
                0.0
            });
        } else {
            h.fps.push(nan);
            h.jank_pct.push(nan);
        }
        self.pending_frames = 0;
        self.pending_janky = 0;

        h.drain_ma.push(
            self.battery
                .as_ref()
                .and_then(|b| b.drain_ma())
                .unwrap_or(nan),
        );

        match &self.thermal {
            Some(t) => {
                h.thermal_status.push(t.status as f32);
                for sensor in &t.sensors {
                    if !h.temps.iter().any(|(n, _)| *n == sensor.name) {
                        let aligned = h.aligned(ticks);
                        h.temps.push((sensor.name.clone(), aligned));
                    }
                }
                for (name, series) in h.temps.iter_mut() {
                    series.push(
                        t.sensors
                            .iter()
                            .find(|x| x.name == *name)
                            .map_or(nan, |x| x.celsius),
                    );
                }
            }
            None => {
                h.thermal_status.push(nan);
                for (_, series) in h.temps.iter_mut() {
                    series.push(nan);
                }
            }
        }

        match &self.doze {
            Some(d) => {
                h.deep_idle.push(idle_code(&d.deep));
                h.light_idle.push(idle_code(&d.light));
                h.bucket.push(d.bucket.as_deref().map_or(nan, bucket_code));
            }
            None => {
                h.deep_idle.push(nan);
                h.light_idle.push(nan);
                h.bucket.push(nan);
            }
        }

        let tick = self.ticks + 1;
        for t in &s.tasks {
            let e = self.tasks.entry(t.id).or_insert_with(|| TaskHist {
                name: t.name.clone(),
                hist: VecDeque::with_capacity(TASK_HISTORY),
                sum: 0.0,
                n: 0,
                peak: 0.0,
                last_tick: tick,
            });
            if e.name != t.name {
                e.name = t.name.clone();
            }
            if e.hist.len() == TASK_HISTORY {
                e.hist.pop_front();
            }
            e.hist.push_back(t.cpu);
            e.sum += t.cpu as f64;
            e.n += 1;
            e.peak = e.peak.max(t.cpu);
            e.last_tick = tick;
        }
        if tick % 30 == 0 {
            self.tasks.retain(|_, t| tick - t.last_tick < 60);
        }

        self.ticks = tick;
        self.fast = Some(s);
    }

    pub fn tasks_sorted(&self) -> Vec<(&TaskSample, Option<&TaskHist>)> {
        let Some(fast) = &self.fast else {
            return Vec::new();
        };
        let main_pid = fast.app.as_ref().map(|a| a.pid);
        let mut rows: Vec<(&TaskSample, Option<&TaskHist>)> = fast
            .tasks
            .iter()
            .map(|t| (t, self.tasks.get(&t.id)))
            .collect();
        rows.sort_by(|a, b| {
            let am = Some(a.0.id) == main_pid;
            let bm = Some(b.0.id) == main_pid;
            bm.cmp(&am).then(
                b.0.cpu
                    .partial_cmp(&a.0.cpu)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
        });
        rows
    }

    pub fn task_display_name<'a>(&'a self, t: &'a TaskSample) -> &'a str {
        if self.package.is_none() {
            if let Some(name) = self.process_names.get(&t.id) {
                return name;
            }
        }
        &t.name
    }

    pub fn frames_since_start(&self) -> Option<FrameSummary> {
        let now = self.frame_summary.as_ref()?;
        let base = self.frame_base.as_ref()?;
        let d = |a: u64, b: u64| a.saturating_sub(b);
        let mut hist: Vec<(u32, u64)> = now
            .histogram
            .iter()
            .map(|(ms, n)| {
                (
                    *ms,
                    d(
                        *n,
                        base.histogram
                            .iter()
                            .find(|(b, _)| b == ms)
                            .map_or(0, |x| x.1),
                    ),
                )
            })
            .collect();
        let total = d(now.total, base.total);
        if total == 0 {
            hist.clear();
        }
        let pct = |q: f64| -> u32 {
            let target = (total as f64 * q).ceil() as u64;
            let mut acc = 0;
            for (ms, n) in &hist {
                acc += n;
                if acc >= target && target > 0 {
                    return *ms;
                }
            }
            0
        };
        Some(FrameSummary {
            total,
            janky: d(now.janky, base.janky),
            janky_legacy: d(now.janky_legacy, base.janky_legacy),
            p50_ms: pct(0.5),
            p90_ms: pct(0.9),
            p95_ms: pct(0.95),
            p99_ms: pct(0.99),
            gpu_p50_ms: now.gpu_p50_ms,
            gpu_p90_ms: now.gpu_p90_ms,
            gpu_p95_ms: now.gpu_p95_ms,
            gpu_p99_ms: now.gpu_p99_ms,
            missed_vsync: d(now.missed_vsync, base.missed_vsync),
            high_input_latency: d(now.high_input_latency, base.high_input_latency),
            slow_ui_thread: d(now.slow_ui_thread, base.slow_ui_thread),
            slow_bitmap_uploads: d(now.slow_bitmap_uploads, base.slow_bitmap_uploads),
            slow_draw_commands: d(now.slow_draw_commands, base.slow_draw_commands),
            deadline_missed: d(now.deadline_missed, base.deadline_missed),
            histogram: hist,
        })
    }

    pub fn used_mah(&self) -> Option<f32> {
        let first = self.battery_first.as_ref()?.charge_counter_uah?;
        let now = self.battery.as_ref()?.charge_counter_uah?;
        Some((first - now) as f32 / 1000.0)
    }

    pub fn app_uids(&self) -> Option<Vec<i32>> {
        self.capture.uids_for(self.package.as_deref())
    }

    pub fn connections(&self) -> Vec<&ConnRecord> {
        let uids = self.app_uids();
        let mut v: Vec<&ConnRecord> = self
            .capture
            .conns
            .values()
            .filter(|c| uids.as_ref().map_or(true, |u| u.contains(&c.uid)))
            .collect();
        v.sort_by(|a, b| is_open(b).cmp(&is_open(a)).then(b.last_ms.cmp(&a.last_ms)));
        v
    }

    pub fn hosts(&self) -> Vec<HostTotal> {
        let mut map: HashMap<&str, HostTotal> = HashMap::new();
        for c in self.connections() {
            let key = if c.host.is_empty() {
                c.remote.rsplit_once(':').map_or(c.remote.as_str(), |r| r.0)
            } else {
                c.host.as_str()
            };
            let e = map.entry(key).or_insert_with(|| HostTotal {
                host: key.to_string(),
                ..Default::default()
            });
            e.connections += 1;
            e.rx += c.rx;
            e.tx += c.tx;
            if c.state == proto::ConnectionState::Failed as i32 {
                e.failed += 1;
            }
        }
        let mut v: Vec<HostTotal> = map.into_values().collect();
        v.sort_by(|a, b| (b.rx + b.tx).cmp(&(a.rx + a.tx)).then(a.host.cmp(&b.host)));
        v
    }

    pub fn dns(&self) -> Vec<&DnsRecord> {
        let uids = self.app_uids();
        self.capture
            .dns
            .iter()
            .filter(|d| uids.as_ref().map_or(true, |u| u.contains(&d.uid)))
            .collect()
    }

    pub fn marker_window(&self, marker: &Marker, secs: f32) -> (usize, usize) {
        let len = self.history.app_cpu.len() as i64;
        let start = len - (self.ticks - marker.tick) as i64;
        let span = (secs / self.interval_s().max(0.05)).ceil() as i64;
        (
            start.clamp(0, len) as usize,
            (start + span).clamp(0, len) as usize,
        )
    }
}

#[derive(Debug, Clone, Default)]
pub struct HostTotal {
    pub host: String,
    pub connections: u32,
    pub rx: u64,
    pub tx: u64,
    pub failed: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn series_tracks_stats_and_skips_gaps() {
        let mut s = Series::new(3);
        for v in [1.0, f32::NAN, 5.0, 3.0, 9.0] {
            s.push(v);
        }
        assert_eq!(s.len(), 3);
        assert_eq!(s.avg(), Some(4.5));
        assert_eq!(s.max(), Some(9.0));
        assert_eq!(s.first(), Some(1.0));
        assert_eq!(s.last(), Some(9.0));
        assert_eq!(s.percentile(0.5), Some(5.0));
        assert_eq!((s.get(1), s.get(2)), (3.0, 9.0));
    }

    #[test]
    fn slope_and_drops() {
        let mut s = Series::new(100);
        for i in 0..90 {
            s.push(100.0 + i as f32 * 0.5);
        }
        assert!((s.slope_per_min(1.0).unwrap() - 30.0).abs() < 0.01);
        assert!(Series::new(20).slope_per_min(1.0).is_none());
        let mut g = Series::new(10);
        for v in [10.0, 14.0, 18.0, 9.0, 13.0, 17.0, 8.0] {
            g.push(v);
        }
        assert_eq!(g.drops(2.0), 2);
    }

    #[test]
    fn frame_deltas_since_session_start() {
        let mut s = Session::new(
            None,
            Some("com.example.app".into()),
            Duration::from_secs(1),
            60,
        );
        let summary = |total: u64, janky: u64, h: Vec<(u32, u64)>| FrameSummary {
            total,
            janky,
            histogram: h,
            ..Default::default()
        };
        s.apply(Update::Frames(Some(FrameUpdate {
            pid: 1,
            baseline: true,
            frames: vec![],
            summary: summary(100, 5, vec![(5, 90), (20, 10)]),
        })));
        s.apply(Update::Frames(Some(FrameUpdate {
            pid: 1,
            baseline: false,
            frames: vec![],
            summary: summary(200, 7, vec![(5, 170), (20, 30)]),
        })));
        let d = s.frames_since_start().unwrap();
        assert_eq!((d.total, d.janky), (100, 2));
        assert_eq!(d.p50_ms, 5);
        assert_eq!(d.p90_ms, 20);
    }
}
