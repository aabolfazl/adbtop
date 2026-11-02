use std::collections::HashMap;

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Color;

use super::widgets::*;
use super::{app_gate, interval_label, markers, tail, unsupported, xlabels};
use crate::agent::proto::{ConnectionState, HostSource};
use crate::config::{CpuUnits, MemChart, NetScale};
use crate::model::{thermal_level_name, AppState, Source, TaskSample, THERMAL_LEVELS};
use crate::session::{is_open, ConnRecord, TaskHist};
use crate::theme::Theme;
use crate::tui::{App, Sort};

fn split(area: Rect, dir: Direction, c: &[Constraint]) -> Vec<Rect> {
    Layout::default()
        .direction(dir)
        .constraints(c.to_vec())
        .split(area)
        .to_vec()
}

fn rows(area: Rect, c: &[Constraint]) -> Vec<Rect> {
    split(area, Direction::Vertical, c)
}

fn cols(area: Rect, c: &[Constraint]) -> Vec<Rect> {
    split(area, Direction::Horizontal, c)
}

fn side(inner: Rect, pct: u16, min: u16, max: u16) -> (Rect, Rect) {
    let w = (inner.width * pct / 100)
        .clamp(min, max)
        .min(inner.width.saturating_sub(10));
    let left = Rect::new(inner.x, inner.y, inner.width - w - 1, inner.height);
    let right = Rect::new(inner.right() - w, inner.y, w, inner.height);
    (left, right)
}

fn opt(v: Option<f32>, f: impl Fn(f32) -> String) -> String {
    v.filter(|x| x.is_finite())
        .map(f)
        .unwrap_or_else(|| "–".into())
}

fn last(s: &crate::session::Series) -> Option<f32> {
    s.last()
}

struct Win {
    n: usize,
    xl: [String; 3],
    marks: Vec<(usize, String)>,
}

const MIN_SPAN: usize = 10;

fn span_samples(app: &App, full: usize) -> usize {
    full.min(app.session.history.sys_cpu.len().max(MIN_SPAN))
        .max(2)
}

fn span_secs(app: &App, n: usize) -> u32 {
    (n.saturating_sub(1) as f32 * app.session.interval_s()).round() as u32
}

impl Win {
    fn of(app: &App) -> Win {
        let n = span_samples(app, app.window_samples());
        Win {
            n,
            xl: xlabels(span_secs(app, n)),
            marks: markers(app, n),
        }
    }

    fn xl(&self) -> Option<[&str; 3]> {
        Some([
            self.xl[0].as_str(),
            self.xl[1].as_str(),
            self.xl[2].as_str(),
        ])
    }

    fn marks(&self) -> Vec<(usize, &str)> {
        self.marks.iter().map(|(i, s)| (*i, s.as_str())).collect()
    }
}

fn cpu_factor(app: &App) -> f32 {
    if app.config.cpu_units == CpuUnits::OneCore {
        app.session
            .fast
            .as_ref()
            .map_or(1, |f| f.cores.len().max(1)) as f32
    } else {
        1.0
    }
}

fn cpu_units_label(app: &App) -> &'static str {
    if app.config.cpu_units == CpuUnits::OneCore {
        "% of one core"
    } else {
        "% of all cores"
    }
}

fn scaled(v: Vec<f32>, f: f32) -> Vec<f32> {
    if f == 1.0 {
        v
    } else {
        v.into_iter().map(|x| x * f).collect()
    }
}

fn cpu_chart(pen: &mut Pen, app: &App, area: Rect, win: &Win, kernel: bool) {
    let t = pen.t;
    let h = &app.session.history;
    let f = cpu_factor(app);
    let sys = scaled(tail(&h.sys_cpu, win.n), f);
    let has_app = app.package().is_some() && h.app_cpu.has_data();
    let appv = scaled(tail(&h.app_cpu, win.n), f);
    let kern = scaled(tail(&h.app_kernel, win.n), f);
    let peak = sys
        .iter()
        .chain(appv.iter())
        .copied()
        .filter(|v| v.is_finite())
        .fold(0.0, f32::max);
    let ymax = nice_max(peak.max(10.0)).min(100.0 * f);
    let mut lines = vec![ChartLine {
        data: &sys,
        color: t.blue,
        fill: !has_app,
    }];
    if has_app {
        lines.push(ChartLine {
            data: &appv,
            color: t.green,
            fill: true,
        });
        if kernel {
            lines.push(ChartLine {
                data: &kern,
                color: t.orange,
                fill: false,
            });
        }
    }
    let marks = win.marks();
    let ticks = [ymax, ymax / 2.0, 0.0];
    pen.chart(
        area,
        &lines,
        &ChartOpts {
            ymin: 0.0,
            ymax,
            ticks: &ticks,
            fmt: &|v| format!("{v:.0}%"),
            label_w: 5,
            xlabels: win.xl(),
            markers: &marks,
            hlines: &[],
        },
    );
}

fn cluster_rows(pen: &mut Pen, app: &App, x: u16, y: u16, w: u16, max_rows: u16) -> u16 {
    let t = pen.t;
    let Some(caps) = &app.session.caps else {
        return 0;
    };
    let fast = app.session.fast.as_ref();
    let bar_w = w.saturating_sub(22).clamp(4, 14);
    let mut used = 0;
    for (i, c) in caps.clusters.iter().enumerate() {
        if used >= max_rows {
            break;
        }
        let load = fast.map_or(0.0, |f| {
            let v: Vec<f32> = f
                .cores
                .iter()
                .filter(|core| c.cores.contains(&core.id))
                .map(|core| core.usage)
                .collect();
            if v.is_empty() {
                0.0
            } else {
                v.iter().sum::<f32>() / v.len() as f32
            }
        });
        let ghz = app
            .session
            .history
            .cluster_ghz
            .get(i)
            .and_then(|s| s.last());
        let color = cluster_color(t, c.name);
        let yy = y + used;
        pen.text(x, yy, &pad(c.name, 7), color, false, x + w);
        pen.hbar(x + 7, yy, bar_w, load, 100.0, t.usage(load.max(21.0)));
        pen.segs(
            x + 8 + bar_w,
            yy,
            &[
                (&opt(ghz, |g| format!("{g:.2}")), t.fg, false),
                (&format!("/{:.2}", c.max_khz as f32 / 1e6), t.dim, false),
            ],
            x + w,
        );
        used += 1;
    }
    used
}

fn cluster_color(t: &Theme, name: &str) -> Color {
    match name {
        "little" => t.aqua,
        "mid" => t.blue,
        "big" => t.purple,
        "prime" => t.orange,
        other => {
            let i = other.trim_start_matches('c').parse::<usize>().unwrap_or(0);
            [
                t.aqua, t.green, t.blue, t.yellow, t.purple, t.orange, t.red, t.fg,
            ][i % 8]
        }
    }
}

fn cluster_letter(name: &str) -> &'static str {
    match name {
        "little" => "L",
        "mid" => "M",
        "big" => "B",
        "prime" => "P",
        _ => "C",
    }
}

fn throttle_note(app: &App) -> Option<String> {
    let caps = app.session.caps.as_ref()?;
    let h = &app.session.history;
    caps.clusters.iter().enumerate().rev().find_map(|(i, c)| {
        let cap = h.cluster_cap_ghz.get(i)?.last()?;
        let max = c.max_khz as f32 / 1e6;
        (cap + 0.05 < max).then(|| format!("{} capped {:.2}/{:.2} GHz", c.name, cap, max))
    })
}

pub fn overview(pen: &mut Pen, app: &App, area: Rect) {
    let r = rows(
        area,
        &[
            Constraint::Ratio(1, 3),
            Constraint::Ratio(1, 3),
            Constraint::Ratio(1, 3),
        ],
    );
    let c: Vec<Vec<Rect>> = r
        .iter()
        .map(|row| {
            cols(
                *row,
                &[Constraint::Percentage(50), Constraint::Percentage(50)],
            )
        })
        .collect();
    let win = Win::of(app);
    tile_cpu(pen, app, c[0][0], &win);
    tile_memory(pen, app, c[0][1], &win);
    tile_frames(pen, app, c[1][0], &win);
    tile_network(pen, app, c[1][1], &win);
    tile_tasks(pen, app, c[2][0]);
    tile_power(pen, app, c[2][1], &win);
}

fn tile_cpu(pen: &mut Pen, app: &App, area: Rect, win: &Win) {
    let t = pen.t;
    let has_app = app.package().is_some();
    let legend: Vec<Seg> = if has_app {
        vec![
            ("━ app ", t.green, false),
            ("━ system ", t.blue, false),
            ("[2]", t.dim, false),
        ]
    } else {
        vec![("━ system ", t.blue, false), ("[2]", t.dim, false)]
    };
    let inner = pen.panel(area, &[("CPU", t.yellow, true)], &legend, None);
    if inner.height < 3 {
        return;
    }
    let (chart, stats) = side(inner, 36, 26, 40);
    cpu_chart(pen, app, chart, win, false);
    let h = &app.session.history;
    let (x, mut y, right) = (stats.x, stats.y, stats.right());
    if has_app {
        let now = last(&h.app_cpu);
        let f = cpu_factor(app);
        let p = |v: f32| format!("{:.1}%", v * f);
        pen.segs(
            x,
            y,
            &[
                ("APP    ", t.dim, false),
                (&opt(now, p), t.green, true),
                ("  peak ", t.dim, false),
                (&opt(h.app_cpu.max(), p), t.orange, false),
            ],
            right,
        );
        y += 1;
        pen.segs(
            x,
            y,
            &[
                ("  kernel ", t.dim, false),
                (&opt(last(&h.app_kernel), p), t.fg, false),
                ("  avg ", t.dim, false),
                (&opt(h.app_cpu.avg(), p), t.purple, false),
            ],
            right,
        );
        y += 1;
    }
    let f = cpu_factor(app);
    pen.segs(
        x,
        y,
        &[
            ("SYSTEM ", t.dim, false),
            (
                &opt(last(&h.sys_cpu), |v| format!("{:.1}%", v * f)),
                t.blue,
                true,
            ),
        ],
        right,
    );
    y += 1;
    if let Some(a) = app.session.fast.as_ref().and_then(|f| f.app.as_ref()) {
        pen.segs(
            x,
            y,
            &[
                ("threads ", t.dim, false),
                (&a.threads.to_string(), t.fg, false),
                ("  running ", t.dim, false),
                (&a.running.to_string(), t.green, false),
            ],
            right,
        );
        y += 1;
    }
    y += 1;
    let left = stats.bottom().saturating_sub(y);
    let used = cluster_rows(pen, app, x, y, stats.width, left.saturating_sub(1));
    y += used;
    if let Some(note) = throttle_note(app) {
        if y < stats.bottom() {
            pen.text(x, y, &note, t.orange, false, right);
        }
    }
}

fn tile_memory(pen: &mut Pen, app: &App, area: Rect, win: &Win) {
    let t = pen.t;
    let s = &app.session;
    let app_mode =
        app.package().is_some() && s.app_state == AppState::Running && s.app_mem.is_some();
    let inner = pen.panel(
        area,
        &[("Memory", t.yellow, true)],
        &[
            (
                if !app_mode {
                    "device RAM "
                } else if app.config.memory_chart == MemChart::Full {
                    "PSS, MB "
                } else {
                    "app-owned PSS, MB "
                },
                t.dim,
                false,
            ),
            ("[3]", t.dim, false),
        ],
        None,
    );
    if inner.height < 3 {
        return;
    }
    let (chart, stats) = side(inner, 36, 26, 40);
    if app_mode {
        let (x, mut y, right) = (stats.x, stats.y, stats.right());
        let layers = app_layers(app, win.n);
        stacked_chart(pen, app, chart, &layers, win, 10.0, "");
        let m = s.app_mem.as_ref().unwrap();
        pen.segs(
            x,
            y,
            &[
                ("PSS   ", t.dim, false),
                (&format!("{:.1} MB", m.pss_kb as f32 / 1024.0), t.aqua, true),
            ],
            right,
        );
        y += 1;
        pen.segs(
            x,
            y,
            &[
                ("RSS   ", t.dim, false),
                (&format!("{:.0}", m.rss_kb as f32 / 1024.0), t.fg, false),
                ("  swap ", t.dim, false),
                (
                    &format!("{:.0}", m.swap_pss_kb as f32 / 1024.0),
                    t.fg,
                    false,
                ),
            ],
            right,
        );
        y += 2;
        for (name, kb, first, color) in categories(app) {
            if y >= stats.bottom().saturating_sub(2) {
                break;
            }
            let d = (kb as f32 - first as f32) / 1024.0;
            pen.segs(
                x,
                y,
                &[
                    (swatch(app, name), color, false),
                    (&pad(name, 9), t.fg, false),
                    (&rpad(&format!("{:.1}", kb as f32 / 1024.0), 6), t.fg, false),
                    (
                        &rpad(&signed1(d), 6),
                        if d.abs() < 0.5 { t.dim } else { t.orange },
                        false,
                    ),
                ],
                right,
            );
            y += 1;
        }
        if let Some(g) = s.history.pss_mb.slope_per_min(s.interval_s()) {
            if y < stats.bottom() {
                pen.segs(
                    x,
                    y,
                    &[
                        ("trend ", t.dim, false),
                        (
                            &format!("{g:+.2} MB/min"),
                            if g > 0.5 { t.orange } else { t.fg },
                            g > 0.5,
                        ),
                    ],
                    right,
                );
                y += 1;
            }
        }
        if let (Some(o), Some(f)) = (m.objects, s.app_mem_first.as_ref().and_then(|f| f.objects)) {
            if y < stats.bottom() {
                let d = o.app_contexts as i64 - f.app_contexts as i64;
                pen.segs(
                    x,
                    y,
                    &[
                        ("Activities ", t.dim, false),
                        (&o.activities.to_string(), t.fg, false),
                        ("  Ctx ", t.dim, false),
                        (&o.app_contexts.to_string(), t.fg, false),
                        (
                            &if d > 0 {
                                format!(" +{d}⚠")
                            } else {
                                String::new()
                            },
                            t.orange,
                            false,
                        ),
                    ],
                    right,
                );
            }
        }
    } else {
        device_ram_chart(pen, app, chart, win);
        device_ram_stats(pen, app, stats);
    }
}

fn app_layers(app: &App, n: usize) -> Vec<(Vec<f32>, Color, &'static str)> {
    let t = app.t();
    let h = &app.session.history;
    let mut layers = Vec::with_capacity(7);
    if app.config.memory_chart == MemChart::Full {
        layers.push((tail(&h.system_mb, n), t.dim2, "System"));
        layers.push((tail(&h.code_mb, n), t.gray, "Code"));
    }
    layers.extend([
        (tail(&h.other_mb, n), t.blue, "Other"),
        (tail(&h.stack_mb, n), t.yellow, "Stack"),
        (tail(&h.graphics_mb, n), t.purple, "Graphics"),
        (tail(&h.native_mb, n), t.orange, "Native"),
        (tail(&h.java_mb, n), t.green, "Java"),
    ]);
    layers
}

fn swatch(app: &App, name: &str) -> &'static str {
    let shared = name == "Code" || name == "System";
    if shared && app.config.memory_chart != MemChart::Full {
        "□ "
    } else {
        "■ "
    }
}

fn categories(app: &App) -> Vec<(&'static str, u64, u64, Color)> {
    let t = app.t();
    let s = &app.session;
    let Some(m) = &s.app_mem else {
        return Vec::new();
    };
    let f = s.app_mem_first.as_ref().unwrap_or(m);
    vec![
        ("Java", m.java_kb, f.java_kb, t.green),
        ("Native", m.native_kb, f.native_kb, t.orange),
        ("Graphics", m.graphics_kb, f.graphics_kb, t.purple),
        ("Stack", m.stack_kb, f.stack_kb, t.yellow),
        ("Other", m.other_kb, f.other_kb, t.blue),
        ("Code", m.code_kb, f.code_kb, t.gray),
        ("System", m.system_kb, f.system_kb, t.dim2),
    ]
}

fn stacked_chart(
    pen: &mut Pen,
    app: &App,
    area: Rect,
    layers: &[(Vec<f32>, Color, &'static str)],
    win: &Win,
    floor: f32,
    unit: &str,
) {
    let t = pen.t;
    let n = layers.first().map_or(0, |l| l.0.len());
    let peak = (0..n)
        .map(|i| {
            layers
                .iter()
                .map(|l| l.0[i])
                .filter(|v| v.is_finite())
                .sum::<f32>()
        })
        .fold(0.0, f32::max);
    let max = nice_max(peak.max(floor));
    let label_w = 5;
    if area.width <= label_w + 2 || area.height < 3 {
        return;
    }
    let plot = Rect::new(
        area.x + label_w,
        area.y,
        area.width - label_w,
        area.height - 1,
    );
    for r in 0..plot.height {
        pen.put(plot.x - 1, plot.y + r, "│", t.dim2);
    }
    for (v, r) in [
        (max, 0),
        (max / 2.0, (plot.height - 1) / 2),
        (0.0, plot.height - 1),
    ] {
        let label = if max < 10.0 && v.fract().abs() > 1e-3 {
            format!("{v:.1}{unit}")
        } else {
            format!("{v:.0}{unit}")
        };
        pen.text(
            plot.x.saturating_sub(1 + width(&label)),
            plot.y + r,
            &label,
            t.dim,
            false,
            plot.x,
        );
        pen.put(plot.x - 1, plot.y + r, "┤", t.dim2);
    }
    let refs: Vec<(&[f32], Color)> = layers.iter().map(|(d, c, _)| (d.as_slice(), *c)).collect();
    pen.stacked(plot, &refs, max);
    for (idx, label) in &win.marks {
        if n < 2 {
            break;
        }
        let col =
            plot.x + ((*idx as f32 / (n - 1) as f32) * (plot.width - 1) as f32).round() as u16;
        let (start, text) = if col + width(label) + 1 > plot.right() {
            (col.saturating_sub(width(label)), format!("{label}▾"))
        } else {
            (col, format!("▾{label}"))
        };
        let end = (start + width(&text)).min(plot.right());
        for x in start..end {
            pen.bg(x, plot.y, t.bg);
        }
        pen.text(start, plot.y, &text, t.yellow, true, plot.right());
    }
    let y = plot.bottom();
    pen.text(plot.x, y, &win.xl[0], t.dim, false, plot.right());
    pen.text(
        plot.right().saturating_sub(3),
        y,
        "now",
        t.dim,
        false,
        plot.right(),
    );
    let _ = app;
}

fn device_ram_body(pen: &mut Pen, app: &App, inner: Rect, win: &Win) {
    let bar_h = 4.min(inner.height);
    ram_bar(pen, app, Rect::new(inner.x, inner.y, inner.width, bar_h));
    if inner.height > bar_h + 3 {
        let rest = Rect::new(inner.x, inner.y + bar_h, inner.width, inner.height - bar_h);
        let (chart, stats) = side(rest, 30, 30, 44);
        device_ram_chart(pen, app, chart, win);
        device_ram_stats(pen, app, stats);
    }
}

fn process_memory_table(pen: &mut Pen, app: &App, area: Rect) {
    let t = pen.t;
    if area.height < 2 {
        return;
    }
    let s = &app.session;
    let Some(fast) = s.fast.as_ref() else {
        return;
    };
    let page_kb = s
        .caps
        .as_ref()
        .map(|c| c.page_size as u64 / 1024)
        .filter(|kb| *kb > 0)
        .unwrap_or(4);
    let total_kb = fast.mem.map_or(0, |m| m.total_kb).max(1);
    let filter = app.table_filter.to_lowercase();
    let mut rows: Vec<(&TaskSample, &str)> = fast
        .tasks
        .iter()
        .filter(|p| p.rss_pages > 0)
        .map(|p| (p, s.task_display_name(p)))
        .filter(|(p, name)| {
            filter.is_empty()
                || name.to_lowercase().contains(&filter)
                || p.id.to_string().contains(&filter)
        })
        .collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.0.rss_pages));
    let top_kb = rows.first().map_or(1, |r| r.0.rss_pages * page_kb).max(1);
    let visible = (area.height - 1) as usize;
    let offset = app.scroll[app.page].min(rows.len().saturating_sub(visible));
    let w = area.width as usize;
    let fixed = 1 + 7 + 1 + 9 + 7 + 2 + 7 + 2;
    let name_w = rows
        .iter()
        .skip(offset)
        .take(visible)
        .map(|r| width(r.1) as usize)
        .max()
        .unwrap_or(0)
        .clamp(15, 48)
        .min(w.saturating_sub(fixed + 10).max(12));
    let bar_w = w.saturating_sub(fixed + name_w).max(4) as u16;
    let header = format!(
        "{} {} {} {}  {}  {}",
        pad("PID", 6),
        pad("NAME", name_w),
        rpad("RSS", 9),
        rpad("% RAM", 6),
        pad("", bar_w as usize),
        rpad("CPU%", 5),
    );
    pen.text(area.x + 1, area.y, &header, t.dim, false, area.right());
    for (i, (p, name)) in rows.iter().skip(offset).take(visible).enumerate() {
        let y = area.y + 1 + i as u16;
        let kb = p.rss_pages * page_kb;
        let share = kb as f32 * 100.0 / total_kb as f32;
        let mb = kb as f32 / 1024.0;
        let size = if mb >= 1024.0 {
            format!("{:.2} GB", mb / 1024.0)
        } else {
            format!("{mb:.0} MB")
        };
        let color = if share >= 5.0 {
            t.orange
        } else if share >= 1.0 {
            t.yellow
        } else {
            t.fg
        };
        let mut x = area.x + 1;
        x = pen.text(x, y, &pad(&p.id.to_string(), 7), t.dim, false, area.right());
        x = pen.text(x, y, &pad(name, name_w), t.fg, false, area.right()) + 1;
        x = pen.text(x, y, &rpad(&size, 9), color, true, area.right()) + 1;
        x = pen.text(
            x,
            y,
            &rpad(&format!("{share:.1}%"), 6),
            t.dim,
            false,
            area.right(),
        ) + 2;
        pen.hbar(x, y, bar_w, kb as f32, top_kb as f32, color);
        x += bar_w + 2;
        pen.text(
            x,
            y,
            &rpad(&format!("{:.1}", p.cpu), 5),
            if p.cpu > 5.0 { t.orange } else { t.dim },
            false,
            area.right(),
        );
    }
}

fn device_ram_chart(pen: &mut Pen, app: &App, area: Rect, win: &Win) {
    let t = pen.t;
    let h = &app.session.history;
    let layers = [
        (tail(&h.mem_used_gb, win.n), t.orange, "used"),
        (tail(&h.mem_cached_gb, win.n), t.blue, "cached"),
        (tail(&h.mem_free_gb, win.n), t.green, "free"),
    ];
    stacked_chart(pen, app, area, &layers, win, 1.0, "G");
}

fn device_ram_stats(pen: &mut Pen, app: &App, stats: Rect) {
    let t = pen.t;
    let (x, mut y, right) = (stats.x, stats.y, stats.right());
    let Some(m) = app.session.fast.as_ref().and_then(|f| f.mem) else {
        return;
    };
    let gb = |kb: u64| format!("{:.2} GB", kb as f32 / 1048576.0);
    pen.segs(
        x,
        y,
        &[("total     ", t.dim, false), (&gb(m.total_kb), t.fg, false)],
        right,
    );
    y += 1;
    pen.segs(
        x,
        y,
        &[
            ("available ", t.dim, false),
            (&gb(m.available_kb), t.green, true),
        ],
        right,
    );
    y += 1;
    pen.segs(
        x,
        y,
        &[
            ("used      ", t.dim, false),
            (
                &format!(
                    "{} ({:.0}%)",
                    gb(m.used_kb()),
                    m.used_kb() as f32 * 100.0 / m.total_kb.max(1) as f32
                ),
                t.orange,
                false,
            ),
        ],
        right,
    );
    y += 1;
    pen.segs(
        x,
        y,
        &[
            ("cached    ", t.dim, false),
            (&gb(m.cached_kb), t.blue, false),
        ],
        right,
    );
    y += 1;
    pen.segs(
        x,
        y,
        &[
            ("swap used ", t.dim, false),
            (
                &format!("{} / {}", gb(m.swap_used_kb()), gb(m.swap_total_kb)),
                t.purple,
                false,
            ),
        ],
        right,
    );
    y += 1;
    if let Some(p) = app.session.fast.as_ref().and_then(|f| f.psi) {
        pen.segs(
            x,
            y,
            &[
                ("pressure  ", t.dim, false),
                (
                    &format!("{:.2}% some", p.some_avg10),
                    if p.some_avg10 > 10.0 { t.orange } else { t.fg },
                    false,
                ),
            ],
            right,
        );
    }
}

fn frame_color(t: &Theme, ms: f32, budget: f32) -> Color {
    if ms <= budget * 1.01 {
        t.green
    } else if ms <= budget * 2.0 {
        t.yellow
    } else {
        t.red
    }
}

fn frame_bars(pen: &mut Pen, app: &App, area: Rect, label_w: u16) {
    let t = pen.t;
    if area.width <= label_w + 2 || area.height < 3 {
        return;
    }
    let plot = Rect::new(
        area.x + label_w,
        area.y,
        area.width - label_w,
        area.height - 1,
    );
    let budget = app.session.frames.back().map_or(16.67, |f| f.budget_ms);
    let n = plot.width as usize;
    let frames: Vec<f32> = app
        .session
        .frames
        .iter()
        .rev()
        .take(n)
        .rev()
        .map(|f| f.ms)
        .collect();
    let mut sorted: Vec<f32> = frames.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p98 = sorted
        .get(sorted.len().saturating_sub(1) * 98 / 100)
        .copied()
        .unwrap_or(budget);
    let max = nice_max(p98.max(budget * 3.0)).min(100.0);
    let row_of = |v: f32| -> u16 {
        (((1.0 - v / max) * plot.height as f32).floor().max(0.0) as u16).min(plot.height - 1)
    };
    for r in 0..plot.height {
        pen.put(plot.x - 1, plot.y + r, "│", t.dim2);
    }
    pen.vbars(
        plot,
        &frames,
        max,
        &|ms| frame_color(t, ms, budget),
        Some(t.red),
    );
    for (v, c) in [
        (budget, mix(t.green, t.bg, 0.5)),
        (budget * 2.0, mix(t.yellow, t.bg, 0.5)),
    ] {
        let r = row_of(v);
        for i in 0..plot.width {
            if let Some(cell) = pen.cell(plot.x + i, plot.y + r) {
                if cell.symbol() == " " {
                    cell.set_symbol("┄").set_fg(c);
                }
            }
        }
    }
    for (v, label) in [
        (max, format!("{max:.0}ms")),
        (budget * 2.0, format!("{:.1}", budget * 2.0)),
        (budget, format!("{budget:.1}")),
    ] {
        let r = row_of(v);
        pen.text(
            plot.x.saturating_sub(1 + width(&label)),
            plot.y + r,
            &label,
            t.dim,
            false,
            plot.x,
        );
        pen.put(plot.x - 1, plot.y + r, "┤", t.dim2);
    }
    let y = plot.bottom();
    pen.text(
        plot.x,
        y,
        &format!("last {} frames", frames.len()),
        t.dim,
        false,
        plot.right(),
    );
    pen.text(
        plot.right().saturating_sub(6),
        y,
        "newest",
        t.dim,
        false,
        plot.right(),
    );
}

fn mix(a: Color, b: Color, t: f32) -> Color {
    crate::theme::mix(a, b, t)
}

fn signed1(d: f32) -> String {
    if d.abs() < 0.05 {
        "·".to_string()
    } else {
        format!("{d:+.1}")
    }
}

fn frame_buckets(budget: f32) -> Vec<(f32, f32, String)> {
    let mut cuts: Vec<f32> = Vec::with_capacity(4);
    for c in [budget, budget * 2.0, (budget * 3.0).max(50.0), 100.0] {
        if c <= 100.0 && cuts.last().map_or(true, |l| c > *l + 0.5) {
            cuts.push(c);
        }
    }
    let mut edges = Vec::with_capacity(cuts.len() + 1);
    let mut lo = 0.0;
    for (i, &hi) in cuts.iter().enumerate() {
        let label = if i == 0 {
            format!("≤ {hi:.1} ms")
        } else {
            format!("{lo:.0}–{hi:.0}")
        };
        edges.push((lo, hi, label));
        lo = hi;
    }
    edges.push((lo, f32::INFINITY, format!("> {lo:.0}")));
    edges
}

fn frames_gate(app: &App) -> Option<String> {
    if let Some(u) = unsupported(app, Source::Frames) {
        return Some(u);
    }
    if !app.config.frames {
        return Some("Frame stats are off in settings (,)".into());
    }
    if let Some(g) = app_gate(app) {
        return Some(g.into());
    }
    if app.session.frame_summary.is_none() {
        return Some(
            "Waiting for frame stats… (apps that draw only to a SurfaceView show nothing here)"
                .into(),
        );
    }
    None
}

fn tile_frames(pen: &mut Pen, app: &App, area: Rect, win: &Win) {
    let t = pen.t;
    let inner = pen.panel(
        area,
        &[("Frames", t.yellow, true)],
        &[
            ("■", t.green, false),
            (" ok ", t.dim, false),
            ("■", t.yellow, false),
            (" slow ", t.dim, false),
            ("■", t.red, false),
            (" jank ", t.dim, false),
            ("[4]", t.dim, false),
        ],
        None,
    );
    if inner.height < 3 {
        return;
    }
    if let Some(msg) = frames_gate(app) {
        pen.note(inner, &msg, t.dim);
        return;
    }
    let (chart, stats) = side(inner, 36, 26, 40);
    frame_bars(pen, app, chart, 6);
    let s = &app.session;
    let h = &s.history;
    let (x, mut y, right) = (stats.x, stats.y, stats.right());
    let hz = s.frames.back().map(|f| 1000.0 / f.budget_ms);
    pen.segs(
        x,
        y,
        &[
            ("FPS    ", t.dim, false),
            (&opt(last(&h.fps), |v| format!("{v:.0}")), t.green, true),
            (&opt(hz, |v| format!("  @{v:.0} Hz")), t.dim, false),
        ],
        right,
    );
    y += 1;
    if let Some(f) = s.frames_since_start() {
        let jp = if f.total > 0 {
            f.janky as f32 * 100.0 / f.total as f32
        } else {
            0.0
        };
        pen.segs(
            x,
            y,
            &[
                ("jank   ", t.dim, false),
                (
                    &format!("{jp:.1}%"),
                    if jp > 5.0 { t.red } else { t.yellow },
                    true,
                ),
                (&format!("  {}/{}", f.janky, f.total), t.dim, false),
            ],
            right,
        );
        y += 2;
        pen.segs(
            x,
            y,
            &[
                ("p50 ", t.dim, false),
                (&format!("{:>3}", f.p50_ms), t.fg, false),
                ("  p90 ", t.dim, false),
                (&format!("{:>3}", f.p90_ms), t.fg, false),
            ],
            right,
        );
        y += 1;
        pen.segs(
            x,
            y,
            &[
                ("p95 ", t.dim, false),
                (&format!("{:>3}", f.p95_ms), t.yellow, false),
                ("  p99 ", t.dim, false),
                (&format!("{:>3}", f.p99_ms), t.red, false),
                (" ms", t.dim, false),
            ],
            right,
        );
        y += 2;
        for (label, v) in [
            ("slow UI thread", f.slow_ui_thread),
            ("deadline missed", f.deadline_missed),
            ("input latency", f.high_input_latency),
        ] {
            if y + 2 >= stats.bottom() {
                break;
            }
            pen.segs(
                x,
                y,
                &[
                    (&pad(label, 16), t.dim, false),
                    (&rpad(&v.to_string(), 6), t.fg, false),
                ],
                right,
            );
            y += 1;
        }
    }
    if stats.height >= 4 {
        let fps = tail(&h.fps, win.n);
        let by = stats.bottom() - 1;
        pen.text(x, by - 1, "fps", t.dim, false, right);
        let fps_max = hz.unwrap_or(60.0).max(1.0);
        pen.spark(
            x + 5,
            by - 1,
            stats.width.saturating_sub(5),
            &fps,
            fps_max,
            &|_| t.green,
        );
        let jank = tail(&h.jank_pct, win.n);
        let jank_max = jank
            .iter()
            .copied()
            .filter(|v| v.is_finite())
            .fold(25.0, f32::max);
        pen.text(x, by, "jank", t.dim, false, right);
        pen.spark(
            x + 5,
            by,
            stats.width.saturating_sub(5),
            &jank,
            jank_max,
            &|_| t.red,
        );
    }
}

fn net_data(app: &App, n: usize) -> (Vec<f32>, Vec<f32>, &'static str) {
    let h = &app.session.history;
    let newest_capture = h.cap_rx.len() > 0 && h.cap_rx.get(h.cap_rx.len() - 1).is_finite();
    if app.session.capture.snapshot.is_some() || newest_capture {
        (
            tail(&h.cap_rx, n),
            tail(&h.cap_tx, n),
            if app.package().is_some() {
                "app (capture)"
            } else {
                "all apps (capture)"
            },
        )
    } else {
        (tail(&h.dev_rx, n), tail(&h.dev_tx, n), "device")
    }
}

fn net_chart(pen: &mut Pen, app: &App, area: Rect, win: &Win, label_w: u16) {
    let t = pen.t;
    let (rx, tx, _) = net_data(app, win.n);
    let peak = rx
        .iter()
        .chain(tx.iter())
        .copied()
        .filter(|v| v.is_finite())
        .fold(0.0, f32::max);
    let sqrt = app.config.net_scale == NetScale::Sqrt;
    let fwd = |v: f32| if sqrt { v.max(0.0).sqrt() } else { v.max(0.0) };
    let back = |v: f32| if sqrt { v * v } else { v };
    let top = nice_max(peak.max(if sqrt { 10_000.0 } else { 1_000.0 }));
    let sm = fwd(top);
    let rxs: Vec<f32> = rx.iter().map(|v| fwd(*v)).collect();
    let txs: Vec<f32> = tx.iter().map(|v| -fwd(*v)).collect();
    let marks = win.marks();
    let mid = sm / 2.0;
    let ticks: Vec<f32> = vec![0.0, sm, -sm, mid, -mid];
    pen.chart(
        area,
        &[
            ChartLine {
                data: &rxs,
                color: t.aqua,
                fill: true,
            },
            ChartLine {
                data: &txs,
                color: t.purple,
                fill: true,
            },
        ],
        &ChartOpts {
            ymin: -sm,
            ymax: sm,
            ticks: &ticks,
            fmt: &|v| {
                if v.abs() < 1e-3 {
                    "0".into()
                } else {
                    format!(
                        "{}{}",
                        if v > 0.0 { "↓" } else { "↑" },
                        fmt_bytes(back(v.abs()) as f64)
                    )
                }
            },
            label_w,
            xlabels: win.xl(),
            markers: &marks,
            hlines: &[],
        },
    );
}

fn tile_network(pen: &mut Pen, app: &App, area: Rect, win: &Win) {
    let t = pen.t;
    let s = &app.session;
    let (_, _, source) = net_data(app, 2);
    let inner = pen.panel(
        area,
        &[("Network", t.yellow, true)],
        &[
            (
                source,
                if source == "device" { t.dim } else { t.green },
                false,
            ),
            (" [5]", t.dim, false),
        ],
        None,
    );
    if inner.height < 3 {
        return;
    }
    let (chart, stats) = side(inner, 36, 26, 40);
    net_chart(pen, app, chart, win, 7);
    let (rx, tx, _) = net_data(app, 1);
    let (x, mut y, right) = (stats.x, stats.y, stats.right());
    let cap = s.capture.snapshot.is_some();
    let totals = cap.then(|| s.capture.traffic(s.app_uids().as_deref()));
    pen.segs(
        x,
        y,
        &[
            ("↓ ", t.aqua, false),
            (&format!("{}/s", fmt_bytes(rx[0] as f64)), t.aqua, true),
            (
                &totals.map_or(String::new(), |tt| format!("  {}", fmt_bytes(tt.0 as f64))),
                t.dim,
                false,
            ),
        ],
        right,
    );
    y += 1;
    pen.segs(
        x,
        y,
        &[
            ("↑ ", t.purple, false),
            (&format!("{}/s", fmt_bytes(tx[0] as f64)), t.purple, true),
            (
                &totals.map_or(String::new(), |tt| format!("  {}", fmt_bytes(tt.1 as f64))),
                t.dim,
                false,
            ),
        ],
        right,
    );
    y += 1;
    if cap {
        let conns = s.connections();
        let open = conns.iter().filter(|c| is_open(c)).count();
        let failed = conns
            .iter()
            .filter(|c| c.state == ConnectionState::Failed as i32)
            .count();
        pen.segs(
            x,
            y,
            &[
                ("open ", t.dim, false),
                (&open.to_string(), t.green, false),
                ("  total ", t.dim, false),
                (&conns.len().to_string(), t.fg, false),
                ("  fail ", t.dim, false),
                (
                    &failed.to_string(),
                    if failed > 0 { t.red } else { t.fg },
                    false,
                ),
            ],
            right,
        );
        y += 2;
        pen.text(x, y, "TOP HOSTS", t.dim, false, right);
        y += 1;
        for h in s.hosts() {
            if y >= stats.bottom() {
                break;
            }
            let nw = stats.width.saturating_sub(8) as usize;
            pen.segs(
                x,
                y,
                &[
                    (&pad(&h.host, nw), t.fg, false),
                    (&rpad(&fmt_bytes((h.rx + h.tx) as f64), 7), t.aqua, false),
                ],
                right,
            );
            y += 1;
        }
    } else {
        y += 1;
        let msg = match s.support(Source::Capture) {
            crate::model::Support::Unsupported(_) => {
                "per-app traffic needs the capture agent (android-agent/)"
            }
            _ => "press c to capture per-app traffic, hosts and DNS",
        };
        let lines = wrap(msg, stats.width as usize);
        for l in lines {
            if y >= stats.bottom() {
                break;
            }
            pen.text(x, y, &l, t.dim, false, right);
            y += 1;
        }
    }
}

fn wrap(msg: &str, w: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for word in msg.split(' ') {
        if !cur.is_empty() && cur.len() + word.len() + 1 > w {
            out.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(word);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn task_time(ticks: u64) -> String {
    let secs = ticks / 100;
    format!("{}:{:02}.{:02}", secs / 60, secs % 60, ticks % 100)
}

fn pool_name(name: &str) -> String {
    if let Some((prefix, _)) = name.split_once(':') {
        if prefix.to_ascii_lowercase().ends_with("binder") {
            return prefix.to_string();
        }
    }
    let mut out = String::with_capacity(name.len());
    let mut prev: Option<char> = None;
    let mut run: Option<bool> = None;
    for c in name.chars() {
        if c.is_ascii_digit() {
            let keep = *run.get_or_insert(prev.is_some_and(|p| p.is_ascii_alphabetic()));
            if keep {
                out.push(c);
            } else if !prev.is_some_and(|p| p.is_ascii_digit()) {
                out.push('*');
            }
        } else {
            out.push(c);
            run = None;
        }
        prev = Some(c);
    }
    out
}

fn process_group(p: &TaskSample, name: &str) -> &'static str {
    if p.rss_pages == 0 {
        "kernel threads"
    } else if name == "system_server" {
        "system_server"
    } else if name.starts_with("android.hardware.") || name.starts_with("vendor.") {
        "HAL services"
    } else if name.contains('.') {
        "apps"
    } else {
        "native daemons"
    }
}

struct TaskRow<'a> {
    task: &'a TaskSample,
    hist: Option<&'a TaskHist>,
    name: &'a str,
    main: bool,
}

fn task_rows<'a>(app: &'a App) -> Vec<TaskRow<'a>> {
    let s = &app.session;
    let main = s.fast.as_ref().and_then(|f| f.app.as_ref()).map(|a| a.pid);
    let f = app.table_filter.to_lowercase();
    let mut rows: Vec<TaskRow> = s
        .tasks_sorted()
        .into_iter()
        .map(|(task, hist)| TaskRow {
            task,
            hist,
            name: s.task_display_name(task),
            main: Some(task.id) == main,
        })
        .filter(|r| {
            f.is_empty() || r.name.to_lowercase().contains(&f) || r.task.id.to_string().contains(&f)
        })
        .filter(|r| {
            !app.config.hide_idle_threads
                || r.main
                || r.task.cpu > 0.0
                || r.hist.is_some_and(|h| h.peak > 0.0)
        })
        .collect();
    if app.sort == Sort::Name {
        rows.sort_by(|a, b| {
            b.main
                .cmp(&a.main)
                .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
    }
    rows
}

fn task_table(pen: &mut Pen, app: &App, area: Rect, full: bool) {
    let t = pen.t;
    if area.height < 2 {
        return;
    }
    let device = app.package().is_none();
    let rows = task_rows(app);
    let w = area.width as usize;
    let fixed = 1 + 7 + 1 + 3 + 7 + if full { 13 } else { 0 } + 2 + 5 + if full { 10 } else { 0 };
    let min_spark = if full { 16 } else { 10 };
    let visible = (area.height - 1) as usize;
    let offset = app.scroll[app.page].min(rows.len().saturating_sub(visible));
    let longest = rows
        .iter()
        .skip(offset)
        .take(visible)
        .map(|r| width(r.name) as usize)
        .max()
        .unwrap_or(0);
    let room = w.saturating_sub(fixed + min_spark).max(12);
    let name_w = longest.clamp(15, 40).min(room);
    let spark_w = w.saturating_sub(fixed + name_w).max(4) as u16;
    let id = if device { "PID" } else { "TID" };
    let mut header = format!(
        "{} {} S  {}  ",
        pad(id, 6),
        pad("NAME", name_w),
        rpad("CPU%", 5)
    );
    if full {
        header.push_str(&format!("{} {}  ", rpad("AVG", 5), rpad("PEAK", 5)));
    }
    let span = task_span(app);
    header.push_str(&pad(
        &format!("LAST {}", super::span_label(span_secs(app, span))),
        spark_w as usize + 1,
    ));
    header.push_str(" CORE");
    if full {
        header.push_str("  TIME");
    }
    pen.text(area.x + 1, area.y, &header, t.dim, false, area.right());
    for (i, r) in rows.iter().skip(offset).take(visible).enumerate() {
        let y = area.y + 1 + i as u16;
        let cpu = r.task.cpu;
        let cpu_c = if cpu > 20.0 {
            t.red
        } else if cpu > 5.0 {
            t.orange
        } else if cpu > 0.5 {
            t.green
        } else {
            t.dim
        };
        let mut x = area.x + 1;
        x = pen.text(
            x,
            y,
            &pad(&r.task.id.to_string(), 7),
            t.dim,
            false,
            area.right(),
        );
        x = pen.text(
            x,
            y,
            &pad(r.name, name_w),
            if r.main { t.yellow } else { t.fg },
            r.main,
            area.right(),
        ) + 1;
        let state_c = match r.task.state {
            'R' => t.green,
            'D' => t.red,
            _ => t.dim,
        };
        x = pen.text(
            x,
            y,
            &format!("{}  ", r.task.state),
            state_c,
            r.task.state == 'R',
            area.right(),
        );
        x = pen.text(
            x,
            y,
            &format!("{}  ", rpad(&format!("{cpu:.1}"), 5)),
            cpu_c,
            false,
            area.right(),
        );
        if full {
            x = pen.text(
                x,
                y,
                &format!(
                    "{} ",
                    rpad(&format!("{:.1}", r.hist.map_or(0.0, |h| h.avg())), 5)
                ),
                t.dim,
                false,
                area.right(),
            );
            x = pen.text(
                x,
                y,
                &format!(
                    "{}  ",
                    rpad(&format!("{:.1}", r.hist.map_or(0.0, |h| h.peak)), 5)
                ),
                t.purple,
                false,
                area.right(),
            );
        }
        if let Some(h) = r.hist {
            let skip = h.hist.len().saturating_sub(span);
            let data: Vec<f32> = h.hist.iter().skip(skip).copied().collect();
            let padded: Vec<f32> = std::iter::repeat(f32::NAN)
                .take(span.saturating_sub(data.len()))
                .chain(data)
                .collect();
            pen.spark(x, y, spark_w, &padded, 25.0, &|v| {
                if v > 5.0 {
                    t.orange
                } else {
                    t.green
                }
            });
        }
        x += spark_w + 2;
        x = pen.text(
            x,
            y,
            &pad(&format!("c{}", r.task.core), 5),
            t.purple,
            false,
            area.right(),
        );
        if full {
            pen.text(x, y, &task_time(r.task.ticks), t.dim, false, area.right());
        }
    }
    if rows.is_empty() {
        let msg = if app.session.app_state == AppState::NotRunning {
            "The app is not running"
        } else {
            "Waiting for samples…"
        };
        pen.note(
            Rect::new(area.x, area.y + 1, area.width, area.height - 1),
            msg,
            t.dim,
        );
    }
}

fn task_span(app: &App) -> usize {
    span_samples(app, crate::session::TASK_HISTORY)
}

fn tile_tasks(pen: &mut Pen, app: &App, area: Rect) {
    let t = pen.t;
    let device = app.package().is_none();
    let count = app.session.fast.as_ref().map_or(0, |f| f.tasks.len());
    let right = format!("{count} · by CPU ");
    let inner = pen.panel(
        area,
        &[(if device { "Processes" } else { "Threads" }, t.yellow, true)],
        &[(&right, t.dim, false), ("[2]", t.dim, false)],
        None,
    );
    task_table(pen, app, inner, false);
}

fn power_unplugged_note(app: &App) -> Option<String> {
    let b = app.session.battery.as_ref()?;
    if app.tweaks.unplugged || app.tweaks.doze {
        return Some("The battery only reports unplugged (fake unplug or forced Doze), so this current is not the app's drain".into());
    }
    if b.plugged != "none" {
        return Some(format!("Plugged in ({}): the phone charges, so drain is not the app's. Measure over Wi-Fi adb: adb tcpip 5555", b.plugged));
    }
    if b.current_ma.is_none() {
        return Some("This device does not report battery current".into());
    }
    None
}

fn tile_power(pen: &mut Pen, app: &App, area: Rect, win: &Win) {
    let t = pen.t;
    let s = &app.session;
    let inner = pen.panel(
        area,
        &[("Power & Thermal", t.yellow, true)],
        &[("[6]", t.dim, false)],
        None,
    );
    if inner.height < 3 {
        return;
    }
    if !app.config.power {
        pen.note(inner, "Battery & thermal are off in settings (,)", t.dim);
        return;
    }
    let temps_h = (inner.height / 3).clamp(2, 4);
    let top = Rect::new(inner.x, inner.y, inner.width, inner.height - temps_h);
    let bottom = Rect::new(inner.x, inner.bottom() - temps_h, inner.width, temps_h);
    let (chart, stats) = side(top, 40, 26, 44);
    drain_chart(pen, app, chart, win);
    let (x, mut y, right) = (stats.x, stats.y, stats.right());
    if let Some(b) = &s.battery {
        pen.segs(
            x,
            y,
            &[
                ("BAT ", t.dim, false),
                (
                    &format!("{}%", b.level),
                    if b.level > 20 { t.green } else { t.red },
                    true,
                ),
                (&format!("  {}", b.status), t.dim, false),
            ],
            right,
        );
        y += 1;
        match b.drain_ma() {
            Some(ma) => pen.segs(
                x,
                y,
                &[
                    ("drain ", t.dim, false),
                    (&format!("{ma:.0} mA"), t.yellow, true),
                    (
                        &format!("  {:.2} W", ma * b.voltage_mv as f32 / 1e6),
                        t.fg,
                        false,
                    ),
                ],
                right,
            ),
            None => pen.segs(
                x,
                y,
                &[
                    ("current ", t.dim, false),
                    (
                        &b.current_ma.map_or("–".into(), |c| format!("{c} mA")),
                        t.fg,
                        false,
                    ),
                    (
                        &format!("  {:.2} V", b.voltage_mv as f32 / 1000.0),
                        t.dim,
                        false,
                    ),
                ],
                right,
            ),
        };
        y += 1;
        if let Some(mah) = s.used_mah() {
            pen.segs(
                x,
                y,
                &[
                    ("session ", t.dim, false),
                    (
                        &fmt_mah(mah),
                        if mah > 0.5 { t.orange } else { t.green },
                        false,
                    ),
                    (" counter", t.dim, false),
                ],
                right,
            );
            y += 1;
        }
    }
    y += 1;
    if let Some(th) = &s.thermal {
        if y < stats.bottom() {
            let c = if th.status == 0 {
                t.green
            } else if th.status == 1 {
                t.orange
            } else {
                t.red
            };
            pen.segs(
                x,
                y,
                &[
                    ("thermal ", t.dim, false),
                    (thermal_level_name(th.status), c, true),
                ],
                right,
            );
            y += 1;
        }
    }
    if let Some(d) = &s.doze {
        if y < stats.bottom() {
            pen.segs(
                x,
                y,
                &[
                    ("doze ", t.dim, false),
                    (
                        &d.deep,
                        if d.deep == "ACTIVE" {
                            t.green
                        } else {
                            t.yellow
                        },
                        false,
                    ),
                    (
                        &d.bucket
                            .as_ref()
                            .map_or(String::new(), |b| format!("  {b}")),
                        t.dim,
                        false,
                    ),
                ],
                right,
            );
        }
    }
    temp_bars(pen, app, bottom, 2);
}

fn drain_chart(pen: &mut Pen, app: &App, area: Rect, win: &Win) {
    let t = pen.t;
    let note = power_unplugged_note(app);
    let h = &app.session.history;
    let d = tail(&h.drain_ma, win.n);
    let c = tail(&h.charge_ma, win.n);
    if let Some(note) = &note {
        if !d.iter().chain(c.iter()).any(|v| v.is_finite()) {
            pen.note(area, note, t.dim);
            return;
        }
    }
    let peak = d
        .iter()
        .chain(c.iter())
        .copied()
        .filter(|v| v.is_finite())
        .fold(0.0, f32::max);
    let ymax = nice_max(peak.max(200.0));
    let marks = win.marks();
    let ticks = [ymax, ymax / 2.0, 0.0];
    if note.is_some() && area.height > 2 {
        let fake = app.tweaks.unplugged || app.tweaks.doze;
        let room = area.width.saturating_sub(8) as usize;
        let label = match (fake, room >= 64) {
            (true, true) => "reported current · the battery only pretends to be unplugged",
            (true, false) => "fake unplug, not the app's drain",
            (false, true) => "charging current, not the app's drain · measure over Wi-Fi adb",
            (false, false) => "charging, not the app's drain",
        };
        pen.text(area.x + 7, area.y, label, t.dim, false, area.right());
    }
    pen.chart(
        Rect::new(
            area.x,
            area.y + u16::from(note.is_some() && area.height > 2),
            area.width,
            area.height - u16::from(note.is_some() && area.height > 2),
        ),
        &[
            ChartLine {
                data: &d,
                color: t.yellow,
                fill: true,
            },
            ChartLine {
                data: &c,
                color: t.aqua,
                fill: true,
            },
        ],
        &ChartOpts {
            ymin: 0.0,
            ymax,
            ticks: &ticks,
            fmt: &|v| format!("{v:.0}"),
            label_w: 5,
            xlabels: win.xl(),
            markers: &marks,
            hlines: &[],
        },
    );
}

fn temp_bars(pen: &mut Pen, app: &App, area: Rect, columns: u16) {
    let t = pen.t;
    let Some(th) = &app.session.thermal else {
        return;
    };
    let col_w = area.width / columns.max(1);
    for (i, sensor) in th.sensors.iter().enumerate() {
        let col = i as u16 % columns;
        let row = i as u16 / columns;
        if row >= area.height {
            break;
        }
        let x = area.x + col * col_w + 1;
        let y = area.y + row;
        let c = t.temp(sensor.celsius);
        let shown = app.config.temperature.convert(sensor.celsius);
        let end = pen.segs(
            x,
            y,
            &[
                (&pad(&sensor.name, 6), t.dim, false),
                (&rpad(&format!("{shown:.1}°"), 6), c, false),
            ],
            x + col_w,
        );
        pen.hbar(
            end + 1,
            y,
            col_w.saturating_sub(end - x + 3),
            sensor.celsius - 20.0,
            40.0,
            c,
        );
    }
}

pub fn cpu(pen: &mut Pen, app: &App, area: Rect) {
    let t = pen.t;
    let win = Win::of(app);
    let r = rows(
        area,
        &[
            Constraint::Percentage(36),
            Constraint::Percentage(26),
            Constraint::Min(6),
        ],
    );
    let stats_w = (area.width * 30 / 100).clamp(40, 64);
    let top = cols(r[0], &[Constraint::Min(30), Constraint::Length(stats_w)]);
    let mid = cols(r[1], &[Constraint::Min(30), Constraint::Length(stats_w)]);
    let bot = cols(r[2], &[Constraint::Min(30), Constraint::Length(stats_w)]);
    let has_app = app.package().is_some();
    let legend: Vec<Seg> = if has_app {
        vec![
            ("━ app ", t.green, false),
            ("━ app kernel ", t.orange, false),
            ("━ system ", t.blue, false),
            ("┊ marker", t.yellow, false),
        ]
    } else {
        vec![("━ system ", t.blue, false), ("┊ marker", t.yellow, false)]
    };
    let inner = pen.panel(top[0], &[("CPU usage", t.yellow, true)], &legend, None);
    let side_cols = 13.min(inner.width.saturating_sub(20));
    let chart_rect = Rect::new(inner.x, inner.y, inner.width - side_cols, inner.height);
    cpu_chart(pen, app, chart_rect, &win, true);
    {
        let h = &app.session.history;
        let f = cpu_factor(app);
        let x = chart_rect.right() + 1;
        let right = inner.right();
        let rows: Vec<(&str, Option<f32>, Color)> = if app.package().is_some() {
            vec![
                ("app ", last(&h.app_cpu), t.green),
                ("kern", last(&h.app_kernel), t.orange),
                ("sys ", last(&h.sys_cpu), t.blue),
            ]
        } else {
            vec![("sys ", last(&h.sys_cpu), t.blue)]
        };
        for (i, (label, v, c)) in rows.iter().enumerate() {
            let y = inner.y + i as u16;
            if y >= inner.bottom() {
                break;
            }
            pen.segs(
                x,
                y,
                &[
                    (label, t.dim, false),
                    (&opt(*v, |v| format!(" {:.1}%", v * f)), *c, true),
                ],
                right,
            );
        }
    }
    cpu_now(pen, app, top[1]);
    let inner = pen.panel(
        mid[0],
        &[("Per-core load", t.yellow, true)],
        &[(
            &format!("last {} · colour = load", win.xl[0].trim_start_matches('-')),
            t.dim,
            false,
        )],
        None,
    );
    core_heat(pen, app, inner, &win);
    let legend: Vec<(String, Color)> = app
        .session
        .caps
        .as_ref()
        .map(|c| {
            c.clusters
                .iter()
                .map(|k| (format!("━ {} ", k.name), cluster_color(t, k.name)))
                .collect()
        })
        .unwrap_or_default();
    let mut legend_segs: Vec<Seg> = legend
        .iter()
        .map(|(s, c)| (s.as_str(), *c, false))
        .collect();
    legend_segs.push(("GHz", t.dim, false));
    let inner = pen.panel(
        mid[1],
        &[("Cluster frequency", t.yellow, true)],
        &legend_segs,
        None,
    );
    if let Some(u) = unsupported(app, Source::CpuFreq) {
        pen.note(inner, &u, t.dim);
    } else {
        cluster_chart(pen, app, inner, &win);
    }
    let count = task_rows(app).len();
    let title = if has_app { "Threads" } else { "Processes" };
    let info = format!(
        "{count} · {} · CPU% of one core{} ",
        if app.sort == Sort::Name {
            "by name"
        } else {
            "by CPU"
        },
        if has_app { " · main pinned" } else { "" }
    );
    let inner = pen.panel(
        bot[0],
        &[(title, t.yellow, true)],
        &[(&info, t.dim, false)],
        None,
    );
    if app.group_pools && has_app {
        pools_table(pen, app, inner, true);
    } else {
        task_table(pen, app, inner, true);
    }
    let inner = pen.panel(
        bot[1],
        &[(
            if has_app {
                "Thread pools"
            } else {
                "Process groups"
            },
            t.yellow,
            true,
        )],
        &[("g", t.yellow, false), (" group in table", t.dim, false)],
        None,
    );
    pools_table(pen, app, inner, false);
}

fn cpu_now(pen: &mut Pen, app: &App, area: Rect) {
    let t = pen.t;
    let inner = pen.panel(
        area,
        &[("Now", t.yellow, true)],
        &[(cpu_units_label(app), t.dim, false)],
        None,
    );
    let h = &app.session.history;
    let (x, mut y, right) = (inner.x + 1, inner.y, inner.right());
    let ncores = app
        .session
        .fast
        .as_ref()
        .map_or(1, |f| f.cores.len().max(1)) as f32;
    let f = cpu_factor(app);
    let p = |v: f32| format!("{:.1}%", v * f);
    if app.package().is_some() {
        let now = last(&h.app_cpu);
        let other = if f == 1.0 {
            opt(now, |v| format!("  = {:.0}% of one core", v * ncores))
        } else {
            opt(now, |v| format!("  = {v:.1}% of all cores"))
        };
        pen.segs(
            x,
            y,
            &[
                ("APP    ", t.dim, false),
                (&opt(now, p), t.green, true),
                (&other, t.dim, false),
            ],
            right,
        );
        y += 1;
        pen.segs(
            x,
            y,
            &[
                ("  kernel ", t.dim, false),
                (&opt(last(&h.app_kernel), p), t.orange, false),
                ("  peak ", t.dim, false),
                (&opt(h.app_cpu.max(), p), t.orange, false),
                ("  avg ", t.dim, false),
                (&opt(h.app_cpu.avg(), p), t.purple, false),
            ],
            right,
        );
        y += 2;
    }
    let busiest = app.session.fast.as_ref().and_then(|fs| {
        fs.cores
            .iter()
            .max_by(|a, b| {
                a.usage
                    .partial_cmp(&b.usage)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .copied()
    });
    pen.segs(
        x,
        y,
        &[
            ("SYSTEM ", t.dim, false),
            (&opt(last(&h.sys_cpu), p), t.blue, true),
            (
                &busiest.map_or(String::new(), |c| {
                    format!("  busiest cpu{} {:.0}%", c.id, c.usage)
                }),
                t.dim,
                false,
            ),
        ],
        right,
    );
    y += 1;
    if app.package().is_none() {
        if let Some(fs) = app.session.fast.as_ref() {
            pen.segs(
                x,
                y,
                &[
                    ("  peak ", t.dim, false),
                    (&opt(h.sys_cpu.max(), p), t.orange, false),
                    ("  avg ", t.dim, false),
                    (&opt(h.sys_cpu.avg(), p), t.purple, false),
                ],
                right,
            );
            y += 2;
            let running = fs.tasks.iter().filter(|k| k.state == 'R').count();
            let blocked = fs.tasks.iter().filter(|k| k.state == 'D').count();
            pen.segs(
                x,
                y,
                &[
                    ("PROCESSES ", t.dim, false),
                    (&fs.tasks.len().to_string(), t.fg, false),
                    ("  running ", t.dim, false),
                    (&running.to_string(), t.green, false),
                    ("  blocked ", t.dim, false),
                    (
                        &blocked.to_string(),
                        if blocked > 0 { t.red } else { t.fg },
                        false,
                    ),
                ],
                right,
            );
            y += 2;
            let mut top: Vec<&TaskSample> = fs.tasks.iter().collect();
            top.sort_by(|a, b| {
                b.cpu
                    .partial_cmp(&a.cpu)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            pen.text(x, y, "BUSIEST  % of one core", t.dim, false, right);
            y += 1;
            for task in top.iter().take(6) {
                if y >= inner.bottom() {
                    break;
                }
                pen.segs(
                    x,
                    y,
                    &[
                        (
                            &rpad(&format!("{:.1}", task.cpu), 6),
                            if task.cpu > 20.0 { t.red } else { t.orange },
                            false,
                        ),
                        ("  ", t.dim, false),
                        (app.session.task_display_name(task), t.fg, false),
                    ],
                    right,
                );
                y += 1;
            }
        }
    }
    y += 1;
    if let Some(a) = app.session.fast.as_ref().and_then(|f| f.app.as_ref()) {
        let rowsv: Vec<Vec<(String, Color, bool)>> = vec![
            vec![
                ("THREADS ".into(), t.dim, false),
                (a.threads.to_string(), t.fg, false),
                ("  running ".into(), t.dim, false),
                (a.running.to_string(), t.green, false),
                ("  blocked ".into(), t.dim, false),
                (
                    a.blocked.to_string(),
                    if a.blocked > 0 { t.red } else { t.fg },
                    false,
                ),
            ],
            vec![
                ("ctx switches/s ".into(), t.dim, false),
                (format!("{:.0}", a.ctx_voluntary_ps), t.fg, false),
                (" vol  ".into(), t.dim, false),
                (format!("{:.0}", a.ctx_involuntary_ps), t.fg, false),
                (" invol".into(), t.dim, false),
            ],
            vec![
                ("page faults/s  ".into(), t.dim, false),
                (format!("{:.0}", a.minflt_ps), t.fg, false),
                (" minor  ".into(), t.dim, false),
                (
                    format!("{:.0}", a.majflt_ps),
                    if a.majflt_ps > 0.0 { t.orange } else { t.fg },
                    false,
                ),
                (" major".into(), t.dim, false),
            ],
            vec![
                ("process ".into(), t.dim, false),
                (format!("up {}", fmt_dur(a.uptime_s)), t.fg, false),
                ("  nice ".into(), t.dim, false),
                (a.nice.to_string(), t.fg, false),
                ("  oom ".into(), t.dim, false),
                (a.oom_adj.map_or("–".into(), |o| o.to_string()), t.fg, false),
            ],
        ];
        for row in rowsv {
            if y >= inner.bottom() {
                break;
            }
            let refs: Vec<Seg> = row.iter().map(|(s, c, b)| (s.as_str(), *c, *b)).collect();
            pen.segs(x, y, &refs, right);
            y += 1;
        }
        y += 1;
    }
    if let Some(th) = &app.session.thermal {
        if y < inner.bottom() {
            let c = if th.status == 0 {
                t.green
            } else if th.status == 1 {
                t.orange
            } else {
                t.red
            };
            pen.segs(
                x,
                y,
                &[
                    ("THERMAL ", t.dim, false),
                    (thermal_level_name(th.status), c, true),
                ],
                right,
            );
            y += 1;
        }
    }
    if let Some(note) = throttle_note(app) {
        if y < inner.bottom() {
            pen.text(x, y, &note, t.orange, false, right);
        }
    }
}

fn core_heat(pen: &mut Pen, app: &App, area: Rect, win: &Win) {
    let t = pen.t;
    let h = &app.session.history;
    let caps = app.session.caps.as_ref();
    if area.height == 0 || h.cores.is_empty() {
        pen.note(area, "Waiting for samples…", t.dim);
        return;
    }
    let legend_rows = if area.height as usize > h.cores.len() {
        1
    } else {
        0
    };
    let rows_avail = (area.height - legend_rows) as usize;
    let heat_x = area.x + 5;
    let heat_w = area.width.saturating_sub(5 + 13);
    let fast = app.session.fast.as_ref();
    let n_cores = h.cores.len();
    let group = n_cores.div_ceil(rows_avail.max(1)).max(1);
    for (row, start) in (0..n_cores).step_by(group).enumerate() {
        let end = (start + group).min(n_cores);
        let y = area.y + row as u16;
        let cl = caps
            .and_then(|c| c.cluster_of(start as u16))
            .and_then(|ci| caps.map(|c| c.clusters[ci].name));
        let color = cl.map_or(t.fg, |n| cluster_color(t, n));
        let label = if end - start == 1 {
            format!("{start:>2}")
        } else {
            format!("{start}-{}", end - 1)
        };
        pen.segs(
            area.x + 1,
            y,
            &[
                (cl.map_or("C", cluster_letter), color, true),
                (&label, t.dim, false),
            ],
            heat_x,
        );
        let mut data = tail(&h.cores[start], win.n);
        for series in &h.cores[start + 1..end] {
            for (d, v) in data.iter_mut().zip(tail(series, win.n)) {
                if v.is_finite() && (!d.is_finite() || v > *d) {
                    *d = v;
                }
            }
        }
        pen.heat_row(heat_x, y, heat_w, &data);
        let cores: Vec<_> = fast
            .map(|f| {
                f.cores
                    .iter()
                    .filter(|c| (start..end).contains(&(c.id as usize)))
                    .collect()
            })
            .unwrap_or_default();
        let v =
            cores
                .iter()
                .map(|c| c.usage)
                .fold(f32::NAN, |a, b| if a.is_finite() { a.max(b) } else { b });
        let ghz = cores
            .iter()
            .map(|c| c.cur_khz)
            .max()
            .filter(|k| *k > 0)
            .map(|k| format!("{:.2}G", k as f32 / 1e6))
            .unwrap_or_default();
        pen.segs(
            heat_x + heat_w + 1,
            y,
            &[
                (
                    &rpad(&opt(Some(v), |v| format!("{v:.0}%")), 5),
                    t.usage(v.max(21.0)),
                    false,
                ),
                (" ", t.dim, false),
                (&ghz, t.dim, false),
            ],
            area.right(),
        );
    }
    if legend_rows == 1 {
        let y = area.bottom() - 1;
        let mut x = pen.text(heat_x, y, "load ", t.dim, false, area.right());
        for v in [0.0, 10.0, 25.0, 50.0, 75.0, 100.0] {
            for _ in 0..2 {
                pen.bg(x, y, t.heat(v));
                x += 1;
            }
            x = pen.text(x, y, &format!(" {v:.0} "), t.dim, false, area.right());
        }
        if let Some(caps) = caps {
            for c in &caps.clusters {
                x = pen.segs(
                    x + 1,
                    y,
                    &[
                        (cluster_letter(c.name), cluster_color(t, c.name), true),
                        (
                            &format!(
                                " {} {}×{:.2}G ",
                                c.name,
                                c.cores.len(),
                                c.max_khz as f32 / 1e6
                            ),
                            t.dim,
                            false,
                        ),
                    ],
                    area.right(),
                );
            }
        }
    }
}

fn cluster_chart(pen: &mut Pen, app: &App, area: Rect, win: &Win) {
    let t = pen.t;
    let Some(caps) = &app.session.caps else {
        return;
    };
    let h = &app.session.history;
    let data: Vec<(Vec<f32>, Color)> = caps
        .clusters
        .iter()
        .enumerate()
        .map(|(i, c)| {
            (
                h.cluster_ghz
                    .get(i)
                    .map_or_else(Vec::new, |s| tail(s, win.n)),
                cluster_color(t, c.name),
            )
        })
        .collect();
    let lines: Vec<ChartLine> = data
        .iter()
        .map(|(d, c)| ChartLine {
            data: d,
            color: *c,
            fill: false,
        })
        .collect();
    let ymax = caps
        .clusters
        .iter()
        .map(|c| c.max_khz as f32 / 1e6)
        .fold(1.0, f32::max)
        .ceil();
    let ticks = [ymax, ymax / 2.0, 0.0];
    let marks = win.marks();
    pen.chart(
        area,
        &lines,
        &ChartOpts {
            ymin: 0.0,
            ymax,
            ticks: &ticks,
            fmt: &|v| format!("{v:.1}"),
            label_w: 5,
            xlabels: win.xl(),
            markers: &marks,
            hlines: &[],
        },
    );
}

fn pools_table(pen: &mut Pen, app: &App, area: Rect, in_main: bool) {
    let t = pen.t;
    if area.height < 2 {
        return;
    }
    let mut pools: HashMap<String, (u32, f32, f32)> = HashMap::new();
    let device = app.package().is_none();
    for r in task_rows(app) {
        let key = if r.main {
            "main".to_string()
        } else if device {
            process_group(r.task, r.name).to_string()
        } else {
            pool_name(r.name)
        };
        let e = pools.entry(key).or_default();
        e.0 += 1;
        e.1 += r.task.cpu;
        e.2 += r.hist.map_or(0.0, |h| h.avg());
    }
    let mut list: Vec<(String, (u32, f32, f32))> = pools.into_iter().collect();
    list.sort_by(|a, b| {
        b.1 .2
            .partial_cmp(&a.1 .2)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.1 .0.cmp(&a.1 .0))
    });
    let total: f32 = list.iter().map(|p| p.1 .2).sum::<f32>().max(0.01);
    let name_w = if in_main { 28 } else { 19 };
    pen.text(
        area.x + 1,
        area.y,
        &format!(
            "{} {}  {}  {}  SHARE",
            pad("POOL", name_w),
            rpad("N", 3),
            rpad("NOW", 5),
            rpad("AVG", 5)
        ),
        t.dim,
        false,
        area.right(),
    );
    let visible = (area.height - 1) as usize;
    let offset = if in_main {
        app.scroll[app.page].min(list.len().saturating_sub(visible))
    } else {
        0
    };
    for (i, (name, (n, now, avg))) in list.iter().skip(offset).take(visible).enumerate() {
        let y = area.y + 1 + i as u16;
        let x = pen.segs(
            area.x + 1,
            y,
            &[
                (
                    &pad(name, name_w),
                    if name == "main" { t.yellow } else { t.fg },
                    false,
                ),
                (" ", t.dim, false),
                (&rpad(&n.to_string(), 3), t.dim, false),
                ("  ", t.dim, false),
                (&rpad(&format!("{now:.1}"), 5), t.usage(*now * 4.0), false),
                ("  ", t.dim, false),
                (&rpad(&format!("{avg:.1}"), 5), t.purple, false),
                ("  ", t.dim, false),
            ],
            area.right(),
        );
        pen.hbar(
            x,
            y,
            area.right().saturating_sub(x + 1),
            *avg,
            total,
            t.aqua,
        );
    }
}

pub fn memory(pen: &mut Pen, app: &App, area: Rect) {
    let t = pen.t;
    let s = &app.session;
    let win = Win::of(app);
    let app_mode = app.package().is_some();
    let r = rows(
        area,
        &[
            Constraint::Percentage(if app_mode { 40 } else { 55 }),
            Constraint::Percentage(if app_mode { 30 } else { 0 }),
            Constraint::Min(6),
        ],
    );
    if app_mode {
        let top = cols(
            r[0],
            &[
                Constraint::Min(40),
                Constraint::Length((area.width * 32 / 100).clamp(56, 70)),
            ],
        );
        let gate = unsupported(app, Source::AppMemory)
            .or_else(|| app_gate(app).map(String::from))
            .or_else(|| {
                s.app_mem
                    .is_none()
                    .then(|| "Waiting for dumpsys meminfo…".to_string())
            });
        let legend: Vec<(String, Color)> = app_layers(app, 1)
            .iter()
            .rev()
            .map(|(_, c, n)| (format!("■ {n} "), *c))
            .collect();
        let mut legend_refs: Vec<Seg> = legend
            .iter()
            .map(|(s, c)| (s.as_str(), *c, false))
            .collect();
        let shared = s
            .app_mem
            .as_ref()
            .map(|m| {
                format!(
                    "+ {:.0} MB shared Code/System",
                    (m.code_kb + m.system_kb) as f32 / 1024.0
                )
            })
            .unwrap_or_default();
        legend_refs.push((&shared, t.dim, false));
        let full = app.config.memory_chart == MemChart::Full;
        if full {
            legend_refs.pop();
        }
        let inner = pen.panel(
            top[0],
            &[(if full { "PSS" } else { "App-owned PSS" }, t.yellow, true)],
            &legend_refs,
            None,
        );
        match &gate {
            Some(g) => pen.note(inner, g, t.dim),
            None => {
                let layers = app_layers(app, win.n);
                stacked_chart(pen, app, inner, &layers, &win, 10.0, "");
            }
        }
        memory_now(pen, app, top[1]);
        let mid = cols(
            r[1],
            &[
                Constraint::Percentage(42),
                Constraint::Percentage(30),
                Constraint::Min(30),
            ],
        );
        let inner = pen.panel(
            mid[0],
            &[("Java heap", t.yellow, true)],
            &[
                ("━ size ", t.dim, false),
                ("━ allocated ", t.green, false),
                (
                    &format!("drops≈GC {}", s.history.heap_alloc_mb.drops(1.0)),
                    t.dim,
                    false,
                ),
            ],
            None,
        );
        if gate.is_none() {
            let size = tail(&s.history.heap_size_mb, win.n);
            let alloc = tail(&s.history.heap_alloc_mb, win.n);
            let peak = size
                .iter()
                .chain(alloc.iter())
                .copied()
                .filter(|v| v.is_finite())
                .fold(0.0, f32::max);
            let ymax = nice_max(peak.max(8.0));
            let ticks = [ymax, ymax / 2.0, 0.0];
            let marks = win.marks();
            pen.chart(
                inner,
                &[
                    ChartLine {
                        data: &size,
                        color: t.dim,
                        fill: false,
                    },
                    ChartLine {
                        data: &alloc,
                        color: t.green,
                        fill: false,
                    },
                ],
                &ChartOpts {
                    ymin: 0.0,
                    ymax,
                    ticks: &ticks,
                    fmt: &|v| format!("{v:.0}"),
                    label_w: 5,
                    xlabels: win.xl(),
                    markers: &marks,
                    hlines: &[],
                },
            );
        }
        objects_panel(pen, app, mid[1]);
        sqlite_panel(pen, app, mid[2]);
    } else {
        let inner = pen.panel(
            r[0],
            &[("Device RAM", t.yellow, true)],
            &[("GB over time", t.dim, false)],
            None,
        );
        device_ram_body(pen, app, inner, &win);
    }
    let inner = pen.panel(
        r[2],
        &[(
            if app_mode {
                "Device RAM"
            } else {
                "Processes by memory"
            },
            t.yellow,
            true,
        )],
        &[(
            if app_mode { "" } else { "RSS · resident set" },
            t.dim,
            false,
        )],
        None,
    );
    if app_mode {
        device_ram_body(pen, app, inner, &win);
    } else {
        process_memory_table(pen, app, inner);
    }
}

fn memory_now(pen: &mut Pen, app: &App, area: Rect) {
    let t = pen.t;
    let s = &app.session;
    let inner = pen.panel(
        area,
        &[("Now", t.yellow, true)],
        &[("MB · App Summary", t.dim, false)],
        None,
    );
    let Some(m) = &s.app_mem else { return };
    let (x, mut y, right) = (inner.x + 1, inner.y, inner.right());
    pen.text(
        x,
        y,
        "CATEGORY        PSS     RSS    Δ  TREND",
        t.dim,
        false,
        right,
    );
    y += 1;
    let h = &s.history;
    let series = [
        &h.java_mb,
        &h.native_mb,
        &h.graphics_mb,
        &h.stack_mb,
        &h.other_mb,
        &h.code_mb,
        &h.system_mb,
    ];
    let rss = [
        m.java_rss_kb,
        m.native_rss_kb,
        m.graphics_rss_kb,
        m.stack_rss_kb,
        0,
        m.code_rss_kb,
        0,
    ];
    for (i, (name, kb, first, color)) in categories(app).into_iter().enumerate() {
        if y >= inner.bottom() {
            break;
        }
        let d = (kb as f32 - first as f32) / 1024.0;
        let end = pen.segs(
            x,
            y,
            &[
                (swatch(app, name), color, false),
                (&pad(name, 12), t.fg, false),
                (&rpad(&format!("{:.1}", kb as f32 / 1024.0), 6), t.fg, false),
                (
                    &rpad(
                        &if rss[i] > 0 {
                            format!("{:.1}", rss[i] as f32 / 1024.0)
                        } else {
                            "–".into()
                        },
                        8,
                    ),
                    t.dim,
                    false,
                ),
                (
                    &rpad(&signed1(d), 7),
                    if d.abs() < 0.5 { t.dim } else { t.orange },
                    false,
                ),
                ("  ", t.dim, false),
            ],
            right,
        );
        let data: Vec<f32> = tail(series[i], 120)
            .into_iter()
            .skip_while(|v| !v.is_finite())
            .collect();
        let min = data
            .iter()
            .copied()
            .filter(|v| v.is_finite())
            .fold(f32::INFINITY, f32::min);
        let rel: Vec<f32> = data.iter().map(|v| v - min).collect();
        let range = rel
            .iter()
            .copied()
            .filter(|v| v.is_finite())
            .fold(0.0, f32::max);
        let floor = if min.is_finite() {
            (min * 0.05).max(1.0)
        } else {
            1.0
        };
        pen.spark(
            end,
            y,
            right.saturating_sub(end + 1),
            &rel,
            range.max(floor),
            &|_| {
                if color == t.dim2 {
                    t.gray
                } else {
                    color
                }
            },
        );
        y += 1;
    }
    if y + 1 < inner.bottom() {
        y += 1;
        pen.segs(
            x,
            y,
            &[
                (&pad("TOTAL PSS", 14), t.fg, true),
                (
                    &rpad(&format!("{:.1}", m.pss_kb as f32 / 1024.0), 6),
                    t.aqua,
                    true,
                ),
                (
                    &rpad(&format!("{:.1}", m.rss_kb as f32 / 1024.0), 8),
                    t.dim,
                    false,
                ),
                ("  swap ", t.dim, false),
                (
                    &format!("{:.1}", m.swap_pss_kb as f32 / 1024.0),
                    t.fg,
                    false,
                ),
            ],
            right,
        );
        y += 1;
    }
    if y + 1 < inner.bottom() {
        y += 1;
        match h.pss_mb.slope_per_min(s.interval_s()) {
            Some(g) => pen.segs(
                x,
                y,
                &[
                    ("trend ", t.dim, false),
                    (
                        &format!("{g:+.2} MB/min"),
                        if g > 0.5 { t.orange } else { t.fg },
                        g > 0.5,
                    ),
                    (
                        &format!(
                            " over {}",
                            fmt_dur(h.pss_mb.len() as f64 * s.interval_s() as f64)
                        ),
                        t.dim,
                        false,
                    ),
                ],
                right,
            ),
            None => pen.text(x, y, "trend needs a minute of samples", t.dim, false, right),
        };
        y += 1;
    }
    let early = h
        .heap_alloc_mb
        .range_values(0, h.heap_alloc_mb.len() / 3)
        .fold(f32::INFINITY, f32::min);
    let late = h
        .heap_alloc_mb
        .range_values(h.heap_alloc_mb.len() * 2 / 3, h.heap_alloc_mb.len())
        .fold(f32::INFINITY, f32::min);
    if h.heap_alloc_mb.len() >= 60
        && early.is_finite()
        && late.is_finite()
        && late - early > 2.0
        && y < inner.bottom()
    {
        pen.segs(
            x,
            y,
            &[
                ("⚠ ", t.orange, false),
                (
                    &format!("Java floor after GC rose {early:.0} → {late:.0} MB"),
                    t.fg,
                    false,
                ),
            ],
            right,
        );
    }
}

fn objects_panel(pen: &mut Pen, app: &App, area: Rect) {
    let t = pen.t;
    let s = &app.session;
    let inner = pen.panel(
        area,
        &[("Objects", t.yellow, true)],
        &[("Δ since start", t.dim, false)],
        None,
    );
    let (Some(now), Some(first)) = (
        s.app_mem.as_ref().and_then(|m| m.objects),
        s.app_mem_first.as_ref().and_then(|m| m.objects),
    ) else {
        return;
    };
    let items = [
        ("Activities", now.activities, first.activities),
        ("AppContexts", now.app_contexts, first.app_contexts),
        ("Views", now.views, first.views),
        ("ViewRootImpl", now.view_roots, first.view_roots),
        ("WebViews", now.webviews, first.webviews),
        ("Assets", now.assets, first.assets),
        ("Local Binders", now.local_binders, first.local_binders),
        ("Proxy Binders", now.proxy_binders, first.proxy_binders),
        ("Death Recip.", now.death_recipients, first.death_recipients),
        ("Parcel count", now.parcel_count, first.parcel_count),
    ];
    let two = inner.width >= 52;
    let per_col = if two {
        items.len().div_ceil(2)
    } else {
        items.len()
    };
    for (i, (name, v, f)) in items.iter().enumerate() {
        let col = (i / per_col) as u16;
        let row = (i % per_col) as u16;
        if row >= inner.height {
            continue;
        }
        let x = inner.x + 1 + col * (inner.width / 2);
        let d = *v as i64 - *f as i64;
        let warn = *name == "AppContexts" && d > 0 && now.activities <= first.activities;
        pen.segs(
            x,
            inner.y + row,
            &[
                (&pad(name, 14), t.fg, false),
                (&rpad(&v.to_string(), 5), t.fg, false),
                (
                    &rpad(
                        &if d == 0 {
                            "·".into()
                        } else {
                            format!("{d:+}")
                        },
                        5,
                    ),
                    if warn {
                        t.orange
                    } else if d != 0 {
                        t.yellow
                    } else {
                        t.dim
                    },
                    warn,
                ),
                (if warn { " ⚠" } else { "" }, t.orange, false),
            ],
            if two {
                x + inner.width / 2
            } else {
                inner.right()
            },
        );
    }
    let note_y = inner.y + per_col as u16 + 1;
    if note_y < inner.bottom()
        && now.app_contexts > first.app_contexts
        && now.activities <= first.activities
    {
        pen.text(
            inner.x + 1,
            note_y,
            &trunc(
                &format!(
                    "AppContexts {} → {} while Activities stayed {}",
                    first.app_contexts, now.app_contexts, now.activities
                ),
                inner.width as usize - 2,
            ),
            t.orange,
            false,
            inner.right(),
        );
    }
}

fn sqlite_panel(pen: &mut Pen, app: &App, area: Rect) {
    let t = pen.t;
    let sql = app.session.app_mem.as_ref().and_then(|m| m.sqlite.as_ref());
    let used = sql
        .map(|s| format!("{:.1} MB used", s.memory_used_kb as f32 / 1024.0))
        .unwrap_or_default();
    let inner = pen.panel(
        area,
        &[("SQLite", t.yellow, true)],
        &[(&used, t.dim, false)],
        None,
    );
    let Some(sql) = sql else {
        pen.note(inner, "No SQLite databases open", t.dim);
        return;
    };
    let name_w = inner.width.saturating_sub(26).max(10) as usize;
    pen.text(
        inner.x + 1,
        inner.y,
        &format!(
            "{} {}  {}  {}",
            pad("DATABASE", name_w),
            rpad("SIZE", 6),
            rpad("HIT%", 5),
            rpad("MISSES", 7)
        ),
        t.dim,
        false,
        inner.right(),
    );
    for (i, db) in sql.databases.iter().enumerate() {
        let y = inner.y + 1 + i as u16;
        if y >= inner.bottom() {
            break;
        }
        let total = db.cache_hits + db.cache_misses;
        let hit = if total > 0 {
            db.cache_hits as f32 * 100.0 / total as f32
        } else {
            f32::NAN
        };
        pen.segs(
            inner.x + 1,
            y,
            &[
                (&pad(&db.name, name_w), t.fg, false),
                (" ", t.dim, false),
                (
                    &rpad(&fmt_bytes(db.size_kb as f64 * 1024.0), 6),
                    t.fg,
                    false,
                ),
                ("  ", t.dim, false),
                (
                    &rpad(&opt(Some(hit), |v| format!("{v:.0}%")), 5),
                    if hit < 10.0 { t.orange } else { t.fg },
                    false,
                ),
                ("  ", t.dim, false),
                (&rpad(&db.cache_misses.to_string(), 7), t.dim, false),
            ],
            inner.right(),
        );
    }
}

fn ram_bar(pen: &mut Pen, app: &App, area: Rect) {
    let t = pen.t;
    let Some(m) = app.session.fast.as_ref().and_then(|f| f.mem) else {
        return;
    };
    if area.height < 2 {
        return;
    }
    let total = m.total_kb.max(1) as f32;
    let free = m.available_kb.saturating_sub(m.cached_kb) as f32;
    let parts = [
        (m.used_kb() as f32, t.orange, "used"),
        (m.cached_kb.min(m.available_kb) as f32, t.blue, "cached"),
        (free, t.green, "free"),
    ];
    let gb = |kb: f32| format!("{:.1} GB", kb / 1048576.0);
    let mut x = area.x + 1;
    for (kb, c, name) in parts {
        x = pen.segs(
            x,
            area.y,
            &[
                ("■ ", c, false),
                (&format!("{name} {}  ", gb(kb)), t.fg, false),
            ],
            area.right(),
        );
    }
    if let Some(a) = &app.session.app_mem {
        pen.segs(
            x + 2,
            area.y,
            &[
                ("this app ", t.dim, false),
                (&format!("{:.0} MB", a.pss_kb as f32 / 1024.0), t.aqua, true),
                (
                    &format!(" ({:.1}% of RAM)", a.pss_kb as f32 * 100.0 / total),
                    t.dim,
                    false,
                ),
            ],
            area.right(),
        );
    }
    let bar_w = area.width.saturating_sub(2) as f32;
    let mut bx = area.x + 1;
    let bar_rows = (area.height - 1).min(2);
    for (kb, c, _) in parts {
        let w = ((kb / total) * bar_w).round() as u16;
        for i in 0..w.min(area.right().saturating_sub(bx + 1)) {
            for r in 0..bar_rows {
                pen.bg(bx + i, area.y + 1 + r, c);
            }
        }
        bx += w;
    }
    let y = area.y + 1 + bar_rows;
    if y < area.bottom() {
        pen.segs(
            area.x + 1,
            y,
            &[
                ("swap ", t.dim, false),
                (
                    &format!(
                        "{} / {}",
                        gb(m.swap_used_kb() as f32),
                        gb(m.swap_total_kb as f32)
                    ),
                    t.purple,
                    false,
                ),
            ],
            area.right(),
        );
    }
}

pub fn frames(pen: &mut Pen, app: &App, area: Rect) {
    let t = pen.t;
    let s = &app.session;
    let win = Win::of(app);
    let r = rows(
        area,
        &[
            Constraint::Percentage(42),
            Constraint::Percentage(28),
            Constraint::Min(6),
        ],
    );
    let budget = s.frames.back().map(|f| f.budget_ms);
    let right = budget
        .map(|b| format!("budget {b:.1} ms @ {:.0} Hz  ", 1000.0 / b))
        .unwrap_or_default();
    let inner = pen.panel(
        r[0],
        &[("Frame time", t.yellow, true)],
        &[
            (&right, t.dim, false),
            ("■", t.green, false),
            (" on time  ", t.dim, false),
            ("■", t.yellow, false),
            (" late  ", t.dim, false),
            ("■", t.red, false),
            (" janky  ", t.dim, false),
            ("▲", t.red, false),
            (" off scale", t.dim, false),
        ],
        None,
    );
    if let Some(g) = frames_gate(app) {
        pen.note(inner, &g, t.dim);
        return;
    }
    frame_bars(pen, app, inner, 7);
    let mid = cols(
        r[1],
        &[Constraint::Percentage(50), Constraint::Percentage(50)],
    );
    let inner = pen.panel(
        mid[0],
        &[("FPS & jank", t.yellow, true)],
        &[("━ fps ", t.green, false), ("█ jank % ", t.red, false)],
        None,
    );
    if inner.height >= 4 {
        let fps = tail(&s.history.fps, win.n);
        let jank = tail(&s.history.jank_pct, win.n);
        let jank_h = (inner.height / 4).max(1);
        let chart_area = Rect::new(inner.x, inner.y, inner.width, inner.height - jank_h);
        let hz = budget.map_or(60.0, |b| (1000.0 / b).round());
        let ticks = [hz, hz / 2.0, 0.0];
        let marks = win.marks();
        pen.chart(
            chart_area,
            &[ChartLine {
                data: &fps,
                color: t.green,
                fill: true,
            }],
            &ChartOpts {
                ymin: 0.0,
                ymax: hz,
                ticks: &ticks,
                fmt: &|v| format!("{v:.0}"),
                label_w: 5,
                xlabels: win.xl(),
                markers: &marks,
                hlines: &[],
            },
        );
        let jr = Rect::new(
            inner.x + 5,
            inner.bottom() - jank_h,
            inner.width - 5,
            jank_h,
        );
        let cols_data = resample(&jank, jr.width as usize);
        let jank_max = nice_max(
            cols_data
                .iter()
                .copied()
                .filter(|v| v.is_finite())
                .fold(25.0, f32::max),
        )
        .min(100.0);
        pen.text(inner.x, jr.y, "jank", t.dim, false, jr.x);
        if jr.height >= 2 {
            pen.text(
                inner.x,
                jr.y + 1,
                &format!("{jank_max:.0}%"),
                t.dim,
                false,
                jr.x,
            );
        }
        pen.vbars(jr, &cols_data, jank_max, &|_| t.red, Some(t.red));
    }
    let since = s.frames_since_start();
    let inner = pen.panel(
        mid[1],
        &[("Percentiles", t.yellow, true)],
        &[(
            &since.as_ref().map_or(String::new(), |f| {
                format!("since start · {} frames", f.total)
            }),
            t.dim,
            false,
        )],
        None,
    );
    if let Some(f) = &since {
        let budget = budget.unwrap_or(16.67);
        let scale = (f.p99_ms as f32).max(budget * 3.0);
        let bar_w = inner.width.saturating_sub(18);
        let mut y = inner.y;
        for (label, v) in [
            ("p50", f.p50_ms),
            ("p90", f.p90_ms),
            ("p95", f.p95_ms),
            ("p99", f.p99_ms),
        ] {
            if y >= inner.bottom() {
                break;
            }
            let c = frame_color(t, v as f32, budget);
            pen.text(inner.x + 1, y, label, t.dim, false, inner.right());
            pen.hbar(inner.x + 6, y, bar_w, v as f32, scale, c);
            pen.segs(
                inner.x + 7 + bar_w,
                y,
                &[(&rpad(&v.to_string(), 4), c, true), (" ms", t.dim, false)],
                inner.right(),
            );
            y += 1;
        }
        let bcol = inner.x + 6 + ((budget / scale) * bar_w as f32) as u16;
        if y < inner.bottom() {
            pen.put(bcol, y, "▴", t.green);
            pen.text(
                bcol + 2,
                y,
                &format!("{budget:.1} ms budget"),
                t.dim,
                false,
                inner.right(),
            );
            y += 1;
        }
        if y + 1 < inner.bottom() {
            pen.segs(
                inner.x + 1,
                y,
                &[
                    ("GPU p50 ", t.dim, false),
                    (&format!("{} ms", f.gpu_p50_ms), t.fg, false),
                    ("  p99 ", t.dim, false),
                    (&format!("{} ms", f.gpu_p99_ms), t.fg, false),
                ],
                inner.right(),
            );
            y += 1;
            let jp = if f.total > 0 {
                f.janky as f32 * 100.0 / f.total as f32
            } else {
                0.0
            };
            pen.segs(
                inner.x + 1,
                y,
                &[
                    ("janky ", t.dim, false),
                    (
                        &format!("{} ({jp:.1}%)", f.janky),
                        if jp > 5.0 { t.red } else { t.yellow },
                        true,
                    ),
                    ("  legacy ", t.dim, false),
                    (&f.janky_legacy.to_string(), t.fg, false),
                ],
                inner.right(),
            );
        }
    }
    let bot = cols(
        r[2],
        &[Constraint::Percentage(50), Constraint::Percentage(50)],
    );
    let inner = pen.panel(
        bot[0],
        &[("Jank causes", t.yellow, true)],
        &[("since start", t.dim, false)],
        None,
    );
    if let Some(f) = &since {
        let causes = [
            ("High input latency", f.high_input_latency, t.orange),
            ("Frame deadline missed", f.deadline_missed, t.red),
            ("Slow UI thread", f.slow_ui_thread, t.red),
            ("Missed vsync", f.missed_vsync, t.yellow),
            ("Slow draw commands", f.slow_draw_commands, t.yellow),
            ("Slow bitmap uploads", f.slow_bitmap_uploads, t.dim),
        ];
        let max = causes.iter().map(|c| c.1).max().unwrap_or(1).max(1) as f32;
        let gap = if inner.height as usize >= causes.len() * 2 {
            2
        } else {
            1
        };
        let bar_w = inner.width.saturating_sub(33);
        for (i, (name, v, c)) in causes.iter().enumerate() {
            let y = inner.y + (i * gap) as u16;
            if y >= inner.bottom() {
                break;
            }
            pen.text(inner.x + 1, y, &pad(name, 23), t.fg, false, inner.right());
            pen.hbar(inner.x + 24, y, bar_w, *v as f32, max, *c);
            pen.text(
                inner.x + 25 + bar_w,
                y,
                &rpad(&v.to_string(), 6),
                *c,
                true,
                inner.right(),
            );
        }
    }
    let inner = pen.panel(
        bot[1],
        &[("Distribution", t.yellow, true)],
        &[("frame time buckets, √ scale", t.dim, false)],
        None,
    );
    if let Some(f) = &since {
        let b = budget.unwrap_or(16.67);
        let edges = frame_buckets(b);
        let counts: Vec<u64> = edges
            .iter()
            .map(|(lo, hi, _)| {
                f.histogram
                    .iter()
                    .filter(|(ms, _)| (*ms as f32) > *lo && (*ms as f32) <= *hi)
                    .map(|(_, n)| *n)
                    .sum()
            })
            .collect();
        let max = counts.iter().copied().max().unwrap_or(1).max(1) as f32;
        let gap = if inner.height as usize >= edges.len() * 2 {
            2
        } else {
            1
        };
        let bar_w = inner.width.saturating_sub(28);
        for (i, ((lo, _, label), n)) in edges.iter().zip(counts.iter()).enumerate() {
            let y = inner.y + (i * gap) as u16;
            if y >= inner.bottom() {
                break;
            }
            let c = if *lo < b {
                t.green
            } else if *lo < b * 2.0 {
                t.yellow
            } else if *lo < 33.0 {
                t.orange
            } else {
                t.red
            };
            pen.text(inner.x + 1, y, &pad(label, 11), t.fg, false, inner.right());
            pen.hbar(inner.x + 12, y, bar_w, (*n as f32).sqrt(), max.sqrt(), c);
            pen.segs(
                inner.x + 13 + bar_w,
                y,
                &[
                    (&rpad(&n.to_string(), 6), c, true),
                    (
                        &rpad(
                            &format!("{:.1}%", *n as f32 * 100.0 / f.total.max(1) as f32),
                            7,
                        ),
                        t.dim,
                        false,
                    ),
                ],
                inner.right(),
            );
        }
    }
}

fn host_source(source: i32) -> &'static str {
    match HostSource::try_from(source) {
        Ok(HostSource::Dns) => "DNS",
        Ok(HostSource::TlsSni) => "SNI",
        Ok(HostSource::HttpHost) => "HTTP",
        _ => "—",
    }
}

pub fn network(pen: &mut Pen, app: &App, area: Rect) {
    let t = pen.t;
    let s = &app.session;
    let win = Win::of(app);
    let r = rows(
        area,
        &[
            Constraint::Percentage(32),
            Constraint::Percentage(30),
            Constraint::Min(6),
        ],
    );
    let top = cols(
        r[0],
        &[
            Constraint::Min(40),
            Constraint::Length((area.width * 32 / 100).clamp(50, 66)),
        ],
    );
    let (_, _, source) = net_data(app, 1);
    let inner = pen.panel(
        top[0],
        &[("Throughput", t.yellow, true)],
        &[
            ("━ ↓ received ", t.aqua, false),
            ("━ ↑ sent ", t.purple, false),
            (
                &format!(
                    "{source} · per second · {}",
                    if app.config.net_scale == NetScale::Sqrt {
                        "√ scale"
                    } else {
                        "linear"
                    }
                ),
                t.dim,
                false,
            ),
        ],
        None,
    );
    net_chart(pen, app, inner, &win, 7);
    let cap = s.capture.snapshot.as_ref();
    let started = cap
        .map(|c| {
            format!(
                "capture running {}",
                fmt_dur(c.time_ms.saturating_sub(c.started_ms) as f64 / 1000.0)
            )
        })
        .unwrap_or_default();
    let inner = pen.panel(
        top[1],
        &[("Totals", t.yellow, true)],
        &[(&started, t.dim, false)],
        None,
    );
    let (x, mut y, right) = (inner.x + 1, inner.y, inner.right());
    let h = &s.history;
    let (rx_s, tx_s) = if cap.is_some() {
        (&h.cap_rx, &h.cap_tx)
    } else {
        (&h.dev_rx, &h.dev_tx)
    };
    let totals = cap.map(|_| s.capture.traffic(s.app_uids().as_deref()));
    for (arrow, series, color, total) in [
        ("↓ ", rx_s, t.aqua, totals.map(|x| x.0)),
        ("↑ ", tx_s, t.purple, totals.map(|x| x.1)),
    ] {
        pen.segs(
            x,
            y,
            &[
                (arrow, color, false),
                (
                    &rpad(
                        &format!("{}/s", fmt_bytes(series.last().unwrap_or(0.0) as f64)),
                        9,
                    ),
                    color,
                    true,
                ),
                ("  peak ", t.dim, false),
                (
                    &format!("{}/s", fmt_bytes(series.max().unwrap_or(0.0) as f64)),
                    t.fg,
                    false,
                ),
                (
                    &total.map_or(String::new(), |v| {
                        format!("  total {}", fmt_bytes(v as f64))
                    }),
                    t.dim,
                    false,
                ),
            ],
            right,
        );
        y += 1;
    }
    y += 1;
    if let Some(snap) = cap {
        let conns = s.connections();
        let open = conns.iter().filter(|c| is_open(c)).count();
        let failed = conns
            .iter()
            .filter(|c| c.state == ConnectionState::Failed as i32)
            .count();
        let tcp = conns.iter().filter(|c| !c.udp).count();
        let dns = s.dns();
        let dns_failed = dns.iter().filter(|d| d.failed).count();
        let named = conns.iter().filter(|c| !c.host.is_empty()).count().max(1) as f32;
        let share = |src: HostSource| {
            conns.iter().filter(|c| c.source == src as i32).count() as f32 * 100.0 / named
        };
        let lines: Vec<Vec<(String, Color, bool)>> = vec![
            vec![
                ("connections ".into(), t.dim, false),
                (conns.len().to_string(), t.fg, false),
                ("  open ".into(), t.dim, false),
                (open.to_string(), t.green, false),
                ("  failed ".into(), t.dim, false),
                (
                    failed.to_string(),
                    if failed > 0 { t.red } else { t.fg },
                    false,
                ),
            ],
            vec![
                ("  TCP ".into(), t.dim, false),
                (tcp.to_string(), t.fg, false),
                ("  UDP ".into(), t.dim, false),
                ((conns.len() - tcp).to_string(), t.fg, false),
            ],
            vec![
                ("DNS lookups ".into(), t.dim, false),
                (dns.len().to_string(), t.fg, false),
                ("  failed ".into(), t.dim, false),
                (
                    dns_failed.to_string(),
                    if dns_failed > 0 { t.red } else { t.fg },
                    false,
                ),
            ],
            vec![
                ("names from ".into(), t.dim, false),
                (
                    format!(
                        "SNI {:.0}%  DNS {:.0}%  HTTP {:.0}%",
                        share(HostSource::TlsSni),
                        share(HostSource::Dns),
                        share(HostSource::HttpHost)
                    ),
                    t.fg,
                    false,
                ),
            ],
            vec![
                ("packets ".into(), t.dim, false),
                (
                    snap.stats.map_or("–".into(), |st| {
                        format!("↓ {}  ↑ {}", st.packets_to_apps, st.packets_from_apps)
                    }),
                    t.fg,
                    false,
                ),
                ("  unsupported ".into(), t.dim, false),
                (
                    snap.stats
                        .map_or("–".into(), |st| st.unsupported_packets.to_string()),
                    t.fg,
                    false,
                ),
            ],
            vec![
                ("scope ".into(), t.dim, false),
                (
                    app.package().unwrap_or("all apps").to_string(),
                    t.aqua,
                    true,
                ),
                ("  x".into(), t.yellow, false),
                (" all apps".into(), t.dim, false),
            ],
        ];
        for line in lines {
            if y >= inner.bottom() {
                break;
            }
            let refs: Vec<Seg> = line.iter().map(|(a, b, c)| (a.as_str(), *b, *c)).collect();
            pen.segs(x, y, &refs, right);
            y += 1;
        }
    } else if y < inner.bottom() {
        let msg = match s.support(Source::Capture) {
            crate::model::Support::Unsupported(r) => r.clone(),
            _ => "Press c to start the capture agent: per-app traffic, hosts, DNS and every connection".into(),
        };
        for l in wrap(&msg, inner.width as usize - 2) {
            if y >= inner.bottom() {
                break;
            }
            pen.text(x, y, &l, t.dim, false, right);
            y += 1;
        }
    }
    let mid = cols(
        r[1],
        &[Constraint::Percentage(50), Constraint::Percentage(50)],
    );
    let inner = pen.panel(
        mid[0],
        &[("Top hosts", t.yellow, true)],
        &[("by bytes", t.dim, false)],
        None,
    );
    if cap.is_none() {
        pen.note(inner, "Needs the capture (c)", t.dim);
    } else {
        let hosts = s.hosts();
        let host_w = (inner.width as usize).saturating_sub(40).max(16);
        pen.text(
            inner.x + 1,
            inner.y,
            &format!(
                "{} {}  {}  {}  SHARE",
                pad("HOST", host_w),
                rpad("CONN", 4),
                rpad("↓ RX", 7),
                rpad("↑ TX", 7)
            ),
            t.dim,
            false,
            inner.right(),
        );
        let max = hosts.iter().map(|h| h.rx + h.tx).sum::<u64>().max(1) as f32;
        for (i, hst) in hosts.iter().enumerate() {
            let y = inner.y + 1 + i as u16;
            if y >= inner.bottom() {
                break;
            }
            let bad = hst.failed > 0 && hst.rx == 0;
            let x = pen.segs(
                inner.x + 1,
                y,
                &[
                    (
                        &pad(&hst.host, host_w),
                        if bad { t.red } else { t.fg },
                        false,
                    ),
                    (" ", t.dim, false),
                    (&rpad(&hst.connections.to_string(), 4), t.dim, false),
                    ("  ", t.dim, false),
                    (&rpad(&fmt_bytes(hst.rx as f64), 7), t.aqua, false),
                    ("  ", t.dim, false),
                    (&rpad(&fmt_bytes(hst.tx as f64), 7), t.purple, false),
                    ("  ", t.dim, false),
                ],
                inner.right(),
            );
            pen.hbar(
                x,
                y,
                inner.right().saturating_sub(x + 1),
                (hst.rx + hst.tx) as f32,
                max,
                t.aqua,
            );
        }
    }
    let dns = s.dns();
    let failed = dns.iter().filter(|d| d.failed).count();
    let info = format!("{} lookups · {failed} failed", dns.len());
    let inner = pen.panel(
        mid[1],
        &[("DNS", t.yellow, true)],
        &[(&info, t.dim, false)],
        None,
    );
    if cap.is_none() {
        pen.note(inner, "Needs the capture (c)", t.dim);
    } else {
        let now = cap.map_or(0, |c| c.time_ms);
        let name_w = (inner.width as usize / 2).max(20);
        pen.text(
            inner.x + 1,
            inner.y,
            &format!("{}  {} ANSWER", rpad("AGE", 4), pad("NAME", name_w)),
            t.dim,
            false,
            inner.right(),
        );
        for (i, d) in dns.iter().enumerate() {
            let y = inner.y + 1 + i as u16;
            if y >= inner.bottom() {
                break;
            }
            let answer = if d.failed {
                "no answer".to_string()
            } else {
                d.answers.join(", ")
            };
            pen.segs(
                inner.x + 1,
                y,
                &[
                    (
                        &rpad(&fmt_age(now.saturating_sub(d.time_ms) as f64 / 1000.0), 4),
                        t.dim,
                        false,
                    ),
                    ("  ", t.dim, false),
                    (
                        &pad(&d.name, name_w),
                        if d.failed { t.red } else { t.fg },
                        false,
                    ),
                    (" ", t.dim, false),
                    (&answer, if d.failed { t.red } else { t.dim }, false),
                ],
                inner.right(),
            );
        }
    }
    connections_table(pen, app, r[2]);
}

fn connections_table(pen: &mut Pen, app: &App, area: Rect) {
    let t = pen.t;
    let s = &app.session;
    let f = app.table_filter.to_lowercase();
    let mut conns: Vec<&ConnRecord> = s
        .connections()
        .into_iter()
        .filter(|c| f.is_empty() || c.host.to_lowercase().contains(&f) || c.remote.contains(&f))
        .collect();
    match app.sort {
        Sort::Bytes => conns.sort_by_key(|c| std::cmp::Reverse(c.rx + c.tx)),
        Sort::Name => conns.sort_by(|a, b| a.host.cmp(&b.host)),
        _ => {}
    }
    let order = match app.sort {
        Sort::Bytes => "by bytes",
        Sort::Name => "by host",
        _ => "open first, newest first",
    };
    let info = format!("{} · {order} ", conns.len());
    let inner = pen.panel(
        area,
        &[("Connections", t.yellow, true)],
        &[(&info, t.dim, false)],
        None,
    );
    if s.capture.snapshot.is_none() && conns.is_empty() {
        let msg = match s.support(Source::Capture) {
            crate::model::Support::Unsupported(r) => r.clone(),
            _ => {
                "Press c to start the capture agent; then every connection of the app shows up here"
                    .into()
            }
        };
        pen.note(inner, &msg, t.dim);
        return;
    }
    let show_app = app.package().is_none();
    let host_w = (inner.width as usize).saturating_sub(100).clamp(20, 44);
    let mut header = format!("{} {} ", pad("STATE", 7), pad("PROTO", 5));
    if show_app {
        header.push_str(&format!("{} ", pad("APP", 22)));
    }
    header.push_str(&format!(
        "{} {} {} {} {} {} {} {}  ERROR",
        pad("HOST", host_w),
        pad("FROM", 5),
        pad("REMOTE", 22),
        rpad("↓ RX", 7),
        rpad("↑ TX", 7),
        rpad("PKTS", 6),
        rpad("DURATION", 9),
        rpad("IDLE", 5)
    ));
    pen.text(inner.x + 1, inner.y, &header, t.dim, false, inner.right());
    let now = s.capture.snapshot.as_ref().map_or(0, |c| c.time_ms);
    let visible = inner.height.saturating_sub(1) as usize;
    let offset = app.scroll[app.page].min(conns.len().saturating_sub(visible));
    for (i, c) in conns.iter().skip(offset).take(visible).enumerate() {
        let y = inner.y + 1 + i as u16;
        let (state, color) = match ConnectionState::try_from(c.state) {
            Ok(ConnectionState::Open) => ("open", t.green),
            Ok(ConnectionState::Connecting) => ("conn", t.yellow),
            Ok(ConnectionState::Failed) => ("failed", t.red),
            _ => ("closed", t.dim),
        };
        let closed = state == "closed";
        let host = if c.host.is_empty() {
            c.remote
                .rsplit_once(':')
                .map_or(c.remote.clone(), |r| r.0.to_string())
        } else {
            c.host.clone()
        };
        let dur = c.last_ms.saturating_sub(c.start_ms) as f64 / 1000.0;
        let dur_s = if dur < 10.0 {
            format!("{dur:.1}s")
        } else {
            fmt_dur(dur)
        };
        let mut segs: Vec<(String, Color, bool)> = vec![
            (pad(state, 7) + " ", color, !closed),
            (
                pad(if c.udp { "udp" } else { "tcp" }, 5) + " ",
                t.dim,
                false,
            ),
        ];
        if show_app {
            segs.push((pad(s.capture.app_name(c.uid), 22) + " ", t.blue, false));
        }
        segs.extend([
            (
                pad(&host, host_w) + " ",
                if closed { t.dim } else { t.fg },
                false,
            ),
            (pad(host_source(c.source), 5) + " ", t.blue, false),
            (pad(&c.remote, 22) + " ", t.dim, false),
            (rpad(&fmt_bytes(c.rx as f64), 7) + " ", t.aqua, false),
            (rpad(&fmt_bytes(c.tx as f64), 7) + " ", t.purple, false),
            (rpad(&c.packets.to_string(), 6) + " ", t.dim, false),
            (rpad(&dur_s, 9) + " ", t.fg, false),
            (
                rpad(&fmt_age(now.saturating_sub(c.last_ms) as f64 / 1000.0), 5) + "  ",
                t.dim,
                false,
            ),
            (c.error.clone(), t.red, false),
        ]);
        let refs: Vec<Seg> = segs.iter().map(|(a, b, c)| (a.as_str(), *b, *c)).collect();
        pen.segs(inner.x + 1, y, &refs, inner.right());
    }
}

pub fn power(pen: &mut Pen, app: &App, area: Rect) {
    let t = pen.t;
    let s = &app.session;
    let win = Win::of(app);
    if !app.config.power {
        pen.note(
            area,
            "Battery & thermal collection is off in settings (,)",
            t.dim,
        );
        return;
    }
    let r = rows(
        area,
        &[
            Constraint::Percentage(32),
            Constraint::Percentage(38),
            Constraint::Min(6),
        ],
    );
    let side_w = (area.width * 32 / 100).clamp(50, 66);
    let top = cols(r[0], &[Constraint::Min(40), Constraint::Length(side_w)]);
    let mid = cols(r[1], &[Constraint::Min(40), Constraint::Length(side_w)]);
    let h = &s.history;
    let drain_info = match (h.drain_ma.avg(), h.drain_ma.max()) {
        (Some(a), Some(m)) => format!("mA · drain avg {a:.0} · peak {m:.0}"),
        _ => "mA".into(),
    };
    let inner = pen.panel(
        top[0],
        &[("Battery current", t.yellow, true)],
        &[
            ("━ drain ", t.yellow, false),
            ("━ charging  ", t.aqua, false),
            (&drain_info, t.dim, false),
        ],
        None,
    );
    if let Some(u) = unsupported(app, Source::Battery) {
        pen.note(inner, &u, t.dim);
    } else {
        drain_chart(pen, app, inner, &win);
    }
    let inner = pen.panel(top[1], &[("Battery", t.yellow, true)], &[], None);
    if let Some(b) = &s.battery {
        let (x, mut y, right) = (inner.x + 1, inner.y, inner.right());
        let mut lines: Vec<Vec<(String, Color, bool)>> = vec![vec![
            ("level   ".into(), t.dim, false),
            (
                format!("{}%", b.level),
                if b.level > 20 { t.green } else { t.red },
                true,
            ),
            (
                format!(
                    "  {} · {}",
                    b.status,
                    if b.plugged == "none" {
                        "not plugged".into()
                    } else {
                        format!("plugged ({})", b.plugged)
                    }
                ),
                t.dim,
                false,
            ),
        ]];
        match b.drain_ma() {
            Some(ma) => lines.push(vec![
                ("now     ".into(), t.dim, false),
                (format!("{ma:.0} mA"), t.yellow, true),
                (
                    format!(" × {:.2} V = ", b.voltage_mv as f32 / 1000.0),
                    t.dim,
                    false,
                ),
                (
                    format!("{:.2} W", ma * b.voltage_mv as f32 / 1e6),
                    t.fg,
                    true,
                ),
            ]),
            None => lines.push(vec![
                ("current ".into(), t.dim, false),
                (
                    b.current_ma
                        .map_or("not reported".into(), |c| format!("{c} mA")),
                    t.fg,
                    false,
                ),
                (
                    format!("  {:.2} V", b.voltage_mv as f32 / 1000.0),
                    t.dim,
                    false,
                ),
            ]),
        }
        if let (Some(a), Some(m)) = (h.drain_ma.avg(), h.drain_ma.max()) {
            lines.push(vec![
                ("avg     ".into(), t.dim, false),
                (format!("{a:.0} mA"), t.fg, false),
                ("   peak ".into(), t.dim, false),
                (format!("{m:.0} mA"), t.orange, false),
            ]);
        }
        if let Some(mah) = s.used_mah() {
            lines.push(vec![
                ("session ".into(), t.dim, false),
                (
                    fmt_mah(mah),
                    if mah > 0.5 { t.orange } else { t.green },
                    true,
                ),
                (
                    format!(
                        " in {} (charge counter)",
                        fmt_dur(s.elapsed().as_secs_f64())
                    ),
                    t.dim,
                    false,
                ),
            ]);
            if let (Some(avg), Some(cap)) = (
                h.drain_ma.avg(),
                s.battery_first.as_ref().and_then(|f| f.charge_counter_uah),
            ) {
                if avg > 1.0 {
                    lines.push(vec![
                        ("        ".into(), t.dim, false),
                        (
                            format!(
                                "a full {:.0} mAh lasts ~{:.0} h at this rate",
                                cap as f32 / 1000.0 * 100.0 / b.level.max(1) as f32,
                                cap as f32 / 1000.0 / avg
                            ),
                            t.dim,
                            false,
                        ),
                    ]);
                }
            }
        }
        lines.push(vec![
            ("battery ".into(), t.dim, false),
            (
                app.config.temperature.format(b.temperature_c),
                t.temp(b.temperature_c),
                false,
            ),
            ("  health ".into(), t.dim, false),
            (
                b.health.clone(),
                if b.health == "good" {
                    t.green
                } else {
                    t.orange
                },
                false,
            ),
            (format!("  {}", b.technology), t.dim, false),
        ]);
        if b.plugged != "none" {
            lines.push(vec![]);
            lines.push(vec![
                ("⚠ ".into(), t.orange, false),
                ("Charging hides your app's drain".into(), t.fg, false),
            ]);
            lines.push(vec![
                ("  use Wi-Fi adb: ".into(), t.dim, false),
                ("adb tcpip 5555".into(), t.yellow, false),
            ]);
        }
        for line in lines {
            if y >= inner.bottom() {
                break;
            }
            let refs: Vec<Seg> = line.iter().map(|(a, b, c)| (a.as_str(), *b, *c)).collect();
            pen.segs(x, y, &refs, right);
            y += 1;
        }
    }
    let legend: Vec<(String, Color)> = h
        .temps
        .iter()
        .enumerate()
        .map(|(i, (n, _))| (format!("━ {n}  "), temp_color_at(t, i)))
        .collect();
    let mut legend_refs: Vec<Seg> = legend
        .iter()
        .map(|(a, c)| (a.as_str(), *c, false))
        .collect();
    let tu = app.config.temperature;
    legend_refs.push((tu.symbol(), t.dim, false));
    let inner = pen.panel(
        mid[0],
        &[("Temperatures", t.yellow, true)],
        &legend_refs,
        None,
    );
    if let Some(u) = unsupported(app, Source::Thermal) {
        pen.note(inner, &u, t.dim);
    } else if inner.height >= 4 {
        let chart_area = Rect::new(inner.x, inner.y, inner.width, inner.height - 2);
        let data: Vec<(Vec<f32>, Color)> = h
            .temps
            .iter()
            .enumerate()
            .map(|(i, (_, sr))| {
                (
                    tail(sr, win.n).into_iter().map(|c| tu.convert(c)).collect(),
                    temp_color_at(t, i),
                )
            })
            .collect();
        let all: Vec<f32> = data
            .iter()
            .flat_map(|(d, _)| d.iter().copied())
            .filter(|v| v.is_finite())
            .collect();
        let lo = all.iter().copied().fold(f32::INFINITY, f32::min);
        let hi = all.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let (ymin, ymax) = if lo.is_finite() {
            (
                (lo / 5.0).floor() * 5.0,
                ((hi / 5.0).ceil() * 5.0).max((lo / 5.0).floor() * 5.0 + 10.0),
            )
        } else {
            (20.0, 50.0)
        };
        let lines: Vec<ChartLine> = data
            .iter()
            .map(|(d, c)| ChartLine {
                data: d,
                color: *c,
                fill: false,
            })
            .collect();
        let ticks = [ymax, (ymax + ymin) / 2.0, ymin];
        let marks = win.marks();
        pen.chart(
            chart_area,
            &lines,
            &ChartOpts {
                ymin,
                ymax,
                ticks: &ticks,
                fmt: &|v| format!("{v:.0}"),
                label_w: 6,
                xlabels: win.xl(),
                markers: &marks,
                hlines: &[],
            },
        );
        let y = inner.bottom() - 1;
        pen.text(inner.x, y, "status", t.dim, false, inner.x + 6);
        let status = tail(&h.thermal_status, win.n);
        let ok = t.ok_dim();
        pen.strip(
            inner.x + 6,
            y,
            inner.width.saturating_sub(6),
            &status,
            &|v| {
                if !v.is_finite() {
                    None
                } else if v < 0.5 {
                    Some(ok)
                } else if v < 1.5 {
                    Some(t.orange)
                } else {
                    Some(t.red)
                }
            },
        );
    }
    let inner = pen.panel(
        mid[1],
        &[("Thermal now", t.yellow, true)],
        &[("dumpsys thermalservice", t.dim, false)],
        None,
    );
    if let Some(th) = &s.thermal {
        let (x, mut y, right) = (inner.x + 1, inner.y, inner.right());
        pen.text(x, y, "SENSOR   NOW     Δ", t.dim, false, right);
        y += 1;
        for (i, sensor) in th.sensors.iter().enumerate() {
            if y + 3 >= inner.bottom() {
                break;
            }
            let first = h
                .temps
                .iter()
                .find(|(n, _)| *n == sensor.name)
                .and_then(|(_, sr)| sr.first())
                .unwrap_or(sensor.celsius);
            let scale = if tu == crate::config::TempUnit::F {
                1.8
            } else {
                1.0
            };
            let d = (sensor.celsius - first) * scale;
            let c = t.temp(sensor.celsius);
            let end = pen.segs(
                x,
                y,
                &[
                    (
                        &pad(&sensor.name, 7),
                        temp_color_at(
                            t,
                            app.session
                                .history
                                .temps
                                .iter()
                                .position(|(n, _)| *n == sensor.name)
                                .unwrap_or(i),
                        ),
                        true,
                    ),
                    (
                        &rpad(&format!("{:.1}°", tu.convert(sensor.celsius)), 6),
                        c,
                        false,
                    ),
                    (
                        &rpad(&signed1(d), 6),
                        if d.abs() > 1.0 { t.orange } else { t.dim },
                        false,
                    ),
                    ("  ", t.dim, false),
                ],
                right,
            );
            pen.hbar(
                end,
                y,
                right.saturating_sub(end + 1),
                sensor.celsius - 20.0,
                40.0,
                c,
            );
            y += 1;
        }
        y += 1;
        if y < inner.bottom() {
            let c = if th.status == 0 {
                t.green
            } else if th.status == 1 {
                t.orange
            } else {
                t.red
            };
            pen.segs(
                x,
                y,
                &[
                    ("status ", t.dim, false),
                    (
                        &format!("{} ({})", thermal_level_name(th.status), th.status),
                        c,
                        true,
                    ),
                    (
                        &format!("  worst {}", thermal_level_name(s.thermal_worst)),
                        t.dim,
                        false,
                    ),
                ],
                right,
            );
            y += 1;
        }
        if y < inner.bottom() {
            let mut xx = pen.text(x, y, "levels ", t.dim, false, right);
            for (i, name) in THERMAL_LEVELS.iter().enumerate().take(5) {
                let on = i as u8 == th.status;
                xx = pen.text(xx, y, name, if on { t.orange } else { t.dim }, on, right) + 1;
            }
            y += 1;
        }
        if let Some(note) = throttle_note(app) {
            if y < inner.bottom() {
                pen.text(x, y, &format!("effect {note}"), t.fg, false, right);
            }
        }
    }
    let bot = cols(
        r[2],
        &[Constraint::Percentage(50), Constraint::Percentage(50)],
    );
    let inner = pen.panel(
        bot[0],
        &[("Doze & standby", t.yellow, true)],
        &[(
            &format!("last {}", win.xl[0].trim_start_matches('-')),
            t.dim,
            false,
        )],
        None,
    );
    if let Some(u) = unsupported(app, Source::Doze) {
        pen.note(inner, &u, t.dim);
    } else if let Some(d) = &s.doze {
        let strip_w = inner.width.saturating_sub(24);
        let gap = if inner.height >= 8 { 2 } else { 1 };
        let ok = t.ok_dim();
        let rowsd: [(&str, &crate::session::Series, String); 3] = [
            ("deep", &h.deep_idle, d.deep.clone()),
            ("light", &h.light_idle, d.light.clone()),
            (
                "bucket",
                &h.bucket,
                d.bucket.clone().unwrap_or_else(|| "–".into()),
            ),
        ];
        let mut y = inner.y;
        for (label, series, value) in rowsd {
            if y >= inner.bottom() {
                break;
            }
            pen.text(inner.x + 1, y, label, t.dim, false, inner.x + 9);
            pen.strip(inner.x + 9, y, strip_w, &tail(series, win.n), &|v| {
                if !v.is_finite() {
                    None
                } else if v < 0.5 {
                    Some(ok)
                } else if v < 1.5 {
                    Some(t.yellow)
                } else if v < 2.5 {
                    Some(t.orange)
                } else {
                    Some(t.red)
                }
            });
            pen.text(
                inner.x + 10 + strip_w,
                y,
                &value,
                if value == "ACTIVE" { t.green } else { t.yellow },
                true,
                inner.right(),
            );
            y += gap;
        }
        let tw = &app.tweaks;
        let next = crate::actions::next_bucket(tw.bucket_set.as_deref().or(d.bucket.as_deref()));
        let doze_label = if tw.doze {
            "leave forced Doze"
        } else {
            "force Doze"
        };
        let unplug_label = if tw.unplugged {
            "restore real power"
        } else {
            "fake unplug"
        };
        let bucket_label = if app.package().is_some() {
            format!("bucket → {next}")
        } else {
            "bucket (pick an app)".into()
        };
        let keys: Vec<Seg> = vec![
            ("z", t.yellow, true),
            (" ", t.dim, false),
            (doze_label, t.fg, false),
            ("   b", t.yellow, true),
            (" ", t.dim, false),
            (&bucket_label, t.fg, false),
            ("   u", t.yellow, true),
            (" ", t.dim, false),
            (unplug_label, t.fg, false),
        ];
        if y < inner.bottom() {
            pen.segs(inner.x + 1, y, &keys, inner.right());
            y += 1;
        }
        if tw.any() && y < inner.bottom() {
            pen.text(
                inner.x + 1,
                y,
                "the phone is changed; it is put back when you quit",
                t.orange,
                false,
                inner.right(),
            );
        }
    }
    let inner = pen.panel(
        bot[1],
        &[("Jobs & alarms", t.yellow, true)],
        &[(app.package().unwrap_or(""), t.dim, false)],
        None,
    );
    if let Some(u) = unsupported(app, Source::Jobs) {
        pen.note(inner, &u, t.dim);
    } else if app.package().is_none() {
        pen.note(
            inner,
            "Pick an app with p to see its jobs and alarms",
            t.dim,
        );
    } else if let Some(j) = &s.jobs {
        let (x, mut y, right) = (inner.x + 1, inner.y, inner.right());
        pen.segs(
            x,
            y,
            &[
                ("jobs ", t.dim, false),
                (&format!("{} scheduled", j.scheduled), t.fg, false),
                ("  running ", t.dim, false),
                (&j.running.to_string(), t.green, false),
                ("   alarms ", t.dim, false),
                (&j.alarms.map_or("–".into(), |a| a.to_string()), t.fg, false),
            ],
            right,
        );
        y += 2;
        for svc in &j.services {
            if y >= inner.bottom() {
                break;
            }
            pen.segs(
                x,
                y,
                &[
                    (&pad(&svc.service, 36), t.fg, false),
                    (&rpad(&svc.count.to_string(), 4), t.yellow, false),
                ],
                right,
            );
            y += 1;
        }
        if y < inner.bottom() && !j.recent.is_empty() {
            pen.text(x, y, "RECENT", t.dim, false, right);
            y += 1;
            for run in &j.recent {
                if y >= inner.bottom() {
                    break;
                }
                pen.segs(
                    x,
                    y,
                    &[
                        (&pad(&run.ago, 14), t.dim, false),
                        (
                            &pad(&run.event, 6),
                            if run.event == "start" { t.green } else { t.fg },
                            false,
                        ),
                        (&pad(&run.service, 30), t.fg, false),
                        (&run.reason, t.dim, false),
                    ],
                    right,
                );
                y += 1;
            }
        }
    } else {
        pen.note(inner, "Waiting for dumpsys jobscheduler…", t.dim);
    }
    let _ = interval_label;
}

fn fmt_mah(used: f32) -> String {
    if used.abs() < 0.5 {
        "0 mAh".into()
    } else {
        format!("{:+.0} mAh", -used)
    }
}

fn temp_color_at(t: &Theme, i: usize) -> Color {
    [
        t.red, t.orange, t.yellow, t.aqua, t.blue, t.green, t.purple, t.fg,
    ][i % 8]
}

#[cfg(test)]
mod tests {
    use super::{frame_buckets, pool_name, signed1};

    #[test]
    fn thread_pool_names() {
        assert_eq!(pool_name("binder:2728_2"), "binder");
        assert_eq!(pool_name("binder:2728_D"), "binder");
        assert_eq!(pool_name("HwBinder:2728_1"), "HwBinder");
        assert_eq!(pool_name("Lite Thread #7"), "Lite Thread #*");
        assert_eq!(pool_name("pool-3-thread-12"), "pool-*-thread-*");
        assert_eq!(pool_name("V8 DefaultWorke"), "V8 DefaultWorke");
        assert_eq!(pool_name("anrV34-1"), "anrV34-*");
        assert_eq!(pool_name("RenderThread"), "RenderThread");
        assert_eq!(signed1(-0.01), "·");
        assert_eq!(signed1(-3.26), "-3.3");
    }

    #[test]
    fn frame_buckets_are_never_empty_or_inverted() {
        for hz in [10.0f32, 24.0, 30.0, 48.0, 60.0, 90.0, 120.0, 144.0] {
            let edges = frame_buckets(1000.0 / hz);
            assert!(
                edges.iter().all(|(lo, hi, _)| hi > lo),
                "{hz} Hz: {edges:?}"
            );
        }
        let labels: Vec<String> = frame_buckets(1000.0 / 30.0)
            .into_iter()
            .map(|e| e.2)
            .collect();
        assert_eq!(labels, ["≤ 33.3 ms", "33–67", "67–100", "> 100"]);
        let labels: Vec<String> = frame_buckets(1000.0 / 120.0)
            .into_iter()
            .map(|e| e.2)
            .collect();
        assert_eq!(labels, ["≤ 8.3 ms", "8–17", "17–50", "50–100", "> 100"]);
    }
}
