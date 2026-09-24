//! The engine proper: one session per text-service instance (roughly, per
//! UI thread of each program), requests in, states out, and the candidate
//! window kept in step. Platform-independent; the pipe server and the
//! window are plugged in from main.rs.
//!
//! Each session has a Rime session (Chinese, 微软双拼) and, when the Japanese
//! engine is installed, a Mozc session. How keys are shared between the two
//! is in compose.rs.

use crate::compose::{Comp, LastJa, Mode, ModeMemory};
use crate::mixed::{Lang, LangPref};
use crate::mozc::Mozc;
use crate::rime::{Rime, Snapshot};
use ime_proto::{Preedit, Reply, Request, State};
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Caret {
    pub x: i32,
    pub y: i32,
    pub h: i32,
}

/// What the candidate window shows.
#[derive(Debug, Clone, Default)]
pub struct View {
    pub session: u64,
    /// The spelling ("ni hao", "你 hao" after a partial pick, "わたし").
    pub preedit: String,
    pub candidates: Vec<(String, String)>,
    pub labels: Vec<String>,
    pub highlighted: i32,
    pub page_no: i32,
    pub last_page: bool,
    /// A short notice (mode switched): no page arrows, hides by itself.
    pub flash: bool,
}

pub trait Ui: Send + Sync {
    fn show(&self, view: View, caret: Option<Caret>);
    fn move_to(&self, caret: Caret);
    fn hide(&self);
    /// Tell the DLL behind `notify` that it should Poll.
    fn notify(&self, notify: u64);
}

/// What to do with the candidate window after a request.
pub enum UiOut {
    Keep,
    Hide,
    Show(View),
}

pub struct Sess {
    pub rime: usize,
    /// Host program (lower-case exe name).
    pub exe: String,
    pub notify: u64,
    pub caret: Option<Caret>,
    /// A change the DLL has not fetched yet (a candidate picked with the
    /// mouse).
    pub pending: Option<State>,
    pub mode: Mode,
    /// Mozc session id, 0 = none yet.
    pub mozc: u64,
    /// A composition shared between Chinese and Japanese (or handed to
    /// Mozc), see compose.rs. None while Rime alone composes.
    pub comp: Option<Comp>,
    /// Language of the last committed text (context for mixing).
    pub last: Option<Lang>,
    /// The Japanese word just committed, for switching its spelling.
    pub last_ja: Option<LastJa>,
    /// Our own recent output here (Mozc uses it as conversion context).
    pub recent: String,
    /// Typing a "/" command.
    pub cmd: Option<String>,
}

struct Inner {
    sessions: HashMap<u64, Sess>,
    next: u64,
    focused: Option<u64>,
}

pub struct Engine {
    pub(crate) rime: Rime,
    pub(crate) mozc: Option<Mozc>,
    inner: Mutex<Inner>,
    ui: Box<dyn Ui>,
    schema: String,
    pub(crate) settings: Mutex<ime_proto::Settings>,
    pub(crate) pref: Mutex<LangPref>,
    pub(crate) modes: Mutex<ModeMemory>,
}

/// Byte offset in UTF-8 -> offset in UTF-16 units.
pub fn utf16_index(s: &str, byte: usize) -> u32 {
    let byte = byte.min(s.len());
    let mut b = byte;
    while b > 0 && !s.is_char_boundary(b) {
        b -= 1;
    }
    s[..b].encode_utf16().count() as u32
}

impl Engine {
    pub fn new(rime: Rime, mozc: Option<Mozc>, ui: Box<dyn Ui>, schema: &str) -> Engine {
        Engine {
            rime,
            mozc,
            inner: Mutex::new(Inner { sessions: HashMap::new(), next: 1, focused: None }),
            ui,
            schema: schema.to_string(),
            settings: Mutex::new(crate::paths::load_settings()),
            pref: Mutex::new(LangPref::load(&crate::paths::data_file("langpref.json"))),
            modes: Mutex::new(ModeMemory::load(&crate::paths::data_file("modes.json"))),
        }
    }

    #[allow(dead_code)]
    pub fn rime(&self) -> &Rime {
        &self.rime
    }

    fn ready(&self) -> bool {
        !self.rime.is_maintaining()
    }

    pub(crate) fn page_size(&self) -> usize {
        self.settings.lock().map(|s| s.page_size.clamp(3, 9) as usize).unwrap_or(7)
    }

    pub(crate) fn mixed_enabled(&self) -> bool {
        self.mozc.is_some() && self.settings.lock().map(|s| s.mixed).unwrap_or(true)
    }

    /// The Rime session behind ours, recreated if Rime lost it (a redeploy
    /// drops every session).
    pub(crate) fn rime_session(&self, s: &mut Sess) -> usize {
        if s.rime == 0 || !self.rime.find_session(s.rime) {
            s.rime = self.rime.create_session();
            if !self.schema.is_empty() {
                self.rime.select_schema(s.rime, &self.schema);
            }
            // Programs the user wants to start in English (games, terminals).
            let ascii = self.settings.lock().map(|st| st.ascii_apps.iter().any(|a| a.eq_ignore_ascii_case(&s.exe))).unwrap_or(false);
            if ascii {
                self.rime.set_option(s.rime, "ascii_mode", true);
            }
            s.comp = None;
        }
        s.rime
    }

    /// Our Mozc session, created on first use.
    pub(crate) fn mozc_session(&self, s: &mut Sess) -> Option<u64> {
        let m = self.mozc.as_ref()?;
        if s.mozc == 0 {
            s.mozc = m.create_session()?;
        }
        Some(s.mozc)
    }

    pub(crate) fn to_state(&self, eaten: bool, snap: &Snapshot) -> State {
        State {
            eaten,
            commit: snap.commit.clone().filter(|c| !c.is_empty()),
            preedit: snap
                .preedit
                .as_ref()
                .map(|(text, cursor)| Preedit { text: text.clone(), cursor: utf16_index(text, *cursor) }),
            ascii: snap.ascii,
            delete_before: 0,
        }
    }

    pub(crate) fn rime_ui(&self, session: u64, snap: &Snapshot) -> UiOut {
        match &snap.menu {
            Some(m) if !m.candidates.is_empty() => UiOut::Show(View {
                session,
                preedit: snap.preedit.as_ref().map(|p| p.0.clone()).unwrap_or_default(),
                candidates: m.candidates.iter().map(|c| (c.text.clone(), c.comment.clone())).collect(),
                labels: m.labels.clone(),
                highlighted: m.highlighted,
                page_no: m.page_no,
                last_page: m.is_last_page,
                flash: false,
            }),
            _ => UiOut::Hide,
        }
    }

    fn apply_ui(&self, ui: UiOut, caret: Option<Caret>) {
        match ui {
            UiOut::Keep => {}
            UiOut::Hide => self.ui.hide(),
            UiOut::Show(v) => self.ui.show(v, caret),
        }
    }

    /// Throw away whatever is being composed in this session.
    fn clear(&self, s: &mut Sess) {
        if s.rime != 0 {
            self.rime.clear(s.rime);
        }
        if let (Some(m), true) = (&self.mozc, s.mozc != 0) {
            m.revert(s.mozc);
        }
        s.comp = None;
        s.cmd = None;
        s.pending = None;
    }

    pub fn handle(&self, req: Request) -> Reply {
        let mut g = match self.inner.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        match req {
            Request::Hello { version, exe, notify, .. } => {
                if version != ime_proto::VERSION {
                    return Reply::Error { message: format!("protocol {version}, engine speaks {}", ime_proto::VERSION) };
                }
                let id = g.next;
                g.next += 1;
                let default = if self.mixed_enabled() { Mode::Mixed } else { Mode::Zh };
                let mode = self.modes.lock().ok().and_then(|m| m.get(&exe)).unwrap_or(default);
                g.sessions.insert(
                    id,
                    Sess {
                        rime: 0,
                        exe,
                        notify,
                        caret: None,
                        pending: None,
                        mode,
                        mozc: 0,
                        comp: None,
                        last: None,
                        last_ja: None,
                        recent: String::new(),
                        cmd: None,
                    },
                );
                Reply::Hello { version: ime_proto::VERSION, session: id }
            }
            Request::Key { session, keycode, mask } => {
                if !self.ready() {
                    return Reply::Busy;
                }
                let Some(s) = g.sessions.get_mut(&session) else { return Reply::Error { message: "no such session".into() } };
                let (state, ui) = self.key(s, session, keycode, mask);
                let caret = s.caret;
                g.focused = Some(session);
                drop(g);
                self.apply_ui(ui, caret);
                Reply::State(state)
            }
            Request::Caret { session, x, y, h } => {
                let c = Caret { x, y, h };
                if let Some(s) = g.sessions.get_mut(&session) {
                    s.caret = Some(c);
                }
                if g.focused == Some(session) {
                    drop(g);
                    self.ui.move_to(c);
                }
                Reply::Ok
            }
            Request::Focus { session, on } => {
                if on {
                    g.focused = Some(session);
                } else if g.focused == Some(session) {
                    g.focused = None;
                    drop(g);
                    self.ui.hide();
                }
                Reply::Ok
            }
            Request::Poll { session } => {
                let st = g.sessions.get_mut(&session).and_then(|s| s.pending.take()).unwrap_or_default();
                Reply::State(st)
            }
            Request::Reset { session } => {
                if let Some(s) = g.sessions.get_mut(&session) {
                    if self.ready() {
                        self.clear(s);
                    }
                    s.last_ja = None;
                }
                if g.focused == Some(session) {
                    drop(g);
                    self.ui.hide();
                }
                Reply::Ok
            }
            Request::Bye { session } => {
                if let Some(s) = g.sessions.remove(&session) {
                    if s.rime != 0 && self.ready() {
                        self.rime.destroy_session(s.rime);
                    }
                    if let (Some(m), true) = (&self.mozc, s.mozc != 0) {
                        m.delete_session(s.mozc);
                    }
                }
                if g.focused == Some(session) {
                    g.focused = None;
                    drop(g);
                    self.ui.hide();
                }
                Reply::Ok
            }
            Request::Status => Reply::Status {
                librime: self.rime.version(),
                mozc: self.mozc.as_ref().map(|m| m.data_version()).unwrap_or_default(),
                maintaining: self.rime.is_maintaining(),
                clients: g.sessions.len() as u32,
            },
            Request::Deploy => {
                let st = crate::paths::load_settings();
                crate::paths::write_customisations(&crate::paths::user_dir(), &st);
                if let Ok(mut cur) = self.settings.lock() {
                    *cur = st;
                }
                for s in g.sessions.values_mut() {
                    s.comp = None;
                    s.cmd = None;
                }
                drop(g);
                self.ui.hide();
                if !self.rime.is_maintaining() {
                    self.rime.maintain(true, false);
                }
                crate::log("deploy requested from settings");
                Reply::Ok
            }
        }
    }

    /// A candidate clicked in the window (index on the current page), or a
    /// page arrow (`page` = Some(backward)).
    pub fn pick(&self, session: u64, index: Option<usize>, page: Option<bool>) {
        let mut g = match self.inner.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if !self.ready() {
            return;
        }
        let Some(s) = g.sessions.get_mut(&session) else { return };
        let (state, ui) = self.click(s, session, index, page);
        let notify = s.notify;
        let caret = s.caret;
        // Only a commit or a changed preedit needs the DLL; a page flip is
        // the window's business alone.
        if index.is_some() {
            s.pending = Some(state);
        }
        drop(g);
        self.apply_ui(ui, caret);
        if index.is_some() {
            self.ui.notify(notify);
        }
    }

    /// Logoff, shutdown or the installer asking us to go: flush and exit.
    /// Holding the lock keeps pipe threads from touching Rime meanwhile.
    pub fn shutdown(&self) -> ! {
        let _g = self.inner.lock();
        self.ui.hide();
        if let Ok(mut p) = self.pref.lock() {
            p.save();
        }
        if let Some(m) = &self.mozc {
            m.sync();
        }
        self.rime.finalize();
        crate::log("engine stopped");
        std::process::exit(0)
    }

    #[allow(dead_code)]
    pub fn exe_of(&self, session: u64) -> Option<String> {
        self.inner.lock().ok()?.sessions.get(&session).map(|s| s.exe.clone())
    }

    /// For the --cli test: a session without a pipe.
    #[allow(dead_code)]
    pub fn test_session(&self, exe: &str) -> u64 {
        match self.handle(Request::Hello { version: ime_proto::VERSION, pid: 0, exe: exe.into(), notify: 0 }) {
            Reply::Hello { session, .. } => session,
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::utf16_index;

    #[test]
    fn utf16() {
        assert_eq!(utf16_index("你 hao", "你".len()), 1);
        assert_eq!(utf16_index("𠀀a", 4), 2);
        assert_eq!(utf16_index("ab", 9), 2);
    }
}
