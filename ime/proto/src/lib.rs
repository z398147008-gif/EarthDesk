//! What the TSF DLL (inside every program you type in) and the engine
//! process (`EarthDeskIME.exe`) say to each other over a named pipe.
//!
//! One request, one reply, framed as a little-endian u32 length followed by
//! JSON. A keystroke is a few hundred bytes each way; the round trip is far
//! below a millisecond, and JSON keeps the protocol debuggable.
//!
//! The DLL knows nothing about pinyin or candidates: it forwards keys,
//! shows the preedit (the underlined text) it is told to show, and inserts
//! what it is told to commit. The engine owns the candidate window.

use serde::{Deserialize, Serialize};
use std::io::{Read, Write};

/// Bumped when a message changes shape; both sides check it in Hello.
pub const VERSION: u32 = 1;

/// Rime's (X11 keysym) modifier bits, which the DLL sends as `mask`.
pub mod mask {
    pub const SHIFT: u32 = 1 << 0;
    pub const LOCK: u32 = 1 << 1;
    pub const CONTROL: u32 = 1 << 2;
    pub const ALT: u32 = 1 << 3;
    pub const RELEASE: u32 = 1 << 30;
}

/// The pipe for one Windows logon session. Every process in the session,
/// including sandboxed (AppContainer) apps, can compute it.
pub fn pipe_name(session_id: u32) -> String {
    format!(r"\\.\pipe\EarthDeskIME.{session_id}")
}

/// Posted to the DLL's notification window when something happened that the
/// DLL did not ask about (a candidate clicked with the mouse): the DLL then
/// sends `Poll` from inside an edit session.
pub const WM_NOTIFY_OFFSET: u32 = 0x0400 + 0x2A1; // WM_USER + n

/// The text service's class id and language profile (must match the TSF
/// DLL), as Windows writes them in "0804:{clsid}{profile}" strings.
pub const CLSID_STR: &str = "{6F1A7C52-3B8E-4D0A-9E61-2C5D8B7E4A13}";
pub const PROFILE_STR: &str = "{0B9D3E27-71C4-4F5E-8A2D-9C6E1F4B7D20}";
/// Simplified Chinese (PRC).
pub const LANGID: u16 = 0x0804;

/// The installed text service as InstallLayoutOrTip wants it.
pub fn tip_string() -> String {
    format!("{:04X}:{CLSID_STR}{PROFILE_STR}", LANGID)
}

/// The user's input-method settings, written by the EarthDesk settings page
/// to `%APPDATA%\EarthDesk\ime\settings.json` and read by the engine when it
/// (re)deploys.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// Candidates per page (3..=9).
    pub page_size: u32,
    /// Emoji among the candidates.
    pub emoji: bool,
    /// Shift alone switches between Chinese and English.
    pub shift_toggle: bool,
    /// Start in English in these programs (lower-case exe names).
    pub ascii_apps: Vec<String>,
    /// Chinese and Japanese from the same keys (needs the Japanese engine).
    pub mixed: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { page_size: 7, emoji: false, shift_toggle: true, ascii_apps: Vec::new(), mixed: true }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Request {
    Hello {
        version: u32,
        pid: u32,
        /// Lower-case executable name of the host program.
        exe: String,
        /// The DLL's message-only window for `WM_NOTIFY_OFFSET`.
        notify: u64,
    },
    /// A key went down (or up, with mask::RELEASE).
    Key { session: u64, keycode: u32, mask: u32 },
    /// Where the text caret is, in physical screen pixels (bottom-left of
    /// the composition's caret, plus the line height).
    Caret { session: u64, x: i32, y: i32, h: i32 },
    /// The host window gained or lost focus.
    Focus { session: u64, on: bool },
    /// Fetch what changed after a notification.
    Poll { session: u64 },
    /// Throw the current composition away (the document lost focus).
    Reset { session: u64 },
    Bye { session: u64 },
    /// From the EarthDesk settings page: is the engine alive, is it busy.
    Status,
    /// From the EarthDesk settings page: settings.json changed (or the user
    /// asked to rebuild): rewrite the Rime patches and redeploy.
    Deploy,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Preedit {
    pub text: String,
    /// Caret position in UTF-16 units.
    pub cursor: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct State {
    /// The key was consumed by the input method.
    pub eaten: bool,
    /// Text to insert now.
    pub commit: Option<String>,
    /// What the underlined composition should read; None = no composition.
    pub preedit: Option<Preedit>,
    /// English (pass-through) mode.
    pub ascii: bool,
    /// Before inserting `commit`, remove this many UTF-16 units before the
    /// caret (switching a just-committed Japanese word between 漢字 /
    /// ひらがな / カタカナ). Older DLLs ignore it.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub delete_before: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Reply {
    Hello { version: u32, session: u64 },
    State(State),
    Ok,
    /// The engine is not ready (first-run dictionary build): pass keys
    /// through untouched.
    Busy,
    Error { message: String },
    Status {
        librime: String,
        /// The Japanese engine's data version; empty = not available.
        #[serde(default)]
        mozc: String,
        /// Building dictionaries right now.
        maintaining: bool,
        /// Programs connected.
        clients: u32,
    },
}

pub fn write_frame<W: Write, T: Serialize>(w: &mut W, msg: &T) -> std::io::Result<()> {
    let body = serde_json::to_vec(msg).map_err(std::io::Error::other)?;
    let mut buf = Vec::with_capacity(body.len() + 4);
    buf.extend_from_slice(&(body.len() as u32).to_le_bytes());
    buf.extend_from_slice(&body);
    w.write_all(&buf)?;
    w.flush()
}

pub fn read_frame<R: Read, T: for<'de> Deserialize<'de>>(r: &mut R) -> std::io::Result<T> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let n = u32::from_le_bytes(len) as usize;
    if n > 1 << 20 {
        return Err(std::io::Error::other("frame too large"));
    }
    let mut body = vec![0u8; n];
    r.read_exact(&mut body)?;
    serde_json::from_slice(&body).map_err(std::io::Error::other)
}

/// X11 keysyms for the keys that are not plain characters. Printable ASCII
/// keysyms equal their character codes.
pub mod keysym {
    pub const BACKSPACE: u32 = 0xff08;
    pub const TAB: u32 = 0xff09;
    pub const RETURN: u32 = 0xff0d;
    pub const PAUSE: u32 = 0xff13;
    pub const ESCAPE: u32 = 0xff1b;
    pub const HOME: u32 = 0xff50;
    pub const LEFT: u32 = 0xff51;
    pub const UP: u32 = 0xff52;
    pub const RIGHT: u32 = 0xff53;
    pub const DOWN: u32 = 0xff54;
    pub const PAGE_UP: u32 = 0xff55;
    pub const PAGE_DOWN: u32 = 0xff56;
    pub const END: u32 = 0xff57;
    pub const INSERT: u32 = 0xff63;
    pub const KP_ENTER: u32 = 0xff8d;
    pub const F1: u32 = 0xffbe;
    pub const SHIFT_L: u32 = 0xffe1;
    pub const SHIFT_R: u32 = 0xffe2;
    pub const CONTROL_L: u32 = 0xffe3;
    pub const CONTROL_R: u32 = 0xffe4;
    pub const CAPS_LOCK: u32 = 0xffe5;
    pub const ALT_L: u32 = 0xffe9;
    pub const ALT_R: u32 = 0xffea;
    pub const DELETE: u32 = 0xffff;
    pub const KP_0: u32 = 0xffb0;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let mut buf = Vec::new();
        let r = Request::Key { session: 3, keycode: 'a' as u32, mask: mask::SHIFT };
        write_frame(&mut buf, &r).unwrap();
        let back: Request = read_frame(&mut buf.as_slice()).unwrap();
        assert_eq!(r, back);
        let s = Reply::State(State { eaten: true, commit: Some("你好".into()), preedit: None, ascii: false, delete_before: 0 });
        let mut buf = Vec::new();
        write_frame(&mut buf, &s).unwrap();
        assert_eq!(read_frame::<_, Reply>(&mut buf.as_slice()).unwrap(), s);
    }

    #[test]
    fn settings_defaults_fill_gaps() {
        let s: Settings = serde_json::from_str(r#"{"page_size":5}"#).unwrap();
        assert_eq!(s.page_size, 5);
        assert!(s.shift_toggle);
        assert_eq!(tip_string(), "0804:{6F1A7C52-3B8E-4D0A-9E61-2C5D8B7E4A13}{0B9D3E27-71C4-4F5E-8A2D-9C6E1F4B7D20}");
    }
}
