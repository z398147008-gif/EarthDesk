//! The text service object TSF creates once per UI thread of each program.
//!
//! Key flow (the Weasel pattern, which copes with programs that call the
//! sink oddly):
//! - OnTestKeyDown sends the key to the engine; if it was eaten, the
//!   answer is kept and applied in the OnKeyDown that follows (programs
//!   refuse edits during the test: Chromium does, even queued ones).
//!   An answer for a key we let through is applied right away.
//! - OnKeyDown without a preceding test does both.
//! - Key-ups go to the engine too (Shift alone toggles 中/英 on release)
//!   but are never eaten; from OnTestKeyUp, or OnKeyUp when the program
//!   skipped the test.
//! - Keys in a context that takes no text (edit::writable: Chrome while
//!   the page's focus is not in a text field) are the program's: they go
//!   to it untouched, the engine never sees them. Except when Chrome left
//!   the keyboard on its empty document although the page does have a text
//!   field focused: they type into that field (edit::chrome_field).

use crate::candwin;
use crate::client::Client;
use crate::edit::{self, Shared};
use crate::{guard, keys, DISPLAY_ATTR};
use ime_proto::{Request, State};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use windows::core::{implement, w, IUnknown, Interface, Result, BOOL, BSTR, GUID, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::TextServices::*;
use windows::Win32::UI::WindowsAndMessaging::*;

#[implement(
    ITfTextInputProcessorEx,
    ITfTextInputProcessor,
    ITfKeyEventSink,
    ITfThreadMgrEventSink,
    ITfCompositionSink,
    ITfDisplayAttributeProvider
)]
pub struct TextService {
    tid: Cell<u32>,
    thread_mgr: RefCell<Option<ITfThreadMgr>>,
    sink_cookie: Cell<u32>,
    attr: Cell<Option<i32>>,
    test_pending: Cell<bool>,
    notify: Cell<HWND>,
    client: Rc<RefCell<Client>>,
    composition: Shared,
    /// The context the current composition lives in (for notifications).
    context: RefCell<Option<ITfContext>>,
    /// Our composition's text after the program ended it (edit::Orphan).
    orphan: RefCell<Option<edit::Orphan>>,
    /// Edits the program refused, waiting to be tried again (in order).
    retry: RefCell<std::collections::VecDeque<(ITfContext, State, edit::Adopt)>>,
    retries: Cell<u32>,
    /// ← presses owed once the waiting edits are in (bracket pairs).
    left_after: Cell<u32>,
    /// An eaten key's answer, from OnTestKeyDown, for OnKeyDown to apply.
    /// (With the context it goes to: see `target`.)
    deferred: RefCell<Option<(State, edit::Adopt, ITfContext)>>,
    /// The key whose release OnTestKeyUp handled (OnKeyUp then skips it).
    up_done: Cell<Option<u16>>,
    /// Where keys went last when the program's context takes no text (0:
    /// it does; 1: passed to the program; 2: Chrome's field), logged once
    /// per change.
    readonly_noted: Cell<u8>,
    /// The preedit last put in the document (what an ended composition
    /// leaves behind).
    last_preedit: RefCell<Vec<u16>>,
    /// The preedit in the engine's last answer, put in or not: a key-up
    /// that leaves it as it was changes nothing in the document.
    engine_pre: RefCell<Vec<u16>>,
    /// 英数 pressed (a JIS keyboard's Caps Lock key): Caps Lock as it will
    /// be once our toggle is in, for the release's notice.
    alnum_caps: Cell<Option<bool>>,
    ended: edit::Ended,
    /// Interfaces to ourselves, set right after creation.
    me: RefCell<Option<IUnknown>>,
}

thread_local! {
    /// The text service of this thread, for the notification window.
    static CURRENT: RefCell<Option<ITfTextInputProcessor>> = const { RefCell::new(None) };
}

impl TextService {
    pub fn create() -> IUnknown {
        let object = windows_core::ComObject::new(TextService {
            tid: Cell::new(0),
            thread_mgr: RefCell::new(None),
            sink_cookie: Cell::new(TF_INVALID_COOKIE),
            attr: Cell::new(None),
            test_pending: Cell::new(false),
            notify: Cell::new(HWND::default()),
            client: Rc::new(RefCell::new(Client::new(0, false))),
            composition: Rc::new(RefCell::new(None)),
            context: RefCell::new(None),
            orphan: RefCell::new(None),
            deferred: RefCell::new(None),
            up_done: Cell::new(None),
            readonly_noted: Cell::new(0),
            retry: RefCell::new(std::collections::VecDeque::new()),
            retries: Cell::new(0),
            left_after: Cell::new(0),
            last_preedit: RefCell::new(Vec::new()),
            engine_pre: RefCell::new(Vec::new()),
            alnum_caps: Cell::new(None),
            ended: Rc::new(Cell::new(0)),
            me: RefCell::new(None),
        });
        let unknown: IUnknown = object.to_interface();
        *object.me.borrow_mut() = Some(unknown.clone());
        unknown
    }
}

// --- the notification window ---------------------------------------------------------

static CLASS_READY: AtomicBool = AtomicBool::new(false);

/// On the notification window: try refused edits again.
const RETRY_TIMER: usize = 0x4552;

unsafe extern "system" fn notify_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_TIMER && wp.0 == RETRY_TIMER {
        guard((), || {
            CURRENT.with(|c| {
                if let Some(tip) = c.borrow().clone() {
                    if let Ok(s) = tip.cast_object_ref::<TextService>() {
                        s.on_retry();
                    }
                }
            })
        });
        return LRESULT(0);
    }
    if msg == ime_proto::WM_NOTIFY_OFFSET {
        guard((), || {
            CURRENT.with(|c| {
                if let Some(tip) = c.borrow().clone() {
                    if let Ok(s) = tip.cast_object_ref::<TextService>() {
                        s.on_notify();
                    }
                }
            })
        });
        return LRESULT(0);
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

unsafe fn create_notify_window() -> HWND {
    let class = w!("EarthDeskTSFNotify");
    if !CLASS_READY.swap(true, Ordering::SeqCst) {
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(notify_proc),
            hInstance: crate::module(),
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassExW(&wc);
    }
    let hwnd = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        class,
        PCWSTR::null(),
        WINDOW_STYLE(0),
        0,
        0,
        0,
        0,
        Some(HWND_MESSAGE),
        None,
        Some(crate::module()),
        None,
    )
    .unwrap_or_default();
    if !hwnd.0.is_null() {
        // Elevated hosts would otherwise drop the engine's (unelevated)
        // message on the floor.
        let _ = ChangeWindowMessageFilterEx(hwnd, ime_proto::WM_NOTIFY_OFFSET, MSGFLT_ALLOW, None);
    }
    hwnd
}

/// From our candidate window: delete candidate `index` of the page.
pub fn forget_from_window(index: u32) {
    CURRENT.with(|c| {
        let tip = c.try_borrow().ok().and_then(|c| c.clone());
        if let Some(tip) = tip {
            if let Ok(s) = tip.cast_object_ref::<TextService>() {
                s.on_forget(index);
            }
        }
    })
}

/// From our candidate window (candwin.rs): a candidate, a page arrow or the
/// wheel, for this thread's text service.
pub fn pick_from_window(index: Option<u32>, page: Option<bool>) {
    CURRENT.with(|c| {
        let tip = c.try_borrow().ok().and_then(|c| c.clone());
        if let Some(tip) = tip {
            if let Ok(s) = tip.cast_object_ref::<TextService>() {
                s.on_pick(index, page);
            }
        }
    })
}

// --- applying engine answers ---------------------------------------------------------

impl TextService {
    fn sink(&self) -> Option<ITfCompositionSink> {
        self.me.borrow().as_ref()?.cast().ok()
    }

    fn apply(&self, context: &ITfContext, state: &State, sync: bool) {
        self.apply_adopting(context, state, sync, edit::Adopt::No);
    }

    /// Tell the engine's log (never with typed text in it).
    fn note(&self, message: &str) {
        if let Ok(mut c) = self.client.try_borrow_mut() {
            c.tell(|session| Request::Note { session, message: message.to_string() });
        }
    }

    /// false: the program's text field takes no text, everything dropped.
    fn apply_adopting(&self, context: &ITfContext, state: &State, sync: bool, adopt: edit::Adopt) -> bool {
        if let Ok(mut p) = self.last_preedit.try_borrow_mut() {
            match &state.preedit {
                Some(pre) if !pre.text.is_empty() => *p = pre.text.encode_utf16().collect(),
                _ => p.clear(),
            }
        }
        if state.preedit.is_some() {
            *self.context.borrow_mut() = Some(context.clone());
        }
        // Edits still waiting: this one goes behind them, never before.
        let waiting = self.retry.try_borrow().map(|q| !q.is_empty()).unwrap_or(false);
        if waiting {
            self.hold(context, state, adopt);
            return true;
        }
        if let Err(why) = self.edit_now(context, state, sync, adopt.clone()) {
            // Read-only now: it never will take it, do not hold every key
            // after it for seconds.
            if !edit::writable(context) {
                self.note(&format!("edit refused ({why}): the text field takes no text, dropped"));
                self.drop_typing();
                return false;
            }
            self.note(&format!("edit refused ({why}): kept, retrying"));
            self.hold(context, state, adopt);
        }
        true
    }

    /// Something typed, shown or waiting that the engine or the document
    /// still holds.
    fn busy(&self) -> bool {
        self.composition.try_borrow().map(|c| c.is_some()).unwrap_or(false)
            || self.last_preedit.try_borrow().map(|p| !p.is_empty()).unwrap_or(false)
            || self.retry.try_borrow().map(|q| !q.is_empty()).unwrap_or(false)
            || self.orphan.try_borrow().map(|o| o.is_some()).unwrap_or(false)
            || self.deferred.try_borrow().map(|d| d.is_some()).unwrap_or(false)
    }

    /// The program cannot take text where it has the focus now: what was
    /// being typed and the edits waiting are dropped, the engine forgets.
    fn drop_typing(&self) {
        if let Ok(mut q) = self.retry.try_borrow_mut() {
            q.clear();
        }
        self.retries.set(0);
        self.left_after.set(0);
        if let Ok(mut p) = self.last_preedit.try_borrow_mut() {
            p.clear();
        }
        self.reset();
    }

    /// One edit session; Err = the program would not take it at all.
    fn edit_now(&self, context: &ITfContext, state: &State, sync: bool, adopt: edit::Adopt) -> std::result::Result<(), String> {
        let Some(sink) = self.sink() else { return Ok(()) };
        let client = self.client.clone();
        let on_caret: edit::OnCaret = Rc::new(move |rc: RECT| {
            candwin::caret(rc);
            if let Ok(mut c) = client.try_borrow_mut() {
                c.tell(|session| Request::Caret { session, x: rc.left, y: rc.top, h: (rc.bottom - rc.top).max(1) });
            }
        });
        let note = edit::apply(self.tid.get(), context, &sink, &self.composition, state, self.attr.get(), sync, on_caret, adopt, &self.ended)?;
        if let Some(note) = note {
            self.note(&note);
        }
        Ok(())
    }

    /// Keep a refused edit and try again from our own window, once the
    /// program is done with the key (Chrome refuses every edit while it
    /// holds the document, even queued ones).
    fn hold(&self, context: &ITfContext, state: &State, adopt: edit::Adopt) {
        if let Ok(mut q) = self.retry.try_borrow_mut() {
            q.push_back((context.clone(), state.clone(), adopt));
            if q.len() > 64 {
                q.pop_front();
            }
        }
        let n = self.notify.get();
        if !n.0.is_null() {
            unsafe {
                let _ = SetTimer(Some(n), RETRY_TIMER, 15, None);
            }
        }
    }

    /// RETRY_TIMER: the waiting edits, in order, as far as the program
    /// takes them now.
    fn on_retry(&self) {
        let n = self.notify.get();
        loop {
            let next = self.retry.try_borrow_mut().ok().and_then(|mut q| q.pop_front());
            let Some((ctx, state, adopt)) = next else { break };
            if let Err(why) = self.edit_now(&ctx, &state, false, adopt.clone()) {
                if !edit::writable(&ctx) {
                    // It will never take them: drop them now, not after
                    // seconds of every key waiting behind them.
                    self.note(&format!("edits refused ({why}): the text field takes no text, dropped"));
                    if let Ok(mut q) = self.retry.try_borrow_mut() {
                        q.clear();
                    }
                    self.left_after.set(0);
                    candwin::hide();
                    if let Ok(mut c) = self.client.try_borrow_mut() {
                        c.tell(|session| Request::Reset { session });
                    }
                    break;
                }
                if let Ok(mut q) = self.retry.try_borrow_mut() {
                    q.push_front((ctx, state, adopt));
                }
                let tries = self.retries.get() + 1;
                self.retries.set(tries);
                if tries > 200 {
                    // Three seconds of refusals: the program is gone or stuck.
                    self.note(&format!("edits still refused ({why}): dropped"));
                    if let Ok(mut q) = self.retry.try_borrow_mut() {
                        q.clear();
                    }
                    break;
                }
                return;
            }
            self.retries.set(0);
        }
        self.retries.set(0);
        if !n.0.is_null() {
            unsafe {
                let _ = KillTimer(Some(n), RETRY_TIMER);
            }
        }
        let left = self.left_after.replace(0);
        if left > 0 {
            keys::caret_left(left);
        }
    }

    /// Our own candidate window (if any) follows the engine's answer.
    fn show_cands(&self, state: &State) {
        let Some(ui) = &state.cands else { return };
        candwin::update(ui);
        let notes = candwin::take_notes();
        if !notes.is_empty() {
            if let Ok(mut c) = self.client.try_borrow_mut() {
                for n in notes {
                    c.tell(|session| Request::Note { session, message: n.clone() });
                }
            }
        }
    }

    /// The engine has news for us (a candidate was clicked).
    fn on_notify(&self) {
        let Some(ctx) = self.context.borrow().clone() else { return };
        let st = {
            let Ok(mut c) = self.client.try_borrow_mut() else { return };
            match c.call(|session| Request::Poll { session }) {
                Some(ime_proto::Reply::State(s)) => s,
                _ => return,
            }
        };
        self.heard(&st);
        if st.commit.is_some() || st.preedit.is_some() {
            self.apply(&ctx, &st, false);
        }
        self.show_cands(&st);
    }

    /// A click (or the wheel) in our own candidate window.
    fn on_pick(&self, index: Option<u32>, page: Option<bool>) {
        let Some(ctx) = self.context.borrow().clone() else { return };
        let st = {
            let Ok(mut c) = self.client.try_borrow_mut() else { return };
            match c.pick(index, page) {
                Some(s) => s,
                None => return,
            }
        };
        self.heard(&st);
        let composing = self.composition.try_borrow().map(|c| c.is_some()).unwrap_or(false);
        if st.commit.is_some() || st.preedit.is_some() || composing {
            self.apply(&ctx, &st, false);
        }
        self.show_cands(&st);
    }

    /// A candidate deleted in our own window (right click, then click).
    fn on_forget(&self, index: u32) {
        let st = {
            let Ok(mut c) = self.client.try_borrow_mut() else { return };
            match c.forget(index) {
                Some(s) => s,
                None => return,
            }
        };
        self.show_cands(&st);
    }

    /// Send a key; apply the answer. Returns whether it was eaten.
    /// `defer`: called from OnTestKeyDown; an eaten key's edit is kept for
    /// OnKeyDown.
    fn process(&self, context: Option<&ITfContext>, wp: WPARAM, lp: LPARAM, up: bool, defer: bool) -> bool {
        // Keys 地球桌面 sends itself (a gesture's Ctrl+W) are commands for
        // the program, never typing.
        let composing_now = self.composition.try_borrow().map(|c| c.is_some()).unwrap_or(false);
        if !composing_now && unsafe { windows::Win32::UI::WindowsAndMessaging::GetMessageExtraInfo() }.0 as usize == ime_proto::INJECTED {
            return false;
        }
        // Nowhere to type (Chrome while the page's focus is not in a text
        // field, a password box): the key is the program's — a web app's
        // shortcut — and goes to it untouched. Eating it would start a
        // composition every edit of which the program refuses (TS_E_READONLY).
        let Some(ctx) = self.target(context, up) else {
            if !up && self.busy() {
                self.drop_typing();
            }
            return false;
        };
        let context = Some(&ctx);
        // A JIS keyboard's Caps Lock key (see alnum_key), alone.
        if wp.0 == 0xF0 && !keys::modifiers_held() {
            return self.alnum_key(&ctx, lp, up, defer);
        }
        let mut adopt = edit::Adopt::No;
        if !up {
            adopt = self.adopt_orphan(&ctx);
        }
        let state = {
            let Ok(mut c) = self.client.try_borrow_mut() else { return false };
            let (vk, scan, flags, hkl) = keys::raw(wp.0 as u16, lp.0, up);
            match c.raw_key(vk, scan, flags, hkl) {
                Ok(Some(s)) => s,
                Ok(None) => return false,
                // An engine from before RawKey: convert here.
                Err(()) => {
                    let Some(key) = keys::convert(wp.0 as u16, lp.0, up) else { return false };
                    match c.key(key.code, key.mask) {
                        Some(s) => s,
                        None => return false,
                    }
                }
            }
        };
        let before = self.heard(&state);
        // A release that leaves the engine's answer as it was changes
        // nothing in the document — also when the program cut the
        // composition off meanwhile: putting the spelling in again would
        // type it twice ("vemvem"); the next key takes the cut-off text
        // over (Orphan). Programs that report releases through OnKeyUp
        // (Chromium) do exactly that after every key.
        if up {
            let same = state.commit.is_none() && state.preedit.as_ref().map(|p| p.text.encode_utf16().collect::<Vec<u16>>()).unwrap_or_default() == before;
            if same {
                self.show_cands(&state);
                return false;
            }
            adopt = self.adopt_orphan(&ctx);
        }
        // Eaten in OnTestKeyDown: the edit waits for the OnKeyDown that
        // follows. Programs refuse edits while they are only asking whether
        // we want a key (Chromium answers TS_E_SYNCHRONOUS even to a queued
        // request then, and the edit is lost); in OnKeyDown TSF grants them.
        if defer && state.eaten && !up {
            *self.deferred.borrow_mut() = Some((state, adopt, ctx.clone()));
            return true;
        }
        self.finish(context, state, adopt, up)
    }

    /// The engine answered `state`: its preedit before this answer.
    fn heard(&self, state: &State) -> Vec<u16> {
        let now: Vec<u16> = state.preedit.as_ref().map(|p| p.text.encode_utf16().collect()).unwrap_or_default();
        self.engine_pre.try_borrow_mut().map(|mut p| std::mem::replace(&mut *p, now)).unwrap_or_default()
    }

    /// The program cut our composition off since the last key: carry on
    /// over its text if the caret is still right after it, else start
    /// afresh (the engine forgets it).
    fn adopt_orphan(&self, ctx: &ITfContext) -> edit::Adopt {
        let Some(o) = self.orphan.borrow_mut().take() else { return edit::Adopt::No };
        match edit::find_orphan(self.tid.get(), ctx, &o.text) {
            edit::Found::Here(r) => {
                self.note(&format!("cut-off composition ({} chars) taken over", o.text.len()));
                edit::Adopt::Range(r)
            }
            // Cannot look now: look inside the edit itself, if the key
            // comes soon after (the user is still typing on).
            edit::Found::Refused if o.at.elapsed() < std::time::Duration::from_secs(10) => {
                self.note("cut-off composition: program refused a look, trying inside the edit");
                edit::Adopt::Find(o.text)
            }
            _ => {
                self.note(&format!("cut-off composition ({} chars) not at the caret any more: dropped", o.text.len()));
                candwin::hide();
                if let Ok(mut c) = self.client.try_borrow_mut() {
                    c.tell(|session| Request::Reset { session });
                }
                if let Ok(mut p) = self.engine_pre.try_borrow_mut() {
                    p.clear();
                }
                edit::Adopt::No
            }
        }
    }

    /// 英数: the Caps Lock key of a JIS keyboard, as the Japanese layout
    /// we register on such keyboards (register.rs) reports it without
    /// Shift. There it locks nothing (only Shift+英数 is Caps Lock), so
    /// capitals could not be locked at all. Here it is the Caps Lock key it
    /// says it is: to the engine a Caps Lock press (what was typed so far
    /// goes in as it is), then Caps Lock toggled for the program; eaten.
    fn alnum_key(&self, ctx: &ITfContext, lp: LPARAM, up: bool, defer: bool) -> bool {
        use windows::Win32::UI::Input::KeyboardAndMouse::VK_CAPITAL;
        let (vk, scan, flags, hkl) = keys::raw(VK_CAPITAL.0, lp.0, up);
        if up {
            // The release: the engine's notice says which way (大写 / 中文),
            // with Caps Lock as our toggle leaves it.
            let Some(on) = self.alnum_caps.take() else { return false };
            let flags = if on { flags | ime_proto::rawkey::CAPS } else { flags & !ime_proto::rawkey::CAPS };
            let state = self.client.try_borrow_mut().ok().and_then(|mut c| c.raw_key(vk, scan, flags, hkl).ok().flatten());
            if let Some(state) = state {
                self.heard(&state);
                self.show_cands(&state);
            }
            return false;
        }
        let on = flags & ime_proto::rawkey::CAPS != 0;
        let state = self.client.try_borrow_mut().ok().and_then(|mut c| c.raw_key(vk, scan, flags, hkl).ok().flatten());
        if let Some(mut state) = state {
            self.heard(&state);
            let adopt = self.adopt_orphan(ctx);
            // Ours now: the program never sees 英数.
            state.eaten = true;
            if defer {
                *self.deferred.borrow_mut() = Some((state, adopt, ctx.clone()));
            } else {
                self.finish(Some(ctx), state, adopt, false);
            }
        }
        keys::toggle_caps();
        self.alnum_caps.set(Some(!on));
        true
    }

    /// Put the engine's answer into the document and the window.
    fn finish(&self, context: Option<&ITfContext>, state: State, adopt: edit::Adopt, up: bool) -> bool {
        let waiting = self.retry.try_borrow().map(|q| !q.is_empty()).unwrap_or(false);
        // Edits still waiting may hold a composition our slot does not show
        // yet: whatever comes now must follow them (an Esc ends it).
        let composing = self.composition.try_borrow().map(|c| c.is_some()).unwrap_or(false) || !matches!(adopt, edit::Adopt::No) || waiting;
        // A key-up hands back the preedit as it is: no edit for nothing.
        let same = up
            && state.commit.is_none()
            && self.last_preedit.try_borrow().map(|p| match &state.preedit {
                Some(pre) => pre.text.encode_utf16().eq(p.iter().copied()),
                None => p.is_empty() && !composing,
            }).unwrap_or(false);
        if let (Some(ctx), false) = (context, same) {
            if (state.commit.is_some() || state.preedit.is_some() || composing) && !self.apply_adopting(ctx, &state, true, adopt) {
                // Dropped: no candidates, no caret moves for text that is
                // not there.
                return state.eaten && !up;
            }
        }
        // After the edit, so the window opens where the text now is.
        self.show_cands(&state);
        // A pair of brackets went in: the caret goes between them — after
        // the edit, so once waiting edits are in (typed keys would otherwise
        // overtake them).
        if state.caret_back > 0 && !up {
            if self.retry.try_borrow().map(|q| !q.is_empty()).unwrap_or(false) {
                self.left_after.set(self.left_after.get() + state.caret_back);
            } else {
                keys::caret_left(state.caret_back);
            }
        }
        state.eaten && !up
    }

    /// The context a key types into: the program's, if it takes text; when
    /// it is Chrome's empty document, the page's focused field if it has one
    /// (edit::chrome_field). None: the key is the program's.
    fn target(&self, pic: Option<&ITfContext>, up: bool) -> Option<ITfContext> {
        let pic = pic?;
        if edit::writable(pic) {
            self.readonly_noted.set(0);
            return Some(pic.clone());
        }
        let tm = self.thread_mgr.borrow().clone();
        let field = tm.and_then(|tm| edit::chrome_field(self.tid.get(), &tm, pic));
        let how = if field.is_some() { 2 } else { 1 };
        if !up && self.readonly_noted.replace(how) != how {
            self.note(if field.is_some() {
                "keyboard on Chrome's empty document while the page has a text field focused: typing into Chrome's text document"
            } else {
                "keys in a context that takes no text: passed to the program"
            });
        }
        field
    }

    fn reset(&self) {
        candwin::hide();
        *self.orphan.borrow_mut() = None;
        if let Ok(mut p) = self.engine_pre.try_borrow_mut() {
            p.clear();
        }
        self.alnum_caps.set(None);
        if let Ok(mut d) = self.deferred.try_borrow_mut() {
            *d = None;
        }
        // Waiting edits (a word already chosen) go in before the text field
        // is left; what they cannot do is dropped.
        self.on_retry();
        if let Ok(mut q) = self.retry.try_borrow_mut() {
            q.clear();
        }
        self.left_after.set(0);
        if let Ok(mut c) = self.client.try_borrow_mut() {
            c.tell(|session| Request::Reset { session });
        }
        let comp = self.composition.try_borrow_mut().ok().and_then(|mut c| c.take());
        if let (Some(comp), Some(ctx)) = (comp, self.context.borrow().clone()) {
            edit::abort(self.tid.get(), &ctx, comp);
        }
    }
}

// --- TSF interfaces --------------------------------------------------------------------

impl ITfTextInputProcessor_Impl for TextService_Impl {
    fn Activate(&self, ptim: windows_core::Ref<'_, ITfThreadMgr>, tid: u32) -> Result<()> {
        self.ActivateEx(ptim, tid, 0)
    }

    fn Deactivate(&self) -> Result<()> {
        guard(Ok(()), || unsafe {
            self.reset();
            if let Some(tm) = self.thread_mgr.borrow_mut().take() {
                if let Ok(km) = tm.cast::<ITfKeystrokeMgr>() {
                    let _ = km.UnadviseKeyEventSink(self.tid.get());
                }
                if self.sink_cookie.get() != TF_INVALID_COOKIE {
                    if let Ok(src) = tm.cast::<ITfSource>() {
                        let _ = src.UnadviseSink(self.sink_cookie.get());
                    }
                    self.sink_cookie.set(TF_INVALID_COOKIE);
                }
            }
            if let Ok(mut c) = self.client.try_borrow_mut() {
                c.close();
            }
            let n = self.notify.replace(HWND::default());
            if !n.0.is_null() {
                let _ = DestroyWindow(n);
            }
            CURRENT.with(|c| *c.borrow_mut() = None);
            *self.context.borrow_mut() = None;
            candwin::destroy();
            Ok(())
        })
    }
}

impl ITfTextInputProcessorEx_Impl for TextService_Impl {
    fn ActivateEx(&self, ptim: windows_core::Ref<'_, ITfThreadMgr>, tid: u32, _flags: u32) -> Result<()> {
        guard(Err(E_FAIL.into()), || unsafe {
            let tm = ptim.ok()?.clone();
            self.tid.set(tid);
            let me: IUnknown = self.me.borrow().clone().ok_or(E_FAIL)?;
            let key_sink: ITfKeyEventSink = me.cast()?;
            tm.cast::<ITfKeystrokeMgr>()?.AdviseKeyEventSink(tid, &key_sink, true)?;
            let tm_sink: ITfThreadMgrEventSink = me.cast()?;
            let cookie = tm.cast::<ITfSource>()?.AdviseSink(&ITfThreadMgrEventSink::IID, &tm_sink)?;
            self.sink_cookie.set(cookie);
            if let Ok(cats) = CoCreateInstance::<_, ITfCategoryMgr>(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER) {
                if let Ok(atom) = cats.RegisterGUID(&DISPLAY_ATTR) {
                    self.attr.set(Some(atom as i32));
                }
            }
            let hwnd = create_notify_window();
            self.notify.set(hwnd);
            // Immersive hosts: our own candidate window (the engine's
            // cannot get above them). If it cannot be made, the engine's
            // is still better than none.
            let draws = candwin::wanted() && candwin::init();
            *self.client.borrow_mut() = Client::new(hwnd.0 as u64, draws);
            *self.thread_mgr.borrow_mut() = Some(tm);
            CURRENT.with(|c| *c.borrow_mut() = me.cast::<ITfTextInputProcessor>().ok());
            Ok(())
        })
    }
}

impl ITfKeyEventSink_Impl for TextService_Impl {
    fn OnSetFocus(&self, foreground: BOOL) -> Result<()> {
        guard(Ok(()), || {
            if let Ok(mut c) = self.client.try_borrow_mut() {
                let on = foreground.as_bool();
                c.tell(|session| Request::Focus { session, on });
            }
            if !foreground.as_bool() {
                self.reset();
            }
            Ok(())
        })
    }

    fn OnTestKeyDown(&self, pic: windows_core::Ref<'_, ITfContext>, wparam: WPARAM, lparam: LPARAM) -> Result<BOOL> {
        guard(Ok(false.into()), || {
            self.up_done.set(None);
            if self.test_pending.get() {
                return Ok(true.into());
            }
            let eaten = self.process(pic.as_ref(), wparam, lparam, false, true);
            self.test_pending.set(eaten);
            Ok(eaten.into())
        })
    }

    fn OnKeyDown(&self, pic: windows_core::Ref<'_, ITfContext>, wparam: WPARAM, lparam: LPARAM) -> Result<BOOL> {
        guard(Ok(false.into()), || {
            self.up_done.set(None);
            if self.test_pending.replace(false) {
                let deferred = self.deferred.borrow_mut().take();
                if let Some((state, adopt, ctx)) = deferred {
                    self.finish(Some(&ctx), state, adopt, false);
                }
                return Ok(true.into());
            }
            Ok(self.process(pic.as_ref(), wparam, lparam, false, false).into())
        })
    }

    fn OnTestKeyUp(&self, pic: windows_core::Ref<'_, ITfContext>, wparam: WPARAM, lparam: LPARAM) -> Result<BOOL> {
        guard(Ok(false.into()), || {
            self.test_pending.set(false);
            // Key-ups are reported, never eaten: the program must see every
            // release, or it will think the key is still held.
            // An edit still waiting (the program skipped OnKeyDown): now.
            let deferred = self.deferred.borrow_mut().take();
            if let Some((state, adopt, ctx)) = deferred {
                self.finish(Some(&ctx), state, adopt, false);
            }
            self.process(pic.as_ref(), wparam, lparam, true, false);
            self.up_done.set(Some(wparam.0 as u16));
            Ok(false.into())
        })
    }

    fn OnKeyUp(&self, pic: windows_core::Ref<'_, ITfContext>, wparam: WPARAM, lparam: LPARAM) -> Result<BOOL> {
        guard(Ok(false.into()), || {
            // Programs that report the release here without asking first
            // (no OnTestKeyUp): the engine must still see it — a lone
            // Shift switches 中 / 英 on its release.
            if self.up_done.take() != Some(wparam.0 as u16) {
                self.test_pending.set(false);
                let deferred = self.deferred.borrow_mut().take();
                if let Some((state, adopt, ctx)) = deferred {
                    self.finish(Some(&ctx), state, adopt, false);
                }
                self.process(pic.as_ref(), wparam, lparam, true, false);
            }
            Ok(false.into())
        })
    }

    fn OnPreservedKey(&self, _pic: windows_core::Ref<'_, ITfContext>, _rguid: *const GUID) -> Result<BOOL> {
        Ok(false.into())
    }
}

impl ITfThreadMgrEventSink_Impl for TextService_Impl {
    fn OnInitDocumentMgr(&self, _pdim: windows_core::Ref<'_, ITfDocumentMgr>) -> Result<()> {
        Ok(())
    }
    fn OnUninitDocumentMgr(&self, _pdim: windows_core::Ref<'_, ITfDocumentMgr>) -> Result<()> {
        Ok(())
    }
    fn OnSetFocus(&self, focus: windows_core::Ref<'_, ITfDocumentMgr>, _prev: windows_core::Ref<'_, ITfDocumentMgr>) -> Result<()> {
        guard(Ok(()), || {
            // Moving to another text field: whatever was being typed is
            // dropped, as in every Windows input method.
            self.reset();
            if let Ok(mut c) = self.client.try_borrow_mut() {
                let on = !focus.is_null();
                c.tell(|session| Request::Focus { session, on });
            }
            Ok(())
        })
    }
    fn OnPushContext(&self, _pic: windows_core::Ref<'_, ITfContext>) -> Result<()> {
        Ok(())
    }
    fn OnPopContext(&self, _pic: windows_core::Ref<'_, ITfContext>) -> Result<()> {
        Ok(())
    }
}

impl ITfCompositionSink_Impl for TextService_Impl {
    fn OnCompositionTerminated(&self, _ec: u32, _pcomposition: windows_core::Ref<'_, ITfComposition>) -> Result<()> {
        guard(Ok(()), || {
            // The program ended our composition. Some do it on their own in
            // the middle of a word (web apps re-rendering the text box when
            // the first text of a new line arrives), which would leave the
            // spelling in the text ("ran后" for 然后): keep what was cut
            // off, and let the next key decide (Orphan). The candidate
            // window stays: such programs end the composition after every
            // key on that line, and the window would never be seen. It goes
            // with the Reset if the next key finds the text abandoned, or
            // when the focus moves.
            self.ended.set(self.ended.get().wrapping_add(1));
            if let Ok(mut c) = self.composition.try_borrow_mut() {
                *c = None;
            }
            let text = self.last_preedit.try_borrow_mut().map(|mut p| std::mem::take(&mut *p)).unwrap_or_default();
            if text.is_empty() {
                candwin::hide();
                if let Ok(mut c) = self.client.try_borrow_mut() {
                    c.tell(|session| Request::Reset { session });
                }
                return Ok(());
            }
            self.note(&format!("program ended the composition ({} chars kept for the next key)", text.len()));
            if let Ok(mut o) = self.orphan.try_borrow_mut() {
                *o = Some(edit::Orphan { text, at: std::time::Instant::now() });
            }
            Ok(())
        })
    }
}

// --- display attribute: how the preedit is underlined -------------------------------------

impl ITfDisplayAttributeProvider_Impl for TextService_Impl {
    fn EnumDisplayAttributeInfo(&self) -> Result<IEnumTfDisplayAttributeInfo> {
        Ok(EnumAttr { done: Cell::new(false) }.into())
    }
    fn GetDisplayAttributeInfo(&self, guid: *const GUID) -> Result<ITfDisplayAttributeInfo> {
        unsafe {
            if !guid.is_null() && *guid == DISPLAY_ATTR {
                Ok(AttrInfo.into())
            } else {
                Err(E_INVALIDARG.into())
            }
        }
    }
}

#[implement(ITfDisplayAttributeInfo)]
struct AttrInfo;

impl ITfDisplayAttributeInfo_Impl for AttrInfo_Impl {
    fn GetGUID(&self) -> Result<GUID> {
        Ok(DISPLAY_ATTR)
    }
    fn GetDescription(&self) -> Result<BSTR> {
        Ok(BSTR::from("地球桌面输入法 输入中"))
    }
    fn GetAttributeInfo(&self, pda: *mut TF_DISPLAYATTRIBUTE) -> Result<()> {
        unsafe {
            if pda.is_null() {
                return Err(E_INVALIDARG.into());
            }
            *pda = TF_DISPLAYATTRIBUTE {
                crText: TF_DA_COLOR::default(),
                crBk: TF_DA_COLOR::default(),
                lsStyle: TF_LS_DOT,
                fBoldLine: false.into(),
                crLine: TF_DA_COLOR::default(),
                bAttr: TF_ATTR_INPUT,
            };
        }
        Ok(())
    }
    fn SetAttributeInfo(&self, _pda: *const TF_DISPLAYATTRIBUTE) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn Reset(&self) -> Result<()> {
        Ok(())
    }
}

#[implement(IEnumTfDisplayAttributeInfo)]
struct EnumAttr {
    done: Cell<bool>,
}

impl IEnumTfDisplayAttributeInfo_Impl for EnumAttr_Impl {
    fn Clone(&self) -> Result<IEnumTfDisplayAttributeInfo> {
        Ok(EnumAttr { done: Cell::new(self.done.get()) }.into())
    }
    fn Next(&self, count: u32, info: *mut Option<ITfDisplayAttributeInfo>, fetched: *mut u32) -> Result<()> {
        unsafe {
            let mut n = 0;
            if count > 0 && !self.done.get() && !info.is_null() {
                *info = Some(AttrInfo.into());
                self.done.set(true);
                n = 1;
            }
            if !fetched.is_null() {
                *fetched = n;
            }
            if n == count {
                Ok(())
            } else {
                Err(S_FALSE.into())
            }
        }
    }
    fn Reset(&self) -> Result<()> {
        self.done.set(false);
        Ok(())
    }
    fn Skip(&self, count: u32) -> Result<()> {
        if count > 0 {
            self.done.set(true);
        }
        Ok(())
    }
}
