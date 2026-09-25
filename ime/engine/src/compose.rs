//! How keys become text: 中文 (Rime), 日本語 (Mozc), or both at once.
//!
//! Modes, remembered per program and switched with Ctrl+Shift+J or the
//! "/" commands (/zh /ja /mix):
//!
//!   Mixed  the same letters go to both engines; mixed.rs decides which
//!          side's candidates lead. The list is a row: ←→ move the
//!          highlight, ↑↓ turn the page, Space puts the highlighted one in.
//!          On a Japanese candidate ↑↓ open its other spellings instead, in
//!          a column (↑↓ move there, ←→ turn its pages, Esc closes it).
//!   Zh     Rime alone (IME-1 behaviour).
//!   Ja     Mozc alone, native Japanese input.
//!
//! After a Japanese word is committed, ` (the key left of 1) switches it
//! between its original spelling, ひらがな and カタカナ, in place.

use crate::mixed::{self, Ctx, JaInfo, Lang, Merged, ZhInfo};
use crate::mozc::pb::commands::key_event::SpecialKey as Sk;
use crate::mozc::{self, JaKey, JaView};
use crate::session::{Engine, Sess, UiOut, View};
use ime_proto::{keysym as ks, mask, Preedit, State};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    Mixed,
    Zh,
    Ja,
}

impl Mode {
    fn label(self) -> &'static str {
        match self {
            Mode::Mixed => "中日混合",
            Mode::Zh => "中文",
            Mode::Ja => "日本語",
        }
    }
}

/// Mode per program (the user's last choice there).
pub struct ModeMemory {
    path: PathBuf,
    map: HashMap<String, Mode>,
}

impl ModeMemory {
    pub fn load(path: &Path) -> ModeMemory {
        let map = std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        ModeMemory { path: path.to_path_buf(), map }
    }

    pub fn get(&self, exe: &str) -> Option<Mode> {
        self.map.get(exe).copied()
    }

    pub fn set(&mut self, exe: &str, mode: Mode) {
        if exe.is_empty() || self.map.get(exe) == Some(&mode) {
            return;
        }
        self.map.insert(exe.to_string(), mode);
        if let Ok(b) = serde_json::to_vec_pretty(&self.map) {
            let _ = std::fs::write(&self.path, b);
        }
    }
}

/// Where a typed key went (so Backspace can replay the rest exactly).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Route {
    Both,
    Zh,
    Ja,
}

/// A composition in Mixed mode (or one handed to Mozc).
#[derive(Debug, Clone, Default)]
pub struct Comp {
    keys: Vec<(char, Route)>,
    zh: bool,
    ja: bool,
    /// Mozc's own conversion has taken over (or Ja mode).
    native: bool,
    ja_view: JaView,
    zh_pre: String,
    merged: Vec<Merged>,
    /// Highlighted candidate (index into `merged`, or Mozc's list when
    /// native).
    hl: usize,
    /// Japanese leads (decides Space, Enter, '-').
    ja_leads: bool,
    /// An English word leads (the underline shows the letters as typed).
    en_leads: bool,
    /// ↑↓ on a Japanese candidate: its other spellings, in a column.
    expand: Option<Expand>,
}

/// A Japanese word's other spellings (converted in the spare Mozc session).
#[derive(Debug, Clone, Default)]
pub struct Expand {
    view: JaView,
    hl: usize,
    reading: String,
}

impl Comp {
    fn new() -> Comp {
        Comp { zh: true, ja: true, ..Default::default() }
    }

    fn code(&self) -> String {
        self.keys.iter().map(|k| k.0).collect()
    }
}

pub struct LastJa {
    text: String,
    reading: String,
    shown: usize,
}

impl LastJa {
    fn variants(&self) -> Vec<String> {
        let mut v = vec![self.text.clone()];
        if !self.reading.is_empty() {
            for w in [self.reading.clone(), mozc::to_katakana(&self.reading)] {
                if !v.contains(&w) {
                    v.push(w);
                }
            }
        }
        v
    }
}

const COMMANDS: &[(&str, &str, Mode)] = &[
    ("mix", "hh", Mode::Mixed),
    ("zh", "zw", Mode::Zh),
    ("ja", "ry", Mode::Ja),
];

fn matching_commands(typed: &str) -> Vec<(&'static str, Mode)> {
    COMMANDS
        .iter()
        .filter(|(a, b, _)| a.starts_with(typed) || b.starts_with(typed))
        .map(|(a, _, m)| (*a, *m))
        .collect()
}

/// Japanese punctuation after Japanese text (Mixed mode).
fn ja_punct(code: u32) -> Option<&'static str> {
    Some(match char::from_u32(code)? {
        ',' => "、",
        '.' => "。",
        '[' => "「",
        ']' => "」",
        '?' => "？",
        '!' => "！",
        '~' => "～",
        _ => return None,
    })
}

fn is_shift(code: u32) -> bool {
    code == ks::SHIFT_L || code == ks::SHIFT_R
}

fn is_modifier(code: u32) -> bool {
    matches!(code, ks::SHIFT_L | ks::SHIFT_R | ks::CONTROL_L | ks::CONTROL_R | ks::ALT_L | ks::ALT_R | ks::CAPS_LOCK)
}

fn to_jakey(code: u32) -> Option<JaKey> {
    Some(match code {
        0x20 => JaKey::Special(Sk::Space),
        0x21..=0x7e => JaKey::Char(char::from_u32(code)?),
        ks::BACKSPACE => JaKey::Special(Sk::Backspace),
        ks::RETURN | ks::KP_ENTER => JaKey::Special(Sk::Enter),
        ks::ESCAPE => JaKey::Special(Sk::Escape),
        ks::TAB => JaKey::Special(Sk::Tab),
        ks::LEFT => JaKey::Special(Sk::Left),
        ks::RIGHT => JaKey::Special(Sk::Right),
        ks::UP => JaKey::Special(Sk::Up),
        ks::DOWN => JaKey::Special(Sk::Down),
        ks::HOME => JaKey::Special(Sk::Home),
        ks::END => JaKey::Special(Sk::End),
        ks::PAGE_UP => JaKey::Special(Sk::PageUp),
        ks::PAGE_DOWN => JaKey::Special(Sk::PageDown),
        ks::DELETE => JaKey::Special(Sk::Del),
        c if (ks::F1..ks::F1 + 12).contains(&c) => JaKey::Special(match c - ks::F1 {
            0 => Sk::F1,
            1 => Sk::F2,
            2 => Sk::F3,
            3 => Sk::F4,
            4 => Sk::F5,
            5 => Sk::F6,
            6 => Sk::F7,
            7 => Sk::F8,
            8 => Sk::F9,
            9 => Sk::F10,
            10 => Sk::F11,
            _ => Sk::F12,
        }),
        _ => return None,
    })
}

/// Move a highlight over `n` candidates shown `ps` to a page: `step` ±1
/// moves by one, ±ps turns the page keeping the place on it (no turning
/// past the first or last page).
fn step_hl(hl: usize, n: usize, ps: usize, step: i64) -> usize {
    if n == 0 {
        return 0;
    }
    if step.unsigned_abs() as usize >= ps && ps > 0 {
        let page = hl / ps;
        let pages = n.div_ceil(ps);
        let to = if step < 0 { page.checked_sub(1) } else { Some(page + 1).filter(|p| *p < pages) };
        return match to {
            Some(p) => (p * ps + hl % ps).min(n - 1),
            None => hl,
        };
    }
    (hl as i64 + step).clamp(0, n as i64 - 1) as usize
}

fn has_han(s: &str) -> bool {
    s.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c) || ('\u{3400}'..='\u{4dbf}').contains(&c))
}

fn pass() -> State {
    State { eaten: false, ..Default::default() }
}

fn eaten_with(preedit: Option<Preedit>) -> State {
    State { eaten: true, preedit, ..Default::default() }
}

fn preedit_end(text: &str) -> Option<Preedit> {
    (!text.is_empty()).then(|| Preedit { text: text.to_string(), cursor: text.encode_utf16().count() as u32 })
}

/// Join the results of several keys handled in a row.
fn chain(first: State, then: State) -> State {
    let commit = match (first.commit, then.commit) {
        (Some(a), Some(b)) => Some(a + &b),
        (a, b) => a.or(b),
    };
    State { eaten: true, commit, preedit: then.preedit, ascii: then.ascii, delete_before: first.delete_before, cands: None }
}

impl Engine {
    /// Everything after the key: text committed, remember what we know.
    fn committed(&self, s: &mut Sess, text: &str, lang: Lang, code: &str) {
        match lang {
            Lang::Zh => {
                if has_han(text) {
                    s.last = Some(Lang::Zh);
                }
                s.last_ja = None;
            }
            Lang::Ja => s.last = Some(Lang::Ja),
            Lang::En => {
                s.last = Some(Lang::En);
                s.last_ja = None;
            }
        }
        if !code.is_empty() && s.mode == crate::compose::Mode::Mixed {
            if let Ok(mut p) = self.pref.lock() {
                p.record(code, lang);
            }
        }
        // The sentence so far (its punctuation follows what it is mostly
        // written in, not just the last word).
        match lang {
            Lang::Zh => s.sentence[0] += text.chars().filter(|c| has_han(&c.to_string())).count() as u32,
            Lang::Ja => s.sentence[1] += text.chars().filter(|c| !c.is_ascii_punctuation() && !"、。「」？！・".contains(*c)).count() as u32,
            Lang::En => s.sentence[2] += text.chars().any(|c| c.is_ascii_alphanumeric()) as u32,
        }
        if text.chars().last().map(|c| "。！？.!?\n".contains(c)).unwrap_or(false) {
            s.sentence = [0; 3];
        }
        s.recent.push_str(text);
        let n = s.recent.chars().count();
        if n > 24 {
            s.recent = s.recent.chars().skip(n - 24).collect();
        }
    }

    /// The language of the sentence being typed: Japanese if most of it is
    /// Japanese, Chinese if it has Chinese in it, English if it is all
    /// English; the last word's language when nothing is typed yet.
    fn sentence_lang(&self, s: &Sess) -> Option<Lang> {
        let [zh, ja, en] = s.sentence;
        if ja > zh {
            Some(Lang::Ja)
        } else if zh > 0 {
            Some(Lang::Zh)
        } else if en > 0 {
            Some(Lang::En)
        } else {
            s.last
        }
    }

    #[allow(dead_code)]
    pub(crate) fn bench_committed(&self, s: &mut Sess, text: &str, lang: Lang) {
        self.committed(s, text, lang, "");
    }

    fn rime_ascii(&self, s: &mut Sess) -> bool {
        let rs = self.rime_session(s);
        self.rime.get_option(rs, "ascii_mode")
    }

    fn flash(&self, session: u64, text: &str) -> UiOut {
        UiOut::Show(View { session, candidates: vec![(text.to_string(), String::new())], labels: vec![String::new()], highlighted: -1, last_page: true, flash: true, ..Default::default() })
    }

    fn set_mode(&self, s: &mut Sess, session: u64, mode: Mode) -> (State, UiOut) {
        let mode = if mode != Mode::Zh && self.mozc.is_none() { Mode::Zh } else { mode };
        self.clear_comp(s);
        s.mode = mode;
        if let Ok(mut m) = self.modes.lock() {
            m.set(&s.exe, mode);
        }
        let text = if self.mozc.is_none() { "中文（日语引擎没有安装）" } else { mode.label() };
        (eaten_with(None), self.flash(session, text))
    }

    /// The letters of the composition as they were typed (a "/" command,
    /// the Mixed / Japanese composition, or Rime's).
    fn typed_so_far(&self, s: &mut Sess) -> String {
        if let Some(c) = &s.cmd {
            return format!("/{c}");
        }
        if let Some(c) = &s.comp {
            let code = c.code();
            if !code.is_empty() {
                return code;
            }
            return c.ja_view.preedit.clone();
        }
        let rs = self.rime_session(s);
        self.rime.raw_input(rs)
    }

    fn clear_comp(&self, s: &mut Sess) {
        if s.rime != 0 {
            self.rime.clear(s.rime);
        }
        if let (Some(m), true) = (&self.mozc, s.mozc != 0) {
            m.revert(s.mozc);
        }
        s.comp = None;
        s.cmd = None;
    }

    /// One key from the DLL.
    pub(crate) fn key(&self, s: &mut Sess, session: u64, code: u32, m: u32) -> (State, UiOut) {
        let up = m & mask::RELEASE != 0;
        let ctrl_alt = m & (mask::CONTROL | mask::ALT) != 0;
        let plain = !up && !ctrl_alt;

        // Ctrl+Shift+J: next mode.
        if !up && m & mask::CONTROL != 0 && m & mask::SHIFT != 0 && m & mask::ALT == 0 && (code == 'J' as u32 || code == 'j' as u32) {
            let next = match s.mode {
                Mode::Mixed => Mode::Ja,
                Mode::Ja => Mode::Zh,
                Mode::Zh => Mode::Mixed,
            };
            return self.set_mode(s, session, next);
        }

        let idle = s.comp.is_none() && s.cmd.is_none() && !self.rime.snapshot(self.rime_session(s)).composing;

        // Caps Lock: capitals, typed straight into the program. Pressed in
        // the middle of a word, what was typed so far goes in as it is.
        if code == ks::CAPS_LOCK {
            if up || idle {
                return (pass(), UiOut::Keep);
            }
            let typed = self.typed_so_far(s);
            self.clear_comp(s);
            if !typed.is_empty() {
                self.committed(s, &typed, Lang::En, "");
            }
            return (State { eaten: false, commit: (!typed.is_empty()).then_some(typed), ..Default::default() }, UiOut::Hide);
        }
        if m & mask::LOCK != 0 && !ctrl_alt && char::from_u32(code).map(|c| c.is_ascii_alphabetic()).unwrap_or(false) {
            if up {
                return (pass(), UiOut::Keep);
            }
            if idle {
                if let Some(c) = char::from_u32(code) {
                    let t = c.to_string();
                    self.committed(s, &t, Lang::En, "");
                }
                return (pass(), UiOut::Hide);
            }
        }
        if plain && idle && (code == ks::RETURN || code == ks::KP_ENTER) {
            s.sentence = [0; 3];
        }

        // ` right after a Japanese word: switch its spelling.
        if plain && code == '`' as u32 && idle {
            if let Some(last) = s.last_ja.as_mut() {
                let v = last.variants();
                if v.len() > 1 {
                    let old = v[last.shown % v.len()].clone();
                    last.shown = (last.shown + 1) % v.len();
                    let new = v[last.shown].clone();
                    s.recent.push_str(&new);
                    return (
                        State { eaten: true, commit: Some(new), delete_before: old.encode_utf16().count() as u32, ..Default::default() },
                        self.flash(session, &format!("{}／{}", last.shown + 1, v.len())),
                    );
                }
            }
        }
        if !up && !is_modifier(code) {
            s.last_ja = None;
        }

        // "/" commands.
        if s.cmd.is_some() {
            return self.cmd_key(s, session, code, m);
        }
        if plain && code == '/' as u32 && idle && !self.rime_ascii(s) {
            s.cmd = Some(String::new());
            return (eaten_with(preedit_end("/")), self.cmd_view(session, ""));
        }

        self.dispatch(s, session, code, m)
    }

    fn dispatch(&self, s: &mut Sess, session: u64, code: u32, m: u32) -> (State, UiOut) {
        match s.mode {
            Mode::Zh => self.zh_key(s, session, code, m),
            Mode::Ja if self.mozc.is_some() => self.ja_mode_key(s, session, code, m),
            Mode::Mixed if self.mozc.is_some() => self.mixed_key(s, session, code, m),
            _ => self.zh_key(s, session, code, m),
        }
    }

    // --- Chinese only ------------------------------------------------------------

    fn zh_key(&self, s: &mut Sess, session: u64, code: u32, m: u32) -> (State, UiOut) {
        let rs = self.rime_session(s);
        // The list is a row: ←→ move the highlight, ↑↓ turn the page (not
        // the caret in the spelling).
        let code = if matches!(code, ks::LEFT | ks::RIGHT | ks::UP | ks::DOWN) && m & (mask::CONTROL | mask::ALT | mask::SHIFT) == 0 && self.rime.snapshot(rs).composing {
            match code {
                ks::LEFT => ks::UP,
                ks::RIGHT => ks::DOWN,
                ks::UP => ks::PAGE_UP,
                _ => ks::PAGE_DOWN,
            }
        } else {
            code
        };
        // The keypad types digits and signs as they are: finish what is being
        // composed (first candidate), then let the key through.
        if ks::is_keypad(code) && m & mask::RELEASE == 0 {
            let mut commit = None;
            if self.rime.snapshot(rs).composing {
                self.rime.process_key(rs, 0x20, 0);
                commit = self.rime.snapshot(rs).commit.filter(|c| !c.is_empty());
                if let Some(c) = &commit {
                    self.committed(s, c, Lang::Zh, "");
                }
                self.rime.clear(rs);
            }
            return (State { eaten: false, commit, ..Default::default() }, UiOut::Hide);
        }
        let eaten = self.rime.process_key(rs, code, m);
        let snap = self.rime.snapshot(rs);
        if let Some(c) = snap.commit.clone().filter(|c| !c.is_empty()) {
            self.committed(s, &c, Lang::Zh, "");
        }
        (self.to_state(eaten, &snap), self.rime_ui(session, &snap))
    }

    // --- Commands --------------------------------------------------------------------

    fn cmd_view(&self, session: u64, typed: &str) -> UiOut {
        let list = matching_commands(typed);
        UiOut::Show(View {
            session,
            preedit: format!("/{typed}"),
            candidates: list.iter().map(|(name, mode)| (mode.label().to_string(), format!("/{name}"))).collect(),
            labels: (1..=list.len()).map(|i| i.to_string()).collect(),
            highlighted: 0,
            last_page: true,
            ..Default::default()
        })
    }

    fn cmd_key(&self, s: &mut Sess, session: u64, code: u32, m: u32) -> (State, UiOut) {
        let typed = s.cmd.clone().unwrap_or_default();
        if m & mask::RELEASE != 0 || is_modifier(code) {
            return (eaten_with(preedit_end(&format!("/{typed}"))), UiOut::Keep);
        }
        let list = matching_commands(&typed);
        let ch = char::from_u32(code).filter(|c| c.is_ascii_graphic());
        match code {
            ks::ESCAPE => {
                s.cmd = None;
                (eaten_with(None), UiOut::Hide)
            }
            ks::BACKSPACE => {
                if typed.is_empty() {
                    s.cmd = None;
                    return (eaten_with(None), UiOut::Hide);
                }
                let mut t = typed;
                t.pop();
                let v = self.cmd_view(session, &t);
                s.cmd = Some(t.clone());
                (eaten_with(preedit_end(&format!("/{t}"))), v)
            }
            ks::RETURN | ks::KP_ENTER => {
                s.cmd = None;
                (State { eaten: true, commit: Some(format!("/{typed}")), ..Default::default() }, UiOut::Hide)
            }
            0x20 if !list.is_empty() => {
                s.cmd = None;
                self.set_mode(s, session, list[0].1)
            }
            c if (('1' as u32)..=('9' as u32)).contains(&c) && m & (mask::CONTROL | mask::ALT) == 0 => {
                let i = (c - '1' as u32) as usize;
                if i < list.len() {
                    s.cmd = None;
                    return self.set_mode(s, session, list[i].1);
                }
                self.cmd_replay(s, session, code, m)
            }
            _ if ch.map(|c| c.is_ascii_lowercase()).unwrap_or(false) && m & (mask::CONTROL | mask::ALT) == 0 => {
                let t = format!("{typed}{}", ch.unwrap());
                if matching_commands(&t).is_empty() {
                    return self.cmd_replay(s, session, code, m);
                }
                let v = self.cmd_view(session, &t);
                s.cmd = Some(t.clone());
                (eaten_with(preedit_end(&format!("/{t}"))), v)
            }
            _ => self.cmd_replay(s, session, code, m),
        }
    }

    /// Not a command after all: type "/" and what followed as usual, then
    /// this key.
    fn cmd_replay(&self, s: &mut Sess, session: u64, code: u32, m: u32) -> (State, UiOut) {
        let typed = s.cmd.take().unwrap_or_default();
        let (mut st, mut ui) = self.dispatch(s, session, '/' as u32, 0);
        for c in typed.chars() {
            let (b, u) = self.dispatch(s, session, c as u32, 0);
            st = chain(st, b);
            ui = u;
        }
        let (b, u) = self.dispatch(s, session, code, m);
        st = chain(st, b);
        if !matches!(u, UiOut::Keep) {
            ui = u;
        }
        (st, ui)
    }

    // --- Japanese only ---------------------------------------------------------------

    fn ja_mode_key(&self, s: &mut Sess, session: u64, code: u32, m: u32) -> (State, UiOut) {
        let composing = s.comp.as_ref().map(|c| c.ja_view.composing()).unwrap_or(false);
        // English mode (Shift) and everything outside a composition that Mozc
        // would not type anyway goes through Rime's English handling.
        if !composing && (is_shift(code) || self.rime_ascii(s)) {
            return self.zh_key(s, session, code, m);
        }
        if s.comp.is_none() {
            s.comp = Some(Comp { native: true, ja: true, ..Default::default() });
        }
        self.native_key(s, session, code, m)
    }

    /// Mozc handles the key itself (Ja mode, or after Space in Mixed).
    fn native_key(&self, s: &mut Sess, session: u64, code: u32, m: u32) -> (State, UiOut) {
        let up = m & mask::RELEASE != 0;
        let ctrl_alt = m & (mask::CONTROL | mask::ALT) != 0;
        let composing = s.comp.as_ref().map(|c| c.ja_view.composing()).unwrap_or(false);
        if up || is_modifier(code) {
            let pre = s.comp.as_ref().and_then(|c| self.native_preedit(&c.ja_view));
            return (State { eaten: composing, preedit: pre, ..Default::default() }, UiOut::Keep);
        }
        if ctrl_alt && !composing {
            s.comp = None;
            return (pass(), UiOut::Keep);
        }
        let Some(mid) = self.mozc_session(s) else { return self.zh_key(s, session, code, m) };
        let mz = self.mozc.as_ref().unwrap();
        let ps = self.page_size();

        let converting = s.comp.as_ref().map(|c| c.ja_view.converting).unwrap_or(false);
        let n = s.comp.as_ref().map(|c| c.ja_view.candidates.len()).unwrap_or(0);
        if composing && !ctrl_alt {
            // The list is a row: ←→ move the highlight, ↑↓ turn the page;
            // Space puts the highlighted one in.
            let step: i64 = match code {
                ks::LEFT => -1,
                ks::RIGHT | ks::TAB => 1,
                ks::UP | ks::PAGE_UP => -(ps as i64),
                ks::DOWN | ks::PAGE_DOWN => ps as i64,
                _ => 0,
            };
            if step != 0 {
                if converting {
                    // Mozc moves its focus one by one: a page is ps steps.
                    let k = if step < 0 { Sk::Up } else { Sk::Down };
                    let focus = s.comp.as_ref().and_then(|c| c.ja_view.focused).unwrap_or(0) as i64;
                    let target = if step.abs() > 1 { (((focus + step) / ps as i64) * ps as i64).clamp(0, n.max(1) as i64 - 1) } else { focus + step };
                    let times = if step.abs() > 1 { (target - focus).unsigned_abs() as usize } else { 1 };
                    let mut v = None;
                    for _ in 0..times.max(1) {
                        v = mz.send_key(mid, JaKey::Special(k), false, &s.recent);
                        if v.is_none() {
                            break;
                        }
                    }
                    return self.native_done(s, session, v);
                }
                if let Some(c) = s.comp.as_mut() {
                    c.hl = step_hl(c.hl, n, ps, step);
                }
                let c = s.comp.as_ref().unwrap();
                return (eaten_with(self.native_preedit(&c.ja_view)), self.ja_list_view(session, &c.ja_view, c.hl));
            }
            if code == 0x20 && m & mask::SHIFT == 0 {
                let v = if converting {
                    mz.submit(mid)
                } else {
                    match s.comp.as_ref().and_then(|c| c.ja_view.candidates.get(c.hl).map(|x| x.id)) {
                        Some(id) => mz.submit_candidate(mid, id),
                        None => mz.submit(mid),
                    }
                };
                return self.native_done(s, session, v);
            }
        }

        // Digits pick from our page of the list.
        if !ctrl_alt && (('1' as u32)..=('9' as u32)).contains(&code) {
            if let Some(c) = s.comp.as_ref().filter(|c| !c.ja_view.candidates.is_empty()) {
                let page = if c.ja_view.converting { c.ja_view.focused.unwrap_or(0) } else { c.hl } / ps;
                let i = page * ps + (code - '1' as u32) as usize;
                if let Some(cand) = c.ja_view.candidates.get(i) {
                    let id = cand.id;
                    let v = if c.ja_view.converting {
                        let v = mz.select_candidate(mid, id);
                        match v {
                            Some(v) if v.segments <= 1 => mz.submit(mid),
                            other => other,
                        }
                    } else {
                        mz.submit_candidate(mid, id)
                    };
                    return self.native_done(s, session, v);
                }
                return (eaten_with(self.native_preedit(&c.ja_view)), UiOut::Keep);
            }
        }

        let Some(k) = to_jakey(code) else {
            return if composing { (eaten_with(s.comp.as_ref().and_then(|c| self.native_preedit(&c.ja_view))), UiOut::Keep) } else { (pass(), UiOut::Keep) };
        };
        let shift = m & mask::SHIFT != 0 && matches!(k, JaKey::Special(_));
        let v = mz.send_key(mid, k, shift, &s.recent);
        self.native_done(s, session, v)
    }

    fn native_preedit(&self, v: &JaView) -> Option<Preedit> {
        if v.preedit.is_empty() {
            return None;
        }
        let cursor: String = v.preedit.chars().take(v.cursor).collect();
        Some(Preedit { text: v.preedit.clone(), cursor: cursor.encode_utf16().count() as u32 })
    }

    fn native_done(&self, s: &mut Sess, session: u64, v: Option<JaView>) -> (State, UiOut) {
        let Some(v) = v else {
            s.comp = None;
            return (pass(), UiOut::Hide);
        };
        let code = s.comp.as_ref().map(|c| c.code()).unwrap_or_default();
        let was_composing = s.comp.as_ref().map(|c| c.ja_view.composing()).unwrap_or(false);
        let mut st = State { eaten: v.consumed || was_composing, ..Default::default() };
        if let Some((text, reading)) = v.commit.clone() {
            self.committed(s, &text, Lang::Ja, &code);
            s.last_ja = Some(LastJa { text: text.clone(), reading, shown: 0 });
            st.commit = Some(text);
            st.eaten = true;
        }
        if !v.composing() {
            s.comp = None;
            return (st, UiOut::Hide);
        }
        st.preedit = self.native_preedit(&v);
        let ui = self.ja_list_view(session, &v, 0);
        if let Some(c) = s.comp.as_mut() {
            c.hl = 0;
            c.ja_view = v;
        }
        (st, ui)
    }

    /// `hl`: our highlight in the suggestion list (Mozc's own focus is used
    /// while it converts, after F6-F10).
    fn ja_list_view(&self, session: u64, v: &JaView, hl: usize) -> UiOut {
        if v.candidates.is_empty() {
            return UiOut::Hide;
        }
        let ps = self.page_size();
        let focus = if v.converting { v.focused.unwrap_or(0) } else { hl }.min(v.candidates.len() - 1);
        let page = focus / ps;
        let items: Vec<(String, String)> = v.candidates.iter().skip(page * ps).take(ps).map(|c| (c.value.clone(), c.description.clone())).collect();
        UiOut::Show(View {
            session,
            preedit: v.reading.clone(),
            labels: (1..=items.len()).map(|i| i.to_string()).collect(),
            candidates: items,
            highlighted: (focus - page * ps) as i32,
            page_no: page as i32,
            last_page: (page + 1) * ps >= v.candidates.len(),
            flash: false,
            vertical: false,
            ..Default::default()
        })
    }

    // --- Mixed -----------------------------------------------------------------------

    fn mixed_key(&self, s: &mut Sess, session: u64, code: u32, m: u32) -> (State, UiOut) {
        let up = m & mask::RELEASE != 0;
        let ctrl_alt = m & (mask::CONTROL | mask::ALT) != 0;
        let plain = !up && !ctrl_alt;
        let rs = self.rime_session(s);

        if s.comp.is_none() {
            let rime_busy = self.rime.snapshot(rs).composing;
            let lower = plain && (('a' as u32)..=('z' as u32)).contains(&code);
            if lower && !rime_busy && !self.rime_ascii(s) {
                s.comp = Some(Comp::new());
                if let Some(mz) = &self.mozc {
                    if let Some(mid) = self.mozc_session(s) {
                        mz.revert(mid);
                    }
                }
            } else {
                // Punctuation follows the sentence being typed: 、。 in a
                // Japanese one, , . in an English one, ，。 (Rime) in a
                // Chinese one even right after an English word in it.
                let sentence = self.sentence_lang(s);
                if plain && !rime_busy && sentence == Some(Lang::Ja) && !self.rime_ascii(s) {
                    if let Some(p) = ja_punct(code) {
                        self.committed(s, p, Lang::Ja, "");
                        return (State { eaten: true, commit: Some(p.into()), ..Default::default() }, UiOut::Hide);
                    }
                }
                // Right after an English word Space is a space.
                if plain && !rime_busy && code == 0x20 && s.last == Some(Lang::En) && !self.rime_ascii(s) {
                    self.committed(s, " ", Lang::En, "");
                    return (State { eaten: true, commit: Some(" ".into()), ..Default::default() }, UiOut::Hide);
                }
                if plain && !rime_busy && sentence == Some(Lang::En) && !self.rime_ascii(s) {
                    if let Some(p) = char::from_u32(code).filter(|c| ",.?!;:'\"".contains(*c)) {
                        let p = p.to_string();
                        self.committed(s, &p, Lang::En, "");
                        return (State { eaten: true, commit: Some(p), ..Default::default() }, UiOut::Hide);
                    }
                }
                return self.zh_key(s, session, code, m);
            }
        }
        if s.comp.as_ref().map(|c| c.native).unwrap_or(false) {
            return self.native_key(s, session, code, m);
        }

        // Key-ups and modifiers: keep the composition as it is (Rime still
        // sees Shift, for its Chinese / English switch).
        if up || is_modifier(code) {
            if is_shift(code) {
                self.rime.process_key(rs, code, m);
                let snap = self.rime.snapshot(rs);
                if let Some(c) = snap.commit.clone().filter(|c| !c.is_empty()) {
                    // Shift alone: Rime put the letters out as typed and
                    // switched to English.
                    self.clear_comp(s);
                    return (State { eaten: true, commit: Some(c), ascii: snap.ascii, ..Default::default() }, UiOut::Hide);
                }
            }
            return (eaten_with(self.mixed_preedit(s)), UiOut::Keep);
        }
        if ctrl_alt {
            // Ctrl+V, Ctrl+A ... mean the program's command, not typing:
            // drop the unfinished letters and let the key through.
            self.clear_comp(s);
            return (State { eaten: false, ..Default::default() }, UiOut::Hide);
        }

        let ps = self.page_size();
        if s.comp.as_ref().map(|c| c.expand.is_some()).unwrap_or(false) {
            if let Some(r) = self.expand_key(s, session, code) {
                return r;
            }
        }
        let ch = char::from_u32(code);
        let ja_leads = s.comp.as_ref().map(|c| c.ja_leads).unwrap_or(false);
        let hl_ja = s.comp.as_ref().and_then(|c| c.merged.get(c.hl)).map(|m| m.lang == Lang::Ja).unwrap_or(false);
        match code {
            c if (('a' as u32)..=('z' as u32)).contains(&c) => self.push(s, session, ch.unwrap(), Route::Both),
            c if (('A' as u32)..=('Z' as u32)).contains(&c) => {
                let r = if s.comp.as_ref().map(|c| c.zh).unwrap_or(false) { Route::Zh } else { Route::Ja };
                self.push(s, session, ch.unwrap(), r)
            }
            c if c == ';' as u32 || c == '\'' as u32 => {
                let r = if s.comp.as_ref().map(|c| c.zh).unwrap_or(false) { Route::Zh } else { Route::Ja };
                self.push(s, session, ch.unwrap(), r)
            }
            c if c == '-' as u32 && ja_leads => self.push(s, session, '-', Route::Ja),
            c if c == '-' as u32 || c == ks::PAGE_UP => self.move_hl(s, session, -(ps as i64), true),
            c if c == '=' as u32 || c == ks::PAGE_DOWN => self.move_hl(s, session, ps as i64, true),
            // The list is a row: ←→ move the highlight, ↑↓ turn the page —
            // or, on a Japanese word, open its other spellings below it.
            ks::UP | ks::DOWN if hl_ja => self.expand_ja(s, session, code == ks::UP),
            ks::UP => self.move_hl(s, session, -(ps as i64), true),
            ks::DOWN => self.move_hl(s, session, ps as i64, true),
            ks::LEFT => self.move_hl(s, session, -1, false),
            ks::RIGHT | ks::TAB => self.move_hl(s, session, 1, false),
            c if ks::is_keypad(c) => {
                // The keypad types digits and signs, never picks: put the
                // highlighted candidate in, then let the key through.
                let hl = s.comp.as_ref().map(|c| c.hl).unwrap_or(0);
                let has = s.comp.as_ref().map(|c| !c.merged.is_empty()).unwrap_or(false);
                let mut st = if has {
                    self.pick_merged(s, session, hl).0
                } else {
                    let text = s.comp.as_ref().map(|c| c.code()).unwrap_or_default();
                    self.clear_comp(s);
                    State { eaten: true, commit: Some(text), ..Default::default() }
                };
                if s.comp.is_none() {
                    st.eaten = false;
                }
                (st, UiOut::Hide)
            }
            ks::BACKSPACE => {
                if let Some(c) = s.comp.as_mut() {
                    c.keys.pop();
                }
                if s.comp.as_ref().map(|c| c.keys.is_empty()).unwrap_or(true) {
                    self.clear_comp(s);
                    return (eaten_with(None), UiOut::Hide);
                }
                self.rebuild(s);
                self.refresh(s, session)
            }
            ks::ESCAPE => {
                self.clear_comp(s);
                (eaten_with(None), UiOut::Hide)
            }
            ks::RETURN | ks::KP_ENTER => {
                let c = s.comp.clone().unwrap_or_default();
                let text = if c.ja_leads && c.ja { c.ja_view.preedit.clone() } else { c.code() };
                self.clear_comp(s);
                if c.ja_leads {
                    self.committed(s, &text, Lang::Ja, &c.code());
                }
                (State { eaten: true, commit: Some(text), ..Default::default() }, UiOut::Hide)
            }
            0x20 => {
                let c = s.comp.clone().unwrap_or_default();
                if c.merged.is_empty() {
                    let text = c.code();
                    self.clear_comp(s);
                    return (State { eaten: true, commit: Some(text), ..Default::default() }, UiOut::Hide);
                }
                // Space puts in the highlighted candidate, whatever its
                // language (←→↑↓ move the highlight).
                self.pick_merged(s, session, c.hl)
            }
            c if (ks::F1 + 5..=ks::F1 + 9).contains(&c) && s.comp.as_ref().map(|c| c.ja).unwrap_or(false) => {
                // F6-F10: ひらがな / カタカナ / 半角 / 英数 conversions.
                self.hand_to_mozc(s, session, to_jakey(c).unwrap(), false)
            }
            c if (('1' as u32)..=('9' as u32)).contains(&c) => {
                let hl = s.comp.as_ref().map(|c| c.hl).unwrap_or(0);
                let i = (hl / ps) * ps + (c - '1' as u32) as usize;
                if i < s.comp.as_ref().map(|c| c.merged.len()).unwrap_or(0) {
                    self.pick_merged(s, session, i)
                } else {
                    (eaten_with(self.mixed_preedit(s)), UiOut::Keep)
                }
            }
            c if (0x21..=0x7e).contains(&c) => {
                // Punctuation (or 0): commit the highlighted candidate, then
                // type the key as usual.
                let hl = s.comp.as_ref().map(|c| c.hl).unwrap_or(0);
                let has = s.comp.as_ref().map(|c| !c.merged.is_empty()).unwrap_or(false);
                let first = if has {
                    self.pick_merged(s, session, hl).0
                } else {
                    let text = s.comp.as_ref().map(|c| c.code()).unwrap_or_default();
                    self.clear_comp(s);
                    State { eaten: true, commit: Some(text), ..Default::default() }
                };
                if s.comp.is_some() {
                    // A partial Chinese pick: the rest is still composing.
                    return (first, self.rime_ui(session, &self.rime.snapshot(rs)));
                }
                let (then, ui) = self.mixed_key(s, session, code, m);
                (chain(first, then), ui)
            }
            _ => (eaten_with(self.mixed_preedit(s)), UiOut::Keep),
        }
    }

    /// Feed one key to the engines still in play.
    fn feed(&self, s: &mut Sess, c: char, route: Route) {
        let rs = self.rime_session(s);
        let mid = self.mozc_session(s);
        let recent = s.recent.clone();
        let Some(comp) = s.comp.as_mut() else { return };
        match route {
            Route::Zh => comp.ja = false,
            Route::Ja => comp.zh = false,
            Route::Both => {}
        }
        if comp.zh {
            self.rime.process_key(rs, c as u32, if c.is_ascii_uppercase() { mask::SHIFT } else { 0 });
        }
        if comp.ja {
            match (self.mozc.as_ref(), mid) {
                (Some(mz), Some(mid)) => match mz.send_key(mid, JaKey::Char(c), false, &recent) {
                    Some(v) => comp.ja_view = v,
                    None => comp.ja = false,
                },
                _ => comp.ja = false,
            }
        }
    }

    fn push(&self, s: &mut Sess, session: u64, c: char, route: Route) -> (State, UiOut) {
        if let Some(comp) = s.comp.as_mut() {
            comp.keys.push((c, route));
            comp.hl = 0;
        }
        self.feed(s, c, route);
        let (st, ui) = self.refresh(s, session);
        (st, ui)
    }

    /// Replay the typed keys from scratch (after Backspace).
    fn rebuild(&self, s: &mut Sess) {
        let keys = s.comp.as_ref().map(|c| c.keys.clone()).unwrap_or_default();
        if s.rime != 0 {
            self.rime.clear(s.rime);
        }
        if let (Some(m), true) = (&self.mozc, s.mozc != 0) {
            m.revert(s.mozc);
        }
        if let Some(c) = s.comp.as_mut() {
            c.zh = true;
            c.ja = true;
            c.ja_view = JaView::default();
            c.hl = 0;
        }
        for (ch, route) in keys {
            self.feed(s, ch, route);
        }
    }

    fn move_hl(&self, s: &mut Sess, session: u64, by: i64, page: bool) -> (State, UiOut) {
        let ps = self.page_size() as i64;
        if let Some(c) = s.comp.as_mut() {
            let _ = page;
            c.hl = step_hl(c.hl, c.merged.len(), ps as usize, by);
        }
        (eaten_with(self.mixed_preedit(s)), self.mixed_view(s, session))
    }

    /// Recompute both sides and the merged list.
    fn refresh(&self, s: &mut Sess, session: u64) -> (State, UiOut) {
        let rs = self.rime_session(s);
        let last = s.last;
        let s_recent = s.recent.clone();
        let code = s.comp.as_ref().map(|c| c.code()).unwrap_or_default();
        let (zp, jp) = self.pref.lock().map(|p| p.get(&code)).unwrap_or((0, 0));
        let Some(comp) = s.comp.as_mut() else { return (eaten_with(None), UiOut::Hide) };

        let (zh_cands, zinfo) = if comp.zh {
            let snap = self.rime.snapshot(rs);
            if snap.composing {
                comp.zh_pre = snap.preedit.map(|p| p.0).unwrap_or_default();
                let c = self.rime.candidates(rs, 40);
                let info = mixed::zh_info(&comp.zh_pre, &c);
                (c, info)
            } else {
                comp.zh = false;
                (Vec::new(), ZhInfo::default())
            }
        } else {
            (Vec::new(), ZhInfo::default())
        };
        let jinfo = if comp.ja { mixed::ja_info(&comp.ja_view) } else { JaInfo::default() };
        if std::env::var_os("EARTHDESK_IME_TRACE").is_some() {
            eprintln!("  zh={zinfo:?} ja={jinfo:?} ja_pre={:?} reading={:?} ja_cands={:?}", comp.ja_view.preedit, comp.ja_view.reading, comp.ja_view.candidates.iter().take(4).map(|c| (c.value.as_str(), c.key.as_str(), c.segments, c.description.as_str())).collect::<Vec<_>>());
        }
        let (sz, sj) = mixed::score(zinfo, jinfo, Ctx { last, zh_picks: zp, ja_picks: jp });
        // Half-typed romaji: Mozc's predictions are guesses; show only the
        // best two so they do not crowd out the Chinese words.
        let ja_all: &[mozc::JaCand] = if comp.ja { &comp.ja_view.candidates } else { &[] };
        let ja_cands = if jinfo.complete { ja_all } else { &ja_all[..ja_all.len().min(2)] };
        let zh_wins = sz.unwrap_or(f32::MIN) >= sj.unwrap_or(f32::MIN);
        let loser_real = if zh_wins { jinfo.hit } else { zinfo.whole >= 1 && !zinfo.partial };
        comp.merged = mixed::merge_with(&zh_cands, ja_cands, sz, sj, loser_real);
        // English: the letters as typed, if they are an English word.
        let words = if comp.keys.iter().all(|k| k.0.is_ascii_alphabetic()) { self.en.lookup(&code, mixed::sentence_start(&s_recent)) } else { Vec::new() };
        if let Some(se) = mixed::en_score(&code, !words.is_empty(), last) {
            let best = match (sz, sj) {
                (Some(a), Some(b)) => Some(a.max(b)),
                (a, b) => a.or(b),
            };
            mixed::insert_english(&mut comp.merged, &words, se, best);
        }
        // The letters themselves, last (first when nothing reads them):
        // an English word the dictionary does not know ("claude") can be
        // picked like any other, and the window never vanishes mid-word.
        if comp.keys.iter().all(|k| k.0.is_ascii_alphabetic()) && !code.is_empty() && !comp.merged.iter().any(|m| m.text == code) {
            let key = comp.merged.iter().filter(|m| m.lang == Lang::En).count() as i64;
            comp.merged.push(mixed::Merged { lang: Lang::En, key, text: code.clone(), comment: "英".into() });
        }
        comp.en_leads = comp.merged.first().map(|m| m.lang == Lang::En).unwrap_or(false);
        comp.ja_leads = match comp.merged.first() {
            Some(m) => m.lang == Lang::Ja,
            None => matches!((sz, sj), (None, Some(_))) || matches!((sz, sj), (Some(a), Some(b)) if b > a),
        };
        comp.hl = comp.hl.min(comp.merged.len().saturating_sub(1));
        (eaten_with(self.mixed_preedit(s)), self.mixed_view(s, session))
    }

    /// The underlined text in the program: what Enter would put in — the
    /// kana when Japanese leads, otherwise the letters exactly as typed
    /// (双拼 reads "rjhz" as "ran hou", but an English word must be seen
    /// as it is spelled; the candidate window shows the reading).
    fn mixed_preedit(&self, s: &Sess) -> Option<Preedit> {
        let c = s.comp.as_ref()?;
        if let Some(e) = &c.expand {
            if let Some(x) = e.view.candidates.get(e.hl) {
                return preedit_end(&x.value);
            }
        }
        let text = if !c.en_leads && c.ja_leads && c.ja && !c.ja_view.preedit.is_empty() { c.ja_view.preedit.clone() } else { c.code() };
        preedit_end(&text)
    }

    /// The reading for the candidate window's first line: the leading
    /// side's spelling ("ran hou", "わたし"), or the letters.
    fn mixed_reading(&self, c: &Comp) -> String {
        if !c.en_leads && c.ja_leads && c.ja && !c.ja_view.preedit.is_empty() {
            c.ja_view.preedit.clone()
        } else if !c.en_leads && c.zh && !c.zh_pre.is_empty() {
            c.zh_pre.clone()
        } else {
            c.code()
        }
    }

    fn mixed_view(&self, s: &Sess, session: u64) -> UiOut {
        let Some(c) = s.comp.as_ref() else { return UiOut::Hide };
        if c.merged.is_empty() {
            return UiOut::Hide;
        }
        let ps = self.page_size();
        let page = c.hl / ps;
        let items: Vec<(String, String)> = c.merged.iter().skip(page * ps).take(ps).map(|m| (m.text.clone(), m.comment.clone())).collect();
        UiOut::Show(View {
            session,
            preedit: self.mixed_reading(c),
            labels: (1..=items.len()).map(|i| i.to_string()).collect(),
            candidates: items,
            highlighted: (c.hl - page * ps) as i32,
            page_no: page as i32,
            last_page: (page + 1) * ps >= c.merged.len(),
            flash: false,
            vertical: false,
            typed: c.code(),
            ..Default::default()
        })
    }

    /// ↑↓ on a Japanese candidate: list its other spellings in a column.
    fn expand_ja(&self, s: &mut Sess, session: u64, from_below: bool) -> (State, UiOut) {
        let keep = |e: &Engine, s: &Sess| (eaten_with(e.mixed_preedit(s)), UiOut::Keep);
        let Some(c) = s.comp.as_ref() else { return keep(self, s) };
        let Some(item) = c.merged.get(c.hl).cloned() else { return keep(self, s) };
        let reading = c
            .ja_view
            .candidates
            .iter()
            .find(|x| x.id as i64 == item.key)
            .map(|x| x.key.clone())
            .filter(|k| !k.is_empty())
            .unwrap_or_else(|| c.ja_view.reading.clone());
        let recent = s.recent.clone();
        let (Some(mz), Some(aux)) = (self.mozc.as_ref(), self.mozc_aux(s)) else { return keep(self, s) };
        let Some(v) = mz.alternatives(aux, &reading, &recent) else { return keep(self, s) };
        // Start on the word itself (↓ then goes on to the next spelling).
        let at = v.candidates.iter().position(|x| x.value == item.text);
        let hl = match at {
            Some(i) => i,
            None if from_below => v.candidates.len().saturating_sub(1),
            None => 0,
        };
        if let Some(c) = s.comp.as_mut() {
            c.expand = Some(Expand { view: v, hl, reading });
        }
        (eaten_with(self.mixed_preedit(s)), self.expand_view(s, session))
    }

    fn expand_view(&self, s: &Sess, session: u64) -> UiOut {
        let Some(e) = s.comp.as_ref().and_then(|c| c.expand.as_ref()) else { return UiOut::Hide };
        let ps = self.page_size();
        let n = e.view.candidates.len();
        let page = e.hl / ps;
        let items: Vec<(String, String)> = e.view.candidates.iter().skip(page * ps).take(ps).map(|c| (c.value.clone(), c.description.clone())).collect();
        UiOut::Show(View {
            session,
            preedit: e.reading.clone(),
            labels: (1..=items.len()).map(|i| i.to_string()).collect(),
            candidates: items,
            highlighted: (e.hl - page * ps) as i32,
            page_no: page as i32,
            last_page: (page + 1) * ps >= n,
            flash: false,
            vertical: true,
            ..Default::default()
        })
    }

    /// A key while a word's other spellings are open: ↑↓ move (↑ off the top
    /// closes the column), ←→ turn the page, Space / Enter / a digit put one
    /// in, Esc / Backspace close it. Anything else closes it and is then
    /// handled as usual (None).
    fn expand_key(&self, s: &mut Sess, session: u64, code: u32) -> Option<(State, UiOut)> {
        let ps = self.page_size();
        let e = s.comp.as_ref()?.expand.as_ref()?;
        let n = e.view.candidates.len();
        let hl = e.hl;
        let page = hl / ps;
        let set = |s: &mut Sess, h: usize| {
            if let Some(e) = s.comp.as_mut().and_then(|c| c.expand.as_mut()) {
                e.hl = h;
            }
        };
        let close = |e: &Engine, s: &mut Sess| {
            if let Some(c) = s.comp.as_mut() {
                c.expand = None;
            }
            (eaten_with(e.mixed_preedit(s)), e.mixed_view(s, session))
        };
        let moved = |e: &Engine, s: &Sess| Some((eaten_with(e.mixed_preedit(s)), e.expand_view(s, session)));
        match code {
            ks::UP => {
                if hl == 0 {
                    return Some(close(self, s));
                }
                set(s, hl - 1);
                moved(self, s)
            }
            ks::DOWN | ks::TAB => {
                set(s, (hl + 1).min(n.saturating_sub(1)));
                moved(self, s)
            }
            c if c == ks::LEFT || c == ks::PAGE_UP || c == '-' as u32 => {
                set(s, step_hl(hl, n, ps, -(ps as i64)));
                moved(self, s)
            }
            c if c == ks::RIGHT || c == ks::PAGE_DOWN || c == '=' as u32 => {
                set(s, step_hl(hl, n, ps, ps as i64));
                moved(self, s)
            }
            0x20 | ks::RETURN | ks::KP_ENTER => Some(self.pick_expanded(s, session, hl)),
            c if (('1' as u32)..=('9' as u32)).contains(&c) => {
                let i = page * ps + (c - '1' as u32) as usize;
                if i < n {
                    Some(self.pick_expanded(s, session, i))
                } else {
                    moved(self, s)
                }
            }
            ks::ESCAPE | ks::BACKSPACE => Some(close(self, s)),
            _ => {
                if let Some(c) = s.comp.as_mut() {
                    c.expand = None;
                }
                None
            }
        }
    }

    /// Put in spelling `i` of the open column; the composition is done.
    fn pick_expanded(&self, s: &mut Sess, session: u64, i: usize) -> (State, UiOut) {
        let Some(c) = s.comp.clone() else { return (eaten_with(None), UiOut::Hide) };
        let Some(e) = c.expand.as_ref() else { return (eaten_with(None), UiOut::Hide) };
        let Some(cand) = e.view.candidates.get(i).cloned() else { return (eaten_with(self.mixed_preedit(s)), self.expand_view(s, session)) };
        let mut text = cand.value.clone();
        if let (Some(mz), true) = (self.mozc.as_ref(), s.mozc_aux != 0) {
            // Through Mozc, so it learns the choice.
            let _ = mz.select_candidate(s.mozc_aux, cand.id);
            if let Some((t, _)) = mz.submit(s.mozc_aux).and_then(|v| v.commit) {
                text = t;
            }
        }
        let code = c.code();
        self.clear_comp(s);
        self.committed(s, &text, Lang::Ja, &code);
        s.last_ja = Some(LastJa { text: text.clone(), reading: e.reading.clone(), shown: 0 });
        (State { eaten: true, commit: Some(text), ..Default::default() }, UiOut::Hide)
    }

    /// Space (or F6-F10) while Japanese leads: Mozc converts from here on.
    fn hand_to_mozc(&self, s: &mut Sess, session: u64, key: JaKey, shift: bool) -> (State, UiOut) {
        if s.rime != 0 {
            self.rime.clear(s.rime);
        }
        let Some(mid) = self.mozc_session(s) else { return (eaten_with(None), UiOut::Hide) };
        if let Some(c) = s.comp.as_mut() {
            c.native = true;
            c.zh = false;
        }
        let recent = s.recent.clone();
        let v = self.mozc.as_ref().unwrap().send_key(mid, key, shift, &recent);
        self.native_done(s, session, v)
    }

    /// Commit merged[i].
    fn pick_merged(&self, s: &mut Sess, session: u64, i: usize) -> (State, UiOut) {
        let Some(c) = s.comp.clone() else { return (eaten_with(None), UiOut::Hide) };
        let Some(item) = c.merged.get(i).cloned() else { return (eaten_with(self.mixed_preedit(s)), UiOut::Keep) };
        let code = c.code();
        match item.lang {
            Lang::Zh => {
                let rs = self.rime_session(s);
                self.rime.select_candidate(rs, item.key as usize);
                let snap = self.rime.snapshot(rs);
                if let (Some(m), true) = (&self.mozc, s.mozc != 0) {
                    m.revert(s.mozc);
                }
                s.comp = None;
                if let Some(t) = snap.commit.clone().filter(|t| !t.is_empty()) {
                    self.committed(s, &t, Lang::Zh, &code);
                }
                // Partly picked ("你 hao"): the rest continues in Rime alone.
                (self.to_state(true, &snap), self.rime_ui(session, &snap))
            }
            Lang::En => {
                self.clear_comp(s);
                self.committed(s, &item.text, Lang::En, &code);
                (State { eaten: true, commit: Some(item.text), ..Default::default() }, UiOut::Hide)
            }
            Lang::Ja => {
                if s.rime != 0 {
                    self.rime.clear(s.rime);
                }
                let v = self.mozc_session(s).and_then(|mid| self.mozc.as_ref().unwrap().submit_candidate(mid, item.key as i32));
                if let Some(cm) = s.comp.as_mut() {
                    cm.native = true;
                }
                self.native_done(s, session, v)
            }
        }
    }

    /// A click in the candidate window: `index` on the current page, or a
    /// page arrow.
    pub(crate) fn click(&self, s: &mut Sess, session: u64, index: Option<usize>, page: Option<bool>) -> (State, UiOut) {
        let ps = self.page_size();
        match s.comp.as_ref().map(|c| c.native) {
            // Mixed composition.
            Some(false) if s.comp.as_ref().map(|c| c.expand.is_some()).unwrap_or(false) => {
                if let Some(back) = page {
                    let (st, ui) = self.expand_key(s, session, if back { ks::PAGE_UP } else { ks::PAGE_DOWN }).unwrap_or((eaten_with(None), UiOut::Keep));
                    return (st, ui);
                }
                let hl = s.comp.as_ref().and_then(|c| c.expand.as_ref()).map(|e| e.hl).unwrap_or(0);
                self.pick_expanded(s, session, (hl / ps) * ps + index.unwrap_or(0))
            }
            Some(false) => {
                if let Some(back) = page {
                    return self.move_hl(s, session, if back { -(ps as i64) } else { ps as i64 }, true);
                }
                let hl = s.comp.as_ref().map(|c| c.hl).unwrap_or(0);
                self.pick_merged(s, session, (hl / ps) * ps + index.unwrap_or(0))
            }
            // Mozc's list.
            Some(true) => {
                if let Some(back) = page {
                    return self.native_key(s, session, if back { ks::PAGE_UP } else { ks::PAGE_DOWN }, 0);
                }
                self.native_key(s, session, '1' as u32 + index.unwrap_or(0) as u32, 0)
            }
            // Rime alone.
            None => {
                let rs = self.rime_session(s);
                if let Some(i) = index {
                    self.rime.select_on_page(rs, i);
                }
                if let Some(back) = page {
                    self.rime.change_page(rs, back);
                }
                let snap = self.rime.snapshot(rs);
                if let Some(c) = snap.commit.clone().filter(|c| !c.is_empty()) {
                    self.committed(s, &c, Lang::Zh, "");
                }
                (self.to_state(true, &snap), self.rime_ui(session, &snap))
            }
        }
    }
}

