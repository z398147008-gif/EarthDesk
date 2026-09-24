//! How keys become text: 中文 (Rime), 日本語 (Mozc), or both at once.
//!
//! Modes, remembered per program and switched with Ctrl+Shift+J or the
//! "/" commands (/zh /ja /mix):
//!
//!   Mixed  the same letters go to both engines; mixed.rs decides which
//!          side's candidates lead. Space picks the highlighted candidate,
//!          except when Japanese leads: then Space converts the way a
//!          Japanese input method does (Mozc takes over this composition:
//!          segments, ←→, Space for the next candidate, Enter to commit).
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
    State { eaten: true, commit, preedit: then.preedit, ascii: then.ascii, delete_before: first.delete_before }
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
        }
        if !code.is_empty() && s.mode == crate::compose::Mode::Mixed {
            if let Ok(mut p) = self.pref.lock() {
                p.record(code, lang);
            }
        }
        s.recent.push_str(text);
        let n = s.recent.chars().count();
        if n > 24 {
            s.recent = s.recent.chars().skip(n - 24).collect();
        }
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

        // Digits pick from our page of the list.
        if !ctrl_alt && (('1' as u32)..=('9' as u32)).contains(&code) {
            if let Some(c) = s.comp.as_ref().filter(|c| !c.ja_view.candidates.is_empty()) {
                let page = c.ja_view.focused.unwrap_or(0) / ps;
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
        let ui = self.ja_list_view(session, &v);
        if let Some(c) = s.comp.as_mut() {
            c.ja_view = v;
        }
        (st, ui)
    }

    fn ja_list_view(&self, session: u64, v: &JaView) -> UiOut {
        if v.candidates.is_empty() {
            return UiOut::Hide;
        }
        let ps = self.page_size();
        let focus = v.focused.unwrap_or(0).min(v.candidates.len() - 1);
        let page = focus / ps;
        let items: Vec<(String, String)> = v.candidates.iter().skip(page * ps).take(ps).map(|c| (c.value.clone(), c.description.clone())).collect();
        UiOut::Show(View {
            session,
            preedit: v.reading.clone(),
            labels: (1..=items.len()).map(|i| i.to_string()).collect(),
            candidates: items,
            highlighted: if v.focused.is_some() { (focus - page * ps) as i32 } else { 0 },
            page_no: page as i32,
            last_page: (page + 1) * ps >= v.candidates.len(),
            flash: false,
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
                if plain && !rime_busy && s.last == Some(Lang::Ja) && !self.rime_ascii(s) {
                    if let Some(p) = ja_punct(code) {
                        self.committed(s, p, Lang::Ja, "");
                        return (State { eaten: true, commit: Some(p.into()), ..Default::default() }, UiOut::Hide);
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
            return (eaten_with(self.mixed_preedit(s)), UiOut::Keep);
        }

        let ps = self.page_size();
        let ch = char::from_u32(code);
        let ja_leads = s.comp.as_ref().map(|c| c.ja_leads).unwrap_or(false);
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
            ks::UP | ks::LEFT => self.move_hl(s, session, -1, false),
            ks::DOWN | ks::RIGHT | ks::TAB => self.move_hl(s, session, 1, false),
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
                if c.ja_leads && c.ja && c.hl == 0 {
                    return self.hand_to_mozc(s, session, JaKey::Special(Sk::Space), false);
                }
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
            let n = c.merged.len() as i64;
            if n > 0 {
                let mut h = c.hl as i64 + by;
                if page {
                    h = (h / ps) * ps;
                }
                c.hl = h.clamp(0, n - 1) as usize;
            }
        }
        (eaten_with(self.mixed_preedit(s)), self.mixed_view(s, session))
    }

    /// Recompute both sides and the merged list.
    fn refresh(&self, s: &mut Sess, session: u64) -> (State, UiOut) {
        let rs = self.rime_session(s);
        let last = s.last;
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
        comp.merged = mixed::merge(&zh_cands, ja_cands, sz, sj);
        comp.ja_leads = match comp.merged.first() {
            Some(m) => m.lang == Lang::Ja,
            None => matches!((sz, sj), (None, Some(_))) || matches!((sz, sj), (Some(a), Some(b)) if b > a),
        };
        comp.hl = comp.hl.min(comp.merged.len().saturating_sub(1));
        (eaten_with(self.mixed_preedit(s)), self.mixed_view(s, session))
    }

    /// The underlined text in the program: the leading side's spelling.
    fn mixed_preedit(&self, s: &Sess) -> Option<Preedit> {
        let c = s.comp.as_ref()?;
        let text = if c.ja_leads && c.ja && !c.ja_view.preedit.is_empty() {
            c.ja_view.preedit.clone()
        } else if c.zh && !c.zh_pre.is_empty() {
            c.zh_pre.clone()
        } else {
            c.code()
        };
        preedit_end(&text)
    }

    fn mixed_view(&self, s: &Sess, session: u64) -> UiOut {
        let Some(c) = s.comp.as_ref() else { return UiOut::Hide };
        if c.merged.is_empty() {
            return UiOut::Hide;
        }
        let ps = self.page_size();
        let page = c.hl / ps;
        let items: Vec<(String, String)> = c.merged.iter().skip(page * ps).take(ps).map(|m| (m.text.clone(), m.comment.clone())).collect();
        let pre = self.mixed_preedit(s).map(|p| p.text).unwrap_or_default();
        UiOut::Show(View {
            session,
            preedit: pre,
            labels: (1..=items.len()).map(|i| i.to_string()).collect(),
            candidates: items,
            highlighted: (c.hl - page * ps) as i32,
            page_no: page as i32,
            last_page: (page + 1) * ps >= c.merged.len(),
            flash: false,
        })
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

