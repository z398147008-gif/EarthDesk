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
    /// What went wrong inside the session, for the engine's log.
    err: Rc<RefCell<Option<String>>>,
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
        let r = self.edit(ec, &mut comp);
        if let Ok(mut slot) = self.composition.try_borrow_mut() {
            *slot = comp;
        }
        r
    }

    unsafe fn edit(&self, ec: u32, comp: &mut Option<ITfComposition>) -> Result<()> {
        let ctx = &self.context;

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
) -> Option<String> {
    let err: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let make = || -> ITfEditSession {
        Apply {
            context: context.clone(),
            sink: sink.clone(),
            composition: composition.clone(),
            state: state.clone(),
            attr,
            on_caret: on_caret.clone(),
            err: err.clone(),
        }
        .into()
    };
    let async_flags = TF_ES_ASYNCDONTCARE | TF_ES_READWRITE;
    unsafe {
        if sync {
            let r = context.RequestEditSession(tid, &make(), TF_ES_SYNC | TF_ES_READWRITE);
            match r {
                Ok(hr) if hr.is_ok() => return err.borrow_mut().take(),
                Ok(hr) => {
                    let r2 = context.RequestEditSession(tid, &make(), async_flags);
                    return Some(format!("sync edit refused ({hr:?}), async -> {:?}", r2.map(|h| h.0)));
                }
                Err(e) => {
                    let r2 = context.RequestEditSession(tid, &make(), async_flags);
                    return Some(format!("sync edit request failed ({e}), async -> {:?}", r2.map(|h| h.0)));
                }
            }
        }
        match context.RequestEditSession(tid, &make(), async_flags) {
            Ok(hr) if hr.is_ok() || hr == TF_S_ASYNC => None,
            Ok(hr) => Some(format!("async edit refused ({hr:?})")),
            Err(e) => Some(format!("async edit request failed ({e})")),
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
        let _ = context.RequestEditSession(tid, &es, TF_ES_ASYNCDONTCARE | TF_ES_READWRITE);
    }
}
