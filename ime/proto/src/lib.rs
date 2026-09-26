//! What the TSF DLL (inside every program you type in) and the engine
//! process (`EarthDeskIME.exe`) say to each other over a named pipe.
//!
//! One request, one reply, framed as a little-endian u32 length followed by
//! JSON. A keystroke is a few hundred bytes each way; the round trip is far
//! below a millisecond, and JSON keeps the protocol debuggable.
//!
//! The DLL knows nothing about pinyin or candidates: it forwards keys,
//! shows the preedit (the underlined text) it is told to show, and inserts
//! what it is told to commit. The engine owns the candidate window, except
//! in immersive hosts (the Start menu's search), where the DLL draws it from
//! what the engine sends in `State::cands` (see `Request::Hello::draws`).

use serde::{Deserialize, Serialize};
use std::io::{Read, Write};

/// The build this was compiled in (tools/build-ime.ps1 sets EARTHDESK_BUILD
/// to the commit and time); empty in other builds.
pub const BUILD: &str = match option_env!("EARTHDESK_BUILD") {
    Some(b) => b,
    None => "",
};

/// Bumped when a message changes shape; both sides check it in Hello.
pub const VERSION: u32 = 1;

/// dwExtraInfo of the keys 地球桌面 itself sends (gestures, hotkeys:
/// "Ctrl+W" and the like). The input method lets them through untouched.
pub const INJECTED: usize = 0x4544_4B31;

/// Bits of `Request::RawKey::flags`.
pub mod rawkey {
    pub const UP: u32 = 1 << 0;
    pub const EXTENDED: u32 = 1 << 1;
    pub const SHIFT: u32 = 1 << 2;
    pub const CONTROL: u32 = 1 << 3;
    pub const ALT: u32 = 1 << 4;
    /// Caps Lock is on (after this key, as Windows reports it).
    pub const CAPS: u32 = 1 << 5;
}

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
    /// The candidate window's look: "" follows the system's light / dark
    /// theme, "weather" paints the coming hours' weather along the bar,
    /// "time" tints it by the time of day. Applied without a redeploy.
    pub skin: String,
    /// An opening bracket or quote brings its partner: （ gives （|）.
    pub auto_pair: bool,
    /// Caps Lock switches to English (small letters, Shift for capitals),
    /// as on a Mac; off: it types capitals.
    pub caps_english: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { page_size: 7, emoji: false, shift_toggle: true, ascii_apps: Vec::new(), mixed: true, skin: String::new(), auto_pair: true, caps_english: true }
    }
}

/// Opening marks the input method puts in with their partner (auto_pair).
pub const PAIRS: &[(char, char)] = &[
    ('（', '）'),
    ('【', '】'),
    ('「', '」'),
    ('『', '』'),
    ('《', '》'),
    ('〈', '〉'),
    ('〔', '〕'),
    ('｛', '｝'),
    ('［', '］'),
    ('“', '”'),
    ('‘', '’'),
];

/// The partner of an opening mark.
pub fn closer_of(c: char) -> Option<char> {
    PAIRS.iter().find(|p| p.0 == c).map(|p| p.1)
}

/// Is this a closing mark of a pair?
pub fn is_closer(c: char) -> bool {
    PAIRS.iter().any(|p| p.1 == c)
}

impl Settings {
    /// The same apart from the look (which needs no redeploy).
    pub fn same_but_skin(&self, other: &Settings) -> bool {
        Settings { skin: String::new(), ..self.clone() } == Settings { skin: String::new(), ..other.clone() }
    }
}

/// The weather as EarthDesk last fetched it, written to
/// `%APPDATA%\EarthDesk\ime\weather.json` for the weather skin.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Weather {
    /// When it was fetched (Unix seconds).
    pub at: u64,
    /// Temperature now, °C.
    pub temp: f32,
    /// This hour and the ones after it: (WMO weather code, daytime).
    pub hours: Vec<(u8, bool)>,
}

/// What a WMO weather code (as Open-Meteo reports it) is called.
pub fn wmo_label(code: u8) -> &'static str {
    match code {
        0 => "晴",
        1 => "晴间多云",
        2 => "多云",
        3 => "阴",
        45 | 48 => "雾",
        51..=55 => "毛毛雨",
        56 | 57 | 66 | 67 => "冻雨",
        61 => "小雨",
        63 => "中雨",
        65 => "大雨",
        71 => "小雪",
        73 => "中雪",
        75 => "大雪",
        77 => "米雪",
        80 | 81 => "阵雨",
        82 => "暴雨",
        85 | 86 => "阵雪",
        95..=99 => "雷阵雨",
        _ => "",
    }
}

impl Weather {
    /// "18° 小雨" or, when it changes in the hours shown, "18° 小雨转晴".
    pub fn label(&self, hours: usize) -> String {
        let Some(&(now, _)) = self.hours.first() else { return String::new() };
        let mut s = format!("{}° {}", self.temp.round() as i32, wmo_label(now));
        let now_l = wmo_label(now);
        if let Some(&(later, _)) = self.hours.iter().take(hours.max(1)).find(|h| wmo_label(h.0) != now_l) {
            if !wmo_label(later).is_empty() {
                s.push('转');
                s.push_str(wmo_label(later));
            }
        }
        s
    }
}

/// How the candidate window looks (filled in by the engine from the
/// settings and the weather file).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Skin {
    /// "", "weather" or "time" (see `Settings::skin`).
    pub kind: String,
    /// Weather: this hour and the ones after it, left to right along the
    /// bar: (WMO weather code, daytime). Empty = no weather known.
    pub hours: Vec<(u8, bool)>,
    /// Weather: the temperature and conditions, for the corner.
    pub label: String,
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
        /// The DLL draws the candidate window itself, inside the host (the
        /// Start menu's search and other immersive programs, whose windows
        /// sit in a layer no ordinary topmost window can reach). The engine
        /// then sends the candidates in `State::cands` instead of showing
        /// its own window. Older DLLs leave it out.
        #[serde(default)]
        draws: bool,
        /// Which build the DLL is (`BUILD`); a program opened before an
        /// update keeps the old DLL until it is reopened. Older DLLs leave
        /// it out.
        #[serde(default)]
        build: String,
    },
    /// A key went down (or up, with mask::RELEASE).
    Key { session: u64, keycode: u32, mask: u32 },
    /// A key exactly as Windows reported it (see `rawkey`); the engine turns
    /// it into a Rime key itself, so fixes to key handling reach programs
    /// that are already open when the engine is restarted.
    RawKey { session: u64, vk: u16, scan: u16, flags: u32, hkl: u64 },
    /// Where the text caret is, in physical screen pixels (bottom-left of
    /// the composition's caret, plus the line height).
    Caret { session: u64, x: i32, y: i32, h: i32 },
    /// The host window gained or lost focus.
    Focus { session: u64, on: bool },
    /// Fetch what changed after a notification.
    Poll { session: u64 },
    /// From a DLL that draws the candidates itself: a candidate on the
    /// current page was clicked (`index`), or a page arrow / the wheel
    /// (`page` = Some(backward)). Answered with the resulting State.
    Pick { session: u64, index: Option<u32>, page: Option<bool> },
    /// From a DLL that draws the candidates itself: the user deleted
    /// candidate `index` of the current page (right click). Answered with
    /// the resulting State.
    Forget { session: u64, index: u32 },
    /// Throw the current composition away (the document lost focus).
    Reset { session: u64 },
    Bye { session: u64 },
    /// From the DLL: something went wrong on its side (an edit session the
    /// program refused...). Written to ime.log; never contains typed text.
    Note { session: u64, message: String },
    /// From the EarthDesk settings page: is the engine alive, is it busy.
    Status,
    /// From EarthDesk (tray 「刷新所有组件」): save and quit; EarthDesk starts
    /// the engine again.
    Restart,
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
    /// After inserting `commit`, put the caret this many UTF-16 units back
    /// (between a pair of brackets typed as one: （|）). Older DLLs ignore it.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub caret_back: u32,
    /// Only for sessions whose DLL draws the candidates (`Hello::draws`):
    /// what the candidate window should do now. None = leave it as it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cands: Option<CandUi>,
}

/// What the candidate window shows (the engine's `View`, minus bookkeeping).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Cands {
    /// The spelling ("ni hao", "你 hao" after a partial pick, "わたし").
    pub preedit: String,
    /// (text, comment) for each candidate on the page.
    pub candidates: Vec<(String, String)>,
    pub labels: Vec<String>,
    pub highlighted: i32,
    pub page_no: i32,
    pub last_page: bool,
    /// A short notice (mode switched): no page arrows, hides by itself.
    pub flash: bool,
    /// A column instead of a row (a Japanese word's other spellings).
    pub vertical: bool,
    /// The letters as typed, shown before the spelling when it reads
    /// differently ("rjhz" before "ran hou"); empty = the same.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub typed: String,
    #[serde(default, skip_serializing_if = "skin_is_default")]
    pub skin: Skin,
}

fn skin_is_default(s: &Skin) -> bool {
    *s == Skin::default()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum CandUi {
    Hide,
    Show { view: Cands },
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
    pub const KP_MULTIPLY: u32 = 0xffaa;
    pub const KP_ADD: u32 = 0xffab;
    pub const KP_SEPARATOR: u32 = 0xffac;
    pub const KP_SUBTRACT: u32 = 0xffad;
    pub const KP_DECIMAL: u32 = 0xffae;
    pub const KP_DIVIDE: u32 = 0xffaf;
    /// Any numeric keypad key (digits, operators, decimal point).
    pub fn is_keypad(code: u32) -> bool {
        (KP_MULTIPLY..=KP_0 + 9).contains(&code)
    }
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
        let s = Reply::State(State { eaten: true, commit: Some("你好".into()), preedit: None, ascii: false, delete_before: 0, caret_back: 0, cands: None });
        let mut buf = Vec::new();
        write_frame(&mut buf, &s).unwrap();
        assert_eq!(read_frame::<_, Reply>(&mut buf.as_slice()).unwrap(), s);
        let view = Cands { preedit: "ni".into(), candidates: vec![("你".into(), String::new())], labels: vec!["1".into()], ..Default::default() };
        let s = Reply::State(State { eaten: true, cands: Some(CandUi::Show { view }), ..Default::default() });
        let mut buf = Vec::new();
        write_frame(&mut buf, &s).unwrap();
        assert_eq!(read_frame::<_, Reply>(&mut buf.as_slice()).unwrap(), s);
        // A Hello from an older DLL (no `draws`) still parses.
        let old: Request = serde_json::from_str(r#"{"t":"hello","version":1,"pid":1,"exe":"a.exe","notify":0}"#).unwrap();
        assert!(matches!(old, Request::Hello { draws: false, ref build, .. } if build.is_empty()));
    }

    #[test]
    fn settings_defaults_fill_gaps() {
        let s: Settings = serde_json::from_str(r#"{"page_size":5}"#).unwrap();
        assert_eq!(s.page_size, 5);
        assert!(s.shift_toggle);
        assert!(s.skin.is_empty());
        assert!(s.auto_pair);
        assert_eq!(closer_of('（'), Some('）'));
        assert!(is_closer('」') && !is_closer('（'));
        let t = Settings { skin: "weather".into(), ..s.clone() };
        assert!(t.same_but_skin(&s) && t != s);
        assert!(!Settings { page_size: 6, ..s.clone() }.same_but_skin(&s));
        let w = Weather { at: 1, temp: 17.6, hours: vec![(61, true), (61, true), (0, true)] };
        assert_eq!(w.label(2), "18° 小雨");
        assert_eq!(w.label(3), "18° 小雨转晴");
        assert_eq!(tip_string(), "0804:{6F1A7C52-3B8E-4D0A-9E61-2C5D8B7E4A13}{0B9D3E27-71C4-4F5E-8A2D-9C6E1F4B7D20}");
    }
}
