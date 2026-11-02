use ratatui::style::Color;

#[derive(Debug, Clone)]
pub struct Theme {
    pub name: String,
    pub bg: Color,
    pub hi: Color,
    pub fg: Color,
    pub dim: Color,
    pub dim2: Color,
    pub gray: Color,
    pub track: Color,
    pub sel: Color,
    pub red: Color,
    pub green: Color,
    pub yellow: Color,
    pub blue: Color,
    pub purple: Color,
    pub aqua: Color,
    pub orange: Color,
}

const ROLES: [&str; 15] = [
    "bg", "hi", "fg", "dim", "dim2", "gray", "track", "sel", "red", "green", "yellow", "blue",
    "purple", "aqua", "orange",
];

const BUILTIN: [(&str, [u32; 15]); 14] = [
    (
        "Gruvbox Dark",
        [
            0x282828, 0x3c3836, 0xebdbb2, 0x928374, 0x504945, 0x665c54, 0x32302f, 0x45403d,
            0xfb4934, 0xb8bb26, 0xfabd2f, 0x83a598, 0xd3869b, 0x8ec07c, 0xfe8019,
        ],
    ),
    (
        "Catppuccin Mocha",
        [
            0x1e1e2e, 0x313244, 0xcdd6f4, 0x7f849c, 0x45475a, 0x585b70, 0x2a2b3c, 0x363a4f,
            0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xcba6f7, 0x94e2d5, 0xfab387,
        ],
    ),
    (
        "Tokyo Night",
        [
            0x1a1b26, 0x24283b, 0xc0caf5, 0x565f89, 0x3b4261, 0x414868, 0x1f2335, 0x283457,
            0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xff9e64,
        ],
    ),
    (
        "Nord",
        [
            0x2e3440, 0x3b4252, 0xe5e9f0, 0x7b88a1, 0x4c566a, 0x616e88, 0x353c4a, 0x434c5e,
            0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x88c0d0, 0xd08770,
        ],
    ),
    (
        "Dracula",
        [
            0x282a36, 0x343746, 0xf8f8f2, 0x6272a4, 0x44475a, 0x565a73, 0x2f3141, 0x44475a,
            0xff5555, 0x50fa7b, 0xf1fa8c, 0xbd93f9, 0xff79c6, 0x8be9fd, 0xffb86c,
        ],
    ),
    (
        "One Dark",
        [
            0x282c34, 0x2f343f, 0xabb2bf, 0x5c6370, 0x3e4451, 0x4b5263, 0x2c313a, 0x3e4451,
            0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xd19a66,
        ],
    ),
    (
        "Solarized Dark",
        [
            0x002b36, 0x073642, 0x93a1a1, 0x657b83, 0x0e4b59, 0x586e75, 0x04313d, 0x0a4250,
            0xdc322f, 0x859900, 0xb58900, 0x268bd2, 0xd33682, 0x2aa198, 0xcb4b16,
        ],
    ),
    (
        "Kanagawa",
        [
            0x1f1f28, 0x2a2a37, 0xdcd7ba, 0x727169, 0x363646, 0x54546d, 0x252530, 0x2d4f67,
            0xe46876, 0x98bb6c, 0xe6c384, 0x7e9cd8, 0x957fb8, 0x7aa89f, 0xffa066,
        ],
    ),
    (
        "Everforest Dark",
        [
            0x2d353b, 0x3d484d, 0xd3c6aa, 0x859289, 0x475258, 0x56635f, 0x343f44, 0x4f585e,
            0xe67e80, 0xa7c080, 0xdbbc7f, 0x7fbbb3, 0xd699b6, 0x83c092, 0xe69875,
        ],
    ),
    (
        "Monokai",
        [
            0x272822, 0x3e3d32, 0xf8f8f2, 0x75715e, 0x49483e, 0x5f5d50, 0x2f302a, 0x49483e,
            0xf92672, 0xa6e22e, 0xe6db74, 0x66d9ef, 0xae81ff, 0xa1efe4, 0xfd971f,
        ],
    ),
    (
        "GitHub Dark",
        [
            0x0d1117, 0x161b22, 0xe6edf3, 0x7d8590, 0x30363d, 0x484f58, 0x151b23, 0x1f2a37,
            0xff7b72, 0x3fb950, 0xd29922, 0x58a6ff, 0xbc8cff, 0x39c5cf, 0xffa657,
        ],
    ),
    (
        "Ayu Dark",
        [
            0x0b0e14, 0x131721, 0xbfbdb6, 0x565b66, 0x2d3640, 0x3d424d, 0x0f131a, 0x273747,
            0xf07178, 0xaad94c, 0xe6b450, 0x59c2ff, 0xd2a6ff, 0x95e6cb, 0xff8f40,
        ],
    ),
    (
        "Night Owl",
        [
            0x011627, 0x0b2942, 0xd6deeb, 0x637777, 0x1d3b53, 0x4b6479, 0x0b253a, 0x234d70,
            0xef5350, 0xaddb67, 0xecc48d, 0x82aaff, 0xc792ea, 0x7fdbca, 0xf78c6c,
        ],
    ),
    (
        "Catppuccin Latte",
        [
            0xeff1f5, 0xdce0e8, 0x4c4f69, 0x8c8fa1, 0xbcc0cc, 0x9ca0b0, 0xe3e6ed, 0xd0d4de,
            0xd20f39, 0x40a02b, 0xdf8e1d, 0x1e66f5, 0x8839ef, 0x179299, 0xfe640b,
        ],
    ),
];

fn rgb(v: u32) -> Color {
    Color::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

pub fn rgb_of(c: Color) -> (u8, u8, u8) {
    match c {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => (128, 128, 128),
    }
}

pub fn mix(a: Color, b: Color, t: f32) -> Color {
    let (ar, ag, ab) = rgb_of(a);
    let (br, bg, bb) = rgb_of(b);
    let m = |x: u8, y: u8| {
        (x as f32 * t + y as f32 * (1.0 - t))
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Color::Rgb(m(ar, br), m(ag, bg), m(ab, bb))
}

fn parse_hex(s: &str) -> Option<u32> {
    u32::from_str_radix(s.trim().trim_start_matches('#'), 16)
        .ok()
        .filter(|v| *v <= 0xffffff)
}

impl Theme {
    fn from_values(name: &str, v: [u32; 15]) -> Theme {
        Theme {
            name: name.to_string(),
            bg: rgb(v[0]),
            hi: rgb(v[1]),
            fg: rgb(v[2]),
            dim: rgb(v[3]),
            dim2: rgb(v[4]),
            gray: rgb(v[5]),
            track: rgb(v[6]),
            sel: rgb(v[7]),
            red: rgb(v[8]),
            green: rgb(v[9]),
            yellow: rgb(v[10]),
            blue: rgb(v[11]),
            purple: rgb(v[12]),
            aqua: rgb(v[13]),
            orange: rgb(v[14]),
        }
    }

    pub fn swatches(&self) -> [Color; 10] {
        [
            self.bg,
            self.hi,
            self.fg,
            self.red,
            self.orange,
            self.yellow,
            self.green,
            self.aqua,
            self.blue,
            self.purple,
        ]
    }

    pub fn ok_dim(&self) -> Color {
        mix(self.green, self.bg, 0.3)
    }

    pub fn heat(&self, v: f32) -> Color {
        let stops = [
            (0.0, self.track),
            (8.0, mix(self.green, self.bg, 0.3)),
            (25.0, mix(self.green, self.bg, 0.75)),
            (50.0, self.yellow),
            (75.0, self.orange),
            (100.0, self.red),
        ];
        let v = if v.is_finite() {
            v.clamp(0.0, 100.0)
        } else {
            return self.bg;
        };
        for w in stops.windows(2) {
            let (a, ca) = w[0];
            let (b, cb) = w[1];
            if v <= b {
                return mix(cb, ca, (v - a) / (b - a));
            }
        }
        self.red
    }

    pub fn usage(&self, v: f32) -> Color {
        if v > 80.0 {
            self.red
        } else if v > 50.0 {
            self.orange
        } else if v > 20.0 {
            self.green
        } else {
            self.dim
        }
    }

    pub fn temp(&self, c: f32) -> Color {
        if c > 45.0 {
            self.red
        } else if c > 38.0 {
            self.orange
        } else {
            self.green
        }
    }
}

pub fn normalize(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect()
}

pub fn all(
    custom: &std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>,
) -> Vec<Theme> {
    let mut themes: Vec<Theme> = BUILTIN
        .iter()
        .map(|(n, v)| Theme::from_values(n, *v))
        .collect();
    for (name, roles) in custom {
        let base_name = roles.get("base").map(|b| normalize(b)).unwrap_or_default();
        let base = BUILTIN
            .iter()
            .find(|(n, _)| normalize(n) == base_name)
            .map(|(_, v)| *v)
            .unwrap_or(BUILTIN[0].1);
        let mut values = base;
        for (i, role) in ROLES.iter().enumerate() {
            if let Some(v) = roles.get(*role).and_then(|s| parse_hex(s)) {
                values[i] = v;
            }
        }
        if let Some(accent) = roles.get("accent").and_then(|s| parse_hex(s)) {
            values[10] = accent;
        }
        themes.push(Theme::from_values(name, values));
    }
    themes
}

pub fn find(themes: &[Theme], name: &str) -> Option<usize> {
    let n = normalize(name);
    themes.iter().position(|t| normalize(&t.name) == n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn builtin_and_custom_themes() {
        let mut custom = BTreeMap::new();
        custom.insert(
            "Mine".to_string(),
            BTreeMap::from([
                ("base".to_string(), "nord".to_string()),
                ("bg".to_string(), "#000000".to_string()),
            ]),
        );
        let themes = all(&custom);
        assert_eq!(themes.len(), BUILTIN.len() + 1);
        assert_eq!(find(&themes, "tokyo night"), Some(2));
        assert_eq!(find(&themes, "TokyoNight"), Some(2));
        assert!(find(&themes, "night owl").is_some());
        let mine = themes.last().unwrap();
        assert_eq!(mine.bg, Color::Rgb(0, 0, 0));
        assert_eq!(mine.fg, themes[3].fg);
    }

    #[test]
    fn heat_ramp_ends() {
        let t = &all(&BTreeMap::new())[0];
        assert_eq!(t.heat(0.0), t.track);
        assert_eq!(t.heat(100.0), t.red);
    }
}
