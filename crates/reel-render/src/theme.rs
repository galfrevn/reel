//! Color themes and the ColorRef → RGBA resolution rules.

use reel_term::ColorRef;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Rgba { r, g, b, a: 255 }
    }

    pub fn from_hex(s: &str) -> Option<Self> {
        let h = s.trim().strip_prefix('#')?;
        let parse = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
        match h.len() {
            6 => Some(Rgba { r: parse(0)?, g: parse(2)?, b: parse(4)?, a: 255 }),
            8 => Some(Rgba { r: parse(0)?, g: parse(2)?, b: parse(4)?, a: parse(6)? }),
            _ => None,
        }
    }

    pub fn scaled(self, f: f32) -> Self {
        Rgba {
            r: (self.r as f32 * f).clamp(0.0, 255.0) as u8,
            g: (self.g as f32 * f).clamp(0.0, 255.0) as u8,
            b: (self.b as f32 * f).clamp(0.0, 255.0) as u8,
            a: self.a,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Theme {
    pub name: &'static str,
    pub fg: Rgba,
    pub bg: Rgba,
    pub cursor: Rgba,
    /// ANSI 0-15.
    pub ansi: [Rgba; 16],
}

impl Theme {
    /// Resolves an abstract cell color. `overrides` are the snapshot's OSC 4
    /// palette redefinitions.
    pub fn resolve(&self, c: ColorRef, overrides: &[(u8, (u8, u8, u8))]) -> Rgba {
        match c {
            ColorRef::Fg => self.fg,
            ColorRef::Bg => self.bg,
            ColorRef::Cursor => self.cursor,
            ColorRef::Rgb(r, g, b) => Rgba::rgb(r, g, b),
            ColorRef::Indexed(i) => {
                if let Some(&(_, (r, g, b))) = overrides.iter().find(|(idx, _)| *idx == i) {
                    return Rgba::rgb(r, g, b);
                }
                if i < 16 {
                    self.ansi[i as usize]
                } else {
                    xterm_256(i)
                }
            }
        }
    }
}

/// Standard xterm 256-color cube and grayscale ramp for indices 16-255.
fn xterm_256(i: u8) -> Rgba {
    if i < 16 {
        unreachable!("handled by the theme");
    }
    if i < 232 {
        let i = i - 16;
        let comp = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
        Rgba::rgb(comp(i / 36), comp((i / 6) % 6), comp(i % 6))
    } else {
        let v = 8 + (i - 232) * 10;
        Rgba::rgb(v, v, v)
    }
}

fn hex(s: &str) -> Rgba {
    Rgba::from_hex(s).expect("built-in theme hex")
}

pub fn builtin(name: &str) -> Option<Theme> {
    let t = match name {
        "reel-dark" => Theme {
            name: "reel-dark",
            fg: hex("#e6e6eb"),
            bg: hex("#101014"),
            cursor: hex("#8ab4f8"),
            ansi: [
                hex("#1c1c22"), hex("#f28b82"), hex("#81c995"), hex("#fdd663"),
                hex("#8ab4f8"), hex("#d7aefb"), hex("#78d9ec"), hex("#e6e6eb"),
                hex("#5f6368"), hex("#f6aea9"), hex("#a8dab5"), hex("#fde293"),
                hex("#aecbfa"), hex("#e9d2fd"), hex("#a1e4f2"), hex("#ffffff"),
            ],
        },
        "catppuccin-mocha" => Theme {
            name: "catppuccin-mocha",
            fg: hex("#cdd6f4"),
            bg: hex("#1e1e2e"),
            cursor: hex("#f5e0dc"),
            ansi: [
                hex("#45475a"), hex("#f38ba8"), hex("#a6e3a1"), hex("#f9e2af"),
                hex("#89b4fa"), hex("#f5c2e7"), hex("#94e2d5"), hex("#bac2de"),
                hex("#585b70"), hex("#f38ba8"), hex("#a6e3a1"), hex("#f9e2af"),
                hex("#89b4fa"), hex("#f5c2e7"), hex("#94e2d5"), hex("#a6adc8"),
            ],
        },
        "tokyo-night" => Theme {
            name: "tokyo-night",
            fg: hex("#c0caf5"),
            bg: hex("#1a1b26"),
            cursor: hex("#c0caf5"),
            ansi: [
                hex("#15161e"), hex("#f7768e"), hex("#9ece6a"), hex("#e0af68"),
                hex("#7aa2f7"), hex("#bb9af7"), hex("#7dcfff"), hex("#a9b1d6"),
                hex("#414868"), hex("#f7768e"), hex("#9ece6a"), hex("#e0af68"),
                hex("#7aa2f7"), hex("#bb9af7"), hex("#7dcfff"), hex("#c0caf5"),
            ],
        },
        "paper-light" => Theme {
            name: "paper-light",
            fg: hex("#2d2d2d"),
            bg: hex("#f7f7f2"),
            cursor: hex("#0f62fe"),
            ansi: [
                hex("#2d2d2d"), hex("#c4331d"), hex("#237a3b"), hex("#a1740c"),
                hex("#0f62fe"), hex("#8a3ffc"), hex("#007d79"), hex("#6f6f6f"),
                hex("#525252"), hex("#e05243"), hex("#3fa860"), hex("#c9a03a"),
                hex("#4589ff"), hex("#a56eff"), hex("#08bdba"), hex("#161616"),
            ],
        },
        _ => return None,
    };
    Some(t)
}

pub fn theme_names() -> &'static [&'static str] {
    &["reel-dark", "catppuccin-mocha", "tokyo-night", "paper-light"]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cube_matches_xterm_reference() {
        assert_eq!(xterm_256(16), Rgba::rgb(0, 0, 0));
        assert_eq!(xterm_256(21), Rgba::rgb(0, 0, 255));
        assert_eq!(xterm_256(196), Rgba::rgb(255, 0, 0));
        assert_eq!(xterm_256(231), Rgba::rgb(255, 255, 255));
        assert_eq!(xterm_256(232), Rgba::rgb(8, 8, 8));
        assert_eq!(xterm_256(255), Rgba::rgb(238, 238, 238));
    }

    #[test]
    fn overrides_win() {
        let t = builtin("reel-dark").unwrap();
        let c = t.resolve(ColorRef::Indexed(1), &[(1, (9, 9, 9))]);
        assert_eq!(c, Rgba::rgb(9, 9, 9));
    }
}
