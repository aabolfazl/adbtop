use std::collections::{BTreeSet, HashMap};

use crate::model::*;

pub const USER_HZ: f64 = 100.0;

pub fn is_valid_package_name(name: &str) -> bool {
    name.contains('.')
        && name.split('.').all(|part| {
            !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
}

fn num<T: std::str::FromStr>(s: &str) -> Option<T> {
    s.trim().parse().ok()
}

fn first_num<T: std::str::FromStr>(s: &str) -> Option<T> {
    s.split_ascii_whitespace()
        .next()
        .and_then(|t| t.parse().ok())
}

pub fn parse_devices(output: &str) -> Vec<Device> {
    output
        .lines()
        .filter(|l| !l.starts_with("List of devices") && !l.starts_with('*'))
        .filter_map(|line| {
            let mut parts = line.split_ascii_whitespace();
            let serial = parts.next()?.to_string();
            let state = parts.next()?.to_string();
            let mut model = String::new();
            let mut product = String::new();
            for part in parts {
                if let Some(v) = part.strip_prefix("model:") {
                    model = v.to_string();
                } else if let Some(v) = part.strip_prefix("product:") {
                    product = v.to_string();
                }
            }
            Some(Device {
                serial,
                model,
                product,
                state,
            })
        })
        .collect()
}

fn is_app_user(user: &str) -> bool {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    user.strip_prefix('u')
        .and_then(|rest| rest.split_once("_a"))
        .is_some_and(|(u, a)| digits(u) && digits(a))
}

pub fn parse_ps(output: &str) -> (Vec<String>, Vec<ProcessName>) {
    let mut packages = BTreeSet::new();
    let mut names = Vec::new();
    for line in output.lines() {
        let mut parts = line.split_ascii_whitespace();
        let (Some(pid), Some(user), Some(name)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let Ok(pid) = pid.parse::<u32>() else {
            continue;
        };
        if is_app_user(user) {
            let package = name.split(':').next().unwrap_or(name);
            if is_valid_package_name(package) {
                packages.insert(package.to_string());
            }
        }
        names.push(ProcessName {
            pid,
            name: name.to_string(),
        });
    }
    (packages.into_iter().collect(), names)
}

pub fn probe_command() -> &'static str {
    "getprop ro.build.version.sdk; getprop ro.build.version.release; getprop ro.product.model; getprop ro.product.manufacturer; \
echo ==C; grep -H '' /sys/devices/system/cpu/cpu[0-9]*/cpufreq/cpuinfo_max_freq /sys/devices/system/cpu/cpu[0-9]*/cpufreq/related_cpus 2>/dev/null; \
echo ==S; for s in gfxinfo thermalservice battery deviceidle jobscheduler meminfo; do service check $s; done; \
echo ==P; [ -r /proc/pressure/memory ] && echo psi; [ -r /sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq ] && echo cpufreq; echo page $(getconf PAGESIZE 2>/dev/null); echo ==E"
}

pub fn parse_probe(output: &str) -> Capabilities {
    let mut caps = Capabilities::default();
    let mut section = 0;
    let mut props = Vec::new();
    let mut max_khz: HashMap<u16, u32> = HashMap::new();
    let mut related: HashMap<u16, Vec<u16>> = HashMap::new();
    for line in output.lines() {
        let line = line.trim_end();
        match line {
            "==C" => {
                section = 1;
                continue;
            }
            "==S" => {
                section = 2;
                continue;
            }
            "==P" => {
                section = 3;
                continue;
            }
            "==E" => break,
            _ => {}
        }
        match section {
            0 => props.push(line.trim().to_string()),
            1 => {
                let Some((path, value)) = line.split_once(':') else {
                    continue;
                };
                let Some(id) = cpu_id_from_path(path) else {
                    continue;
                };
                if path.ends_with("cpuinfo_max_freq") {
                    if let Some(v) = num(value) {
                        max_khz.insert(id, v);
                    }
                } else if path.ends_with("related_cpus") {
                    related.insert(id, value.split_ascii_whitespace().filter_map(num).collect());
                }
            }
            2 => {
                let found = line.ends_with(": found");
                if let Some(name) = line
                    .strip_prefix("Service ")
                    .and_then(|r| r.split(':').next())
                {
                    match name {
                        "gfxinfo" => caps.gfxinfo = found,
                        "thermalservice" => caps.thermal = found,
                        "battery" => caps.battery = found,
                        "deviceidle" => caps.deviceidle = found,
                        "jobscheduler" => caps.jobscheduler = found,
                        "meminfo" => caps.meminfo = found,
                        _ => {}
                    }
                }
            }
            3 => match line.trim() {
                "psi" => caps.psi = true,
                "cpufreq" => caps.cpufreq = true,
                other => {
                    if let Some(size) = other.strip_prefix("page ").and_then(num) {
                        caps.page_size = size;
                    }
                }
            },
            _ => {}
        }
    }
    let mut props = props.into_iter();
    caps.sdk = props.next().and_then(|s| num(&s)).unwrap_or(0);
    caps.release = props.next().unwrap_or_default();
    caps.model = props.next().unwrap_or_default();
    caps.manufacturer = props.next().unwrap_or_default();
    caps.clusters = build_clusters(&max_khz, &related);
    caps
}

fn cpu_id_from_path(path: &str) -> Option<u16> {
    let rest = path.strip_prefix("/sys/devices/system/cpu/cpu")?;
    num(rest.split('/').next()?)
}

fn build_clusters(max_khz: &HashMap<u16, u32>, related: &HashMap<u16, Vec<u16>>) -> Vec<Cluster> {
    let mut groups: Vec<(u32, Vec<u16>)> = Vec::new();
    let mut ids: Vec<u16> = max_khz.keys().copied().collect();
    ids.sort_unstable();
    for id in ids {
        if groups.iter().any(|(_, cores)| cores.contains(&id)) {
            continue;
        }
        let mut cores = related
            .get(&id)
            .cloned()
            .filter(|c| c.contains(&id))
            .unwrap_or_else(|| {
                let khz = max_khz[&id];
                let mut same: Vec<u16> = max_khz
                    .iter()
                    .filter(|(_, &k)| k == khz)
                    .map(|(&c, _)| c)
                    .collect();
                same.sort_unstable();
                same
            });
        cores.sort_unstable();
        let khz = cores
            .iter()
            .filter_map(|c| max_khz.get(c))
            .copied()
            .max()
            .unwrap_or(0);
        groups.push((khz, cores));
    }
    groups.sort_by_key(|(khz, cores)| (*khz, cores.first().copied()));
    let names: &[&'static str] = match groups.len() {
        1 => &["all"],
        2 => &["little", "big"],
        3 if groups[2].1.len() <= 2 => &["little", "big", "prime"],
        3 => &["little", "mid", "big"],
        4 => &["little", "mid", "big", "prime"],
        _ => &["c0", "c1", "c2", "c3", "c4", "c5", "c6", "c7"],
    };
    groups
        .into_iter()
        .enumerate()
        .map(|(i, (max_khz, cores))| Cluster {
            name: names.get(i).copied().unwrap_or("cx"),
            cores,
            max_khz,
        })
        .collect()
}

pub struct ProcStat<'a> {
    pub id: u32,
    pub name: &'a str,
    pub state: char,
    pub minflt: u64,
    pub majflt: u64,
    pub utime: u64,
    pub stime: u64,
    pub nice: i32,
    pub threads: u32,
    pub start: u64,
    pub rss_pages: u64,
    pub processor: u16,
}

pub fn parse_proc_stat(line: &str) -> Option<ProcStat<'_>> {
    let open = line.find('(')?;
    let close = line.rfind(')')?;
    if close < open {
        return None;
    }
    let id = num(&line[..open])?;
    let name = &line[open + 1..close];
    let mut f = line[close + 1..].split_ascii_whitespace();
    let state = f.next()?.chars().next()?;
    let mut rest = [0i64; 37];
    for (i, slot) in rest.iter_mut().enumerate().skip(1) {
        match f.next() {
            Some(v) => *slot = v.parse().unwrap_or(0),
            None if i > 19 => break,
            None => return None,
        }
    }
    Some(ProcStat {
        id,
        name,
        state,
        minflt: rest[7] as u64,
        majflt: rest[9] as u64,
        utime: rest[11] as u64,
        stime: rest[12] as u64,
        nice: rest[16] as i32,
        threads: rest[17] as u32,
        start: rest[19] as u64,
        rss_pages: rest[21].max(0) as u64,
        processor: rest[36] as u16,
    })
}

pub fn fast_command(
    app: Option<(&str, Option<u32>)>,
    cpufreq: bool,
    psi: bool,
    processes: bool,
) -> String {
    let mut files = String::from("/proc/uptime /proc/stat /proc/meminfo /proc/net/dev");
    if psi {
        files.push_str(" /proc/pressure/memory");
    }
    if cpufreq {
        files.push_str(" /sys/devices/system/cpu/cpu[0-9]*/cpufreq/scaling_cur_freq /sys/devices/system/cpu/cpu[0-9]*/cpufreq/scaling_max_freq");
    }
    match app {
        Some((_, Some(pid))) => {
            files.push_str(&format!(" /proc/{pid}/stat /proc/{pid}/status /proc/{pid}/oom_score_adj /proc/{pid}/task/*/stat"));
            format!("grep -HvE '^(intr|softirq) ' {files} 2>/dev/null")
        }
        Some((package, None)) => format!(
            "P=$(pidof -s {package}); echo \"pid=$P\"; if [ -n \"$P\" ]; then grep -HvE '^(intr|softirq) ' {files} /proc/$P/stat /proc/$P/status /proc/$P/oom_score_adj /proc/$P/task/*/stat 2>/dev/null; else grep -HvE '^(intr|softirq) ' {files} 2>/dev/null; fi"
        ),
        None if processes => format!("grep -HvE '^(intr|softirq) ' {files} /proc/[0-9]*/stat 2>/dev/null"),
        None => format!("grep -HvE '^(intr|softirq) ' {files} 2>/dev/null"),
    }
}

#[derive(Clone, Copy, Default)]
struct AppPrev {
    pid: u32,
    start: u64,
    utime: u64,
    stime: u64,
    minflt: u64,
    majflt: u64,
    voluntary: u64,
    involuntary: u64,
    uptime: f64,
}

#[derive(Clone, Copy, Default)]
struct NetBytes {
    wifi_rx: u64,
    wifi_tx: u64,
    mobile_rx: u64,
    mobile_tx: u64,
    wifi_seen: bool,
    mobile_seen: bool,
}

#[derive(Default)]
pub struct FastParser {
    cores: HashMap<u16, (u64, u64)>,
    total: Option<(u64, u64)>,
    app: Option<AppPrev>,
    identity: Option<(u32, u64)>,
    tasks: HashMap<u32, u64>,
    net: Option<(NetBytes, f64)>,
}

pub struct FastResult {
    pub sample: Option<FastSample>,
    pub pid: Option<u32>,
}

fn stat_times(rest: &str) -> Option<(u64, u64)> {
    let mut f = rest.split_ascii_whitespace().skip(1);
    let mut total = 0u64;
    let mut idle = 0u64;
    for i in 0..8 {
        let v: u64 = f.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        if i < 7 {
            total += v;
        }
        if i == 3 || i == 4 {
            idle += v;
        }
    }
    Some((total, idle))
}

impl FastParser {
    pub fn reset_app(&mut self) {
        self.forget_process();
        self.identity = None;
    }

    fn forget_process(&mut self) {
        self.app = None;
        self.tasks.clear();
    }

    pub fn parse(
        &mut self,
        output: &str,
        app_selected: bool,
        known_pid: Option<u32>,
    ) -> FastResult {
        let mut uptime = 0f64;
        let mut total_now: Option<(u64, u64)> = None;
        let mut core_now: Vec<(u16, u64, u64)> = Vec::with_capacity(16);
        let mut freq: HashMap<u16, (u32, u32)> = HashMap::new();
        let mut mem = SysMem::default();
        let mut mem_seen = false;
        let mut psi: Option<Psi> = None;
        let mut net = NetBytes::default();
        let mut net_seen = false;
        let mut pid = known_pid;
        let mut app_stat_line: Option<&str> = None;
        let mut status: HashMap<&str, u64> = HashMap::new();
        let mut oom_adj = None;
        let mut task_lines: Vec<&str> = Vec::new();

        for line in output.lines() {
            if let Some(p) = line.strip_prefix("pid=") {
                pid = num(p);
                continue;
            }
            let Some((path, rest)) = line.split_once(':') else {
                continue;
            };
            match path {
                "/proc/uptime" => uptime = first_num(rest).unwrap_or(0.0),
                "/proc/stat" => {
                    if let Some(name) = rest.split_ascii_whitespace().next() {
                        if name == "cpu" {
                            total_now = stat_times(rest);
                        } else if let Some(id) = name.strip_prefix("cpu").and_then(num::<u16>) {
                            if let Some((t, i)) = stat_times(rest) {
                                core_now.push((id, t, i));
                            }
                        }
                    }
                }
                "/proc/meminfo" => {
                    let mut it = rest.split_ascii_whitespace();
                    let (Some(key), Some(v)) =
                        (it.next(), it.next().and_then(|v| v.parse::<u64>().ok()))
                    else {
                        continue;
                    };
                    mem_seen = true;
                    match key {
                        "MemTotal:" => mem.total_kb = v,
                        "MemFree:" => mem.free_kb = v,
                        "MemAvailable:" => mem.available_kb = v,
                        "Buffers:" => mem.buffers_kb = v,
                        "Cached:" => mem.cached_kb = v,
                        "SwapTotal:" => mem.swap_total_kb = v,
                        "SwapFree:" => mem.swap_free_kb = v,
                        _ => {}
                    }
                }
                "/proc/net/dev" => {
                    let Some((iface, counters)) = rest.split_once(':') else {
                        continue;
                    };
                    let iface = iface.trim();
                    let mut c = counters.split_ascii_whitespace();
                    let rx: u64 = c.next().and_then(|v| v.parse().ok()).unwrap_or(0);
                    let tx: u64 = c.nth(7).and_then(|v| v.parse().ok()).unwrap_or(0);
                    net_seen = true;
                    if iface.starts_with("wlan") {
                        net.wifi_rx += rx;
                        net.wifi_tx += tx;
                        net.wifi_seen |= rx > 0 || tx > 0;
                    } else if iface.starts_with("rmnet")
                        || iface.starts_with("ccmni")
                        || iface.starts_with("seth")
                        || iface.starts_with("radio")
                    {
                        net.mobile_rx += rx;
                        net.mobile_tx += tx;
                        net.mobile_seen |= rx > 0 || tx > 0;
                    }
                }
                "/proc/pressure/memory" => {
                    let avg10 = rest
                        .split_ascii_whitespace()
                        .find_map(|t| t.strip_prefix("avg10="))
                        .and_then(num::<f32>)
                        .unwrap_or(0.0);
                    let p = psi.get_or_insert_with(Psi::default);
                    if rest.starts_with("some") {
                        p.some_avg10 = avg10;
                    } else if rest.starts_with("full") {
                        p.full_avg10 = avg10;
                    }
                }
                _ if path.starts_with("/sys/devices/system/cpu/cpu") => {
                    let Some(id) = cpu_id_from_path(path) else {
                        continue;
                    };
                    let v: u32 = num(rest).unwrap_or(0);
                    let entry = freq.entry(id).or_default();
                    if path.ends_with("scaling_cur_freq") {
                        entry.0 = v;
                    } else {
                        entry.1 = v;
                    }
                }
                _ if path.starts_with("/proc/") => {
                    let tail = &path[6..];
                    if tail.contains("/task/") {
                        task_lines.push(rest);
                    } else if let Some(p) = tail.strip_suffix("/stat") {
                        if app_selected {
                            if num::<u32>(p) == pid || pid.is_none() {
                                app_stat_line = Some(rest);
                            }
                        } else {
                            task_lines.push(rest);
                        }
                    } else if tail.ends_with("/status") {
                        let Some((k, v)) = rest.split_once(':') else {
                            continue;
                        };
                        if let Some(v) = first_num::<u64>(v) {
                            status.insert(k, v);
                        }
                    } else if tail.ends_with("/oom_score_adj") {
                        oom_adj = num(rest);
                    }
                }
                _ => {}
            }
        }

        let Some(total) = total_now else {
            return FastResult { sample: None, pid };
        };
        let prev_total = self.total.replace(total);
        let d_total = prev_total
            .map(|(t, _)| total.0.saturating_sub(t))
            .unwrap_or(0);
        let online = core_now.len().max(1) as f64;

        let mut cores: Vec<CoreSample> = core_now
            .iter()
            .map(|&(id, t, i)| {
                let usage = match self.cores.get(&id) {
                    Some(&(pt, pi)) => {
                        let dt = t.saturating_sub(pt);
                        let di = i.saturating_sub(pi);
                        if dt > 0 {
                            (dt.saturating_sub(di)) as f32 / dt as f32 * 100.0
                        } else {
                            0.0
                        }
                    }
                    None => 0.0,
                };
                let (cur, max) = freq.get(&id).copied().unwrap_or((0, 0));
                CoreSample {
                    id,
                    usage,
                    cur_khz: cur,
                    max_khz: max,
                }
            })
            .collect();
        cores.sort_by_key(|c| c.id);
        self.cores = core_now.iter().map(|&(id, t, i)| (id, (t, i))).collect();

        let sys_cpu = match prev_total {
            Some((_, pi)) if d_total > 0 => {
                let di = total.1.saturating_sub(pi);
                d_total.saturating_sub(di) as f32 / d_total as f32 * 100.0
            }
            _ => 0.0,
        };

        let net_rates = if net_seen {
            let rates = match self.net {
                Some((prev, prev_up)) if uptime > prev_up => {
                    let dt = uptime - prev_up;
                    let r = |now: u64, before: u64| now.saturating_sub(before) as f64 / dt;
                    Some(NetRates {
                        wifi_rx_bps: r(net.wifi_rx, prev.wifi_rx),
                        wifi_tx_bps: r(net.wifi_tx, prev.wifi_tx),
                        mobile_rx_bps: r(net.mobile_rx, prev.mobile_rx),
                        mobile_tx_bps: r(net.mobile_tx, prev.mobile_tx),
                        wifi_up: net.wifi_seen,
                        mobile_up: net.mobile_seen,
                    })
                }
                _ => None,
            };
            self.net = Some((net, uptime));
            rates
        } else {
            None
        };

        let mut app_state = if app_selected {
            AppState::NotRunning
        } else {
            AppState::NotSelected
        };
        let mut app = None;
        let mut restarted = false;
        let task_scale = if d_total > 0 {
            100.0 * online / d_total as f64
        } else {
            0.0
        };

        if let Some(stat) = app_stat_line.and_then(parse_proc_stat) {
            app_state = AppState::Running;
            pid = Some(stat.id);
            let voluntary = status.get("voluntary_ctxt_switches").copied().unwrap_or(0);
            let involuntary = status
                .get("nonvoluntary_ctxt_switches")
                .copied()
                .unwrap_or(0);
            let now = AppPrev {
                pid: stat.id,
                start: stat.start,
                utime: stat.utime,
                stime: stat.stime,
                minflt: stat.minflt,
                majflt: stat.majflt,
                voluntary,
                involuntary,
                uptime,
            };
            let mut proc_ = AppProc {
                pid: stat.id,
                threads: status
                    .get("Threads")
                    .copied()
                    .unwrap_or(stat.threads as u64) as u32,
                rss_kb: status.get("VmRSS").copied().unwrap_or(0),
                swap_kb: status.get("VmSwap").copied().unwrap_or(0),
                oom_adj,
                nice: stat.nice,
                uptime_s: (uptime - stat.start as f64 / USER_HZ).max(0.0),
                cpu: f32::NAN,
                kernel: f32::NAN,
                ..Default::default()
            };
            restarted = self.identity.is_some_and(|id| id != (now.pid, now.start));
            self.identity = Some((now.pid, now.start));
            match self.app {
                Some(prev) if prev.pid == now.pid && prev.start == now.start && d_total > 0 => {
                    let share = 100.0 / d_total as f64;
                    proc_.cpu = ((now.utime + now.stime).saturating_sub(prev.utime + prev.stime)
                        as f64
                        * share) as f32;
                    proc_.kernel = (now.stime.saturating_sub(prev.stime) as f64 * share) as f32;
                    let dt = (uptime - prev.uptime).max(1e-3);
                    let rate = |a: u64, b: u64| (a.saturating_sub(b) as f64 / dt) as f32;
                    proc_.minflt_ps = rate(now.minflt, prev.minflt);
                    proc_.majflt_ps = rate(now.majflt, prev.majflt);
                    proc_.ctx_voluntary_ps = rate(now.voluntary, prev.voluntary);
                    proc_.ctx_involuntary_ps = rate(now.involuntary, prev.involuntary);
                }
                Some(prev) if prev.pid != now.pid || prev.start != now.start => self.tasks.clear(),
                _ => {}
            }
            self.app = Some(now);
            app = Some(proc_);
        } else if app_selected {
            self.forget_process();
            pid = None;
        }

        let mut tasks = Vec::with_capacity(task_lines.len());
        let mut next_ticks = HashMap::with_capacity(task_lines.len());
        for line in task_lines {
            let Some(stat) = parse_proc_stat(line) else {
                continue;
            };
            let ticks = stat.utime + stat.stime;
            let cpu = match self.tasks.get(&stat.id) {
                Some(&prev) => (ticks.saturating_sub(prev) as f64 * task_scale) as f32,
                None => 0.0,
            };
            next_ticks.insert(stat.id, ticks);
            tasks.push(TaskSample {
                id: stat.id,
                name: stat.name.to_string(),
                state: stat.state,
                cpu,
                core: stat.processor,
                ticks,
                rss_pages: stat.rss_pages,
            });
        }
        self.tasks = next_ticks;
        if let Some(a) = app.as_mut() {
            a.running = tasks.iter().filter(|t| t.state == 'R').count() as u32;
            a.blocked = tasks.iter().filter(|t| t.state == 'D').count() as u32;
        }

        if prev_total.is_none() {
            return FastResult { sample: None, pid };
        }

        FastResult {
            sample: Some(FastSample {
                cores,
                sys_cpu,
                mem: mem_seen.then_some(mem),
                psi,
                net: net_rates,
                app_state,
                app,
                tasks,
                restarted,
            }),
            pid,
        }
    }
}

fn kv_pairs(line: &str) -> Vec<(String, &str)> {
    let mut out = Vec::new();
    let mut words: Vec<&str> = Vec::new();
    let mut pending: Option<String> = None;
    for token in line.split_ascii_whitespace() {
        if let Some(key) = pending.take() {
            out.push((key, token));
            continue;
        }
        if let Some(word) = token.strip_suffix(':') {
            words.push(word);
            pending = Some(words.join(" "));
            words.clear();
        } else {
            words.push(token);
        }
    }
    out
}

pub fn parse_app_meminfo(output: &str) -> Option<AppMemory> {
    let mut m = AppMemory::default();
    let mut found = false;
    let mut section = "";
    let mut objects = Objects::default();
    let mut objects_seen = false;
    let mut sqlite = Sqlite::default();
    let mut sqlite_seen = false;
    let mut db_sizes: Vec<(String, u64)> = Vec::new();
    let mut pool: HashMap<String, (u64, u64)> = HashMap::new();

    for raw in output.lines() {
        let line = raw.trim();
        if let Some(rest) = line.strip_prefix("** MEMINFO in pid ") {
            m.pid = first_num(rest).unwrap_or(0);
            found = true;
            section = "table";
            continue;
        }
        if !found {
            continue;
        }
        match line {
            "App Summary" => {
                section = "summary";
                continue;
            }
            "Objects" => {
                section = "objects";
                continue;
            }
            "SQL" => {
                section = "sql";
                continue;
            }
            "DATABASES" => {
                section = "databases";
                continue;
            }
            "POOL STATS" => {
                section = "pool";
                continue;
            }
            "Native Allocations" | "Mapped Bitmaps" | "Unreachable memory"
            | "Asset Allocations" => {
                section = "skip";
                continue;
            }
            _ => {}
        }
        match section {
            "table" => {
                let heap = if line.starts_with("Native Heap") {
                    Some(false)
                } else if line.starts_with("Dalvik Heap") {
                    Some(true)
                } else {
                    None
                };
                if let Some(java) = heap {
                    let nums: Vec<u64> = line
                        .split_ascii_whitespace()
                        .filter_map(|t| t.parse().ok())
                        .collect();
                    if nums.len() >= 6 {
                        let size = nums[nums.len() - 3];
                        let alloc = nums[nums.len() - 2];
                        if java {
                            m.java_heap_size_kb = size;
                            m.java_heap_alloc_kb = alloc;
                        } else {
                            m.native_heap_size_kb = size;
                            m.native_heap_alloc_kb = alloc;
                        }
                    }
                } else if let Some(rest) = line.strip_prefix("TOTAL") {
                    if rest.starts_with(char::is_whitespace) {
                        let nums: Vec<u64> = rest
                            .split_ascii_whitespace()
                            .filter_map(|t| t.parse().ok())
                            .collect();
                        if let Some(&pss) = nums.first() {
                            m.pss_kb = pss;
                        }
                    }
                }
            }
            "summary" => {
                if line.starts_with("TOTAL") {
                    for (k, v) in kv_pairs(line) {
                        let v: u64 = num(v).unwrap_or(0);
                        match k.as_str() {
                            "TOTAL PSS" | "TOTAL" => m.pss_kb = v,
                            "TOTAL RSS" => m.rss_kb = v,
                            "TOTAL SWAP PSS" | "TOTAL SWAP (KB)" => m.swap_pss_kb = v,
                            _ => {}
                        }
                    }
                    continue;
                }
                let Some((key, values)) = line.split_once(':') else {
                    continue;
                };
                let mut it = values
                    .split_ascii_whitespace()
                    .filter_map(|t| t.parse::<u64>().ok());
                let pss = it.next().unwrap_or(0);
                let rss = it.next().unwrap_or(0);
                match key.trim() {
                    "Java Heap" => {
                        m.java_kb = pss;
                        m.java_rss_kb = rss;
                    }
                    "Native Heap" => {
                        m.native_kb = pss;
                        m.native_rss_kb = rss;
                    }
                    "Code" => {
                        m.code_kb = pss;
                        m.code_rss_kb = rss;
                    }
                    "Stack" => {
                        m.stack_kb = pss;
                        m.stack_rss_kb = rss;
                    }
                    "Graphics" => {
                        m.graphics_kb = pss;
                        m.graphics_rss_kb = rss;
                    }
                    "Private Other" => m.other_kb = pss,
                    "System" => m.system_kb = pss,
                    _ => {}
                }
            }
            "objects" => {
                for (k, v) in kv_pairs(line) {
                    let v: u32 = num(v).unwrap_or(0);
                    objects_seen = true;
                    match k.as_str() {
                        "Views" => objects.views = v,
                        "ViewRootImpl" => objects.view_roots = v,
                        "AppContexts" => objects.app_contexts = v,
                        "Activities" => objects.activities = v,
                        "Assets" => objects.assets = v,
                        "AssetManagers" => objects.asset_managers = v,
                        "Local Binders" => objects.local_binders = v,
                        "Proxy Binders" => objects.proxy_binders = v,
                        "Parcel memory" => objects.parcel_memory_kb = v,
                        "Parcel count" => objects.parcel_count = v,
                        "Death Recipients" => objects.death_recipients = v,
                        "WebViews" => objects.webviews = v,
                        _ => {}
                    }
                }
            }
            "sql" => {
                for (k, v) in kv_pairs(line) {
                    let v: u64 = num(v).unwrap_or(0);
                    sqlite_seen = true;
                    match k.as_str() {
                        "MEMORY_USED" => sqlite.memory_used_kb = v,
                        "PAGECACHE_OVERFLOW" => sqlite.pagecache_overflow_kb = v,
                        "MALLOC_SIZE" => sqlite.malloc_size_kb = v,
                        _ => {}
                    }
                }
            }
            "databases" => {
                let Some(pos) = line.find('/') else { continue };
                let (nums, name) = line.split_at(pos);
                if name.contains("(attached)") {
                    continue;
                }
                let base = db_name(name);
                let n: Vec<u64> = nums
                    .split_ascii_whitespace()
                    .filter_map(|t| t.parse().ok())
                    .collect();
                if n.len() >= 2 && !db_sizes.iter().any(|(d, _)| *d == base) {
                    db_sizes.push((base, n[0] * n[1]));
                }
            }
            "pool" => {
                let Some(pos) = line.find('/') else { continue };
                let (nums, name) = line.split_at(pos);
                let n: Vec<u64> = nums
                    .split_ascii_whitespace()
                    .filter_map(|t| t.parse().ok())
                    .collect();
                if n.len() >= 2 {
                    let e = pool.entry(db_name(name)).or_default();
                    e.0 += n[0];
                    e.1 += n[1];
                }
            }
            _ => {}
        }
    }
    if !found {
        return None;
    }
    if objects_seen {
        m.objects = Some(objects);
    }
    if sqlite_seen || !db_sizes.is_empty() {
        sqlite.databases = db_sizes
            .into_iter()
            .map(|(name, size_kb)| {
                let (hits, misses) = pool.get(&name).copied().unwrap_or((0, 0));
                Database {
                    name,
                    size_kb,
                    cache_hits: hits,
                    cache_misses: misses,
                }
            })
            .collect();
        m.sqlite = Some(sqlite);
    }
    Some(m)
}

fn db_name(path: &str) -> String {
    let file = path.trim().rsplit('/').next().unwrap_or(path).trim();
    file.split(" (").next().unwrap_or(file).trim().to_string()
}

#[derive(Default)]
pub struct FrameParser {
    pid: u32,
    last_vsync: u64,
}

impl FrameParser {
    pub fn reset(&mut self) {
        self.pid = 0;
        self.last_vsync = 0;
    }

    pub fn parse(&mut self, output: &str) -> Option<FrameUpdate> {
        let (output, uptime) = match output.rsplit_once("==U") {
            Some((body, tail)) => (
                body,
                tail.split_whitespace()
                    .next()
                    .and_then(|v| v.parse::<f64>().ok()),
            ),
            None => (output, None),
        };
        let mut update = FrameUpdate {
            uptime_s: uptime,
            ..FrameUpdate::default()
        };
        let mut found = false;
        let mut columns: Option<FrameColumns> = None;
        let mut frames: Vec<Frame> = Vec::new();
        let s = &mut update.summary;
        for raw in output.lines() {
            let line = raw.trim();
            if let Some(rest) = line.strip_prefix("** Graphics info for pid ") {
                update.pid = first_num(rest).unwrap_or(0);
                found = true;
                continue;
            }
            if !found {
                continue;
            }
            if line == "---PROFILEDATA---" {
                columns = None;
                continue;
            }
            if line.starts_with("Flags,") {
                columns = FrameColumns::from_header(line);
                continue;
            }
            if let Some(cols) = &columns {
                if line.as_bytes().first().is_some_and(|b| b.is_ascii_digit()) {
                    if let Some(f) = cols.frame(line) {
                        frames.push(f);
                    }
                    continue;
                }
            }
            if let Some(rest) = line.strip_prefix("HISTOGRAM: ") {
                s.histogram = rest
                    .split_ascii_whitespace()
                    .filter_map(|t| {
                        let (ms, n) = t.split_once("ms=")?;
                        Some((ms.parse().ok()?, n.parse().ok()?))
                    })
                    .collect();
                continue;
            }
            let Some((key, value)) = line.split_once(": ") else {
                continue;
            };
            let v: u64 = value
                .split_ascii_whitespace()
                .next()
                .map(|t| t.trim_end_matches("ms"))
                .and_then(num)
                .unwrap_or(0);
            match key {
                "Total frames rendered" => s.total = v,
                "Janky frames" => s.janky = v,
                "Janky frames (legacy)" => s.janky_legacy = v,
                "50th percentile" => s.p50_ms = v as u32,
                "90th percentile" => s.p90_ms = v as u32,
                "95th percentile" => s.p95_ms = v as u32,
                "99th percentile" => s.p99_ms = v as u32,
                "50th gpu percentile" => s.gpu_p50_ms = v as u32,
                "90th gpu percentile" => s.gpu_p90_ms = v as u32,
                "95th gpu percentile" => s.gpu_p95_ms = v as u32,
                "99th gpu percentile" => s.gpu_p99_ms = v as u32,
                "Number Missed Vsync" => s.missed_vsync = v,
                "Number High input latency" => s.high_input_latency = v,
                "Number Slow UI thread" => s.slow_ui_thread = v,
                "Number Slow bitmap uploads" => s.slow_bitmap_uploads = v,
                "Number Slow issue draw commands" => s.slow_draw_commands = v,
                "Number Frame deadline missed" => s.deadline_missed = v,
                _ => {}
            }
        }
        if !found {
            return None;
        }
        if update.pid != self.pid {
            self.pid = update.pid;
            self.last_vsync = 0;
        }
        frames.sort_by_key(|f| f.vsync_ns);
        frames.dedup_by_key(|f| f.vsync_ns);
        let last = self.last_vsync;
        update.baseline = last == 0;
        update.frames = frames.into_iter().filter(|f| f.vsync_ns > last).collect();
        if let Some(f) = update.frames.last() {
            self.last_vsync = f.vsync_ns;
        }
        Some(update)
    }
}

struct FrameColumns {
    flags: usize,
    intended: usize,
    completed: usize,
    deadline: Option<usize>,
    interval: Option<usize>,
}

impl FrameColumns {
    fn from_header(header: &str) -> Option<Self> {
        let names: Vec<&str> = header.split(',').collect();
        let find = |n: &str| names.iter().position(|c| *c == n);
        Some(Self {
            flags: find("Flags")?,
            intended: find("IntendedVsync")?,
            completed: find("FrameCompleted")?,
            deadline: find("FrameDeadline"),
            interval: find("FrameInterval"),
        })
    }

    fn frame(&self, line: &str) -> Option<Frame> {
        let values: Vec<u64> = line
            .split(',')
            .map(|v| v.trim().parse().unwrap_or(0))
            .collect();
        let get = |i: usize| values.get(i).copied().unwrap_or(0);
        if get(self.flags) != 0 {
            return None;
        }
        let intended = get(self.intended);
        let completed = get(self.completed);
        if intended == 0 || completed <= intended {
            return None;
        }
        let interval = self
            .interval
            .map(get)
            .filter(|v| *v > 1_000_000 && *v < 100_000_000);
        let budget_ms = interval.map(|v| v as f32 / 1e6).unwrap_or(16.67);
        let ms = (completed - intended) as f32 / 1e6;
        let janky = match self.deadline.map(get).filter(|d| *d > intended) {
            Some(deadline) => completed > deadline,
            None => ms > budget_ms,
        };
        Some(Frame {
            vsync_ns: intended,
            ms,
            budget_ms,
            janky,
        })
    }
}

pub fn battery_command() -> &'static str {
    "dumpsys battery; echo ==I; cat /sys/class/power_supply/battery/current_now 2>/dev/null"
}

pub fn parse_battery(output: &str) -> Option<Battery> {
    let (main, sysfs) = output.split_once("==I").unwrap_or((output, ""));
    let mut b = Battery {
        plugged: "none".into(),
        ..Default::default()
    };
    let mut seen = false;
    for line in main.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        match key {
            "level" => {
                b.level = num(value).unwrap_or(0);
                seen = true;
            }
            "voltage" => b.voltage_mv = num(value).unwrap_or(0),
            "temperature" => b.temperature_c = num::<f32>(value).unwrap_or(0.0) / 10.0,
            "technology" => b.technology = value.to_string(),
            "current now" => b.current_ma = num::<i64>(value).map(normalize_current),
            "Charge counter" | "charge counter" => {
                if b.charge_counter_uah.is_none() {
                    b.charge_counter_uah = num(value);
                }
            }
            "status" => {
                b.status = match value {
                    "2" => "charging",
                    "3" => "discharging",
                    "4" => "not charging",
                    "5" => "full",
                    _ => "unknown",
                }
                .into()
            }
            "health" => {
                b.health = match value {
                    "2" => "good",
                    "3" => "overheat",
                    "4" => "dead",
                    "5" => "over voltage",
                    "6" => "failure",
                    "7" => "cold",
                    _ => "unknown",
                }
                .into()
            }
            "AC powered" | "USB powered" | "Wireless powered" | "Dock powered"
                if value == "true" =>
            {
                b.plugged = key.trim_end_matches(" powered").to_lowercase();
            }
            _ => {}
        }
    }
    if !seen {
        return None;
    }
    if b.current_ma.is_none() {
        b.current_ma = first_num::<i64>(sysfs).map(normalize_current);
    }
    Some(b)
}

fn normalize_current(v: i64) -> i32 {
    if v.abs() > 20_000 {
        (v / 1000) as i32
    } else {
        v as i32
    }
}

pub fn parse_thermal(output: &str) -> Option<Thermal> {
    let mut status = None;
    let mut hal: Vec<Sensor> = Vec::new();
    let mut cached: Vec<Sensor> = Vec::new();
    let mut section = 0;
    for raw in output.lines() {
        let line = raw.trim();
        if let Some(v) = line.strip_prefix("Thermal Status:") {
            status = num::<u8>(v);
            continue;
        }
        if line.starts_with("Cached temperatures") {
            section = 1;
            continue;
        }
        if line.starts_with("Current temperatures from HAL") {
            section = 2;
            continue;
        }
        if !line.starts_with("Temperature{") {
            if line.ends_with(':') {
                section = 0;
            }
            continue;
        }
        let field = |name: &str| {
            line.split(name)
                .nth(1)
                .and_then(|r| r.split([',', '}']).next())
        };
        let (Some(value), Some(name)) = (field("mValue=").and_then(num::<f32>), field("mName="))
        else {
            continue;
        };
        if !value.is_finite() || value == 0.0 {
            continue;
        }
        let target = match section {
            1 => &mut cached,
            2 => &mut hal,
            _ => continue,
        };
        if !target.iter().any(|s| s.name == name) {
            target.push(Sensor {
                name: name.to_string(),
                celsius: value,
            });
        }
    }
    let status = status?;
    Some(Thermal {
        status,
        sensors: if hal.is_empty() { cached } else { hal },
    })
}

pub fn parse_doze(output: &str) -> Doze {
    let (states, bucket) = output.split_once("==B").unwrap_or((output, ""));
    let mut d = Doze::default();
    for token in states.split_ascii_whitespace() {
        if let Some(v) = token.strip_prefix("mState=") {
            if d.deep.is_empty() {
                d.deep = v.to_string();
            }
        } else if let Some(v) = token.strip_prefix("mLightState=") {
            if d.light.is_empty() {
                d.light = v.to_string();
            }
        }
    }
    if d.deep.is_empty() {
        d.deep = "ACTIVE".into();
    }
    if d.light.is_empty() {
        d.light = "ACTIVE".into();
    }
    d.bucket = bucket
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .and_then(|b| {
            let name = match b {
                "5" => "EXEMPTED",
                "10" => "ACTIVE",
                "20" => "WORKING_SET",
                "30" => "FREQUENT",
                "40" => "RARE",
                "45" => "RESTRICTED",
                "50" => "NEVER",
                other if other.chars().all(|c| c.is_ascii_digit()) => other,
                _ => return None,
            };
            Some(name.to_string())
        });
    d
}

pub fn jobs_command(package: &str) -> String {
    format!(
        "dumpsys jobscheduler {package} | grep -E '^Registered |^  JOB #|START: |STOP: |^Active jobs|^  Slot #'; echo ==A; dumpsys alarm | grep -c 'Alarm{{.* {package}}}'"
    )
}

pub fn parse_jobs(output: &str, package: &str) -> Jobs {
    let (main, alarms) = output.split_once("==A").unwrap_or((output, ""));
    let mut jobs = Jobs {
        alarms: first_num(alarms),
        ..Default::default()
    };
    let mut services: Vec<JobInfo> = Vec::new();
    let mut active = false;
    let prefix = format!("{package}/");
    for raw in main.lines() {
        let line = raw.trim();
        if line.starts_with("Active jobs") {
            active = true;
            continue;
        }
        if let Some(rest) = line.strip_prefix("JOB #") {
            let Some(service) = rest
                .split_ascii_whitespace()
                .find_map(|t| t.strip_prefix(prefix.as_str()))
            else {
                continue;
            };
            jobs.scheduled += 1;
            let short = short_class(service);
            match services.iter_mut().find(|s| s.service == short) {
                Some(s) => s.count += 1,
                None => services.push(JobInfo {
                    service: short,
                    count: 1,
                    running: 0,
                }),
            }
            continue;
        }
        if active && line.starts_with("Slot #") && line.contains(&prefix) {
            jobs.running += 1;
            continue;
        }
        for event in ["START: ", "STOP: "] {
            let Some(pos) = line.find(event) else {
                continue;
            };
            let ago = line[..pos].trim().to_string();
            let mut parts = line[pos + event.len()..].split_ascii_whitespace();
            let _id = parts.next();
            let Some(service) = parts.next().and_then(|t| t.strip_prefix(prefix.as_str())) else {
                continue;
            };
            let reason: Vec<&str> = parts.collect();
            jobs.recent.push(JobRun {
                ago,
                service: short_class(service),
                event: event.trim_end_matches(": ").to_lowercase(),
                reason: reason.join(" "),
            });
        }
    }
    jobs.recent.reverse();
    jobs.recent.truncate(12);
    services.sort_by(|a, b| b.count.cmp(&a.count));
    jobs.services = services;
    jobs
}

fn short_class(class: &str) -> String {
    class.rsplit('.').next().unwrap_or(class).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stat_line(
        id: u32,
        name: &str,
        state: char,
        utime: u64,
        stime: u64,
        start: u64,
        processor: u16,
    ) -> String {
        let mut f = vec!["0".to_string(); 37];
        f[0] = state.to_string();
        f[11] = utime.to_string();
        f[12] = stime.to_string();
        f[17] = "2".into();
        f[19] = start.to_string();
        f[36] = processor.to_string();
        format!("{id} ({name}) {}", f.join(" "))
    }

    fn fast_output(
        total: u64,
        idle: u64,
        uptime: f64,
        app: Option<(u64, u64)>,
        threads: &[(u32, &str, u64)],
    ) -> String {
        let mut s = format!(
            "/proc/uptime:{uptime} 1.0\n/proc/stat:cpu  {} 0 0 {idle} 0 0 0 0 0 0\n/proc/stat:cpu0 {} 0 0 {} 0 0 0 0 0 0\n/proc/stat:cpu2 {} 0 0 {} 0 0 0 0 0 0\n\
/proc/meminfo:MemTotal:        7500000 kB\n/proc/meminfo:MemAvailable:    3000000 kB\n\
/proc/net/dev:  wlan0: {} 10 0 0 0 0 0 0 {} 5 0 0 0 0 0 0\n\
/sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq:1959000\n/sys/devices/system/cpu/cpu0/cpufreq/scaling_max_freq:1959000\n",
            total - idle,
            (total - idle) / 2,
            idle / 2,
            (total - idle) / 2,
            idle / 2,
            (uptime * 1000.0) as u64,
            (uptime * 100.0) as u64,
        );
        if let Some((u, k)) = app {
            s.push_str(&format!(
                "/proc/4321/stat:{}\n",
                stat_line(4321, "com.example.app", 'S', u, k, 500, 3)
            ));
            s.push_str("/proc/4321/status:Threads:\t2\n/proc/4321/status:voluntary_ctxt_switches:\t100\n/proc/4321/oom_score_adj:0\n");
            for (tid, name, ticks) in threads {
                s.push_str(&format!(
                    "/proc/4321/task/{tid}/stat:{}\n",
                    stat_line(*tid, name, 'R', *ticks, 0, 500, 1)
                ));
            }
        }
        s
    }

    #[test]
    fn fast_sample_needs_a_baseline_then_computes_deltas() {
        let mut p = FastParser::default();
        let first = p.parse(
            &fast_output(
                1000,
                800,
                100.0,
                Some((100, 20)),
                &[(4321, "main", 50), (4322, "Render (Thread)", 10)],
            ),
            true,
            Some(4321),
        );
        assert!(first.sample.is_none());
        let r = p.parse(
            &fast_output(
                1400,
                1000,
                102.0,
                Some((300, 60)),
                &[(4321, "main", 150), (4322, "Render (Thread)", 210)],
            ),
            true,
            Some(4321),
        );
        let s = r.sample.unwrap();
        assert_eq!(s.app_state, AppState::Running);
        assert!((s.sys_cpu - 50.0).abs() < 0.01);
        let app = s.app.unwrap();
        assert!((app.cpu - 60.0).abs() < 0.01, "{}", app.cpu);
        assert!((app.kernel - 10.0).abs() < 0.01);
        assert_eq!(s.cores.len(), 2);
        assert_eq!(s.cores[0].cur_khz, 1959000);
        let render = s.tasks.iter().find(|t| t.id == 4322).unwrap();
        assert_eq!(render.name, "Render (Thread)");
        assert!((render.cpu - 100.0).abs() < 0.01, "{}", render.cpu);
        let net = s.net.unwrap();
        assert!((net.wifi_rx_bps - 1000.0).abs() < 1.0);
        assert_eq!(s.mem.unwrap().used_kb(), 4_500_000);
    }

    #[test]
    fn app_restart_resets_deltas() {
        let mut p = FastParser::default();
        p.parse(
            &fast_output(1000, 800, 100.0, Some((100, 20)), &[]),
            true,
            Some(4321),
        );
        let mut out = fast_output(1400, 1000, 102.0, None, &[]);
        out.push_str(&format!(
            "/proc/4321/stat:{}\n",
            stat_line(4321, "com.example.app", 'S', 5, 1, 900, 3)
        ));
        let s = p.parse(&out, true, Some(4321)).sample.unwrap();
        assert!(s.restarted);
        assert!(s.app.unwrap().cpu.is_nan());
    }

    #[test]
    fn restart_through_a_not_running_gap_is_counted() {
        let mut p = FastParser::default();
        p.parse(
            &fast_output(1000, 800, 100.0, Some((100, 20)), &[]),
            true,
            Some(4321),
        );
        let gone = p.parse(&fast_output(1200, 900, 101.0, None, &[]), true, Some(4321));
        assert_eq!(gone.sample.unwrap().app_state, AppState::NotRunning);
        let mut back = fast_output(1400, 1000, 102.0, None, &[]);
        back.push_str(&format!(
            "pid=5555\n/proc/5555/stat:{}\n",
            stat_line(5555, "com.example.app", 'S', 5, 1, 9000, 3)
        ));
        assert!(p.parse(&back, true, None).sample.unwrap().restarted);
    }

    #[test]
    fn missing_app_process_is_not_running() {
        let mut p = FastParser::default();
        p.parse(
            "pid=\n/proc/uptime:1 1\n/proc/stat:cpu  10 0 0 10 0 0 0\n",
            true,
            None,
        );
        let r = p.parse(
            "pid=\n/proc/uptime:2 1\n/proc/stat:cpu  20 0 0 20 0 0 0\n",
            true,
            None,
        );
        assert_eq!(r.pid, None);
        assert_eq!(r.sample.unwrap().app_state, AppState::NotRunning);
    }

    #[test]
    fn device_mode_reads_every_process() {
        let mut p = FastParser::default();
        let out = |t: u64, a: u64| {
            format!("/proc/uptime:1 1\n/proc/stat:cpu  {t} 0 0 {t} 0 0 0\n/proc/stat:cpu0 {t} 0 0 {t} 0 0 0\n/proc/1/stat:{}\n/proc/77/stat:{}\n", stat_line(1, "init", 'S', 1, 0, 0, 0), stat_line(77, "system_server", 'R', a, 0, 0, 0))
        };
        p.parse(&out(100, 10), false, None);
        let s = p.parse(&out(200, 60), false, None).sample.unwrap();
        assert_eq!(s.app_state, AppState::NotSelected);
        let ss = s.tasks.iter().find(|t| t.id == 77).unwrap();
        assert!((ss.cpu - 25.0).abs() < 0.01, "{}", ss.cpu);
    }

    #[test]
    fn probe_builds_s24_clusters() {
        let out = "37\n17\nSM-S921B\nsamsung\n==C\n\
/sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_max_freq:1959000\n/sys/devices/system/cpu/cpu1/cpufreq/cpuinfo_max_freq:1959000\n\
/sys/devices/system/cpu/cpu4/cpufreq/cpuinfo_max_freq:2592000\n/sys/devices/system/cpu/cpu7/cpufreq/cpuinfo_max_freq:2900000\n\
/sys/devices/system/cpu/cpu9/cpufreq/cpuinfo_max_freq:3207000\n\
/sys/devices/system/cpu/cpu0/cpufreq/related_cpus:0 1 2 3\n/sys/devices/system/cpu/cpu1/cpufreq/related_cpus:0 1 2 3\n\
/sys/devices/system/cpu/cpu4/cpufreq/related_cpus:4 5 6\n/sys/devices/system/cpu/cpu7/cpufreq/related_cpus:7 8\n/sys/devices/system/cpu/cpu9/cpufreq/related_cpus:9\n\
==S\nService gfxinfo: found\nService thermalservice: found\nService battery: found\nService deviceidle: not found\n==P\npsi\ncpufreq\n==E\n";
        let caps = parse_probe(out);
        assert_eq!(caps.sdk, 37);
        assert_eq!(caps.model, "SM-S921B");
        assert!(caps.gfxinfo && caps.thermal && !caps.deviceidle && caps.psi && caps.cpufreq);
        let names: Vec<&str> = caps.clusters.iter().map(|c| c.name).collect();
        assert_eq!(names, ["little", "mid", "big", "prime"]);
        assert_eq!(caps.clusters[1].cores, [4, 5, 6]);
        assert_eq!(caps.cluster_of(8), Some(2));
    }

    const MEMINFO: &str = "Applications Memory Usage (in Kilobytes):\n\
** MEMINFO in pid 4321 [com.example.app] **\n\
                   Pss  Private  Private  SwapPss      Rss     Heap     Heap     Heap\n\
                 Total    Dirty    Clean    Dirty    Total     Size    Alloc     Free\n\
                ------   ------   ------   ------   ------   ------   ------   ------\n\
  Native Heap     8980     6472     2492    22700    11092    56052    26571    24903\n\
  Dalvik Heap    12570    11208      392    12029    18128    73956    24804    49152\n\
 Dalvik Other    30226    13968      432      352    46440                           \n\
        TOTAL   218434    39084    14176   137553   226908   130008    51375    74055\n\
 \n\
 App Summary\n\
                       Pss(KB)                        Rss(KB)\n\
                        ------                         ------\n\
           Java Heap:    14756                          37208\n\
         Native Heap:     6472                          11092\n\
                Code:     7784                         152796\n\
               Stack:     1236                           1404\n\
            Graphics:     5368                           5368\n\
       Private Other:    17644\n\
              System:   165174\n\
             Unknown:                                   19040\n\
 \n\
           TOTAL PSS:   218434            TOTAL RSS:   226908       TOTAL SWAP PSS:   137553\n\
 \n\
 Objects\n\
               Views:       39         ViewRootImpl:        1\n\
         AppContexts:       17           Activities:        1\n\
       Local Binders:       44        Proxy Binders:       87\n\
       Parcel memory:       37         Parcel count:      138\n\
    Death Recipients:        5             WebViews:        0\n\
 \n\
 Native Allocations\n\
    Other (malloced):      668                             62\n\
 \n\
 SQL\n\
         MEMORY_USED:     1488\n\
  PAGECACHE_OVERFLOW:      588          MALLOC_SIZE:       46\n\
 \n\
 DATABASES\n\
      pgsz     dbsz   Lookaside(b) cache hits cache misses cache size  Dbname\n\
PER CONNECTION STATS\n\
         4      116            116   116  2044    19  /data/user/0/com.example.app/databases/main_db\n\
         4       16                    0     0     0    (attached) temp\n\
         4      116            110     9  1172     4  /data/user/0/com.example.app/databases/main_db (2)\n\
         4      104            123   319  2089    25  /data/user/0/com.example.app/no_backup/androidx.work.workdb\n\
POOL STATS\n\
     cache hits  cache misses    cache size  Dbname\n\
            182          5432            36  /data/user/0/com.example.app/databases/main_db\n\
            349          3248            31  /data/user/0/com.example.app/no_backup/androidx.work.workdb\n";

    #[test]
    fn meminfo_reads_summary_heaps_objects_and_sqlite() {
        let m = parse_app_meminfo(MEMINFO).unwrap();
        assert_eq!(m.pid, 4321);
        assert_eq!(
            (m.pss_kb, m.rss_kb, m.swap_pss_kb),
            (218434, 226908, 137553)
        );
        assert_eq!(
            (
                m.java_kb,
                m.native_kb,
                m.code_kb,
                m.stack_kb,
                m.graphics_kb,
                m.other_kb,
                m.system_kb
            ),
            (14756, 6472, 7784, 1236, 5368, 17644, 165174)
        );
        assert_eq!(m.java_rss_kb, 37208);
        assert_eq!((m.java_heap_size_kb, m.java_heap_alloc_kb), (73956, 24804));
        assert_eq!(
            (m.native_heap_size_kb, m.native_heap_alloc_kb),
            (56052, 26571)
        );
        let o = m.objects.unwrap();
        assert_eq!(
            (o.views, o.view_roots, o.app_contexts, o.activities),
            (39, 1, 17, 1)
        );
        assert_eq!(
            (
                o.local_binders,
                o.proxy_binders,
                o.parcel_count,
                o.death_recipients,
                o.webviews
            ),
            (44, 87, 138, 5, 0)
        );
        let sql = m.sqlite.unwrap();
        assert_eq!(
            (
                sql.memory_used_kb,
                sql.pagecache_overflow_kb,
                sql.malloc_size_kb
            ),
            (1488, 588, 46)
        );
        assert_eq!(sql.databases.len(), 2);
        assert_eq!(sql.databases[0].name, "main_db");
        assert_eq!(sql.databases[0].size_kb, 464);
        assert_eq!(
            (sql.databases[0].cache_hits, sql.databases[0].cache_misses),
            (182, 5432)
        );
    }

    #[test]
    fn meminfo_for_a_missing_process() {
        assert!(parse_app_meminfo("No process found for: com.example.app\n").is_none());
    }

    const GFX: &str = "Applications Graphics Acceleration Info:\n\
** Graphics info for pid 4321 [com.example.app] **\n\
Total frames rendered: 2832\n\
Janky frames: 76 (2.68%)\n\
Janky frames (legacy): 102 (3.60%)\n\
50th percentile: 6ms\n\
90th percentile: 9ms\n\
95th percentile: 14ms\n\
99th percentile: 61ms\n\
Number Missed Vsync: 27\n\
Number High input latency: 1009\n\
Number Slow UI thread: 69\n\
Number Slow bitmap uploads: 0\n\
Number Slow issue draw commands: 2\n\
Number Frame deadline missed: 76\n\
HISTOGRAM: 5ms=846 6ms=785 7ms=533\n\
50th gpu percentile: 2ms\n\
99th gpu percentile: 3ms\n\
GPU HISTOGRAM: 1ms=240 2ms=2544\n\
---PROFILEDATA---\n\
Flags,FrameTimelineVsyncId,IntendedVsync,Vsync,InputEventId,HandleInputStart,AnimationStart,PerformTraversalsStart,DrawStart,FrameDeadline,FrameStartTime,FrameInterval,WorkloadTarget,AnimationTime,SyncQueued,SyncStart,IssueDrawCommandsStart,SwapBuffers,FrameCompleted,DequeueBufferDuration,QueueBufferDuration,GpuCompleted,SwapBuffersCompleted,DisplayPresentTime,CommandSubmissionCompleted,\n\
0,3436896,52613974954999,52613974954999,92410935,52613975012420,52613978226874,52613979066209,52613979134139,52613991688493,52613975008670,17012514,16733494,52613974954999,52613980363631,52613980473045,52613980599803,52613981767811,52613982989647,13046,378554,52613982989647,52613982262616,52614008530897,52613981767811,\n\
0,3436909,52613992149298,52613992149298,911203078,52613993022811,52613993053319,52613994085975,52613994159920,52614008815965,52613993018202,17028190,16666667,52613992149298,52613995150272,52613995263983,52613995372420,52613996576288,52614018660936,11445,370118,52613998660936,52613997047616,52614025486639,52613996576288,\n\
1,3436910,52614009149298,52614009149298,0,0,0,0,0,0,0,0,0,0,0,0,0,0,52614019149298,0,0,0,0,0,0,\n\
---PROFILEDATA---\n";

    #[test]
    fn gfxinfo_summary_and_new_frames_only() {
        let mut p = FrameParser::default();
        let u = p.parse(GFX).unwrap();
        assert_eq!(u.pid, 4321);
        assert_eq!(u.uptime_s, None);
        let with_uptime = format!("{GFX}==U\n67123.45 501234.00\n");
        let u2 = FrameParser::default().parse(&with_uptime).unwrap();
        assert_eq!(u2.uptime_s, Some(67123.45));
        assert_eq!(u2.summary.total, 2832);
        assert_eq!(
            (u.summary.total, u.summary.janky, u.summary.janky_legacy),
            (2832, 76, 102)
        );
        assert_eq!(
            (u.summary.p50_ms, u.summary.p99_ms, u.summary.gpu_p99_ms),
            (6, 61, 3)
        );
        assert_eq!(
            (
                u.summary.slow_ui_thread,
                u.summary.high_input_latency,
                u.summary.deadline_missed
            ),
            (69, 1009, 76)
        );
        assert_eq!(u.summary.histogram, vec![(5, 846), (6, 785), (7, 533)]);
        assert_eq!(u.frames.len(), 2);
        assert!((u.frames[0].ms - 8.03).abs() < 0.01);
        assert!((u.frames[0].budget_ms - 17.01).abs() < 0.01);
        assert!(!u.frames[0].janky);
        assert!(u.frames[1].janky);
        assert!(p.parse(GFX).unwrap().frames.is_empty());
    }

    #[test]
    fn battery_with_samsung_current_and_charge_counter() {
        let out = "Current Battery Service state:\n  AC powered: false\n  USB powered: true\n  Charge counter: 3880000\n  status: 3\n  health: 2\n  level: 87\n  voltage: 4210\n  temperature: 335\n  technology: Li-ion\n  current now: -352\n==I\n";
        let b = parse_battery(out).unwrap();
        assert_eq!(
            (b.level, b.voltage_mv, b.plugged.as_str(), b.status.as_str()),
            (87, 4210, "usb", "discharging")
        );
        assert_eq!(b.current_ma, Some(-352));
        assert_eq!(b.charge_counter_uah, Some(3880000));
        assert!((b.temperature_c - 33.5).abs() < 0.01);
        assert_eq!(b.drain_ma(), None);
    }

    #[test]
    fn battery_current_falls_back_to_sysfs_microamps() {
        let b = parse_battery("  status: 3\n  level: 50\n==I\n-412000\n").unwrap();
        assert_eq!(b.current_ma, Some(-412));
        assert_eq!(b.drain_ma(), Some(412.0));
    }

    #[test]
    fn thermal_prefers_live_hal_values() {
        let out = "Thermal Status: 1\nCached temperatures:\n\tTemperature{mValue=0.0, mType=2, mName=SUBBAT, mStatus=0}\n\tTemperature{mValue=48.3, mType=0, mName=AP, mStatus=0}\nHAL Ready: true\nCurrent temperatures from HAL:\n\tTemperature{mValue=35.4, mType=0, mName=AP, mStatus=0}\n\tTemperature{mValue=34.5, mType=3, mName=SKIN, mStatus=0}\n\tTemperature{mValue=0.0, mType=2, mName=SUBBAT, mStatus=0}\nCurrent cooling devices from HAL:\n";
        let t = parse_thermal(out).unwrap();
        assert_eq!(t.status, 1);
        assert_eq!(t.sensors.len(), 2);
        assert_eq!(t.sensors[0].name, "AP");
        assert!((t.sensors[0].celsius - 35.4).abs() < 0.01);
    }

    #[test]
    fn doze_and_bucket() {
        let d = parse_doze("  mState=IDLE mLightState=OVERRIDE\n==B\n10\n");
        assert_eq!(
            (d.deep.as_str(), d.light.as_str(), d.bucket.as_deref()),
            ("IDLE", "OVERRIDE", Some("ACTIVE"))
        );
        assert_eq!(parse_doze("==B\n").bucket, None);
    }

    #[test]
    fn jobs_for_one_package() {
        let out = "Registered 3 jobs:\n  JOB #u0a661/12: 6f2b com.example.app/androidx.work.impl.background.systemjob.SystemJobService\n  JOB #u0a661/13: 6f2c com.example.app/androidx.work.impl.background.systemjob.SystemJobService\n  JOB #u0a99/5: 1a com.other/x.Job\n       -14m59s499ms   START: #u0a661/-35 com.example.app/com.google.JobInfoSchedulerService\n       -14m58s472ms    STOP: #u0a661/-35 com.example.app/com.google.JobInfoSchedulerService app called jobFinished\nActive jobs:\n  N/A\n==A\n2\n";
        let j = parse_jobs(out, "com.example.app");
        assert_eq!((j.scheduled, j.running, j.alarms), (2, 0, Some(2)));
        assert_eq!(j.services[0].service, "SystemJobService");
        assert_eq!(j.services[0].count, 2);
        assert_eq!(j.recent.len(), 2);
        assert_eq!(j.recent[0].event, "stop");
        assert_eq!(j.recent[0].reason, "app called jobFinished");
    }

    #[test]
    fn devices_and_processes() {
        let d = parse_devices("List of devices attached\nRFCX11HALEL            device usb:34603008X product:e1sxtur model:SM_S921B device:e1s transport_id:4\nemulator-5554 offline\n");
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].model, "SM_S921B");
        assert_eq!(d[1].state, "offline");
        let (pkgs, names) = parse_ps("PID USER NAME\n1 root init\n4321 u0_a101 com.example.app\n4322 u0_a101 com.example.app:remote\n99 u0_i5 com.android.chrome:sandboxed_process0\n");
        assert_eq!(pkgs, ["com.example.app"]);
        assert_eq!(names.len(), 4);
        assert_eq!(names[1].name, "com.example.app");
    }

    #[test]
    fn package_name_validation() {
        assert!(is_valid_package_name("com.example.app_2"));
        assert!(!is_valid_package_name("app"));
        assert!(!is_valid_package_name("com.example'; reboot; '"));
        assert!(!is_valid_package_name("com..app"));
    }
}
