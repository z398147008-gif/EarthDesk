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
use ime_proto::{CandUi, Cands, Preedit, Reply, Request, State};
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
    /// A column instead of a row (a Japanese word's other spellings).
    pub vertical: bool,
    /// The letters as typed, when the spelling reads differently.
    pub typed: String,
    pub skin: ime_proto::Skin,
}

impl View {
    /// As sent to a DLL that draws the candidates itself.
    pub fn cands(&self) -> Cands {
        Cands {
            preedit: self.preedit.clone(),
            candidates: self.candidates.clone(),
            labels: self.labels.clone(),
            highlighted: self.highlighted,
            page_no: self.page_no,
            last_page: self.last_page,
            flash: self.flash,
            vertical: self.vertical,
            typed: if self.typed == self.preedit { String::new() } else { self.typed.clone() },
            skin: self.skin.clone(),
        }
    }
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
    /// The DLL draws the candidate window itself (immersive hosts such as
    /// the Start menu's search): candidates go back in `State::cands`.
    pub draws: bool,
    pub caret: Option<Caret>,
    /// A change the DLL has not fetched yet (a candidate picked with the
    /// mouse).
    pub pending: Option<State>,
    pub mode: Mode,
    /// Mozc session id, 0 = none yet.
    pub mozc: u64,
    /// A spare Mozc session for looking up a word's other spellings.
    pub mozc_aux: u64,
    /// A composition shared between Chinese and Japanese (or handed to
    /// Mozc), see compose.rs. None while Rime alone composes.
    pub comp: Option<Comp>,
    /// Language of the last committed text (context for mixing).
    pub last: Option<Lang>,
    /// The Japanese word just committed, for switching its spelling.
    pub last_ja: Option<LastJa>,
    /// Our own recent output here (Mozc uses it as conversion context).
    pub recent: String,
    /// What the sentence being typed is made of so far (Chinese characters,
    /// Japanese characters, English words), for its punctuation.
    pub sentence: [u32; 3],
    /// Typing a "/" command.
    pub cmd: Option<String>,
    /// The DLL is from an older build: say so once, when nothing else is
    /// on show.
    pub stale: bool,
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
    pub(crate) en: crate::mixed::EnDict,
    pub(crate) hidden: Mutex<crate::mixed::Hidden>,
    look: Mutex<Look>,
}

/// The candidate window's skin, re-read from settings.json and weather.json
/// when they change (a skin switch needs no redeploy).
#[derive(Default)]
struct Look {
    checked: Option<std::time::Instant>,
    stamps: (Option<std::time::SystemTime>, Option<std::time::SystemTime>),
    kind: Option<String>,
    weather: Option<ime_proto::Weather>,
    skin: ime_proto::Skin,
}

/// How many hours the weather skin paints along the bar (as many as fit).
const SKIN_HOURS: usize = 8;

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// The skin from the settings and the weather EarthDesk last saved. The
/// weather is used while it is less than six hours old, from the hour it
/// is now (the file may be a few hours behind).
pub fn make_skin(kind: &str, weather: Option<&ime_proto::Weather>, now: u64) -> ime_proto::Skin {
    let mut skin = ime_proto::Skin { kind: kind.to_string(), ..Default::default() };
    if kind != "weather" {
        return skin;
    }
    if let Some(w) = weather.filter(|w| w.at <= now + 600 && now.saturating_sub(w.at) < 6 * 3600) {
        let late = (now.saturating_sub(w.at) / 3600) as usize;
        let mut w = w.clone();
        w.hours = w.hours.into_iter().skip(late).take(SKIN_HOURS).collect();
        if !w.hours.is_empty() {
            skin.label = w.label(SKIN_HOURS);
            skin.hours = w.hours;
        }
    }
    skin
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

/// A commit ending in an opening mark gets its partner, the caret going
/// between them. Only marks we put in ourselves (full-width ones): what a
/// program gets straight from the keyboard is left to it (code editors
/// pair their own ASCII brackets).
pub fn add_pair(st: &mut State) {
    if st.preedit.is_some() {
        return;
    }
    let Some(close) = st.commit.as_ref().and_then(|c| c.chars().last()).and_then(ime_proto::closer_of) else { return };
    if let Some(c) = st.commit.as_mut() {
        c.push(close);
    }
    st.caret_back = close.len_utf16() as u32;
}

/// Rime's input while nothing of it has been picked yet (only letters).
fn typed_letters(snap: &Snapshot) -> Option<&str> {
    let pre = &snap.preedit.as_ref()?.0;
    let raw = snap.input.as_str();
    let letters = !raw.is_empty() && raw.chars().all(|c| c.is_ascii_alphabetic() || c == '\'' || c == ';');
    let picked = !pre.is_ascii();
    (letters && !picked).then_some(raw)
}

impl Engine {
    pub fn new(rime: Rime, mozc: Option<Mozc>, ui: Box<dyn Ui>, schema: &str) -> Engine {
        let mut en = crate::mixed::EnDict::load(&crate::paths::shared_dir().join("en_dicts"));
        en.load_personal(&crate::paths::user_dir().join("en_personal.txt"));
        crate::log(&format!("english words: {}", en.len()));
        Engine {
            rime,
            mozc,
            inner: Mutex::new(Inner { sessions: HashMap::new(), next: 1, focused: None }),
            ui,
            schema: schema.to_string(),
            settings: Mutex::new(crate::paths::load_settings()),
            pref: Mutex::new(LangPref::load(&crate::paths::data_file("langpref.json"))),
            modes: Mutex::new(ModeMemory::load(&crate::paths::data_file("modes.json"))),
            en,
            hidden: Mutex::new(crate::mixed::Hidden::load(&crate::paths::data_file("hidden.json"))),
            look: Mutex::new(Look::default()),
        }
    }

    fn skin(&self) -> ime_proto::Skin {
        let Ok(mut l) = self.look.lock() else { return Default::default() };
        if l.checked.map(|t| t.elapsed() < std::time::Duration::from_secs(3)).unwrap_or(false) {
            return l.skin.clone();
        }
        l.checked = Some(std::time::Instant::now());
        let settings = crate::paths::settings_path();
        let weather = crate::paths::weather_path();
        let stamp = |p: &std::path::Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
        let stamps = (stamp(&settings), stamp(&weather));
        if stamps != l.stamps || l.kind.is_none() {
            l.stamps = stamps;
            l.kind = Some(crate::paths::load_settings().skin);
            l.weather = std::fs::read(&weather).ok().and_then(|b| serde_json::from_slice(&b).ok());
        }
        // Derived again each time: the hour moves on.
        l.skin = make_skin(l.kind.as_deref().unwrap_or(""), l.weather.as_ref(), now_secs());
        l.skin.clone()
    }

    fn flash_note(&self, session: u64, text: &str) -> UiOut {
        UiOut::Show(View { session, candidates: vec![(text.to_string(), String::new())], labels: vec![String::new()], highlighted: -1, last_page: true, flash: true, ..Default::default() })
    }

    /// （ typed: put in （） with the caret between (settings: auto_pair).
    fn pair(&self, st: &mut State) {
        if self.settings.lock().map(|s| s.auto_pair).unwrap_or(true) {
            add_pair(st);
        }
    }

    /// Put the current skin on a window about to be shown.
    fn dress(&self, ui: UiOut) -> UiOut {
        match ui {
            UiOut::Show(mut v) => {
                v.skin = self.skin();
                UiOut::Show(v)
            }
            other => other,
        }
    }

    #[allow(dead_code)]
    pub fn rime(&self) -> &Rime {
        &self.rime
    }

    fn ready(&self) -> bool {
        !self.rime.is_maintaining()
    }

    pub fn page_size(&self) -> usize {
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

    pub(crate) fn mozc_aux(&self, s: &mut Sess) -> Option<u64> {
        let m = self.mozc.as_ref()?;
        if s.mozc_aux == 0 {
            s.mozc_aux = m.create_session()?;
        }
        Some(s.mozc_aux)
    }

    pub(crate) fn to_state(&self, eaten: bool, snap: &Snapshot) -> State {
        State {
            eaten,
            commit: snap.commit.clone().filter(|c| !c.is_empty()),
            preedit: snap.preedit.as_ref().map(|(text, cursor)| match typed_letters(snap) {
                // Nothing picked yet: the letters as typed (what Enter puts
                // in), the reading is in the candidate window.
                Some(t) => Preedit { text: t.to_string(), cursor: t.encode_utf16().count() as u32 },
                None => Preedit { text: text.clone(), cursor: utf16_index(text, *cursor) },
            }),
            ascii: snap.ascii,
            delete_before: 0,
            caret_back: 0,
            cands: None,
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
                vertical: false,
                typed: typed_letters(snap).unwrap_or_default().to_string(),
                ..Default::default()
            }),
            _ => UiOut::Hide,
        }
    }

    /// For a session whose DLL draws the candidates: turn what the window
    /// should do into `State::cands`, and keep our own window out of it
    /// (hidden, in case it still shows another program's candidates).
    fn route(draws: bool, ui: UiOut) -> (Option<CandUi>, UiOut) {
        if !draws {
            return (None, ui);
        }
        match ui {
            UiOut::Keep => (None, UiOut::Keep),
            UiOut::Hide => (Some(CandUi::Hide), UiOut::Keep),
            UiOut::Show(v) => (Some(CandUi::Show { view: v.cands() }), UiOut::Hide),
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
            Request::Hello { version, exe, notify, draws, build, .. } => {
                // A program opened before the last update still has the old
                // DLL: its fixes do not reach it until it is reopened.
                let stale = !build.is_empty() && !ime_proto::BUILD.is_empty() && build != ime_proto::BUILD;
                if stale {
                    crate::log(&format!("dll [{exe}] is build {build}, engine {}: the program needs reopening", ime_proto::BUILD));
                }
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
                        draws,
                        caret: None,
                        pending: None,
                        mode,
                        mozc: 0,
                        mozc_aux: 0,
                        comp: None,
                        last: None,
                        last_ja: None,
                        recent: String::new(),
                        sentence: [0; 3],
                        cmd: None,
                        stale,
                    },
                );
                Reply::Hello { version: ime_proto::VERSION, session: id }
            }
            Request::RawKey { session, vk, scan, flags, hkl } => {
                if flags & ime_proto::rawkey::UP == 0 && crate::paths::JIS_ONLY_SCANS.contains(&scan) && crate::paths::note_jis_key() {
                    // A Japanese keyboard after all: its punctuation layout
                    // (Shift+comma for 、) needs the dictionaries rebuilt.
                    crate::log("Japanese keyboard detected");
                    drop(g);
                    self.handle(Request::Deploy);
                    return self.handle(Request::RawKey { session, vk, scan, flags, hkl });
                }
                #[cfg(windows)]
                let key = crate::win::keyconv::convert(vk, scan, flags, hkl);
                #[cfg(not(windows))]
                let key: Option<(u32, u32)> = {
                    let _ = (vk, scan, flags, hkl);
                    None
                };
                let Some((keycode, mask)) = key else { return Reply::State(State::default()) };
                drop(g);
                self.handle(Request::Key { session, keycode, mask })
            }
            Request::Key { session, keycode, mask } => {
                if !self.ready() {
                    return Reply::Busy;
                }
                let Some(s) = g.sessions.get_mut(&session) else { return Reply::Error { message: "no such session".into() } };
                let (mut state, mut ui) = self.key(s, session, keycode, mask);
                self.pair(&mut state);
                if s.stale && mask & ime_proto::mask::RELEASE == 0 && !matches!(ui, UiOut::Show(_)) && state.preedit.is_none() {
                    s.stale = false;
                    ui = self.flash_note(session, "输入法已更新：重新打开这个程序才会用上新版本");
                }
                let (cands, ui) = Self::route(s.draws, self.dress(ui));
                state.cands = cands;
                let caret = s.caret;
                g.focused = Some(session);
                drop(g);
                self.apply_ui(ui, caret);
                Reply::State(state)
            }
            Request::Pick { session, index, page } => {
                if !self.ready() {
                    return Reply::Busy;
                }
                let Some(s) = g.sessions.get_mut(&session) else { return Reply::Error { message: "no such session".into() } };
                let (mut state, ui) = self.click(s, session, index.map(|i| i as usize), page);
                let (cands, ui) = Self::route(s.draws, self.dress(ui));
                state.cands = cands;
                let caret = s.caret;
                drop(g);
                self.apply_ui(ui, caret);
                Reply::State(state)
            }
            Request::Forget { session, index } => {
                if !self.ready() {
                    return Reply::Busy;
                }
                let Some(s) = g.sessions.get_mut(&session) else { return Reply::Error { message: "no such session".into() } };
                let (mut state, ui) = self.forget(s, session, index as usize);
                let (cands, ui) = Self::route(s.draws, self.dress(ui));
                state.cands = cands;
                let caret = s.caret;
                drop(g);
                self.apply_ui(ui, caret);
                Reply::State(state)
            }
            Request::Caret { session, x, y, h } => {
                let c = Caret { x, y, h };
                let mut draws = false;
                if let Some(s) = g.sessions.get_mut(&session) {
                    s.caret = Some(c);
                    draws = s.draws;
                }
                if g.focused == Some(session) && !draws {
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
                    s.sentence = [0; 3];
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
                    if let (Some(m), true) = (&self.mozc, s.mozc_aux != 0) {
                        m.delete_session(s.mozc_aux);
                    }
                }
                if g.focused == Some(session) {
                    g.focused = None;
                    drop(g);
                    self.ui.hide();
                }
                Reply::Ok
            }
            Request::Note { session, message } => {
                let exe = g.sessions.get(&session).map(|s| s.exe.clone()).unwrap_or_default();
                drop(g);
                crate::log(&format!("dll [{exe}]: {message}"));
                Reply::Ok
            }
            Request::Restart => {
                drop(g);
                crate::log("restart requested");
                #[cfg(windows)]
                std::thread::spawn(|| {
                    std::thread::sleep(std::time::Duration::from_millis(150));
                    if let Some(e) = crate::win::ENGINE.get() {
                        e.shutdown();
                    }
                });
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
        let ui = self.dress(ui);
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

    /// Right click, then click, in our own window: delete candidate `index`
    /// of the current page. The spelling in the program stays as it is.
    pub fn forget_from_window(&self, session: u64, index: usize) {
        let mut g = match self.inner.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if !self.ready() {
            return;
        }
        let Some(s) = g.sessions.get_mut(&session) else { return };
        let (_, ui) = self.forget(s, session, index);
        let ui = self.dress(ui);
        let caret = s.caret;
        drop(g);
        self.apply_ui(ui, caret);
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
        // Everything is saved. Leave without running the libraries' exit
        // handlers: librime / Mozc tear down in an order that crashed in
        // KERNELBASE (0x87a) on every installer stop.
        #[cfg(windows)]
        unsafe {
            use windows::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
            let _ = TerminateProcess(GetCurrentProcess(), 0);
        }
        std::process::exit(0)
    }

    #[allow(dead_code)]
    pub fn exe_of(&self, session: u64) -> Option<String> {
        self.inner.lock().ok()?.sessions.get(&session).map(|s| s.exe.clone())
    }

    /// For --bench: text the user typed some other way (the word was not
    /// offered): clear the composition and record it as our output.
    #[allow(dead_code)]
    pub fn bench_commit(&self, session: u64, text: &str, lang: crate::mixed::Lang) {
        let mut g = match self.inner.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if let Some(s) = g.sessions.get_mut(&session) {
            self.clear(s);
            self.bench_committed(s, text, lang);
        }
    }

    #[allow(dead_code)]
    pub fn bench_set_mode(&self, session: u64, mode: crate::compose::Mode) {
        if let Ok(mut g) = self.inner.lock() {
            if let Some(s) = g.sessions.get_mut(&session) {
                s.mode = mode;
            }
        }
    }

    /// For the --cli test: a session without a pipe.
    #[allow(dead_code)]
    pub fn test_session(&self, exe: &str) -> u64 {
        match self.handle(Request::Hello { version: ime_proto::VERSION, pid: 0, exe: exe.into(), notify: 0, draws: false, build: String::new() }) {
            Reply::Hello { session, .. } => session,
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{utf16_index, Engine, UiOut, View};
    use ime_proto::CandUi;

    #[test]
    fn route_to_dll() {
        let v = View { session: 1, preedit: "ni".into(), candidates: vec![("你".into(), String::new())], labels: vec!["1".into()], ..Default::default() };
        // Ordinary programs: the engine's window, nothing in the State.
        let (c, ui) = Engine::route(false, UiOut::Show(v.clone()));
        assert!(c.is_none() && matches!(ui, UiOut::Show(_)));
        // Drawn by the DLL: the view goes back, our window hides.
        let (c, ui) = Engine::route(true, UiOut::Show(v));
        assert!(matches!(c, Some(CandUi::Show { ref view }) if view.preedit == "ni" && view.candidates.len() == 1));
        assert!(matches!(ui, UiOut::Hide));
        let (c, ui) = Engine::route(true, UiOut::Hide);
        assert!(matches!(c, Some(CandUi::Hide)) && matches!(ui, UiOut::Keep));
        let (c, ui) = Engine::route(true, UiOut::Keep);
        assert!(c.is_none() && matches!(ui, UiOut::Keep));
    }

    #[test]
    fn skins() {
        use super::make_skin;
        use ime_proto::Weather;
        let w = Weather { at: 10_000, temp: 20.0, hours: (0..12).map(|i| (if i < 3 { 61 } else { 0 }, true)).collect() };
        assert!(make_skin("time", Some(&w), 10_000).hours.is_empty());
        let s = make_skin("weather", Some(&w), 10_000);
        assert_eq!(s.hours.len(), 8);
        assert_eq!(s.label, "20° 小雨转晴");
        // Two hours later the bar starts two hours in.
        assert_eq!(make_skin("weather", Some(&w), 10_000 + 7300).hours[0].0, 61);
        assert_eq!(make_skin("weather", Some(&w), 10_000 + 3 * 3600).hours[0].0, 0);
        // Too old: no weather.
        assert!(make_skin("weather", Some(&w), 10_000 + 7 * 3600).hours.is_empty());
    }

    #[test]
    fn pairs() {
        use super::add_pair;
        use ime_proto::{Preedit, State};
        let mut st = State { eaten: true, commit: Some("你好（".into()), ..Default::default() };
        add_pair(&mut st);
        assert_eq!((st.commit.as_deref(), st.caret_back), (Some("你好（）"), 1));
        // Closers and plain text stay as they are.
        for c in ["）", "你好", "("] {
            let mut st = State { eaten: true, commit: Some(c.into()), ..Default::default() };
            add_pair(&mut st);
            assert_eq!((st.commit.as_deref(), st.caret_back), (Some(c), 0));
        }
        // Still composing (a partial pick): not yet.
        let mut st = State { eaten: true, commit: Some("「".into()), preedit: Some(Preedit { text: "hao".into(), cursor: 3 }), ..Default::default() };
        add_pair(&mut st);
        assert_eq!(st.caret_back, 0);
    }

    #[test]
    fn utf16() {
        assert_eq!(utf16_index("你 hao", "你".len()), 1);
        assert_eq!(utf16_index("𠀀a", 4), 2);
        assert_eq!(utf16_index("ab", 9), 2);
    }
}
