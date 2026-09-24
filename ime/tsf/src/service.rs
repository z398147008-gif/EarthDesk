//! The text service object TSF creates once per UI thread of each program.
//!
//! Key flow (the Weasel pattern, which copes with programs that call the
//! sink oddly):
//! - OnTestKeyDown sends the key to the engine and applies the answer;
//!   if it was eaten, remember that, so the OnKeyDown that follows only
//!   confirms it.
//! - OnKeyDown without a preceding test does both.
//! - Key-ups go to the engine too (Shift alone toggles 中/英 on release)
//!   but are never eaten.

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
            client: Rc::new(RefCell::new(Client::new(0))),
            composition: Rc::new(RefCell::new(None)),
            context: RefCell::new(None),
            me: RefCell::new(None),
        });
        let unknown: IUnknown = object.to_interface();
        *object.me.borrow_mut() = Some(unknown.clone());
        unknown
    }
}

// --- the notification window ---------------------------------------------------------

static CLASS_READY: AtomicBool = AtomicBool::new(false);

unsafe extern "system" fn notify_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
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

// --- applying engine answers ---------------------------------------------------------

impl TextService {
    fn sink(&self) -> Option<ITfCompositionSink> {
        self.me.borrow().as_ref()?.cast().ok()
    }

    fn apply(&self, context: &ITfContext, state: &State, sync: bool) {
        let Some(sink) = self.sink() else { return };
        let client = self.client.clone();
        let on_caret: edit::OnCaret = Rc::new(move |rc: RECT| {
            if let Ok(mut c) = client.try_borrow_mut() {
                c.tell(|session| Request::Caret { session, x: rc.left, y: rc.top, h: (rc.bottom - rc.top).max(1) });
            }
        });
        edit::apply(self.tid.get(), context, &sink, &self.composition, state, self.attr.get(), sync, on_caret);
        if state.preedit.is_some() {
            *self.context.borrow_mut() = Some(context.clone());
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
        if st.commit.is_some() || st.preedit.is_some() {
            self.apply(&ctx, &st, false);
        }
    }

    /// Send a key; apply the answer. Returns whether it was eaten.
    fn process(&self, context: Option<&ITfContext>, wp: WPARAM, lp: LPARAM, up: bool) -> bool {
        let Some(key) = keys::convert(wp.0 as u16, lp.0, up) else { return false };
        let state = {
            let Ok(mut c) = self.client.try_borrow_mut() else { return false };
            match c.key(key.code, key.mask) {
                Some(s) => s,
                None => return false,
            }
        };
        let composing = self.composition.try_borrow().map(|c| c.is_some()).unwrap_or(false);
        if let Some(ctx) = context {
            if state.commit.is_some() || state.preedit.is_some() || composing {
                self.apply(ctx, &state, true);
            }
        }
        state.eaten && !up
    }

    fn reset(&self) {
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
            *self.client.borrow_mut() = Client::new(hwnd.0 as u64);
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
            if self.test_pending.get() {
                return Ok(true.into());
            }
            let eaten = self.process(pic.as_ref(), wparam, lparam, false);
            self.test_pending.set(eaten);
            Ok(eaten.into())
        })
    }

    fn OnKeyDown(&self, pic: windows_core::Ref<'_, ITfContext>, wparam: WPARAM, lparam: LPARAM) -> Result<BOOL> {
        guard(Ok(false.into()), || {
            if self.test_pending.replace(false) {
                return Ok(true.into());
            }
            Ok(self.process(pic.as_ref(), wparam, lparam, false).into())
        })
    }

    fn OnTestKeyUp(&self, pic: windows_core::Ref<'_, ITfContext>, wparam: WPARAM, lparam: LPARAM) -> Result<BOOL> {
        guard(Ok(false.into()), || {
            self.test_pending.set(false);
            // Key-ups are reported, never eaten: the program must see every
            // release, or it will think the key is still held.
            self.process(pic.as_ref(), wparam, lparam, true);
            Ok(false.into())
        })
    }

    fn OnKeyUp(&self, _pic: windows_core::Ref<'_, ITfContext>, _wparam: WPARAM, _lparam: LPARAM) -> Result<BOOL> {
        Ok(false.into())
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
            // The program ended our composition (clicked elsewhere, for
            // example): forget it and tell the engine.
            if let Ok(mut c) = self.composition.try_borrow_mut() {
                *c = None;
            }
            if let Ok(mut c) = self.client.try_borrow_mut() {
                c.tell(|session| Request::Reset { session });
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
