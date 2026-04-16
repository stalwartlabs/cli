/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

pub mod describe;
pub mod object;
pub mod set_error;
pub mod value;

#[derive(Clone, Copy)]
pub struct Ansi {
    on: bool,
}

impl Ansi {
    pub fn new(on: bool) -> Self {
        Ansi { on }
    }

    pub const fn reset(self) -> &'static str {
        if self.on { "\x1b[0m" } else { "" }
    }
    pub const fn bold(self) -> &'static str {
        if self.on { "\x1b[1m" } else { "" }
    }
    pub const fn dim(self) -> &'static str {
        if self.on { "\x1b[2m" } else { "" }
    }
    pub const fn red(self) -> &'static str {
        if self.on { "\x1b[31m" } else { "" }
    }
    pub const fn green(self) -> &'static str {
        if self.on { "\x1b[32m" } else { "" }
    }
    pub const fn yellow(self) -> &'static str {
        if self.on { "\x1b[33m" } else { "" }
    }
    pub const fn blue(self) -> &'static str {
        if self.on { "\x1b[34m" } else { "" }
    }
    pub const fn cyan(self) -> &'static str {
        if self.on { "\x1b[36m" } else { "" }
    }

    pub fn named(self, name: &str) -> String {
        if !self.on {
            return String::new();
        }
        let s = match name.to_ascii_lowercase().as_str() {
            "black" => Some("\x1b[30m"),
            "red" => Some("\x1b[31m"),
            "green" => Some("\x1b[32m"),
            "yellow" => Some("\x1b[33m"),
            "blue" => Some("\x1b[34m"),
            "cyan" => Some("\x1b[36m"),
            "white" => Some("\x1b[37m"),
            "magenta" => Some("\x1b[35m"),
            _ => None,
        };
        if let Some(s) = s {
            return s.to_string();
        }
        if let Some((r, g, b)) = parse_hex_rgb(name) {
            return format!("\x1b[38;2;{};{};{}m", r, g, b);
        }
        "\x1b[34m".to_string()
    }
}

fn parse_hex_rgb(raw: &str) -> Option<(u8, u8, u8)> {
    let s = raw.trim().strip_prefix('#')?;
    match s.len() {
        6 => {
            let r = u8::from_str_radix(&s[0..2], 16).ok()?;
            let g = u8::from_str_radix(&s[2..4], 16).ok()?;
            let b = u8::from_str_radix(&s[4..6], 16).ok()?;
            Some((r, g, b))
        }
        3 => {
            let r = u8::from_str_radix(&s[0..1], 16).ok()? * 17;
            let g = u8::from_str_radix(&s[1..2], 16).ok()? * 17;
            let b = u8::from_str_radix(&s[2..3], 16).ok()? * 17;
            Some((r, g, b))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_rgb_6digit() {
        assert_eq!(parse_hex_rgb("#b91c1c"), Some((0xb9, 0x1c, 0x1c)));
        assert_eq!(parse_hex_rgb("#000000"), Some((0, 0, 0)));
        assert_eq!(parse_hex_rgb("#ffffff"), Some((255, 255, 255)));
    }

    #[test]
    fn hex_rgb_3digit() {
        assert_eq!(parse_hex_rgb("#abc"), Some((0xaa, 0xbb, 0xcc)));
    }

    #[test]
    fn hex_rgb_rejects_garbage() {
        assert_eq!(parse_hex_rgb("blue"), None);
        assert_eq!(parse_hex_rgb("#xyz"), None);
        assert_eq!(parse_hex_rgb(""), None);
    }

    #[test]
    fn named_returns_truecolor_for_hex() {
        let ansi = Ansi::new(true);
        let s = ansi.named("#b91c1c");
        assert_eq!(s, "\x1b[38;2;185;28;28m");
    }

    #[test]
    fn named_returns_empty_when_color_off() {
        let ansi = Ansi::new(false);
        assert_eq!(ansi.named("#b91c1c"), "");
        assert_eq!(ansi.named("red"), "");
    }
}
