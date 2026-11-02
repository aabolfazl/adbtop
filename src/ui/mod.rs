mod pages;
pub mod widgets;

use ratatui::layout::Rect;
use ratatui::Frame;

use crate::model::{thermal_level_name, AppState, Source, Support};
use crate::session::Series;
use crate::tui::{settings_rows, App, Popup, PAGES};
use widgets::*;

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let t = app.t();
    let mut pen = Pen {
        buf: frame.buffer_mut(),
        t,
        braille: app.braille(),
    };
    pen.clear(area);
    if area.width < 80 || area.height < 24 {
        pen.note(
            area,
            &format!(
                "terminal is {}×{}; make it at least 80×24",
                area.width, area.height
            ),
            t.yellow,
        );
        return;
    }
    tab_bar(&mut pen, app, Rect::new(area.x, area.y, area.width, 1));
    status_bar(&mut pen, app, Rect::new(area.x, area.y + 1, area.width, 1));
    footer(
        &mut pen,
        app,
        Rect::new(area.x, area.bottom() - 1, area.width, 1),
    );
    let body = Rect::new(area.x, area.y + 2, area.width, area.height - 3);
    if app.serial.is_none() {
        let msg = if app.devices.iter().any(|d| d.state == "device") {
            "Pick a device with d"
        } else if app.devices.is_empty() {
            "Waiting for an Android device (adb devices)…"
        } else {
            "The device is not ready: unlock it and accept the USB debugging prompt"
        };
        pen.note(body, msg, t.dim);
    } else {
        match app.page {
            0 => pages::overview(&mut pen, app, body),
            1 => pages::cpu(&mut pen, app, body),
            2 => pages::memory(&mut pen, app, body),
            3 => pages::frames(&mut pen, app, body),
            4 => pages::network(&mut pen, app, body),
            _ => pages::power(&mut pen, app, body),
        }
    }
    match app.popup {
        Popup::Device => device_popup(&mut pen, app, area),
        Popup::Package => package_popup(&mut pen, app, area),
        Popup::Settings => settings_popup(&mut pen, app, area),
        Popup::Marker => marker_popup(&mut pen, app, area),
        Popup::Help => help_popup(&mut pen, app, area),
        Popup::Actions => actions_popup(&mut pen, app, area),
        Popup::Console => console_popup(&mut pen, app, area),
        Popup::Prompt => {
            if !app.console.title.is_empty() {
                console_popup(&mut pen, app, area);
            }
            prompt(&mut pen, app, area);
        }
        Popup::None => {}
    }
}

fn actions_popup(pen: &mut Pen, app: &App, area: Rect) {
    let t = pen.t;
    let items = crate::actions::menu(
        &app.tweaks,
        app.session.doze.as_ref().and_then(|d| d.bucket.as_deref()),
    );
    let groups = items
        .iter()
        .map(|i| i.group)
        .fold(Vec::new(), |mut v: Vec<&str>, g| {
            if v.last() != Some(&g) {
                v.push(g);
            }
            v
        });
    let r = centered(area, 92, (items.len() + groups.len() * 2 + 3) as u16);
    pen.clear(r);
    let target = app.package().unwrap_or("no app selected");
    let inner = pen.panel(
        r,
        &[("Actions", t.yellow, true)],
        &[(
            target,
            if app.package().is_some() {
                t.aqua
            } else {
                t.dim
            },
            false,
        )],
        Some(t.yellow),
    );
    let mut y = inner.y;
    let mut last_group = "";
    for (i, item) in items.iter().enumerate() {
        if item.group != last_group {
            if !last_group.is_empty() {
                y += 1;
            }
            pen.text(inner.x + 2, y, item.group, t.dim, true, inner.right());
            y += 1;
            last_group = item.group;
        }
        if y >= inner.bottom() {
            break;
        }
        let enabled = !item.needs_app || app.package().is_some();
        let sel = i == app.cursor;
        if sel {
            pen.fill(Rect::new(inner.x, y, inner.width, 1), t.sel);
        }
        let key = item.key.to_string();
        pen.segs(
            inner.x + 2,
            y,
            &[
                (&key, if enabled { t.yellow } else { t.dim }, true),
                ("   ", t.dim, false),
                (
                    &pad(&item.label, 32),
                    if enabled { t.fg } else { t.dim },
                    sel,
                ),
                (
                    if enabled {
                        item.detail
                    } else {
                        "pick an app with p first"
                    },
                    t.dim,
                    false,
                ),
            ],
            inner.right(),
        );
        y += 1;
    }
    if inner.height > 0 {
        pen.segs(
            inner.x + 2,
            inner.bottom() - 1,
            &[
                ("press a key, or ↑↓ enter", t.dim, false),
                ("   esc", t.yellow, false),
                (" close", t.dim, false),
                (
                    "   changes to the phone are undone when you quit",
                    t.dim,
                    false,
                ),
            ],
            inner.right(),
        );
    }
}

fn console_popup(pen: &mut Pen, app: &App, area: Rect) {
    let t = pen.t;
    let c = &app.console;
    let r = Rect::new(
        area.x + 2,
        area.y + 2,
        area.width.saturating_sub(4),
        area.height.saturating_sub(5),
    );
    pen.clear(r);
    let status = if c.running {
        format!(
            "running {:.0}s · {} lines",
            c.started.map_or(0.0, |s| s.elapsed().as_secs_f32()),
            c.lines.len()
        )
    } else {
        let code = c
            .code
            .map_or("stopped".to_string(), |code| format!("exit {code}"));
        format!(
            "{code} · {:.1}s · {} lines",
            c.took.map_or(0.0, |d| d.as_secs_f32()),
            c.lines.len()
        )
    };
    let status_color = if c.running {
        t.yellow
    } else if c.code == Some(0) {
        t.green
    } else {
        t.red
    };
    let inner = pen.panel(
        r,
        &[(&trunc(&c.title, r.width as usize / 2), t.yellow, true)],
        &[(&status, status_color, false)],
        Some(t.aqua),
    );
    if inner.height < 2 {
        return;
    }
    let body = Rect::new(
        inner.x + 1,
        inner.y,
        inner.width.saturating_sub(2),
        inner.height - 1,
    );
    let visible = body.height as usize;
    let end = c.lines.len().saturating_sub(c.scroll);
    let start = end.saturating_sub(visible);
    for (i, (line, err)) in c.lines.iter().skip(start).take(end - start).enumerate() {
        pen.text(
            body.x,
            body.y + i as u16,
            line,
            if *err { t.orange } else { t.fg },
            false,
            body.right(),
        );
    }
    if c.lines.is_empty() && !c.running {
        pen.note(body, "(no output)", t.dim);
    }
    let hint: Vec<Seg> = if c.running {
        vec![
            ("esc", t.yellow, false),
            (" stop  ", t.dim, false),
            ("↑↓ pgup pgdn", t.yellow, false),
            (" scroll  ", t.dim, false),
            ("end", t.yellow, false),
            (" follow", t.dim, false),
        ]
    } else {
        vec![
            (":", t.yellow, false),
            (" new command  ", t.dim, false),
            ("↑↓ pgup pgdn home end", t.yellow, false),
            (" scroll  ", t.dim, false),
            ("esc", t.yellow, false),
            (" close", t.dim, false),
        ]
    };
    let x = pen.segs(inner.x + 1, inner.bottom() - 1, &hint, inner.right());
    if c.scroll > 0 {
        pen.text(
            x + 2,
            inner.bottom() - 1,
            &format!("↑ {} lines above the end", c.scroll),
            t.dim,
            false,
            inner.right(),
        );
    }
}

fn prompt(pen: &mut Pen, app: &App, area: Rect) {
    let t = pen.t;
    let r = Rect::new(area.x, area.bottom().saturating_sub(4), area.width, 3);
    pen.clear(r);
    let device = app.serial.clone().unwrap_or_else(|| "no device".into());
    let inner = pen.panel(
        r,
        &[("adb", t.yellow, true)],
        &[
            ("-s ", t.dim, false),
            (&device, t.purple, false),
            ("   enter", t.yellow, false),
            (" run  ", t.dim, false),
            ("↑↓", t.yellow, false),
            (" history  ", t.dim, false),
            ("esc", t.yellow, false),
            (" cancel", t.dim, false),
        ],
        Some(t.yellow),
    );
    let placeholder = "shell dumpsys battery · logcat -d -t 200 · shell am start -n …";
    let shown = if app.console.input.is_empty() {
        placeholder
    } else {
        app.console.input.as_str()
    };
    let x = pen.segs(
        inner.x + 1,
        inner.y,
        &[("adb › ", t.yellow, true)],
        inner.right(),
    );
    if app.console.input.is_empty() {
        pen.text(x, inner.y, "_", t.yellow, false, inner.right());
        pen.text(x + 2, inner.y, shown, t.dim, false, inner.right());
    } else {
        let end = pen.text(x, inner.y, shown, t.fg, false, inner.right());
        pen.text(end, inner.y, "_", t.yellow, false, inner.right());
    }
}

pub fn tail(s: &Series, n: usize) -> Vec<f32> {
    let len = s.len();
    let mut out = vec![f32::NAN; n.saturating_sub(len)];
    let start = len.saturating_sub(n);
    out.extend((start..len).map(|i| s.get(i)));
    out
}

pub fn markers(app: &App, n: usize) -> Vec<(usize, String)> {
    let ticks = app.session.ticks;
    app.session
        .markers
        .iter()
        .filter_map(|m| {
            let back = (ticks - m.tick) as usize;
            (back < n).then(|| (n - 1 - back, m.label.clone()))
        })
        .collect()
}

pub fn span_label(s: u32) -> String {
    match (s / 60, s % 60) {
        (0, sec) => format!("{sec}s"),
        (m, 0) => format!("{m}m"),
        (m, sec) => format!("{m}m{sec:02}s"),
    }
}

pub fn xlabels(window_s: u32) -> [String; 3] {
    [
        format!("-{}", span_label(window_s)),
        format!("-{}", span_label(window_s / 2)),
        "now".into(),
    ]
}

pub fn unsupported(app: &App, source: Source) -> Option<String> {
    match app.session.support(source) {
        Support::Unsupported(r) => Some(format!("Not supported on this device: {r}")),
        _ => None,
    }
}

pub fn app_gate(app: &App) -> Option<&'static str> {
    match app.session.app_state {
        AppState::NotSelected if app.package().is_none() => Some("Pick an app with p to see this"),
        AppState::NotRunning => Some("The app is not running; start it on the phone"),
        _ => None,
    }
}

fn tab_bar(pen: &mut Pen, app: &App, r: Rect) {
    let t = pen.t;
    pen.fill(r, t.hi);
    let mut x = pen.text(r.x + 1, r.y, "◉ adbtop", t.yellow, true, r.right()) + 2;
    for (i, name) in PAGES.iter().enumerate() {
        let label = format!(" {} {} ", i + 1, name);
        let on = i == app.page;
        let start = x;
        x = pen.text(x, r.y, &label, if on { t.bg } else { t.dim }, on, r.right());
        if on {
            for xx in start..x {
                if let Some(c) = pen.cell(xx, r.y) {
                    c.set_bg(t.yellow);
                }
            }
        }
        x += 1;
    }
    let caps = app.session.caps.as_ref();
    let model = caps
        .map(|c| c.model.clone())
        .or_else(|| {
            app.devices
                .iter()
                .find(|d| Some(&d.serial) == app.serial.as_ref())
                .map(|d| d.model.clone())
        })
        .unwrap_or_else(|| "no device".into());
    let android = caps
        .map(|c| format!("  Android {}  ", c.release))
        .unwrap_or_else(|| "  ".into());
    let pkg = app.package().unwrap_or("whole device").to_string();
    let pid = app
        .session
        .fast
        .as_ref()
        .and_then(|f| f.app.as_ref())
        .map(|a| format!("  pid {}", a.pid))
        .unwrap_or_default();
    pen.rsegs(
        r.right() - 1,
        r.y,
        &[
            (&model, t.purple, false),
            (&android, t.dim, false),
            (&pkg, t.aqua, false),
            (&pid, t.dim, false),
        ],
        x + 2,
    );
}

fn status_bar(pen: &mut Pen, app: &App, r: Rect) {
    let t = pen.t;
    let s = &app.session;
    let sep = ("  │  ", t.gray, false);
    let elapsed = fmt_dur(s.elapsed().as_secs_f64());
    let rec: (&str, ratatui::style::Color, bool) = if app.paused {
        ("❚❚ PAUSED ", t.yellow, true)
    } else {
        ("● REC ", t.green, true)
    };
    let ticks = format!(
        "  #{} @{}",
        s.ticks,
        interval_label(s.interval.as_secs_f32())
    );
    let window = xlabels(app.window_s)[0].trim_start_matches('-').to_string();
    let (mode_label, mode_value, mode_color) = match app.package() {
        Some(p) => ("PACKAGE ", p.to_string(), t.aqua),
        None => ("", "DEVICE-WIDE".to_string(), t.purple),
    };
    let capture = if s.capture.snapshot.is_some() {
        let open = s
            .capture
            .snapshot
            .as_ref()
            .and_then(|x| x.stats)
            .map_or(0, |st| st.open_tcp + st.open_udp);
        (format!("● {open} open"), t.green)
    } else if app.capture_busy || app.capture_on {
        ("starting…".to_string(), t.yellow)
    } else if s.support(Source::Capture).is_unsupported() {
        ("not installed".to_string(), t.dim)
    } else {
        ("off (c)".to_string(), t.dim)
    };
    let mut segs: Vec<(String, ratatui::style::Color, bool)> = vec![
        (rec.0.to_string(), rec.1, rec.2),
        (elapsed, t.fg, false),
        (ticks, t.dim, false),
        (sep.0.into(), sep.1, false),
        ("window ".into(), t.dim, false),
        (window, t.fg, false),
        (sep.0.into(), sep.1, false),
        (mode_label.into(), t.dim, false),
        (mode_value, mode_color, true),
    ];
    if s.app_state == AppState::NotRunning {
        segs.push(("  NOT RUNNING".into(), t.red, true));
    }
    segs.extend([
        (sep.0.into(), sep.1, false),
        ("CAPTURE ".into(), t.dim, false),
        (capture.0, capture.1, false),
    ]);
    if let Some(th) = &s.thermal {
        let c = if th.status == 0 {
            t.green
        } else if th.status == 1 {
            t.orange
        } else {
            t.red
        };
        segs.extend([
            (sep.0.into(), sep.1, false),
            ("THERMAL ".into(), t.dim, false),
            (thermal_level_name(th.status).into(), c, true),
        ]);
    }
    if let Some(b) = &s.battery {
        segs.extend([
            (sep.0.into(), sep.1, false),
            ("BAT ".into(), t.dim, false),
            (
                format!("{}% ", b.level),
                if b.level > 20 { t.green } else { t.red },
                false,
            ),
        ]);
        let faked = app.tweaks.unplugged || app.tweaks.doze;
        match b.drain_ma().filter(|_| !faked) {
            Some(ma) => segs.push((format!("↓{ma:.0} mA"), t.yellow, false)),
            None if faked => segs.push(("fake unplugged".into(), t.dim, false)),
            None => segs.push((
                if b.plugged == "none" {
                    b.status.clone()
                } else {
                    format!("⚡ {}", b.plugged)
                },
                t.dim,
                false,
            )),
        }
    }
    let mut badges: Vec<String> = Vec::new();
    if app.tweaks.doze {
        badges.push("DOZE FORCED".into());
    } else if app.tweaks.unplugged {
        badges.push("FAKE UNPLUG".into());
    }
    if let Some(b) = &app.tweaks.bucket_set {
        badges.push(format!("BUCKET {}", b.to_uppercase()));
    }
    if !badges.is_empty() {
        segs.extend([
            (sep.0.into(), sep.1, false),
            (format!("⚠ {}", badges.join(" · ")), t.orange, true),
        ]);
    }
    if app.console.running && app.popup != Popup::Console {
        segs.extend([
            (sep.0.into(), sep.1, false),
            ("adb running".into(), t.yellow, false),
        ]);
    }
    if let Some(m) = s.markers.last() {
        let ago = s.elapsed().as_secs_f64() - m.secs;
        segs.extend([
            (sep.0.into(), sep.1, false),
            (format!("▾{}", m.label), t.yellow, true),
            (format!(" {} ago", fmt_age(ago)), t.dim, false),
        ]);
    }
    let refs: Vec<Seg> = segs.iter().map(|(a, b, c)| (a.as_str(), *b, *c)).collect();
    pen.segs(r.x + 1, r.y, &refs, r.right());
}

pub fn interval_label(s: f32) -> String {
    if s < 1.0 {
        format!("{:.0}ms", s * 1000.0)
    } else if s.fract() == 0.0 {
        format!("{s:.0}s")
    } else {
        format!("{s:.1}s")
    }
}

fn footer(pen: &mut Pen, app: &App, r: Rect) {
    let t = pen.t;
    if let Some(toast) = &app.toast {
        pen.text(
            r.x + 1,
            r.y,
            &trunc(&toast.text, r.width as usize - 2),
            if toast.error { t.red } else { t.aqua },
            false,
            r.right(),
        );
        return;
    }
    if app.filtering {
        pen.segs(
            r.x + 1,
            r.y,
            &[
                ("filter: ", t.dim, false),
                (&app.table_filter, t.fg, false),
                ("_", t.yellow, false),
                ("   enter keep · esc clear", t.dim, false),
            ],
            r.right(),
        );
        return;
    }
    if let Some((src, e)) = &app.session.last_error {
        pen.text(
            r.x + 1,
            r.y,
            &trunc(&format!("[{}] {e}", src.label()), r.width as usize - 2),
            t.red,
            false,
            r.right(),
        );
        return;
    }
    if !app.config.key_hints {
        pen.segs(
            r.x + 1,
            r.y,
            &[("?", t.yellow, false), (" keys", t.dim, false)],
            r.right(),
        );
        return;
    }
    let page_keys: &[(&str, &str)] = match app.page {
        1 => &[
            ("s", "sort"),
            ("g", "pools"),
            ("/", "filter"),
            ("↑↓", "scroll"),
        ],
        3 => &[("r", "reset stats")],
        4 => &[("s", "sort"), ("/", "filter"), ("↑↓", "scroll")],
        5 => &[("z", "doze"), ("b", "bucket"), ("u", "fake unplug")],
        _ => &[],
    };
    let common: &[(&str, &str)] = &[
        ("1-6", "pages"),
        ("a", "actions"),
        (":", "adb"),
        ("m", "mark"),
        ("w", "window"),
        ("space", "pause"),
        ("p", "app"),
        ("d", "device"),
        ("c", "capture"),
        ("e", "export"),
        ("t", "theme"),
        (",", "settings"),
        ("?", "help"),
        ("q", "quit"),
    ];
    let mut x = r.x + 1;
    for (k, v) in page_keys {
        x = pen.segs(
            x,
            r.y,
            &[
                (k, t.yellow, false),
                (" ", t.dim, false),
                (v, t.dim, false),
                ("  ", t.dim, false),
            ],
            r.right(),
        );
    }
    if !page_keys.is_empty() {
        x = pen.text(x, r.y, "│  ", t.gray, false, r.right());
    }
    if !app.table_filter.is_empty() {
        x = pen.segs(
            x,
            r.y,
            &[
                ("filter ", t.dim, false),
                (&app.table_filter, t.yellow, false),
                ("  ", t.dim, false),
            ],
            r.right(),
        );
    }
    for (k, v) in common {
        x = pen.segs(
            x,
            r.y,
            &[
                (k, t.yellow, false),
                (" ", t.dim, false),
                (v, t.dim, false),
                ("  ", t.dim, false),
            ],
            r.right(),
        );
    }
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width.saturating_sub(4));
    let h = h.min(area.height.saturating_sub(2));
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

fn device_popup(pen: &mut Pen, app: &App, area: Rect) {
    let t = pen.t;
    let r = centered(area, 70, (app.devices.len() as u16 + 4).max(7));
    pen.clear(r);
    let inner = pen.panel(
        r,
        &[("Select device", t.yellow, true)],
        &[
            ("enter", t.yellow, false),
            (" pick  ", t.dim, false),
            ("esc", t.yellow, false),
            (" close", t.dim, false),
        ],
        Some(t.purple),
    );
    if app.devices.is_empty() {
        pen.note(
            inner,
            "No devices. Connect one and enable USB debugging.",
            t.orange,
        );
        return;
    }
    for (i, d) in app.devices.iter().enumerate() {
        let y = inner.y + i as u16;
        if y >= inner.bottom() {
            break;
        }
        let sel = i == app.cursor;
        if sel {
            pen.fill(Rect::new(inner.x, y, inner.width, 1), t.sel);
        }
        let ready = d.state == "device";
        let current = app.serial.as_ref() == Some(&d.serial);
        pen.segs(
            inner.x + 1,
            y,
            &[
                (if current { "● " } else { "  " }, t.green, false),
                (&pad(&d.model, 24), if ready { t.fg } else { t.dim }, sel),
                (&pad(&d.serial, 26), t.dim, false),
                (&d.state, if ready { t.green } else { t.red }, false),
            ],
            inner.right(),
        );
    }
}

fn package_popup(pen: &mut Pen, app: &App, area: Rect) {
    let t = pen.t;
    let r = centered(area, 76, area.height * 3 / 4);
    pen.clear(r);
    let list = app.filtered_packages();
    let count = format!("{} running", list.len());
    let inner = pen.panel(
        r,
        &[("Select app", t.yellow, true)],
        &[(&count, t.dim, false)],
        Some(t.aqua),
    );
    if inner.height < 4 {
        return;
    }
    let placeholder = if app.filter.is_empty() {
        "type to filter…"
    } else {
        app.filter.as_str()
    };
    pen.segs(
        inner.x + 1,
        inner.y,
        &[
            ("› ", t.yellow, true),
            (
                placeholder,
                if app.filter.is_empty() { t.dim } else { t.fg },
                false,
            ),
            ("_", t.yellow, false),
        ],
        inner.right(),
    );
    for x in inner.x..inner.right() {
        pen.put(x, inner.y + 1, "─", t.dim2);
    }
    let rows = (inner.height - 2) as usize;
    let total = list.len() + 1;
    let offset = (app.cursor + 1)
        .saturating_sub(rows)
        .min(total.saturating_sub(rows));
    for i in 0..rows.min(total - offset) {
        let idx = offset + i;
        let y = inner.y + 2 + i as u16;
        let sel = idx == app.cursor;
        if sel {
            pen.fill(Rect::new(inner.x, y, inner.width, 1), t.sel);
        }
        let (name, color) = if idx == 0 {
            ("(whole device)".to_string(), t.purple)
        } else {
            (list[idx - 1].clone(), t.fg)
        };
        let current = if idx == 0 {
            app.package().is_none()
        } else {
            app.package() == Some(name.as_str())
        };
        pen.segs(
            inner.x + 1,
            y,
            &[
                (if current { "● " } else { "  " }, t.green, false),
                (&name, color, sel),
            ],
            inner.right(),
        );
    }
}

fn marker_popup(pen: &mut Pen, app: &App, area: Rect) {
    let t = pen.t;
    let r = centered(area, 56, 5);
    pen.clear(r);
    let inner = pen.panel(
        r,
        &[("Add marker", t.yellow, true)],
        &[
            ("enter", t.yellow, false),
            (" add  ", t.dim, false),
            ("esc", t.yellow, false),
            (" cancel", t.dim, false),
        ],
        Some(t.yellow),
    );
    let default = format!("m{}", app.session.markers.len() + 1);
    let shown = if app.input.is_empty() {
        default.as_str()
    } else {
        app.input.as_str()
    };
    pen.segs(
        inner.x + 1,
        inner.y + 1,
        &[
            ("label › ", t.dim, false),
            (
                shown,
                if app.input.is_empty() { t.dim } else { t.fg },
                false,
            ),
            ("_", t.yellow, false),
        ],
        inner.right(),
    );
}

fn help_popup(pen: &mut Pen, _app: &App, area: Rect) {
    let t = pen.t;
    let r = centered(area, 82, 30);
    pen.clear(r);
    let inner = pen.panel(
        r,
        &[("Keys", t.yellow, true)],
        &[("any key closes", t.dim, false)],
        Some(t.yellow),
    );
    let rows: &[(&str, &str)] = &[
        ("1-6  tab  shift+tab", "switch pages"),
        ("esc", "back to Overview, or clear the table filter"),
        ("p / x", "pick an app / measure the whole device"),
        ("d", "pick a device"),
        (
            "a",
            "actions: launch, cold restart, force stop, doze, bucket…",
        ),
        (":", "run any adb command on this device; output streams in"),
        (
            "z / b / u",
            "Power page: force Doze, next standby bucket, fake unplug",
        ),
        ("r", "Frames page: reset frame stats"),
        ("c", "start or stop the network capture agent"),
        ("m / M", "drop a marker now / with a label"),
        ("w", "chart window: 1, 2, 5, 15, 30 min"),
        ("space", "pause"),
        ("s", "sort the table on this page"),
        ("g", "group threads into pools (CPU page)"),
        ("/", "filter the table on this page"),
        ("↑ ↓ pgup pgdn home", "scroll the table"),
        ("e", "export a JSON report of this session"),
        ("t / T", "next / previous theme"),
        (",", "settings (saved to ~/.config/adbtop)"),
        ("q  ctrl+c", "quit (stops the capture agent)"),
        ("", ""),
        ("headless", "adbtop record --help"),
    ];
    for (i, (k, v)) in rows.iter().enumerate() {
        let y = inner.y + 1 + i as u16;
        if y >= inner.bottom() {
            break;
        }
        pen.segs(
            inner.x + 2,
            y,
            &[(&pad(k, 22), t.yellow, false), (v, t.fg, false)],
            inner.right(),
        );
    }
}

fn settings_popup(pen: &mut Pen, app: &App, area: Rect) {
    let t = pen.t;
    let rows = settings_rows(app);
    let sections = rows.iter().filter(|r| !r.section.is_empty()).count() as u16;
    let needed = rows.len() as u16 + sections * 2 + 4;
    let r = centered(area, 156, needed.max(app.themes.len() as u16 + 10));
    pen.clear(r);
    let path = crate::config::path()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let saved = format!("saved to {path}");
    let inner = pen.panel(
        r,
        &[("Settings", t.yellow, true)],
        &[(&saved, t.dim, false)],
        Some(t.yellow),
    );
    if inner.height < 10 {
        return;
    }
    let gap = inner.height >= rows.len() as u16 + sections * 2 + 2;
    let left_w = (inner.width * 58 / 100).max(50).min(inner.width);
    let right_x = inner.x + left_w + 2;
    let label_x = inner.x + 3;
    let value_x = inner.x + 24;
    let mut y = inner.y + if gap { 1 } else { 0 };
    for (i, row) in rows.iter().enumerate() {
        if !row.section.is_empty() {
            if i > 0 && gap {
                y += 1;
            }
            pen.text(label_x, y, row.section, t.dim, true, inner.x + left_w);
            y += 1;
        }
        if y >= inner.bottom() - 2 {
            break;
        }
        let sel = i == app.cursor;
        if sel {
            pen.fill(Rect::new(inner.x, y, left_w, 1), t.sel);
            pen.text(label_x - 2, y, "▸", t.yellow, true, inner.x + left_w);
        }
        pen.text(label_x, y, row.label, t.fg, false, value_x);
        let mut x = value_x;
        if sel {
            x = pen.text(x, y, "◂ ", t.yellow, false, inner.x + left_w);
        }
        x = pen.text(
            x,
            y,
            &row.value,
            if sel { t.yellow } else { t.aqua },
            sel,
            inner.x + left_w,
        );
        if sel {
            x = pen.text(x, y, " ▸", t.yellow, false, inner.x + left_w);
        }
        if !row.note.is_empty() {
            pen.text(x + 2, y, row.note, t.dim, false, inner.x + left_w);
        }
        y += 1;
    }
    if right_x + 30 < inner.right() {
        for yy in inner.y..inner.bottom() {
            pen.put(right_x - 2, yy, "│", t.dim2);
        }
        pen.text(right_x, inner.y + 1, "THEMES", t.dim, true, inner.right());
        pen.text(
            right_x + 20,
            inner.y + 1,
            "bg     text accents",
            t.dim,
            false,
            inner.right(),
        );
        let room = inner.height.saturating_sub(7);
        let step = if app.themes.len() as u16 * 2 <= room {
            2
        } else {
            1
        };
        let first = if app.themes.len() as u16 > room {
            (app.theme as u16 + 1).saturating_sub(room) as usize
        } else {
            0
        };
        for (i, th) in app.themes.iter().enumerate().skip(first) {
            let yy = inner.y + 2 + (i - first) as u16 * step;
            if yy >= inner.bottom() - 4 {
                break;
            }
            let on = i == app.theme;
            if on {
                pen.fill(
                    Rect::new(right_x - 1, yy, inner.right() - right_x + 1, 1),
                    t.sel,
                );
            }
            pen.segs(
                right_x,
                yy,
                &[
                    (
                        if on { "● " } else { "○ " },
                        if on { t.yellow } else { t.dim },
                        on,
                    ),
                    (&trunc(&th.name, 17), if on { t.yellow } else { t.fg }, on),
                ],
                right_x + 20,
            );
            let mut sx = right_x + 20;
            for (k, c) in th.swatches().iter().enumerate() {
                for _ in 0..3 {
                    pen.bg(sx, yy, *c);
                    sx += 1;
                }
                if k == 1 || k == 2 {
                    sx += 1;
                }
            }
        }
        let hint_y = inner.bottom() - 4;
        pen.text(
            right_x,
            hint_y,
            "The screen behind shows the selected theme.",
            t.dim,
            false,
            inner.right(),
        );
        pen.segs(
            right_x,
            hint_y + 1,
            &[
                ("Add your own: ", t.dim, false),
                ("[themes.mine]", t.purple, false),
                (" in config.toml", t.dim, false),
            ],
            inner.right(),
        );
        pen.segs(
            right_x,
            hint_y + 2,
            &[
                ("with ", t.dim, false),
                ("base = \"nord\"", t.fg, false),
                (" and any of bg, fg, accent…", t.dim, false),
            ],
            inner.right(),
        );
    }
    pen.segs(
        inner.x + 2,
        inner.bottom() - 1,
        &[
            ("↑↓", t.yellow, false),
            (" move   ", t.dim, false),
            ("←→ enter", t.yellow, false),
            (" change   ", t.dim, false),
            ("r", t.yellow, false),
            (" defaults   ", t.dim, false),
            ("esc", t.yellow, false),
            (" save and close", t.dim, false),
        ],
        inner.right(),
    );
}
