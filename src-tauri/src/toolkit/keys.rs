//! Key combinations as text ("Ctrl+Shift+T") and as Windows virtual-key codes.
//!
//! The names are the ones the settings page writes (settings-tools.js
//! `keyName`), so a combination recorded there round-trips exactly.

pub const CTRL: u8 = 1;
pub const ALT: u8 = 2;
pub const SHIFT: u8 = 4;
pub const WIN: u8 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Combo {
    pub mods: u8,
    pub vk: u16,
}

/// Names first, virtual-key code second. The first name for a code is the
/// canonical spelling used when formatting.
const NAMES: &[(&str, u16)] = &[
    ("Backspace", 0x08),
    ("Tab", 0x09),
    ("Enter", 0x0D),
    ("Return", 0x0D),
    ("Pause", 0x13),
    ("CapsLock", 0x14),
    ("Esc", 0x1B),
    ("Escape", 0x1B),
    ("Space", 0x20),
    ("PageUp", 0x21),
    ("PgUp", 0x21),
    ("PageDown", 0x22),
    ("PgDn", 0x22),
    ("End", 0x23),
    ("Home", 0x24),
    ("Left", 0x25),
    ("Up", 0x26),
    ("Right", 0x27),
    ("Down", 0x28),
    ("PrintScreen", 0x2C),
    ("PrtSc", 0x2C),
    ("Insert", 0x2D),
    ("Ins", 0x2D),
    ("Delete", 0x2E),
    ("Del", 0x2E),
    ("Menu", 0x5D),
    ("Apps", 0x5D),
    ("Num0", 0x60),
    ("Num1", 0x61),
    ("Num2", 0x62),
    ("Num3", 0x63),
    ("Num4", 0x64),
    ("Num5", 0x65),
    ("Num6", 0x66),
    ("Num7", 0x67),
    ("Num8", 0x68),
    ("Num9", 0x69),
    ("Num*", 0x6A),
    ("Num+", 0x6B),
    ("Num-", 0x6D),
    ("Num.", 0x6E),
    ("Num/", 0x6F),
    ("NumLock", 0x90),
    ("ScrollLock", 0x91),
    ("BrowserBack", 0xA6),
    ("BrowserForward", 0xA7),
    ("BrowserRefresh", 0xA8),
    ("BrowserStop", 0xA9),
    ("BrowserSearch", 0xAA),
    ("BrowserFavorites", 0xAB),
    ("BrowserHome", 0xAC),
    ("VolumeMute", 0xAD),
    ("VolumeDown", 0xAE),
    ("VolumeUp", 0xAF),
    ("MediaNext", 0xB0),
    ("MediaPrev", 0xB1),
    ("MediaStop", 0xB2),
    ("MediaPlayPause", 0xB3),
    (";", 0xBA),
    ("=", 0xBB),
    (",", 0xBC),
    ("-", 0xBD),
    (".", 0xBE),
    ("/", 0xBF),
    ("`", 0xC0),
    ("[", 0xDB),
    ("\\", 0xDC),
    ("]", 0xDD),
    ("'", 0xDE),
];

pub fn vk_of(name: &str) -> Option<u16> {
    let n = name.trim();
    if n.is_empty() {
        return None;
    }
    if let Some(&(_, vk)) = NAMES.iter().find(|(k, _)| k.eq_ignore_ascii_case(n)) {
        return Some(vk);
    }
    let up = n.to_ascii_uppercase();
    let b = up.as_bytes();
    if b.len() == 1 && (b[0].is_ascii_uppercase() || b[0].is_ascii_digit()) {
        return Some(b[0] as u16);
    }
    if let Some(num) = up.strip_prefix('F').and_then(|d| d.parse::<u16>().ok()) {
        if (1..=24).contains(&num) {
            return Some(0x70 + num - 1);
        }
    }
    None
}

pub fn name_of(vk: u16) -> String {
    if let Some(&(n, _)) = NAMES.iter().find(|(_, v)| *v == vk) {
        return n.to_string();
    }
    match vk {
        0x30..=0x39 | 0x41..=0x5A => (vk as u8 as char).to_string(),
        0x70..=0x87 => format!("F{}", vk - 0x70 + 1),
        _ => format!("VK{vk:02X}"),
    }
}

pub fn is_modifier(vk: u16) -> bool {
    matches!(vk, 0x10 | 0x11 | 0x12 | 0x5B | 0x5C | 0xA0..=0xA5)
}

/// The modifier bit a modifier key sets.
pub fn modifier_bit(vk: u16) -> u8 {
    match vk {
        0x11 | 0xA2 | 0xA3 => CTRL,
        0x12 | 0xA4 | 0xA5 => ALT,
        0x10 | 0xA0 | 0xA1 => SHIFT,
        0x5B | 0x5C => WIN,
        _ => 0,
    }
}

impl Combo {
    pub fn parse(text: &str) -> Option<Combo> {
        let mut mods = 0u8;
        let mut vk = None;
        // "Num+" contains the separator; split on '+' only between parts.
        let mut parts: Vec<String> = Vec::new();
        for piece in text.split('+') {
            if piece.is_empty() {
                // "Ctrl++" or "Num+" -> the '+' key belongs to the last part.
                if let Some(last) = parts.last_mut() {
                    last.push('+');
                    continue;
                }
            }
            parts.push(piece.to_string());
        }
        for p in parts {
            match p.trim().to_ascii_lowercase().as_str() {
                "ctrl" | "control" => mods |= CTRL,
                "alt" => mods |= ALT,
                "shift" => mods |= SHIFT,
                "win" | "meta" | "super" => mods |= WIN,
                other if other.is_empty() => {}
                _ => {
                    if vk.is_some() {
                        return None;
                    }
                    vk = Some(vk_of(&p)?);
                }
            }
        }
        Some(Combo { mods, vk: vk? })
    }

    pub fn format(&self) -> String {
        let mut s = String::new();
        for (bit, name) in [(WIN, "Win"), (CTRL, "Ctrl"), (ALT, "Alt"), (SHIFT, "Shift")] {
            if self.mods & bit != 0 {
                s.push_str(name);
                s.push('+');
            }
        }
        s.push_str(&name_of(self.vk));
        s
    }
}

/// A sequence of combinations separated by spaces: "Ctrl+K Ctrl+C".
pub fn parse_sequence(text: &str) -> Option<Vec<Combo>> {
    let v: Option<Vec<Combo>> = text.split_whitespace().map(Combo::parse).collect();
    v.filter(|v| !v.is_empty())
}

/// Keys whose scan code needs the "extended" flag when injected, or the
/// target sees the numeric-keypad twin (Left arrow would arrive as Num4).
pub fn is_extended(vk: u16) -> bool {
    matches!(
        vk,
        0x21..=0x28 | 0x2C | 0x2D | 0x2E | 0x5B | 0x5C | 0x5D | 0x6F | 0x90 | 0xA3 | 0xA5 | 0xA6..=0xB7
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        for s in ["Ctrl+Alt+T", "F1", "Alt+Left", "Ctrl+Shift+Tab", "Win+Shift+S", "Ctrl+Num+", "Alt+V", "Ctrl+,"] {
            assert_eq!(Combo::parse(s).unwrap().format(), s, "{s}");
        }
    }

    #[test]
    fn aliases() {
        assert_eq!(Combo::parse("control+esc").unwrap().format(), "Ctrl+Esc");
        assert_eq!(Combo::parse("ctrl+pgdn").unwrap().format(), "Ctrl+PageDown");
    }

    #[test]
    fn rejects_nonsense() {
        assert!(Combo::parse("Ctrl+Alt").is_none());
        assert!(Combo::parse("Ctrl+A+B").is_none());
        assert!(Combo::parse("Ctrl+Nope").is_none());
    }

    #[test]
    fn sequences() {
        assert_eq!(parse_sequence("Ctrl+K Ctrl+C").unwrap().len(), 2);
    }
}
