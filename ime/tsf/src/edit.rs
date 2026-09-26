//! Changing the document: everything goes through one edit session type,
//! `Apply`, which commits text, starts / updates / ends the composition and
//! reports where the caret ended up (for the candidate window).

use ime_proto::State;
use std::cell::RefCell;
use std::mem::ManuallyDrop;
use std::rc::Rc;
use windows::core::{implement, Interface, Result, BOOL};
use windows::Win32::Foundation::{POINT, RECT};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::HiDpi::LogicalToPhysicalPointForPerMonitorDPI;
use windows::Win32::UI::TextServices::*;

pub type Shared = Rc<RefCell<Option<ITfComposition>>>;

/// Runs inside the session: gets the caret rectangle (physical screen
/// pixels) whenever the program reports one.
pub type OnCaret = Rc<dyn Fn(RECT)>;

#[implement(ITfEditSession)]
pub struct Apply {
    context: ITfContext,
    sink: ITfCompositionSink,
    composition: Shared,
    state: State,
    attr: Option<i32>,
    on_caret: OnCaret,
    /// Text the program cut off from our composition (see `Orphan`): the
    /// composition starts again over it.
    adopt: Adopt,
    ended: Ended,
    /// What went wrong inside the session, for the engine's log.
    err: Rc<RefCell<Option<String>>>,
}

/// What was left of our composition when the program ended it (web apps
/// do when the first text on a new line re-renders the box): its text is
/// still there, underlined no more. If it is still right before the caret
/// when the next key comes, the composition takes it over again as if
/// nothing happened; otherwise the engine forgets it.
pub struct Orphan {
    /// The preedit we last put there.
    pub text: Vec<u16>,
    pub at: std::time::Instant,
}

/// How the next edit starts its composition when there is none.
#[derive(Clone)]
pub enum Adopt {
    /// At the selection, as usual.
    No,
    /// Over this range (the orphan, found just now).
    Range(ITfRange),
    /// Over this text if it is right before the caret (looked for inside
    /// the edit, for programs that refuse to be asked beforehand).
    Find(Vec<u16>),
}

/// Counts compositions the program ended, so an edit session during which
/// ours was ended does not keep using it.
pub type Ended = Rc<std::cell::Cell<u32>>;

unsafe fn range_text(ec: u32, range: &ITfRange, max: usize) -> Result<Vec<u16>> {
    let mut buf = vec![0u16; max + 1];
    let mut n = 0u32;
    range.GetText(ec, 0, &mut buf, &mut n)?;
    buf.truncate(n as usize);
    Ok(buf)
}

/// `text` right before an empty selection: its range. Positions are taken
/// from the caret, not from the old composition's range, which programs
/// shift about when they rebuild the text box.
unsafe fn before_caret(ctx: &ITfContext, ec: u32, text: &[u16]) -> Option<ITfRange> {
    let sel = selection_range(ctx, ec).ok()?;
    if !sel.IsEmpty(ec).ok()?.as_bool() {
        return None;
    }
    // Some pages put a space after text the moment it is typed (while it
    // should still be composing): allow a few spaces between it and the
    // caret, and take them into the composition (they then go).
    const SLACK: usize = 3;
    let r = sel.Clone().ok()?;
    let want = (text.len() + SLACK) as i32;
    let mut moved = 0i32;
    r.ShiftStart(ec, -want, &mut moved, std::ptr::null()).ok()?;
    let got = range_text(ec, &r, want as usize + 2).ok()?;
    let is_space = |u: &u16| matches!(*u, 0x20 | 0xA0 | 0x3000);
    let end = got.len() - got.iter().rev().take_while(|u| is_space(u)).count();
    if end < text.len() || got.len() - end > SLACK || got[end - text.len()..end] != *text {
        return None;
    }
    // Start the range at the text itself.
    let skip = (end - text.len()) as i32;
    let mut m = 0i32;
    r.ShiftStart(ec, skip, &mut m, std::ptr::null()).ok()?;
    Some(r)
}

pub enum Found {
    Here(ITfRange),
    Gone,
    /// The program would not let us look now.
    Refused,
}

#[implement(ITfEditSession)]
struct Check {
    context: ITfContext,
    text: Vec<u16>,
    out: Rc<RefCell<Option<ITfRange>>>,
}

impl ITfEditSession_Impl for Check_Impl {
    fn DoEditSession(&self, ec: u32) -> Result<()> {
        crate::guard(Ok(()), || unsafe {
            *self.out.borrow_mut() = before_caret(&self.context, ec, &self.text);
            Ok(())
        })
    }
}

/// Is the orphaned text still right before the caret?
pub fn find_orphan(tid: u32, context: &ITfContext, text: &[u16]) -> Found {
    let out = Rc::new(RefCell::new(None));
    let es: ITfEditSession = Check { context: context.clone(), text: text.to_vec(), out: out.clone() }.into();
    let r = unsafe { context.RequestEditSession(tid, &es, TF_ES_SYNC | TF_ES_READ) };
    if !matches!(r, Ok(hr) if hr.is_ok()) {
        return Found::Refused;
    }
    let found = out.borrow_mut().take();
    match found {
        Some(r) => Found::Here(r),
        None => Found::Gone,
    }
}

/// Can text go into this context at all? Programs point the keyboard at
/// contexts that take none while nothing editable has the focus: Chrome at
/// its "empty" document (read-only, keyboard disabled) whenever the page's
/// focus is not in a text field, and its password fields are disabled for
/// input methods. Every edit there fails (TS_E_READONLY), so keys there are
/// the program's (a web app's shortcuts), never typing.
pub fn writable(ctx: &ITfContext) -> bool {
    unsafe {
        if ctx.GetStatus().map(|st| st.dwDynamicFlags & TS_SD_READONLY != 0).unwrap_or(false) {
            return false;
        }
        let Ok(cm) = ctx.cast::<ITfCompartmentMgr>() else { return true };
        for guid in [&GUID_COMPARTMENT_KEYBOARD_DISABLED, &GUID_COMPARTMENT_EMPTYCONTEXT] {
            if let Ok(v) = cm.GetCompartment(guid).and_then(|c| c.GetValue()) {
                if v.vt() == windows::Win32::System::Variant::VT_I4 && v.Anonymous.Anonymous.Anonymous.lVal != 0 {
                    return false;
                }
            }
        }
        true
    }
}

unsafe fn selection_range(ctx: &ITfContext, ec: u32) -> Result<ITfRange> {
    ctx.cast::<ITfInsertAtSelection>()?.InsertTextAtSelection(ec, TF_IAS_QUERYONLY, &[])
}

unsafe fn set_caret(ctx: &ITfContext, ec: u32, range: &ITfRange) -> Result<()> {
    let sel = TF_SELECTION {
        range: ManuallyDrop::new(Some(range.clone())),
        style: TF_SELECTIONSTYLE { ase: TF_AE_NONE, fInterimChar: false.into() },
    };
    let r = ctx.SetSelection(ec, &[sel.clone()]);
    let _ = ManuallyDrop::into_inner(sel.range);
    r
}

unsafe fn text_ext(ctx: &ITfContext, ec: u32, range: &ITfRange) -> Option<RECT> {
    let view = ctx.GetActiveView().ok()?;
    let mut rc = RECT::default();
    let mut clipped = BOOL::default();
    view.GetTextExt(ec, range, &mut rc, &mut clipped).ok()?;
    if rc.left == 0 && rc.top == 0 && rc.right == 0 && rc.bottom == 0 {
        return None;
    }
    // Programs that are not per-monitor DPI aware answer in their own
    // (virtualised) coordinates.
    if let Ok(hwnd) = view.GetWnd() {
        let mut a = POINT { x: rc.left, y: rc.top };
        let mut b = POINT { x: rc.right, y: rc.bottom };
        if LogicalToPhysicalPointForPerMonitorDPI(Some(hwnd), &mut a).as_bool()
            && LogicalToPhysicalPointForPerMonitorDPI(Some(hwnd), &mut b).as_bool()
        {
            rc = RECT { left: a.x, top: a.y, right: b.x, bottom: b.y };
        }
    }
    Some(rc)
}

impl ITfEditSession_Impl for Apply_Impl {
    fn DoEditSession(&self, ec: u32) -> Result<()> {
        crate::guard(Ok(()), || unsafe {
            let r = self.run(ec);
            if let Err(e) = &r {
                if let Ok(mut slot) = self.err.try_borrow_mut() {
                    *slot = Some(format!("edit failed: {e}"));
                }
            }
            r
        })
    }
}

impl Apply {
    unsafe fn run(&self, ec: u32) -> Result<()> {
        // Taken out and put back so no borrow is held while TSF calls back
        // into us (OnCompositionTerminated).
        let mut comp = self.composition.try_borrow_mut().ok().and_then(|mut c| c.take());
        let ended = self.ended.get();
        let r = self.edit(ec, &mut comp);
        // The program ended the composition while we were editing (some do
        // it right as the first text of a line arrives): it is dead, never
        // put it back (every later edit would fail on it).
        if self.ended.get() != ended {
            comp = None;
        }
        if let Ok(mut slot) = self.composition.try_borrow_mut() {
            *slot = comp;
        }
        r
    }

    unsafe fn edit(&self, ec: u32, comp: &mut Option<ITfComposition>) -> Result<()> {
        let ctx = &self.context;

        if comp.is_none() {
            let range = match &self.adopt {
                Adopt::No => None,
                Adopt::Range(r) => Some(r.clone()),
                Adopt::Find(text) => {
                    let r = before_caret(ctx, ec, text);
                    if let Ok(mut slot) = self.err.try_borrow_mut() {
                        *slot = Some(format!("cut-off composition ({} chars) {}", text.len(), if r.is_some() { "taken over in the edit" } else { "no longer at the caret" }));
                    }
                    r
                }
            };
            if let Some(range) = range {
                // Whatever comes now (a longer spelling, the word, or
                // nothing after Esc) replaces the text that was cut off.
                *comp = Some(ctx.cast::<ITfContextComposition>()?.StartComposition(ec, &range, &self.sink)?);
            }
        }

        if let Some(text) = &self.state.commit {
            let wide: Vec<u16> = text.encode_utf16().collect();
            match comp.take() {
                Some(c) => {
                    let range = c.GetRange()?;
                    range.SetText(ec, 0, &wide)?;
                    if let Ok(p) = ctx.GetProperty(&GUID_PROP_ATTRIBUTE) {
                        let _ = p.Clear(ec, &range);
                    }
                    range.Collapse(ec, TF_ANCHOR_END)?;
                    set_caret(ctx, ec, &range)?;
                    c.EndComposition(ec)?;
                }
                None => {
                    let range = selection_range(ctx, ec)?;
                    // A closing mark right where one already is (the
                    // partner we put in with its opening one): step over it.
                    if self.state.delete_before == 0 && self.state.caret_back == 0 {
                        if let Some(next) = closer_ahead(ec, &range, text) {
                            set_caret(ctx, ec, &next)?;
                            return self.finish_preedit(ec, comp);
                        }
                    }
                    if self.state.delete_before > 0 {
                        // Replace the word we committed just before.
                        let mut moved = 0i32;
                        let _ = range.ShiftStart(ec, -(self.state.delete_before as i32), &mut moved, std::ptr::null());
                    }
                    range.SetText(ec, 0, &wide)?;
                    range.Collapse(ec, TF_ANCHOR_END)?;
                    set_caret(ctx, ec, &range)?;
                }
            }
        }

        self.finish_preedit(ec, comp)
    }

    /// The preedit part of the edit (after any commit).
    unsafe fn finish_preedit(&self, ec: u32, comp: &mut Option<ITfComposition>) -> Result<()> {
        let ctx = &self.context;
        match &self.state.preedit {
            Some(p) if !p.text.is_empty() => {
                if comp.is_none() {
                    let range = selection_range(ctx, ec)?;
                    let c = ctx.cast::<ITfContextComposition>()?.StartComposition(ec, &range, &self.sink)?;
                    *comp = Some(c);
                }
                let c = comp.as_ref().unwrap();
                let range = c.GetRange()?;
                let wide: Vec<u16> = p.text.encode_utf16().collect();
                range.SetText(ec, TF_ST_CORRECTION, &wide)?;
                if let (Some(atom), Ok(prop)) = (self.attr, ctx.GetProperty(&GUID_PROP_ATTRIBUTE)) {
                    let v = VARIANT::from(atom);
                    let _ = prop.SetValue(ec, &range, &v);
                }
                // Caret inside the preedit where Rime says it is.
                let caret = range.Clone()?;
                caret.Collapse(ec, TF_ANCHOR_START)?;
                let mut moved = 0i32;
                let n = (p.cursor as i32).min(wide.len() as i32);
                caret.ShiftStart(ec, n, &mut moved, std::ptr::null())?;
                caret.ShiftEnd(ec, n, &mut moved, std::ptr::null())?;
                let _ = set_caret(ctx, ec, &caret);
                // The start of the composition, not the caret inside it: the
                // candidate window should stay put while the user types.
                let start = range.Clone()?;
                start.Collapse(ec, TF_ANCHOR_START)?;
                if let Some(rc) = text_ext(ctx, ec, &start).or_else(|| text_ext(ctx, ec, &range)) {
                    (self.on_caret)(rc);
                }
            }
            _ => {
                if let Some(c) = comp.take() {
                    if self.state.commit.is_none() {
                        if let Ok(range) = c.GetRange() {
                            let _ = range.SetText(ec, 0, &[]);
                        }
                    }
                    let _ = c.EndComposition(ec);
                }
            }
        }
        Ok(())
    }
}

/// `text` is one closing mark and the document has that same mark right
/// after the selection: the caret position past it.
unsafe fn closer_ahead(ec: u32, sel: &ITfRange, text: &str) -> Option<ITfRange> {
    let mut chars = text.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else { return None };
    if !ime_proto::is_closer(c) || !sel.IsEmpty(ec).ok()?.as_bool() {
        return None;
    }
    let next = sel.Clone().ok()?;
    let mut moved = 0i32;
    next.ShiftEnd(ec, c.len_utf16() as i32, &mut moved, std::ptr::null()).ok()?;
    let want: Vec<u16> = text.encode_utf16().collect();
    if range_text(ec, &next, want.len() + 1).ok()? != want {
        return None;
    }
    next.Collapse(ec, TF_ANCHOR_END).ok()?;
    Some(next)
}

/// Apply `state` to `context`. Ok: done or queued by the program (with a
/// note for the log when something inside went wrong); Err: the program
/// refused every way of asking, nothing happened.
///
/// Apply `state` to `context`. Inside key handling we ask for the edit
/// synchronously (the text must be there before the program sees the next
/// key); programs that refuse synchronous edits (Chromium-based apps
/// sometimes do) get it asynchronously instead. Returns a note for the
/// engine's log when something failed.
#[allow(clippy::too_many_arguments)]
pub fn apply(
    tid: u32,
    context: &ITfContext,
    sink: &ITfCompositionSink,
    composition: &Shared,
    state: &State,
    attr: Option<i32>,
    sync: bool,
    on_caret: OnCaret,
    adopt: Adopt,
    ended: &Ended,
) -> std::result::Result<Option<String>, String> {
    let err: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let make = || -> ITfEditSession {
        Apply {
            context: context.clone(),
            sink: sink.clone(),
            composition: composition.clone(),
            state: state.clone(),
            attr,
            on_caret: on_caret.clone(),
            adopt: adopt.clone(),
            ended: ended.clone(),
            err: err.clone(),
        }
        .into()
    };
    // Queued for when the program lets go of its document. Not
    // TF_ES_ASYNCDONTCARE: Chromium answers that with TS_E_SYNCHRONOUS
    // while it holds the document itself (during its own key handling), and
    // the edit is lost — the program's idea of the composition then drifts
    // from ours until it ends it (the text left behind after a new line).
    let queue = TF_ES_ASYNC | TF_ES_READWRITE;
    let queued = |hr: windows::core::HRESULT| hr.is_ok() || hr == TF_S_ASYNC;
    unsafe {
        if sync {
            match context.RequestEditSession(tid, &make(), TF_ES_SYNC | TF_ES_READWRITE) {
                Ok(hr) if hr.is_ok() => return Ok(err.borrow_mut().take()),
                // Refused now (Chromium, often): later, in order.
                Ok(_) | Err(_) => {}
            }
        } else if let Ok(hr) = context.RequestEditSession(tid, &make(), TF_ES_ASYNCDONTCARE | TF_ES_READWRITE) {
            // Outside key handling (a retry, a click): now if the program
            // lets us, which it then does.
            if queued(hr) {
                return Ok(err.borrow_mut().take());
            }
        }
        match context.RequestEditSession(tid, &make(), queue) {
            Ok(hr) if queued(hr) => Ok(None),
            // Nothing was done: the caller keeps the edit and tries again.
            Ok(hr) => Err(format!("{hr:?}")),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// End a composition without committing (focus moved away).
#[implement(ITfEditSession)]
struct Abort {
    composition: ITfComposition,
}

impl ITfEditSession_Impl for Abort_Impl {
    fn DoEditSession(&self, ec: u32) -> Result<()> {
        crate::guard(Ok(()), || unsafe {
            if let Ok(r) = self.composition.GetRange() {
                let _ = r.SetText(ec, 0, &[]);
            }
            self.composition.EndComposition(ec)
        })
    }
}

pub fn abort(tid: u32, context: &ITfContext, composition: ITfComposition) {
    let es: ITfEditSession = Abort { composition }.into();
    unsafe {
        let _ = context.RequestEditSession(tid, &es, TF_ES_ASYNC | TF_ES_READWRITE);
    }
}
