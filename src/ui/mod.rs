use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, Paragraph, Sparkline},
    Frame,
};

use crate::app::{App, FocusedPanel, Popup, Tab};

const BG: Color = Color::Rgb(40, 40, 40);
const BG_HIGHLIGHT: Color = Color::Rgb(60, 56, 54);
const FG: Color = Color::Rgb(235, 219, 178);
const FG_DIM: Color = Color::Rgb(146, 131, 116);
const RED: Color = Color::Rgb(251, 73, 52);
const GREEN: Color = Color::Rgb(184, 187, 38);
const YELLOW: Color = Color::Rgb(250, 189, 47);
const BLUE: Color = Color::Rgb(131, 165, 152);
const PURPLE: Color = Color::Rgb(211, 134, 155);
const AQUA: Color = Color::Rgb(142, 192, 124);
const ORANGE: Color = Color::Rgb(254, 128, 25);

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();

    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(10),
            Constraint::Length(1),
        ])
        .split(area);

    draw_tab_bar(frame, app, main_chunks[0]);

    match app.current_tab {
        Tab::Monitor => draw_monitor_tab(frame, app, main_chunks[1]),
    }

    draw_footer(frame, app, main_chunks[2]);

    match &app.popup {
        Popup::SelectDevice => draw_device_popup(frame, app),
        Popup::SelectPackage => draw_package_popup(frame, app),
        Popup::None => {}
    }
}

fn draw_tab_bar(frame: &mut Frame, app: &App, area: Rect) {
    let tabs: Vec<Span> = Tab::all()
        .iter()
        .flat_map(|tab| {
            let is_active = *tab == app.current_tab;
            let style = if is_active {
                Style::default().fg(BG).bg(YELLOW).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(FG_DIM)
            };

            vec![
                Span::styled(format!(" {} ", tab.key()), Style::default().fg(if is_active { YELLOW } else { FG_DIM })),
                Span::styled(format!("{} ", tab.name()), style),
                Span::styled(" ", Style::default()),
            ]
        })
        .collect();

    let mut line_spans = vec![Span::styled(" ", Style::default())];
    line_spans.extend(tabs);

    let device_name = app.current_device()
        .map(|d| d.model.as_str())
        .unwrap_or("--");
    let pkg_name = app.package.as_deref()
        .map(|p| p.split('.').last().unwrap_or(p))
        .unwrap_or("--");

    line_spans.extend(vec![
        Span::styled(" | ", Style::default().fg(FG_DIM)),
        Span::styled(device_name, Style::default().fg(PURPLE)),
        Span::styled(" | ", Style::default().fg(FG_DIM)),
        Span::styled(pkg_name, Style::default().fg(AQUA)),
    ]);

    frame.render_widget(
        Paragraph::new(Line::from(line_spans)).style(Style::default().bg(BG_HIGHLIGHT)),
        area,
    );
}

fn draw_monitor_tab(frame: &mut Frame, app: &App, area: Rect) {
    if app.focused_panel != FocusedPanel::None {
        match app.focused_panel {
            FocusedPanel::Threads => draw_threads_fullscreen(frame, app, area),
            FocusedPanel::None => draw_monitor_panels(frame, app, area),
        }
        return;
    }

    draw_monitor_panels(frame, app, area);
}

fn draw_monitor_panels(frame: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(6),
            Constraint::Length(4),
            Constraint::Length(5),
            Constraint::Min(6),
        ])
        .split(area);

    draw_status_bar(frame, app, chunks[0]);
    draw_cpu_stats_row(frame, app, chunks[1]);
    draw_sparklines_row(frame, app, chunks[2]);
    draw_memory_battery_doze_row(frame, app, chunks[3]);
    draw_threads_jobs_network_row(frame, app, chunks[4]);
}

fn draw_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    let elapsed = app.elapsed();
    let status = if app.paused { "PAUSED" } else { "REC" };
    let status_color = if app.paused { YELLOW } else { GREEN };

    let (mode_text, mode_color) = match app.tracking_mode {
        crate::runner::TrackingMode::Device => ("DEVICE", PURPLE),
        crate::runner::TrackingMode::Package => ("PACKAGE", AQUA),
    };

    let mut spans = vec![
        Span::styled(format!(" {} ", status), Style::default().fg(status_color)),
        Span::styled(
            format!("{}:{:02}", elapsed.as_secs() / 60, elapsed.as_secs() % 60),
            Style::default().fg(FG),
        ),
        Span::styled(" | ", Style::default().fg(FG_DIM)),
        Span::styled(format!("{}", mode_text), Style::default().fg(mode_color).add_modifier(Modifier::BOLD)),
        Span::styled(" | ", Style::default().fg(FG_DIM)),
        Span::styled(format!("#{}", app.sample_count), Style::default().fg(BLUE)),
        Span::styled(" | ", Style::default().fg(FG_DIM)),
        Span::styled(format!("Bat {}%", app.metrics.battery_level), Style::default().fg(GREEN)),
    ];

    if app.package.is_some() && app.metrics.threads.is_empty() {
        spans.push(Span::styled(" | ", Style::default().fg(FG_DIM)));
        spans.push(Span::styled("NOT RUNNING", Style::default().fg(RED).add_modifier(Modifier::BOLD)));
    }

    let line = Line::from(spans);

    frame.render_widget(Paragraph::new(line), area);
}

fn styled_block(title: &str) -> Block<'_> {
    Block::default()
        .title(format!(" {} ", title))
        .title_style(Style::default().fg(YELLOW).add_modifier(Modifier::BOLD))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(FG_DIM))
}

fn draw_cpu_stats_row(frame: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
        .split(area);

    draw_cpu_panel(frame, app, chunks[0]);
    draw_stats_panel(frame, app, chunks[1]);
}

fn draw_cpu_panel(frame: &mut Frame, app: &App, area: Rect) {
    let title = if app.package.is_some() { "System CPU" } else { "CPU" };
    let block = styled_block(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let num_cores = app.metrics.cpu_cores.len();
    if num_cores == 0 || inner.width == 0 || inner.height == 0 {
        return;
    }

    let min_col_width = 14u16;
    let max_cols = (inner.width / min_col_width).max(1) as usize;

    let rows_available = inner.height as usize;
    let cols = ((num_cores + rows_available - 1) / rows_available).clamp(1, max_cols);

    let col_width = inner.width / cols as u16;
    if col_width == 0 {
        return;
    }

    for (i, core) in app.metrics.cpu_cores.iter().enumerate() {
        let col = i % cols;
        let row = i / cols;
        if row as u16 >= inner.height {
            break;
        }

        let x = inner.x + (col as u16 * col_width);
        let y = inner.y + row as u16;

        let color = usage_color(core.usage_percent);
        let bar_chars = ((col_width as f32 - 14.0).max(0.0) * (core.usage_percent / 100.0)) as usize;
        let empty_chars = ((col_width as f32 - 14.0).max(0.0) as usize).saturating_sub(bar_chars);

        let line = Line::from(vec![
            Span::styled(format!("{} ", core.id), Style::default().fg(FG_DIM)),
            Span::styled("[", Style::default().fg(FG_DIM)),
            Span::styled("|".repeat(bar_chars), Style::default().fg(color)),
            Span::styled("|".repeat(empty_chars), Style::default().fg(Color::Rgb(60, 56, 54))),
            Span::styled("]", Style::default().fg(FG_DIM)),
            Span::styled(format!("{:3.0}%", core.usage_percent), Style::default().fg(color)),
        ]);

        frame.render_widget(Paragraph::new(line), Rect::new(x, y, col_width, 1));
    }
}

fn draw_stats_panel(frame: &mut Frame, app: &App, area: Rect) {
    let block = styled_block("Stats");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let num_cores = app.metrics.cpu_cores.len();
    let total_cpu: f32 = if num_cores > 0 {
        app.metrics.cpu_cores.iter().map(|c| c.usage_percent).sum::<f32>() / num_cores as f32
    } else { 0.0 };
    let avg_freq: u32 = if num_cores > 0 {
        app.metrics.cpu_cores.iter().map(|c| c.frequency_mhz).sum::<u32>() / num_cores as u32
    } else { 0 };

    let stats = if app.package.is_some() {
        vec![
            ("APP", format!("{:5.1}%", app.metrics.app_cpu_total), usage_color(app.metrics.app_cpu_total)),
            ("SYS", format!("{:5.1}%", total_cpu), usage_color(total_cpu)),
            ("AVG", format!("{:5.1}%", app.metrics.app_cpu_avg), PURPLE),
            ("FRQ", format!("{:4}M", avg_freq), BLUE),
        ]
    } else {
        vec![
            ("SYS", format!("{:5.1}%", total_cpu), usage_color(total_cpu)),
            ("FRQ", format!("{:4}M", avg_freq), BLUE),
        ]
    };

    for (i, (label, value, color)) in stats.iter().enumerate() {
        if (i as u16) < inner.height {
            let style = if i == 0 && app.package.is_some() {
                Style::default().fg(*color).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(*color)
            };
            let line = Line::from(vec![
                Span::styled(format!("{} ", label), Style::default().fg(FG_DIM)),
                Span::styled(value, style),
            ]);
            frame.render_widget(Paragraph::new(line), Rect::new(inner.x, inner.y + i as u16, inner.width, 1));
        }
    }
}

fn draw_sparklines_row(frame: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(33), Constraint::Percentage(34), Constraint::Percentage(33)])
        .split(area);

    let cpu_label = if app.package.is_some() { "App CPU %" } else { "CPU %" };
    let block = styled_block(cpu_label);
    let inner = block.inner(chunks[0]);
    frame.render_widget(block, chunks[0]);
    if !app.cpu_history.is_empty() {
        let data: Vec<u64> = app.cpu_history.iter().map(|v| (*v as u64).min(100)).collect();
        frame.render_widget(Sparkline::default().data(&data).max(100).style(Style::default().fg(GREEN)), inner);
    }

    let block = styled_block("Mem %");
    let inner = block.inner(chunks[1]);
    frame.render_widget(block, chunks[1]);
    let mem = &app.memory_data;
    let mem_pct = if mem.total_kb > 0 {
        ((mem.total_kb - mem.available_kb) as f32 / mem.total_kb as f32 * 100.0) as u64
    } else { 0 };
    let mem_data = vec![mem_pct; 1];
    frame.render_widget(
        Sparkline::default().data(&mem_data).max(100).style(Style::default().fg(PURPLE)),
        inner
    );

    let block = styled_block("Temp");
    let inner = block.inner(chunks[2]);
    frame.render_widget(block, chunks[2]);
    if !app.temp_history.is_empty() {
        let data: Vec<u64> = app.temp_history.iter().map(|v| *v as u64).collect();
        frame.render_widget(Sparkline::default().data(&data).max(60).style(Style::default().fg(ORANGE)), inner);
    }
}

fn draw_memory_battery_doze_row(frame: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(35), Constraint::Percentage(35), Constraint::Percentage(30)])
        .split(area);

    draw_memory_panel(frame, app, chunks[0]);
    draw_battery_panel(frame, app, chunks[1]);
    draw_doze_panel(frame, app, chunks[2]);
}

fn draw_memory_panel(frame: &mut Frame, app: &App, area: Rect) {
    let block = styled_block("Memory");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mem = &app.memory_data;

    if app.package.is_some() && mem.app_pss_kb > 0 {
        let stats = [
            ("PSS", format!("{:5} MB", mem.app_pss_kb / 1024), AQUA),
            ("Heap", format!("{:5} MB", mem.app_heap_size_kb / 1024), PURPLE),
            ("Alloc", format!("{:5} MB", mem.app_heap_alloc_kb / 1024), GREEN),
        ];

        for (i, (label, value, color)) in stats.iter().enumerate() {
            if (i as u16) < inner.height {
                let line = Line::from(vec![
                    Span::styled(format!("{:<5}", label), Style::default().fg(FG_DIM)),
                    Span::styled(value, Style::default().fg(*color)),
                ]);
                frame.render_widget(Paragraph::new(line), Rect::new(inner.x, inner.y + i as u16, inner.width, 1));
            }
        }
    } else if app.package.is_some() {
        frame.render_widget(
            Paragraph::new("Process not running").style(Style::default().fg(FG_DIM)),
            inner
        );
    } else {
        let total_mb = mem.total_kb / 1024;
        let used_mb = (mem.total_kb - mem.available_kb) / 1024;
        let used_pct = if mem.total_kb > 0 { (used_mb as f32 / total_mb as f32) * 100.0 } else { 0.0 };

        let stats = [
            ("Used", format!("{:5} MB ({:.0}%)", used_mb, used_pct), usage_color(used_pct)),
            ("Free", format!("{:5} MB", mem.available_kb / 1024), GREEN),
            ("Swap", format!("{:5} MB", (mem.swap_total_kb - mem.swap_free_kb) / 1024), PURPLE),
        ];

        for (i, (label, value, color)) in stats.iter().enumerate() {
            if (i as u16) < inner.height {
                let line = Line::from(vec![
                    Span::styled(format!("{:<5}", label), Style::default().fg(FG_DIM)),
                    Span::styled(value, Style::default().fg(*color)),
                ]);
                frame.render_widget(Paragraph::new(line), Rect::new(inner.x, inner.y + i as u16, inner.width, 1));
            }
        }
    }
}

fn draw_battery_panel(frame: &mut Frame, app: &App, area: Rect) {
    let block = styled_block("Battery");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let bat = &app.battery_data;
    let level_color = if bat.level > 50 { GREEN } else if bat.level > 20 { YELLOW } else { RED };
    let status_color = if bat.status == "Charging" { AQUA } else { FG_DIM };

    let stats = [
        (format!("{}%", bat.level), level_color),
        (format!("{:.1}\u{00B0}C", bat.temperature), temp_color(bat.temperature)),
        (bat.status.clone(), status_color),
    ];

    for (i, (value, color)) in stats.iter().enumerate() {
        if (i as u16) < inner.height {
            frame.render_widget(
                Paragraph::new(value.as_str()).style(Style::default().fg(*color)),
                Rect::new(inner.x, inner.y + i as u16, inner.width, 1)
            );
        }
    }
}

fn draw_thermal_panel(frame: &mut Frame, app: &App, area: Rect) {
    let block = styled_block("Thermal");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let thermal = &app.metrics.thermal;
    let temps = [("AP", thermal.ap), ("SK", thermal.skin), ("BT", thermal.battery)];
    for (i, (name, val)) in temps.iter().enumerate() {
        if (i as u16) < inner.height {
            let line = Line::from(vec![
                Span::styled(format!("{} ", name), Style::default().fg(FG_DIM)),
                Span::styled(format!("{:4.1}\u{00B0}C", val), Style::default().fg(temp_color(*val))),
            ]);
            frame.render_widget(Paragraph::new(line), Rect::new(inner.x, inner.y + i as u16, inner.width, 1));
        }
    }
}

fn draw_network_panel(frame: &mut Frame, app: &App, area: Rect) {
    let block = styled_block("Network");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 { return; }

    let net = &app.network_data;

    let format_bytes = |b: u64| -> String {
        if b > 1_000_000_000 { format!("{:.1}G", b as f64 / 1_000_000_000.0) }
        else if b > 1_000_000 { format!("{:.1}M", b as f64 / 1_000_000.0) }
        else if b > 1_000 { format!("{:.1}K", b as f64 / 1_000.0) }
        else { format!("{}B", b) }
    };

    let text_lines: usize = 2;
    if app.package.is_some() && (net.app_rx_bytes > 0 || net.app_tx_bytes > 0) {
        let stats = [
            (format!("RX {}", format_bytes(net.app_rx_bytes)), AQUA),
            (format!("TX {}", format_bytes(net.app_tx_bytes)), PURPLE),
        ];
        for (i, (value, color)) in stats.iter().enumerate() {
            if (i as u16) < inner.height {
                frame.render_widget(
                    Paragraph::new(value.as_str()).style(Style::default().fg(*color)),
                    Rect::new(inner.x, inner.y + i as u16, inner.width, 1),
                );
            }
        }
    } else {
        let wifi_icon = if net.wifi_connected { "\u{25CF}" } else { "\u{25CB}" };
        let mobile_icon = if net.mobile_connected { "\u{25CF}" } else { "\u{25CB}" };
        let stats = [
            (format!("{} Wi \u{2193}{} \u{2191}{}", wifi_icon, format_bytes(net.wifi_rx_bytes), format_bytes(net.wifi_tx_bytes)), if net.wifi_connected { AQUA } else { FG_DIM }),
            (format!("{} Mo \u{2193}{} \u{2191}{}", mobile_icon, format_bytes(net.mobile_rx_bytes), format_bytes(net.mobile_tx_bytes)), if net.mobile_connected { PURPLE } else { FG_DIM }),
        ];
        for (i, (value, color)) in stats.iter().enumerate() {
            if (i as u16) < inner.height {
                frame.render_widget(
                    Paragraph::new(value.as_str()).style(Style::default().fg(*color)),
                    Rect::new(inner.x, inner.y + i as u16, inner.width, 1),
                );
            }
        }
    }

    let sparkline_height = (inner.height as usize).saturating_sub(text_lines);
    if sparkline_height > 0 && !app.net_rx_history.is_empty() {
        let spark_area = Rect::new(
            inner.x,
            inner.y + text_lines as u16,
            inner.width,
            sparkline_height as u16,
        );
        let max_val = app.net_rx_history.iter().copied().max().unwrap_or(1).max(1);
        frame.render_widget(
            Sparkline::default()
                .data(&app.net_rx_history)
                .max(max_val)
                .style(Style::default().fg(AQUA)),
            spark_area,
        );
    }
}

fn draw_doze_panel(frame: &mut Frame, app: &App, area: Rect) {
    let block = styled_block("Power");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let doze = &app.doze_data;
    let idle_color = if doze.device_idle_state == "ACTIVE" { GREEN } else { YELLOW };
    let bucket_color = match doze.app_standby_bucket.as_str() {
        "ACTIVE" => GREEN,
        "WORKING_SET" => AQUA,
        "FREQUENT" => YELLOW,
        "RARE" | "RESTRICTED" => RED,
        _ => FG_DIM,
    };

    let stats = [
        (format!("Idle: {}", doze.device_idle_state), idle_color),
        (format!("Bucket: {}", doze.app_standby_bucket), bucket_color),
    ];

    for (i, (value, color)) in stats.iter().enumerate() {
        if (i as u16) < inner.height {
            frame.render_widget(
                Paragraph::new(value.as_str()).style(Style::default().fg(*color)),
                Rect::new(inner.x, inner.y + i as u16, inner.width, 1)
            );
        }
    }
}

fn draw_threads_jobs_network_row(frame: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    draw_threads_compact(frame, app, chunks[0]);
    draw_right_panels(frame, app, chunks[1]);
}

fn draw_right_panels(frame: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(area);

    let top_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[0]);

    draw_jobs_panel(frame, app, top_chunks[0]);
    draw_alarms_panel(frame, app, top_chunks[1]);

    let bottom_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(chunks[1]);

    draw_network_panel(frame, app, bottom_chunks[0]);
    draw_thermal_panel(frame, app, bottom_chunks[1]);
}

fn draw_jobs_panel(frame: &mut Frame, app: &App, area: Rect) {
    let block = styled_block("Jobs");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 { return; }

    let jobs = &app.jobs_data;

    let summary = if app.package.is_some() {
        format!("App P:{} R:{}", jobs.app_pending, jobs.app_running)
    } else {
        format!("Reg:{} Run:{}", jobs.total_pending, jobs.total_running)
    };
    frame.render_widget(
        Paragraph::new(summary).style(Style::default().fg(AQUA)),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );

    if !jobs.top_job_packages.is_empty() {
        let max_rows = (inner.height as usize).saturating_sub(1);
        for (i, (pkg, count)) in jobs.top_job_packages.iter().take(max_rows).enumerate() {
            let y = inner.y + 1 + i as u16;
            let is_running = jobs.running_packages.iter().any(|r| r == pkg);
            let marker = if is_running { "\u{25CF} " } else { "  " };
            let short = pkg.split('.').last().unwrap_or(pkg);
            let w = inner.width as usize;
            let count_str = format!("{:>3} ", count);
            let name_max = w.saturating_sub(count_str.len() + marker.len());
            let name = truncate_str(short, name_max);
            let line = Line::from(vec![
                Span::styled(marker, Style::default().fg(GREEN)),
                Span::styled(count_str, Style::default().fg(YELLOW)),
                Span::styled(name, Style::default().fg(if is_running { FG } else { FG_DIM })),
            ]);
            frame.render_widget(Paragraph::new(line), Rect::new(inner.x, y, inner.width, 1));
        }
    }
}

fn draw_alarms_panel(frame: &mut Frame, app: &App, area: Rect) {
    let block = styled_block("Alarms");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 { return; }

    let alarms = &app.alarms_data;

    let summary = if app.package.is_some() {
        format!("App: {}", alarms.app_pending)
    } else {
        format!("Pending: {}", alarms.total_pending)
    };
    frame.render_widget(
        Paragraph::new(summary).style(Style::default().fg(YELLOW)),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );

    if !alarms.top_alarm_packages.is_empty() {
        let max_rows = (inner.height as usize).saturating_sub(1);
        for (i, (pkg, count)) in alarms.top_alarm_packages.iter().take(max_rows).enumerate() {
            let y = inner.y + 1 + i as u16;
            let short = pkg.split('.').last().unwrap_or(pkg);
            let w = inner.width as usize;
            let count_str = format!("{:>3} ", count);
            let name_max = w.saturating_sub(count_str.len());
            let name = truncate_str(short, name_max);
            let line = Line::from(vec![
                Span::styled(count_str, Style::default().fg(ORANGE)),
                Span::styled(name, Style::default().fg(FG_DIM)),
            ]);
            frame.render_widget(Paragraph::new(line), Rect::new(inner.x, y, inner.width, 1));
        }
    }
}

fn draw_threads_compact(frame: &mut Frame, app: &App, area: Rect) {
    let total = app.metrics.threads.len();
    let title = format!("Threads [{}] [f] fullscreen", total);
    let block = styled_block(&title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 { return; }

    let header = " PID     CPU   MEM   TIME       NAME";
    frame.render_widget(Paragraph::new(header).style(Style::default().fg(FG_DIM)), Rect::new(inner.x, inner.y, inner.width, 1));

    let visible_rows = (inner.height as usize).saturating_sub(1);
    for (i, thread) in app.metrics.threads.iter().take(visible_rows).enumerate() {
        let y = inner.y + 1 + i as u16;
        if y >= inner.y + inner.height { break; }

        let line = Line::from(vec![
            Span::styled(format!(" {:<7}", thread.pid), Style::default().fg(FG_DIM)),
            Span::styled(format!("{:5.1}%", thread.cpu_percent), Style::default().fg(usage_color(thread.cpu_percent))),
            Span::styled(format!("{:5.1}%", thread.mem_percent), Style::default().fg(PURPLE)),
            Span::styled(format!(" {:<10}", thread.time), Style::default().fg(FG_DIM)),
            Span::styled(truncate_str(&thread.name, (inner.width as usize).saturating_sub(32)), Style::default().fg(FG)),
        ]);
        frame.render_widget(Paragraph::new(line), Rect::new(inner.x, y, inner.width, 1));
    }
}

fn draw_threads_fullscreen(frame: &mut Frame, app: &App, area: Rect) {
    let total = app.metrics.threads.len();
    let title = format!("Threads [{}] [f] exit fullscreen", total);
    let block = styled_block(&title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 { return; }

    let header = " PID       CPU%    MEM%    TIME         NAME";
    frame.render_widget(Paragraph::new(header).style(Style::default().fg(FG_DIM)), Rect::new(inner.x, inner.y, inner.width, 1));

    let visible_rows = (inner.height as usize).saturating_sub(1);
    for (i, thread) in app.metrics.threads.iter().take(visible_rows).enumerate() {
        let y = inner.y + 1 + i as u16;
        if y >= inner.y + inner.height { break; }

        let line = Line::from(vec![
            Span::styled(format!(" {:<9}", thread.pid), Style::default().fg(FG_DIM)),
            Span::styled(format!("{:6.1}%  ", thread.cpu_percent), Style::default().fg(usage_color(thread.cpu_percent))),
            Span::styled(format!("{:6.1}%  ", thread.mem_percent), Style::default().fg(PURPLE)),
            Span::styled(format!("{:<12} ", thread.time), Style::default().fg(FG_DIM)),
            Span::styled(&thread.name, Style::default().fg(FG)),
        ]);
        frame.render_widget(Paragraph::new(line), Rect::new(inner.x, y, inner.width, 1));
    }
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let shortcuts = "q:quit  space:pause  d:device  p:package  x:clear pkg  f:fullscreen";

    let mut spans = vec![
        Span::styled(format!(" {} ", shortcuts), Style::default().fg(FG_DIM)),
    ];

    if let Some(ref msg) = app.status_message {
        spans.push(Span::styled(" | ", Style::default().fg(FG_DIM)));
        let msg_color = if msg.starts_with("Error") { RED } else { GREEN };
        spans.push(Span::styled(msg.as_str(), Style::default().fg(msg_color)));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_device_popup(frame: &mut Frame, app: &App) {
    let popup = centered_rect(50, 50, frame.area());
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .title(" Select Device ")
        .title_style(Style::default().fg(YELLOW).add_modifier(Modifier::BOLD))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(PURPLE))
        .style(Style::default().bg(BG));

    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    if app.devices.is_empty() {
        frame.render_widget(Paragraph::new("No devices...").style(Style::default().fg(ORANGE)), inner);
        return;
    }

    let items: Vec<ListItem> = app.devices.iter().enumerate().map(|(i, d)| {
        let style = if i == app.selected_device { Style::default().fg(GREEN).add_modifier(Modifier::BOLD) } else { Style::default().fg(FG_DIM) };
        let prefix = if i == app.selected_device { "> " } else { "  " };
        ListItem::new(format!("{}{} ({})", prefix, d.model, d.serial)).style(style)
    }).collect();

    frame.render_widget(List::new(items), inner);
}

fn draw_package_popup(frame: &mut Frame, app: &App) {
    let popup = centered_rect(60, 70, frame.area());
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .title(" Select Package ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(AQUA))
        .style(Style::default().bg(BG));

    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    if inner.height < 3 { return; }

    let search_text = if app.package_search.is_empty() { "filter..." } else { &app.package_search };
    let line = Line::from(vec![
        Span::styled(" > ", Style::default().fg(YELLOW)),
        Span::styled(search_text, Style::default().fg(if app.package_search.is_empty() { FG_DIM } else { FG })),
        Span::styled("_", Style::default().fg(YELLOW)),
    ]);
    frame.render_widget(Paragraph::new(line), Rect::new(inner.x, inner.y, inner.width, 1));
    frame.render_widget(Paragraph::new("-".repeat(inner.width as usize)).style(Style::default().fg(FG_DIM)), Rect::new(inner.x, inner.y + 1, inner.width, 1));

    let list_area = Rect::new(inner.x, inner.y + 2, inner.width, inner.height.saturating_sub(2));
    if app.filtered_packages.is_empty() {
        frame.render_widget(Paragraph::new("No matches").style(Style::default().fg(ORANGE)), list_area);
        return;
    }

    let visible = (list_area.height as usize).min(20);
    let items: Vec<ListItem> = app.filtered_packages.iter().take(visible).enumerate().map(|(i, &idx)| {
        let pkg = &app.packages[idx];
        let style = if i == app.selected_package { Style::default().fg(GREEN).add_modifier(Modifier::BOLD) } else { Style::default().fg(FG_DIM) };
        ListItem::new(format!("{}{}", if i == app.selected_package { "> " } else { "  " }, pkg)).style(style)
    }).collect();

    frame.render_widget(List::new(items), list_area);
}

fn usage_color(percent: f32) -> Color {
    if percent > 80.0 { RED } else if percent > 50.0 { ORANGE } else if percent > 20.0 { GREEN } else { FG_DIM }
}

fn temp_color(temp: f32) -> Color {
    if temp > 45.0 { RED } else if temp > 38.0 { ORANGE } else { GREEN }
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.len() <= max { s.to_string() } else if max > 3 { format!("{}...", &s[..max-3]) } else { s[..max].to_string() }
}

fn centered_rect(px: u16, py: u16, r: Rect) -> Rect {
    let v = Layout::default().direction(Direction::Vertical)
        .constraints([Constraint::Percentage((100-py)/2), Constraint::Percentage(py), Constraint::Percentage((100-py)/2)])
        .split(r);
    Layout::default().direction(Direction::Horizontal)
        .constraints([Constraint::Percentage((100-px)/2), Constraint::Percentage(px), Constraint::Percentage((100-px)/2)])
        .split(v[1])[1]
}
