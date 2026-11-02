use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Widget};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::theme::Theme;

pub type Seg<'a> = (&'a str, Color, bool);

pub struct Pen<'a> {
    pub buf: &'a mut Buffer,
    pub t: &'a Theme,
    pub braille: bool,
}

const BITS: [[u8; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];
const VBLOCKS: [&str; 9] = [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];

pub fn style(fg: Color, bold: bool) -> Style {
    let s = Style::default().fg(fg);
    if bold {
        s.add_modifier(Modifier::BOLD)
    } else {
        s
    }
}

impl<'a> Pen<'a> {
    fn inside(&self, x: u16, y: u16) -> bool {
        let a = self.buf.area;
        x >= a.x && y >= a.y && x < a.right() && y < a.bottom()
    }

    pub fn text(&mut self, x: u16, y: u16, s: &str, fg: Color, bold: bool, max_x: u16) -> u16 {
        let max_x = max_x.min(self.buf.area.right());
        if x >= max_x || !self.inside(x, y) {
            return x;
        }
        self.buf
            .set_stringn(x, y, s, (max_x - x) as usize, style(fg, bold))
            .0
    }

    pub fn segs(&mut self, x: u16, y: u16, segs: &[Seg], max_x: u16) -> u16 {
        let mut x = x;
        for (s, c, b) in segs {
            x = self.text(x, y, s, *c, *b, max_x);
        }
        x
    }

    pub fn rsegs(&mut self, right: u16, y: u16, segs: &[Seg], min_x: u16) -> u16 {
        let w: u16 = segs.iter().map(|(s, _, _)| width(s)).sum();
        let x = right.saturating_sub(w).max(min_x);
        self.segs(x, y, segs, right)
    }

    pub fn cell(&mut self, x: u16, y: u16) -> Option<&mut ratatui::buffer::Cell> {
        if self.inside(x, y) {
            self.buf.cell_mut((x, y))
        } else {
            None
        }
    }

    pub fn put(&mut self, x: u16, y: u16, sym: &str, fg: Color) {
        if let Some(c) = self.cell(x, y) {
            c.set_symbol(sym).set_fg(fg);
        }
    }

    pub fn bg(&mut self, x: u16, y: u16, bg: Color) {
        if let Some(c) = self.cell(x, y) {
            c.set_symbol(" ").set_bg(bg);
        }
    }

    pub fn fill(&mut self, r: Rect, bg: Color) {
        for y in r.top()..r.bottom() {
            for x in r.left()..r.right() {
                if let Some(c) = self.cell(x, y) {
                    c.set_bg(bg);
                }
            }
        }
    }

    pub fn clear(&mut self, r: Rect) {
        let (fg, bg) = (self.t.fg, self.t.bg);
        for y in r.top()..r.bottom() {
            for x in r.left()..r.right() {
                if let Some(c) = self.cell(x, y) {
                    c.reset();
                    c.set_fg(fg).set_bg(bg);
                }
            }
        }
    }

    pub fn panel(
        &mut self,
        area: Rect,
        title: &[Seg],
        right: &[Seg],
        border: Option<Color>,
    ) -> Rect {
        if area.width < 4 || area.height < 3 {
            return Rect::default();
        }
        let t = self.t;
        let bc = border.unwrap_or(t.gray);
        let mut left = vec![Span::styled(" ", style(bc, false))];
        left.extend(
            title
                .iter()
                .map(|(s, c, b)| Span::styled(s.to_string(), style(*c, *b))),
        );
        left.push(Span::styled(" ", style(bc, false)));
        let mut block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(bc))
            .title(Line::from(left));
        if right.iter().any(|(s, _, _)| !s.is_empty()) {
            let mut spans = vec![Span::raw(" ")];
            spans.extend(
                right
                    .iter()
                    .map(|(s, c, b)| Span::styled(s.to_string(), style(*c, *b))),
            );
            spans.push(Span::raw(" "));
            block = block.title_top(Line::from(spans).right_aligned());
        }
        let inner = block.inner(area);
        block.render(area, self.buf);
        inner
    }

    pub fn note(&mut self, area: Rect, msg: &str, color: Color) {
        if area.height == 0 || area.width == 0 {
            return;
        }
        let y = area.y + area.height / 2;
        let mut lines: Vec<String> = Vec::new();
        let mut cur = String::new();
        for word in msg.split(' ') {
            if width(&cur) + width(word) + 1 > area.width.saturating_sub(2) && !cur.is_empty() {
                lines.push(std::mem::take(&mut cur));
            }
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(word);
        }
        lines.push(cur);
        let start = y.saturating_sub(lines.len() as u16 / 2).max(area.y);
        for (i, l) in lines.iter().enumerate() {
            let yy = start + i as u16;
            if yy >= area.bottom() {
                break;
            }
            let x = area.x + area.width.saturating_sub(width(l)) / 2;
            self.text(x, yy, l, color, false, area.right());
        }
    }

    pub fn hbar(&mut self, x: u16, y: u16, w: u16, v: f32, max: f32, color: Color) {
        let filled = if v.is_finite() && max > 0.0 {
            ((v / max).clamp(0.0, 1.0) * w as f32).round() as u16
        } else {
            0
        };
        let filled = if v > 0.0 && filled == 0 { 1 } else { filled };
        let track = self.t.track;
        for i in 0..w {
            self.put(x + i, y, "▆", if i < filled { color } else { track });
        }
    }

    pub fn spark(
        &mut self,
        x: u16,
        y: u16,
        w: u16,
        data: &[f32],
        max: f32,
        color: &dyn Fn(f32) -> Color,
    ) {
        let cols = resample(data, w as usize);
        let max = if max > 0.0 {
            max
        } else {
            cols.iter()
                .copied()
                .filter(|v| v.is_finite())
                .fold(0.0, f32::max)
                .max(1e-6)
        };
        for (i, v) in cols.iter().enumerate() {
            let xx = x + i as u16;
            if !v.is_finite() {
                continue;
            }
            let n = ((v / max).clamp(0.0, 1.0) * 8.0).round() as usize;
            if n == 0 {
                self.put(xx, y, "·", self.t.dim2);
            } else {
                self.put(xx, y, VBLOCKS[n], color(*v));
            }
        }
    }

    pub fn heat_row(&mut self, x: u16, y: u16, w: u16, data: &[f32]) {
        for (i, v) in resample(data, w as usize).iter().enumerate() {
            let c = if v.is_finite() {
                self.t.heat(*v)
            } else {
                self.t.bg
            };
            self.bg(x + i as u16, y, c);
        }
    }

    pub fn strip(
        &mut self,
        x: u16,
        y: u16,
        w: u16,
        data: &[f32],
        color: &dyn Fn(f32) -> Option<Color>,
    ) {
        for (i, v) in resample(data, w as usize).iter().enumerate() {
            if let Some(c) = color(*v) {
                self.bg(x + i as u16, y, c);
            }
        }
    }

    pub fn vbars(
        &mut self,
        area: Rect,
        data: &[f32],
        max: f32,
        color: &dyn Fn(f32) -> Color,
        over: Option<Color>,
    ) {
        let n = data.len().min(area.width as usize);
        let start_x = area.right() - n as u16;
        let skip = data.len() - n;
        for (i, &v) in data.iter().skip(skip).enumerate() {
            if !v.is_finite() || v <= 0.0 {
                continue;
            }
            let x = start_x + i as u16;
            let mut e = ((v.min(max) / max) * area.height as f32 * 8.0).round() as i32;
            if e == 0 {
                e = 1;
            }
            let c = color(v);
            for r in 0..area.height {
                let rem = e - r as i32 * 8;
                if rem <= 0 {
                    break;
                }
                self.put(x, area.bottom() - 1 - r, VBLOCKS[rem.min(8) as usize], c);
            }
            if v > max {
                if let Some(oc) = over {
                    if let Some(cell) = self.cell(x, area.y) {
                        cell.set_symbol("▲").set_fg(oc);
                    }
                }
            }
        }
    }

    pub fn stacked(&mut self, area: Rect, layers: &[(&[f32], Color)], max: f32) {
        let w = area.width as usize;
        let data: Vec<&[f32]> = layers.iter().map(|(d, _)| *d).collect();
        let picks = pick_peaks(&data, w);
        let cols: Vec<Vec<f32>> = data
            .iter()
            .map(|d| {
                picks
                    .iter()
                    .map(|p| p.and_then(|j| d.get(j).copied()).unwrap_or(f32::NAN))
                    .collect()
            })
            .collect();
        let bg = self.t.bg;
        for i in 0..w {
            let mut cum = 0f32;
            let mut segs: Vec<(f32, f32, Color)> = Vec::with_capacity(layers.len());
            let mut any = false;
            for (col, (_, color)) in cols.iter().zip(layers.iter()) {
                let v = col[i];
                if v.is_finite() {
                    any = true;
                    segs.push((cum, cum + v, *color));
                    cum += v;
                }
            }
            if !any {
                continue;
            }
            let x = area.x + i as u16;
            for r in 0..area.height {
                let lo = r as f32 / area.height as f32 * max;
                let hi = (r + 1) as f32 / area.height as f32 * max;
                if lo >= cum {
                    break;
                }
                let Some(bottom) = segs.iter().find(|s| lo + 1e-6 >= s.0 && lo < s.1) else {
                    break;
                };
                let y = area.bottom() - 1 - r;
                if bottom.1 >= hi {
                    self.put(x, y, "█", bottom.2);
                    continue;
                }
                let overlap =
                    |s: &(f32, f32, Color), a: f32, b: f32| (s.1.min(b) - s.0.max(a)).max(0.0);
                let widest = |a: f32, b: f32| {
                    segs.iter()
                        .max_by(|p, q| {
                            overlap(p, a, b)
                                .partial_cmp(&overlap(q, a, b))
                                .unwrap_or(std::cmp::Ordering::Equal)
                        })
                        .map(|s| s.2)
                };
                if cum < hi {
                    let n = (((cum - lo) / (hi - lo)) * 8.0).round().clamp(1.0, 8.0) as usize;
                    let fg = widest(lo, cum).unwrap_or(bottom.2);
                    if let Some(c) = self.cell(x, y) {
                        c.set_symbol(VBLOCKS[n]).set_fg(fg).set_bg(bg);
                    }
                    continue;
                }
                let n = (((bottom.1 - lo) / (hi - lo)) * 8.0)
                    .round()
                    .clamp(1.0, 7.0) as usize;
                let top = widest(bottom.1, hi).unwrap_or(bg);
                if let Some(c) = self.cell(x, y) {
                    c.set_symbol(VBLOCKS[n]).set_fg(bottom.2).set_bg(top);
                }
            }
        }
    }

    pub fn chart(&mut self, area: Rect, lines: &[ChartLine], o: &ChartOpts) -> Rect {
        if area.width < 6 || area.height < 2 {
            return Rect::default();
        }
        let t = self.t;
        let label_w = o.label_w;
        let x_axis = o.xlabels.is_some() as u16;
        let plot = Rect::new(
            area.x + label_w,
            area.y,
            area.width.saturating_sub(label_w),
            area.height.saturating_sub(x_axis),
        );
        if plot.width < 2 || plot.height < 1 {
            return Rect::default();
        }
        let span = (o.ymax - o.ymin).max(1e-6);
        let row_of = |v: f32| -> u16 {
            (((1.0 - (v - o.ymin) / span) * (plot.height - 1) as f32)
                .round()
                .clamp(0.0, (plot.height - 1) as f32)) as u16
        };
        if label_w > 0 {
            for r in 0..plot.height {
                self.put(plot.x - 1, plot.y + r, "│", t.dim2);
            }
            let mut used: Vec<u16> = Vec::with_capacity(o.ticks.len());
            for &v in o.ticks {
                let r = row_of(v);
                let label = (o.fmt)(v);
                let lw = width(&label);
                if used.contains(&r) || lw + 1 > label_w {
                    continue;
                }
                used.push(r);
                self.text(
                    plot.x.saturating_sub(1 + lw),
                    plot.y + r,
                    &label,
                    t.dim,
                    false,
                    plot.x - 1,
                );
                self.put(plot.x - 1, plot.y + r, "┤", t.dim2);
                if v > o.ymin {
                    for i in 0..plot.width {
                        self.put(plot.x + i, plot.y + r, "┄", t.track);
                    }
                }
            }
        }
        for &(v, color) in o.hlines {
            let r = row_of(v);
            for i in 0..plot.width {
                self.put(plot.x + i, plot.y + r, "┄", color);
            }
        }
        let n = lines.iter().map(|l| l.data.len()).max().unwrap_or(0);
        let marker_cols: Vec<(u16, &str)> = o
            .markers
            .iter()
            .filter(|(idx, _)| n >= 2 && *idx < n)
            .map(|&(idx, label)| {
                (
                    plot.x
                        + ((idx as f32 / (n - 1) as f32) * (plot.width - 1) as f32).round() as u16,
                    label,
                )
            })
            .collect();
        for &(col, _) in &marker_cols {
            for r in 0..plot.height {
                self.put(col, plot.y + r, "┊", t.yellow);
            }
        }
        if self.braille {
            self.braille_lines(plot, lines, o.ymin, span, n);
        } else {
            self.block_lines(plot, lines, o.ymin, span);
        }
        for &(col, label) in &marker_cols {
            let text = format!("▾{label}");
            if col + width(&text) > plot.right() {
                let x = col.saturating_sub(width(label));
                self.text(
                    x,
                    plot.y,
                    &format!("{label}▾"),
                    t.yellow,
                    true,
                    plot.right(),
                );
            } else {
                self.text(col, plot.y, &text, t.yellow, true, plot.right());
            }
        }
        if let Some([l, m, r]) = o.xlabels {
            let y = plot.bottom();
            let fits_ends = plot.width >= width(l) + width(r) + 2;
            let fits_mid = plot.width >= width(l) + width(m) + width(r) + 4;
            if fits_ends {
                self.text(plot.x, y, l, t.dim, false, plot.right());
            }
            if !m.is_empty() && fits_mid {
                self.text(
                    plot.x + plot.width.saturating_sub(width(m)) / 2,
                    y,
                    m,
                    t.dim,
                    false,
                    plot.right(),
                );
            }
            self.text(
                plot.right().saturating_sub(width(r)),
                y,
                r,
                t.dim,
                false,
                plot.right(),
            );
        }
        plot
    }

    fn braille_lines(&mut self, plot: Rect, lines: &[ChartLine], ymin: f32, span: f32, n: usize) {
        let (cw, ch) = (plot.width as usize, plot.height as usize);
        let (pw, ph) = (cw * 2, ch * 4);
        let mut bits = vec![0u8; cw * ch];
        let mut colors = vec![Color::Reset; cw * ch];
        let mut by_line = vec![false; cw * ch];
        let to_py = |v: f32| -> i32 {
            (((1.0 - (v - ymin) / span) * (ph - 1) as f32).round() as i32).clamp(0, ph as i32 - 1)
        };
        let zero = to_py(0f32.max(ymin));
        let ordered = lines
            .iter()
            .filter(|l| l.fill)
            .chain(lines.iter().filter(|l| !l.fill));
        for line in ordered {
            if line.data.is_empty() || n < 1 {
                continue;
            }
            let len = line.data.len();
            let mut prev: Option<(i32, i32)> = None;
            for px in 0..pw {
                let range = if len > pw {
                    let a = px * len / pw;
                    let b = ((px + 1) * len / pw).max(a + 1).min(len);
                    min_max(&line.data[a..b])
                } else {
                    let tpos = if pw > 1 {
                        px as f32 * (len.max(2) - 1) as f32 / (pw - 1) as f32
                    } else {
                        0.0
                    };
                    let v = sample(line.data, tpos);
                    v.is_finite().then_some((v, v))
                };
                let Some((lo, hi)) = range else {
                    prev = None;
                    continue;
                };
                let (top, bottom) = (to_py(hi), to_py(lo));
                let (a, b) = if line.fill {
                    (top.min(zero), bottom.max(zero))
                } else {
                    match prev {
                        Some((pt, _)) if pt > bottom => (top, pt),
                        Some((_, pb)) if pb < top => (pb, bottom),
                        _ => (top, bottom),
                    }
                };
                for yy in a..=b {
                    let idx = (yy as usize / 4) * cw + px / 2;
                    bits[idx] |= BITS[px & 1][yy as usize & 3];
                    if !line.fill || !by_line[idx] {
                        colors[idx] = line.color;
                    }
                    if !line.fill {
                        by_line[idx] = true;
                    }
                }
                prev = Some((top, bottom));
            }
        }
        for (i, &b) in bits.iter().enumerate() {
            if b == 0 {
                continue;
            }
            let x = plot.x + (i % cw) as u16;
            let y = plot.y + (i / cw) as u16;
            if let Some(c) = self.cell(x, y) {
                c.set_char(char::from_u32(0x2800 + b as u32).unwrap_or(' '))
                    .set_fg(colors[i]);
            }
        }
    }

    fn block_lines(&mut self, plot: Rect, lines: &[ChartLine], ymin: f32, span: f32) {
        let h = plot.height as i32;
        let level = |v: f32| (((v - ymin) / span).clamp(0.0, 1.0) * h as f32 * 8.0).round() as i32;
        let base = 0f32.max(ymin);
        let z = level(base);
        let bg = self.t.bg;
        let ordered = lines
            .iter()
            .filter(|l| l.fill)
            .chain(lines.iter().filter(|l| !l.fill));
        for line in ordered {
            let cols = resample_extreme(line.data, plot.width as usize, base);
            for (i, v) in cols.iter().enumerate() {
                if !v.is_finite() {
                    continue;
                }
                let x = plot.x + i as u16;
                let e = level(*v);
                if line.fill {
                    let (lo, hi) = (e.min(z), e.max(z));
                    for r in 0..h {
                        let (cell_lo, cell_hi) = (r * 8, r * 8 + 8);
                        let (a, b) = (lo.max(cell_lo), hi.min(cell_hi));
                        if b <= a {
                            continue;
                        }
                        let y = plot.bottom() - 1 - r as u16;
                        if a > cell_lo && b == cell_hi {
                            if let Some(c) = self.cell(x, y) {
                                c.set_symbol(VBLOCKS[(a - cell_lo) as usize])
                                    .set_fg(bg)
                                    .set_bg(line.color);
                            }
                        } else {
                            self.put(x, y, VBLOCKS[(b - cell_lo) as usize], line.color);
                        }
                    }
                } else {
                    let r = ((e - 1).max(0) / 8) as u16;
                    let sub = ((e - 1).max(0) % 8) as usize;
                    self.put(
                        x,
                        plot.bottom() - 1 - r.min(plot.height - 1),
                        ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"][sub],
                        line.color,
                    );
                }
            }
        }
    }
}

pub struct ChartLine<'a> {
    pub data: &'a [f32],
    pub color: Color,
    pub fill: bool,
}

pub struct ChartOpts<'a> {
    pub ymin: f32,
    pub ymax: f32,
    pub ticks: &'a [f32],
    pub fmt: &'a dyn Fn(f32) -> String,
    pub label_w: u16,
    pub xlabels: Option<[&'a str; 3]>,
    pub markers: &'a [(usize, &'a str)],
    pub hlines: &'a [(f32, Color)],
}

fn sample(d: &[f32], t: f32) -> f32 {
    let n = d.len();
    if n == 0 {
        return f32::NAN;
    }
    if n == 1 {
        return d[0];
    }
    let t = t.clamp(0.0, (n - 1) as f32);
    let i0 = t.floor() as usize;
    let i1 = (i0 + 1).min(n - 1);
    let f = t - i0 as f32;
    let (a, b) = (d[i0], d[i1]);
    match (a.is_finite(), b.is_finite()) {
        (true, true) => a * (1.0 - f) + b * f,
        (true, false) => a,
        (false, true) => b,
        _ => f32::NAN,
    }
}

pub fn resample(d: &[f32], w: usize) -> Vec<f32> {
    let n = d.len();
    if w == 0 {
        return Vec::new();
    }
    if n == 0 {
        return vec![f32::NAN; w];
    }
    (0..w)
        .map(|i| {
            if n >= w {
                let a = i * n / w;
                let b = ((i + 1) * n / w).max(a + 1);
                d[a..b]
                    .iter()
                    .copied()
                    .filter(|v| v.is_finite())
                    .fold(
                        f32::NAN,
                        |acc, v| if acc.is_finite() { acc.max(v) } else { v },
                    )
            } else {
                d[i * n / w]
            }
        })
        .collect()
}

fn min_max(d: &[f32]) -> Option<(f32, f32)> {
    d.iter()
        .copied()
        .filter(|v| v.is_finite())
        .fold(None, |acc, v| match acc {
            None => Some((v, v)),
            Some((lo, hi)) => Some((lo.min(v), hi.max(v))),
        })
}

fn bucket(i: usize, n: usize, w: usize) -> std::ops::Range<usize> {
    if n < w {
        let j = i * n / w;
        j..j + 1
    } else {
        let a = i * n / w;
        a..((i + 1) * n / w).max(a + 1).min(n)
    }
}

pub fn resample_extreme(d: &[f32], w: usize, base: f32) -> Vec<f32> {
    let n = d.len();
    (0..w)
        .map(|i| {
            if n == 0 {
                return f32::NAN;
            }
            d[bucket(i, n, w)]
                .iter()
                .copied()
                .filter(|v| v.is_finite())
                .fold(f32::NAN, |acc, v| {
                    if !acc.is_finite() || (v - base).abs() > (acc - base).abs() {
                        v
                    } else {
                        acc
                    }
                })
        })
        .collect()
}

pub fn pick_peaks(layers: &[&[f32]], w: usize) -> Vec<Option<usize>> {
    let n = layers.iter().map(|d| d.len()).max().unwrap_or(0);
    let value = |j: usize| -> Option<f32> {
        let mut any = false;
        let mut sum = 0.0;
        for d in layers {
            if let Some(v) = d.get(j).copied().filter(|v| v.is_finite()) {
                any = true;
                sum += v;
            }
        }
        any.then_some(sum)
    };
    (0..w)
        .map(|i| {
            if n == 0 {
                return None;
            }
            bucket(i, n, w)
                .filter_map(|j| value(j).map(|v| (j, v)))
                .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(j, _)| j)
        })
        .collect()
}

pub fn width(s: &str) -> u16 {
    unicode_width(s) as u16
}

fn unicode_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

fn char_width(c: char) -> usize {
    UnicodeWidthChar::width(c).unwrap_or(0)
}

pub fn trunc(s: &str, max: usize) -> String {
    if unicode_width(s) <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = char_width(c);
        if w + cw + 1 > max {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

pub fn pad(s: &str, n: usize) -> String {
    let t = trunc(s, n);
    let w = unicode_width(&t);
    if w >= n {
        t
    } else {
        format!("{t}{}", " ".repeat(n - w))
    }
}

pub fn rpad(s: &str, n: usize) -> String {
    let t = trunc(s, n);
    let w = unicode_width(&t);
    if w >= n {
        t
    } else {
        format!("{}{t}", " ".repeat(n - w))
    }
}

pub fn fmt_bytes(b: f64) -> String {
    if !b.is_finite() {
        return "–".into();
    }
    if b >= 1e9 {
        format!("{:.1}G", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.1}M", b / 1e6)
    } else if b >= 1e4 {
        format!("{:.0}K", b / 1e3)
    } else if b >= 1e3 {
        format!("{:.1}K", b / 1e3)
    } else {
        format!("{b:.0}B")
    }
}

pub fn fmt_dur(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

pub fn fmt_age(secs: f64) -> String {
    let s = secs.max(0.0);
    if s < 60.0 {
        format!("{s:.0}s")
    } else if s < 3600.0 {
        format!("{:.0}m", s / 60.0)
    } else {
        format!("{:.0}h", s / 3600.0)
    }
}

pub fn nice_max(v: f32) -> f32 {
    if v.is_nan() || v <= 0.0 || v.is_infinite() {
        return 1.0;
    }
    let p = 10f32.powf(v.log10().floor());
    for m in [1.0, 1.2, 1.5, 2.0, 2.5, 3.0, 4.0, 5.0, 6.0, 8.0, 10.0] {
        if m * p >= v {
            return m * p;
        }
    }
    10.0 * p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampling_keeps_peaks() {
        assert_eq!(resample(&[1.0, 9.0, 2.0, 3.0], 2), vec![9.0, 3.0]);
        assert_eq!(resample(&[1.0, 2.0], 4), vec![1.0, 1.0, 2.0, 2.0]);
        assert!(resample(&[f32::NAN, f32::NAN], 1)[0].is_nan());
    }

    #[test]
    fn text_helpers() {
        assert_eq!(trunc("abcdef", 4), "abc…");
        assert_eq!(pad("ab", 4), "ab  ");
        assert_eq!(rpad("ab", 4), "  ab");
        assert_eq!(fmt_bytes(1_500_000.0), "1.5M");
        assert_eq!(fmt_dur(220.0), "03:40");
        assert_eq!(nice_max(43.0), 50.0);
    }

    #[test]
    fn chart_draws_braille() {
        let theme = crate::theme::all(&Default::default()).remove(0);
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 6));
        let mut pen = Pen {
            buf: &mut buf,
            t: &theme,
            braille: true,
        };
        let data = [0.0, 50.0, 100.0];
        pen.chart(
            Rect::new(0, 0, 20, 6),
            &[ChartLine {
                data: &data,
                color: theme.green,
                fill: false,
            }],
            &ChartOpts {
                ymin: 0.0,
                ymax: 100.0,
                ticks: &[100.0, 0.0],
                fmt: &|v| format!("{v:.0}"),
                label_w: 4,
                xlabels: Some(["a", "", "b"]),
                markers: &[],
                hlines: &[],
            },
        );
        let has_braille = buf.content.iter().any(|c| {
            c.symbol()
                .chars()
                .next()
                .is_some_and(|ch| ('\u{2801}'..='\u{28ff}').contains(&ch))
        });
        assert!(has_braille);
    }

    fn opts<'a>(ymin: f32, ymax: f32, ticks: &'a [f32]) -> ChartOpts<'a> {
        ChartOpts {
            ymin,
            ymax,
            ticks,
            fmt: &|v| format!("{v:.0}"),
            label_w: 0,
            xlabels: None,
            markers: &[],
            hlines: &[],
        }
    }

    fn row_has_ink(buf: &Buffer, y: u16) -> bool {
        (0..buf.area.width).any(|x| {
            let s = buf.cell((x, y)).map(|c| c.symbol()).unwrap_or(" ");
            s != " " && s != "\u{2800}"
        })
    }

    #[test]
    fn braille_keeps_a_one_sample_spike() {
        let theme = crate::theme::all(&Default::default()).remove(0);
        let mut data = vec![5.0; 120];
        data[61] = 100.0;
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 5));
        let mut pen = Pen {
            buf: &mut buf,
            t: &theme,
            braille: true,
        };
        pen.chart(
            Rect::new(0, 0, 10, 5),
            &[ChartLine {
                data: &data,
                color: theme.green,
                fill: false,
            }],
            &opts(0.0, 100.0, &[]),
        );
        assert!(
            row_has_ink(&buf, 0),
            "the spike to 100 must reach the top row"
        );
    }

    #[test]
    fn blocks_mirrored_fill_grows_from_zero() {
        let theme = crate::theme::all(&Default::default()).remove(0);
        let rx = vec![0.5; 8];
        let tx = vec![0.0; 8];
        let mut buf = Buffer::empty(Rect::new(0, 0, 8, 4));
        let mut pen = Pen {
            buf: &mut buf,
            t: &theme,
            braille: false,
        };
        pen.chart(
            Rect::new(0, 0, 8, 4),
            &[
                ChartLine {
                    data: &rx,
                    color: theme.aqua,
                    fill: true,
                },
                ChartLine {
                    data: &tx,
                    color: theme.purple,
                    fill: true,
                },
            ],
            &opts(-1.0, 1.0, &[]),
        );
        assert!(!row_has_ink(&buf, 0), "0.5 of 1 must not reach the top row");
        assert!(row_has_ink(&buf, 1), "0.5 fills the row above zero");
        assert!(!row_has_ink(&buf, 2), "nothing sent, nothing below zero");
        assert!(!row_has_ink(&buf, 3), "nothing sent, nothing below zero");
    }

    #[test]
    fn tick_labels_never_share_a_row() {
        let theme = crate::theme::all(&Default::default()).remove(0);
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 5));
        let mut pen = Pen {
            buf: &mut buf,
            t: &theme,
            braille: true,
        };
        let ticks = [0.0, 100.0, -100.0, 1.0, -1.0];
        pen.chart(
            Rect::new(0, 0, 20, 5),
            &[],
            &ChartOpts {
                label_w: 5,
                ..opts(-100.0, 100.0, &ticks)
            },
        );
        let row = |y: u16| -> String {
            (0..4)
                .map(|x| buf.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "))
                .collect::<String>()
                .trim()
                .to_string()
        };
        assert_eq!(row(2), "0");
        assert_eq!(row(0), "100");
        assert_eq!(row(4), "-100");
    }

    #[test]
    fn stacked_and_blocks_keep_the_peak_sample() {
        let flat = vec![1.0; 100];
        let mut spiky = vec![1.0; 100];
        spiky[37] = 9.0;
        let picks = pick_peaks(&[&flat, &spiky], 10);
        assert_eq!(picks[3], Some(37));
        assert_eq!(
            resample_extreme(&[-1.0, -5.0, 2.0, 0.5], 1, 0.0),
            vec![-5.0]
        );
        assert_eq!(
            resample_extreme(&[3.0, 7.0, 2.0, 1.0], 2, 0.0),
            vec![7.0, 2.0]
        );
    }

    #[test]
    fn stacked_top_is_never_higher_than_the_total() {
        let theme = crate::theme::all(&Default::default()).remove(0);
        let a = [5.0];
        let b = [0.5];
        let mut buf = Buffer::empty(Rect::new(0, 0, 1, 4));
        let mut pen = Pen {
            buf: &mut buf,
            t: &theme,
            braille: true,
        };
        pen.stacked(
            Rect::new(0, 0, 1, 4),
            &[(&a, theme.green), (&b, theme.orange)],
            16.0,
        );
        let cell = |y: u16| buf.cell((0, y)).unwrap().clone();
        assert_eq!(cell(3).symbol(), "█");
        assert_eq!(cell(2).symbol(), "▃");
        assert_eq!(cell(2).bg, theme.bg);
        assert_eq!(cell(1).symbol(), " ");
    }
}
