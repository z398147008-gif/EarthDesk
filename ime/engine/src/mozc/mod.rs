//! Japanese: Mozc's conversion engine, loaded in-process from
//! earthdesk_mozc.dll (built from Mozc by .github/workflows/mozc.yml; see
//! ime/mozc/earthdesk_mozc.cc). We speak Mozc's own protocol to it —
//! serialized `commands::Command` protobufs — exactly like Mozc's clients do
//! over IPC, minus the IPC.
//!
//! Everything else (keys, the candidate window, mixing with Chinese) is
//! ours; this module only turns Mozc outputs into a small `JaView`.

#![allow(clippy::all)]

use libloading::Library;
use prost::Message;
use std::ffi::{c_char, c_int, CStr, CString};
use std::path::Path;

#[allow(dead_code, clippy::all)]
pub mod pb {
    include!("pb/mozc.rs");
    pub mod commands {
        include!("pb/mozc.commands.rs");
    }
    pub mod config {
        include!("pb/mozc.config.rs");
    }
    pub mod user_dictionary {
        include!("pb/mozc.user_dictionary.rs");
    }
}

use pb::commands::{self as cmd, input::CommandType as In, session_command::CommandType as Sc};

type Init = unsafe extern "C" fn(*const c_char) -> c_int;
type Eval = unsafe extern "C" fn(*const u8, usize, *mut *mut u8, *mut usize) -> c_int;
type Free = unsafe extern "C" fn(*mut u8);
type Version = unsafe extern "C" fn() -> *const c_char;
type Shutdown = unsafe extern "C" fn();

pub struct Mozc {
    _lib: Library,
    eval: Eval,
    free: Free,
    version: Version,
    shutdown: Shutdown,
}

/// A key for Mozc: a printable character or one of Mozc's special keys.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum JaKey {
    Char(char),
    Special(cmd::key_event::SpecialKey),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct JaCand {
    /// Mozc's candidate id (for SELECT/SUBMIT_CANDIDATE).
    pub id: i32,
    pub value: String,
    /// Reading (hiragana) of this candidate.
    pub key: String,
    /// How many conversion segments the candidate spans (1 = one word /
    /// dictionary entry).
    pub segments: i32,
    pub description: String,
}

/// What Mozc shows after a request.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct JaView {
    pub consumed: bool,
    /// Committed text and its reading.
    pub commit: Option<(String, String)>,
    /// The composition as displayed (hiragana, or the converted segments).
    pub preedit: String,
    /// Caret inside the preedit, in characters.
    pub cursor: usize,
    /// The composition's reading (segment keys joined; = preedit while
    /// composing).
    pub reading: String,
    /// Candidates: conversion candidates when converting (`converting`),
    /// otherwise the suggestions for what has been typed.
    pub candidates: Vec<JaCand>,
    pub focused: Option<usize>,
    pub converting: bool,
    /// Conversion segments in the preedit.
    pub segments: usize,
}

impl JaView {
    pub fn composing(&self) -> bool {
        !self.preedit.is_empty()
    }
}

fn view(out: &cmd::Output) -> JaView {
    let mut v = JaView { consumed: out.consumed.unwrap_or(false), ..Default::default() };
    if let Some(r) = &out.result {
        if !r.value.is_empty() {
            v.commit = Some((r.value.clone(), r.key.clone().unwrap_or_default()));
        }
    }
    if let Some(p) = &out.preedit {
        for s in &p.segment {
            v.preedit.push_str(&s.value);
            v.reading.push_str(s.key.as_deref().unwrap_or(&s.value));
        }
        v.cursor = p.cursor as usize;
        v.segments = p.segment.len();
    }
    if let Some(w) = &out.candidate_window {
        v.converting = w.focused_index.is_some();
        v.focused = w.focused_index.map(|i| i as usize);
    }
    // all_candidate_words has the whole list (the window only a page).
    if let Some(all) = &out.all_candidate_words {
        v.focused = all.focused_index.map(|i| i as usize).or(v.focused);
        for c in &all.candidates {
            v.candidates.push(JaCand {
                id: c.id.unwrap_or(0),
                value: c.value.clone().unwrap_or_default(),
                key: c.key.clone().unwrap_or_default(),
                segments: c.num_segments_in_candidate.unwrap_or(1),
                description: c.annotation.as_ref().and_then(|a| a.description.clone()).unwrap_or_default(),
            });
        }
    } else if let Some(w) = &out.candidate_window {
        for c in &w.candidate {
            v.candidates.push(JaCand {
                id: c.id.unwrap_or(0),
                value: c.value.clone(),
                key: String::new(),
                segments: 1,
                description: c.annotation.as_ref().and_then(|a| a.description.clone()).unwrap_or_default(),
            });
        }
    }
    v
}

impl Mozc {
    /// Load the library and start the engine with its user data (learning
    /// history, user dictionary) in `profile`.
    pub fn load(lib: &Path, profile: &Path) -> Result<Mozc, String> {
        unsafe {
            let l = Library::new(lib).map_err(|e| format!("cannot load {}: {e}", lib.display()))?;
            let init: Init = *l.get::<Init>(b"ed_mozc_init\0").map_err(|e| e.to_string())?;
            let eval: Eval = *l.get::<Eval>(b"ed_mozc_eval\0").map_err(|e| e.to_string())?;
            let free: Free = *l.get::<Free>(b"ed_mozc_free\0").map_err(|e| e.to_string())?;
            let version: Version = *l.get::<Version>(b"ed_mozc_data_version\0").map_err(|e| e.to_string())?;
            let shutdown: Shutdown = *l.get::<Shutdown>(b"ed_mozc_shutdown\0").map_err(|e| e.to_string())?;
            let _ = std::fs::create_dir_all(profile);
            let dir = CString::new(profile.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
            let r = init(dir.as_ptr());
            if r != 0 {
                return Err(format!("ed_mozc_init failed ({r})"));
            }
            Ok(Mozc { _lib: l, eval, free, version, shutdown })
        }
    }

    pub fn data_version(&self) -> String {
        unsafe {
            let p = (self.version)();
            if p.is_null() {
                String::new()
            } else {
                CStr::from_ptr(p).to_string_lossy().into_owned()
            }
        }
    }

    fn eval(&self, input: cmd::Input) -> Option<cmd::Output> {
        let c = cmd::Command { input, output: None };
        let bytes = c.encode_to_vec();
        let mut out: *mut u8 = std::ptr::null_mut();
        let mut n = 0usize;
        unsafe {
            if (self.eval)(bytes.as_ptr(), bytes.len(), &mut out, &mut n) != 0 || out.is_null() {
                return None;
            }
            let slice = std::slice::from_raw_parts(out, n);
            let r = cmd::Command::decode(slice).ok();
            (self.free)(out);
            r?.output
        }
    }

    fn input(t: In, id: u64) -> cmd::Input {
        cmd::Input { r#type: t as i32, id: Some(id), ..Default::default() }
    }

    pub fn create_session(&self) -> Option<u64> {
        let out = self.eval(Self::input(In::CreateSession, 0))?;
        out.id.filter(|id| *id != 0)
    }

    pub fn delete_session(&self, id: u64) {
        let _ = self.eval(Self::input(In::DeleteSession, id));
    }

    /// Send a key, with what precedes the caret (our own recent output) so
    /// Mozc can use it for conversion.
    pub fn send_key(&self, id: u64, key: JaKey, shift: bool, preceding: &str) -> Option<JaView> {
        let mut k = cmd::KeyEvent {
            // Keep the session switched on in hiragana mode (the way Mozc's
            // ibus client does), whatever state it was in.
            activated: Some(true),
            mode: Some(cmd::CompositionMode::Hiragana as i32),
            ..Default::default()
        };
        match key {
            JaKey::Char(c) => k.key_code = Some(c as u32),
            JaKey::Special(s) => k.special_key = Some(s as i32),
        }
        if shift {
            k.modifier_keys.push(cmd::key_event::ModifierKey::Shift as i32);
        }
        let mut input = Self::input(In::SendKey, id);
        input.key = Some(k);
        if !preceding.is_empty() {
            input.context = Some(cmd::Context { preceding_text: Some(preceding.to_string()), ..Default::default() });
        }
        self.eval(input).map(|o| view(&o))
    }

    /// Every way to write `reading` as one word: put the reading into
    /// session `id` (a spare one) as it is, convert, and widen the first
    /// segment until it covers the whole reading. The session is left
    /// converting; pick with `select_candidate` + `submit`.
    pub fn alternatives(&self, id: u64, reading: &str, preceding: &str) -> Option<JaView> {
        self.revert(id);
        let k = cmd::KeyEvent {
            activated: Some(true),
            mode: Some(cmd::CompositionMode::Hiragana as i32),
            key_string: Some(reading.to_string()),
            ..Default::default()
        };
        let mut input = Self::input(In::SendKey, id);
        input.key = Some(k);
        if !preceding.is_empty() {
            input.context = Some(cmd::Context { preceding_text: Some(preceding.to_string()), ..Default::default() });
        }
        let v = self.eval(input).map(|o| view(&o))?;
        if std::env::var_os("EARTHDESK_IME_TRACE").is_some() {
            eprintln!("  alternatives({reading}): {v:?}");
        }
        if v.preedit.is_empty() {
            return None;
        }
        let mut v = self.send_key(id, JaKey::Special(cmd::key_event::SpecialKey::Space), false, preceding)?;
        for _ in 0..12 {
            if v.segments <= 1 {
                break;
            }
            v = self.send_key(id, JaKey::Special(cmd::key_event::SpecialKey::Right), true, preceding)?;
        }
        // Right after the first Space Mozc has no window yet, only the list.
        v.converting = true;
        (!v.candidates.is_empty()).then_some(v)
    }

    fn command(&self, id: u64, t: Sc, cand: Option<i32>) -> Option<JaView> {
        let mut input = Self::input(In::SendCommand, id);
        input.command = Some(cmd::SessionCommand { r#type: t as i32, id: cand, ..Default::default() });
        self.eval(input).map(|o| view(&o))
    }

    /// Commit a suggestion or candidate by id (the whole composition).
    pub fn submit_candidate(&self, id: u64, cand: i32) -> Option<JaView> {
        self.command(id, Sc::SubmitCandidate, Some(cand))
    }

    /// While converting: pick a candidate for the focused segment.
    pub fn select_candidate(&self, id: u64, cand: i32) -> Option<JaView> {
        self.command(id, Sc::SelectCandidate, Some(cand))
    }

    /// Commit what is there (Enter).
    pub fn submit(&self, id: u64) -> Option<JaView> {
        self.command(id, Sc::Submit, None)
    }

    /// Throw the composition away.
    pub fn revert(&self, id: u64) {
        let _ = self.command(id, Sc::Revert, None);
    }

    pub fn page(&self, id: u64, next: bool) -> Option<JaView> {
        self.command(id, if next { Sc::ConvertNextPage } else { Sc::ConvertPrevPage }, None)
    }

    /// Write the learning data and user dictionary to disk.
    pub fn sync(&self) {
        let _ = self.eval(Self::input(In::SyncData, 0));
    }

    pub fn reset_context(&self, id: u64) {
        let _ = self.command(id, Sc::ResetContext, None);
    }
}

impl Drop for Mozc {
    fn drop(&mut self) {
        unsafe { (self.shutdown)() }
    }
}

// --- Text helpers ----------------------------------------------------------------

/// ひらがな -> カタカナ (the rest unchanged).
pub fn to_katakana(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\u{3041}'..='\u{3096}' | '\u{309D}' | '\u{309E}' => char::from_u32(c as u32 + 0x60).unwrap_or(c),
            _ => c,
        })
        .collect()
}

pub fn is_kana(c: char) -> bool {
    matches!(c, '\u{3041}'..='\u{309F}' | '\u{30A0}'..='\u{30FF}')
}

/// Does `s` contain kana (so it is Japanese, not Chinese)?
#[allow(dead_code)]
pub fn has_kana(s: &str) -> bool {
    s.chars().any(|c| is_kana(c) && c != 'ー' && c != '・')
}

/// Mozc's preedit for letters typed as romaji: a valid reading is all kana,
/// with at most a short tail of letters still waiting for their vowel
/// ("k", "ky", "tt", or "n" for ん). Anything else means the letters are
/// not romaji ("をjn" for "wojn").
pub fn romaji_valid(preedit: &str) -> bool {
    // Mozc shows the letters waiting for a vowel full-width (ｋ).
    let preedit: String = preedit
        .chars()
        .map(|c| match c {
            '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
            _ => c,
        })
        .collect();
    let preedit = preedit.as_str();
    let tail = preedit.chars().rev().take_while(|c| c.is_ascii_alphabetic()).count();
    let head: Vec<char> = preedit.chars().take(preedit.chars().count() - tail).collect();
    let tail_str: String = preedit.chars().skip(head.len()).collect();
    // A vowel would already have become kana.
    if tail > 3 || tail_str.chars().any(|c| "aiueo".contains(c.to_ascii_lowercase())) {
        return false;
    }
    if !pending_romaji(&tail_str.to_ascii_lowercase()) {
        return false;
    }
    if head.is_empty() {
        return (1..=2).contains(&tail);
    }
    head.iter().all(|c| is_kana(*c))
}

/// Letters that can still become kana once a vowel follows.
fn pending_romaji(t: &str) -> bool {
    const PAIRS: &[&str] = &["sh", "ch", "ts", "th", "dh", "wh", "kw", "gw", "qw", "tw", "dw", "fw", "sw", "zw"];
    let b = t.as_bytes();
    let consonant = |c: u8| b"bcdfghjklmnpqrstvwxyz".contains(&c);
    let second = |s: &[u8]| s.len() == 2 && consonant(s[0]) && (s[1] == b'y' || PAIRS.contains(&std::str::from_utf8(s).unwrap_or("")));
    match b.len() {
        0 => true,
        1 => consonant(b[0]),
        // っ (kk, tt...) or a two-letter onset (ky, sh, ts...).
        2 => (consonant(b[0]) && b[0] == b[1] && b[0] != b'n') || second(b),
        // っ + onset (kky, ssh, tch, tts).
        3 => (b[0] == b[1] || (b[0] == b't' && b[1] == b'c')) && consonant(b[0]) && b[0] != b'n' && (second(&b[1..]) || (b[1] == b'c' && b[2] == b'h')),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kana_helpers() {
        assert_eq!(to_katakana("わたしはー"), "ワタシハー");
        assert!(has_kana("私は"));
        assert!(!has_kana("我们"));
        assert!(romaji_valid("わたし"));
        assert!(romaji_valid("わたs"));
        assert!(romaji_valid("わたｓｈ"));
        assert!(!romaji_valid("をｊｎ"));
        assert!(romaji_valid("かn"));
        assert!(!romaji_valid("をjn"));
        assert!(!romaji_valid("をjntm"));
        assert!(!romaji_valid("；"));
        assert!(romaji_valid("k"));
        assert!(romaji_valid("かっt"));
        assert!(romaji_valid("きょうky"));
        assert!(romaji_valid("まtch"));
        assert!(romaji_valid("がss"));
        assert!(!romaji_valid("がsd"));
        assert!(!romaji_valid(""));
    }

    #[test]
    fn protobuf_round_trip() {
        let c = cmd::Command {
            input: cmd::Input { r#type: In::SendKey as i32, id: Some(7), key: Some(cmd::KeyEvent { key_code: Some('a' as u32), ..Default::default() }), ..Default::default() },
            output: Some(cmd::Output {
                result: Some(cmd::Result { r#type: cmd::result::ResultType::String as i32, value: "私".into(), key: Some("わたし".into()), ..Default::default() }),
                preedit: Some(cmd::Preedit {
                    cursor: 1,
                    segment: vec![cmd::preedit::Segment { annotation: 1, value: "あ".into(), value_length: 1, key: Some("あ".into()) }],
                    ..Default::default()
                }),
                ..Default::default()
            }),
        };
        let back = cmd::Command::decode(c.encode_to_vec().as_slice()).unwrap();
        let v = view(back.output.as_ref().unwrap());
        assert_eq!(v.commit, Some(("私".into(), "わたし".into())));
        assert_eq!(v.preedit, "あ");
    }
}
