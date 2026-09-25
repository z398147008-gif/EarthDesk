//! The engine's candidate window, following the text caret of whatever
//! program is typing (drawing is in the shared `ime-candui` crate).
//!
//! Clicking a candidate picks it (the engine then notifies the DLL, which
//! commits); the arrows flip pages.
//!
//! Immersive hosts (the Start menu's search...) put their windows in a layer
//! above every ordinary topmost window, so there the DLL draws the same
//! window inside the host instead (`Hello::draws`), and this one stays
//! hidden.

use crate::session::{Caret, Engine, View};
use ime_candui::{Canvas, Hit};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use windows::core::w;
use windows::Win32::Foundation::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

const WM_CAND_UPDATE: u32 = WM_APP + 1;
const WM_CAND_SHOW: u32 = WM_APP + 2;
const CARET_TIMER: usize = 1;
const FLASH_TIMER: usize = 2;

#[derive(Default)]
struct Shared {
    view: Option<View>,
    caret: Option<Caret>,
    visible: bool,
    /// A new view arrived; its caret usually follows a millisecond later
    /// (the DLL measures it after applying the preedit). Wait briefly for
    /// it rather than drawing at the old place and jumping.
    awaiting_caret: bool,
}

static SHARED: Mutex<Shared> = Mutex::new(Shared { view: None, caret: None, visible: false, awaiting_caret: false });
static HWND_: AtomicIsize = AtomicIsize::new(0);
static ENGINE: OnceLock<Arc<Engine>> = OnceLock::new();

fn post() {
    let h = HWND_.load(Ordering::SeqCst);
    if h != 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND(h as *mut _)), WM_CAND_UPDATE, WPARAM(0), LPARAM(0));
        }
    }
}

fn post_msg(msg: u32) {
    let h = HWND_.load(Ordering::SeqCst);
    if h != 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND(h as *mut _)), msg, WPARAM(0), LPARAM(0));
        }
    }
}

pub fn show(view: View, caret: Option<Caret>) {
    let was_visible;
    if let Ok(mut s) = SHARED.lock() {
        was_visible = s.visible;
        s.view = Some(view);
        if caret.is_some() {
            s.caret = caret;
        }
        s.visible = true;
        s.awaiting_caret = !was_visible;
    } else {
        return;
    }
    // Already on screen: redraw now. Newly appearing: wait for the caret.
    post_msg(if was_visible { WM_CAND_UPDATE } else { WM_CAND_SHOW });
}

pub fn move_to(caret: Caret) {
    if let Ok(mut s) = SHARED.lock() {
        let waiting = s.awaiting_caret;
        s.awaiting_caret = false;
        if s.caret == Some(caret) && !waiting {
            return;
        }
        s.caret = Some(caret);
    }
    post();
}

pub fn hide() {
    if let Ok(mut s) = SHARED.lock() {
        if !s.visible {
            return;
        }
        s.visible = false;
    }
    post();
}

/// The session whose view is on screen (clicks go to it).
static SHOWN_SESSION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn log(m: &str) {
    crate::log(m);
}

/// Draw what SHARED says (or hide).
unsafe fn redraw(c: &mut Canvas, hwnd: HWND) {
    let (view, caret, visible) = {
        let Ok(s) = SHARED.lock() else { return };
        (s.view.clone(), s.caret, s.visible)
    };
    let (Some(view), true) = (view, visible) else {
        c.hide(hwnd);
        return;
    };
    SHOWN_SESSION.store(view.session, Ordering::SeqCst);
    let caret = caret.map(|c| ime_candui::Caret { x: c.x, y: c.y, h: c.h });
    c.draw(hwnd, &view.cands(), caret, &log);
}

fn click(c: &Canvas, x: f32, y: f32) {
    let Some(engine) = ENGINE.get() else { return };
    let session = SHOWN_SESSION.load(Ordering::SeqCst);
    let (index, page) = match c.hit(x, y) {
        Some(Hit::Cand(i)) => (Some(i), None),
        Some(Hit::Prev) => (None, Some(true)),
        Some(Hit::Next) => (None, Some(false)),
        None => return,
    };
    let e = engine.clone();
    std::thread::spawn(move || e.pick(session, index, page));
}

thread_local! {
    static CANVAS: std::cell::RefCell<Option<Canvas>> = const { std::cell::RefCell::new(None) };
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_CAND_SHOW => {
            let _ = SetTimer(Some(hwnd), CARET_TIMER, 40, None);
            if SHARED.lock().map(|s| s.view.as_ref().map(|v| v.flash).unwrap_or(false)).unwrap_or(false) {
                let _ = SetTimer(Some(hwnd), FLASH_TIMER, 1200, None);
            }
            LRESULT(0)
        }
        WM_TIMER if wp.0 == CARET_TIMER => {
            let _ = KillTimer(Some(hwnd), CARET_TIMER);
            if let Ok(mut s) = SHARED.lock() {
                s.awaiting_caret = false;
            }
            CANVAS.with(|c| {
                if let Some(c) = c.borrow_mut().as_mut() {
                    redraw(c, hwnd);
                }
            });
            LRESULT(0)
        }
        WM_TIMER if wp.0 == FLASH_TIMER => {
            let _ = KillTimer(Some(hwnd), FLASH_TIMER);
            if let Ok(mut s) = SHARED.lock() {
                if s.view.as_ref().map(|v| v.flash).unwrap_or(false) {
                    s.visible = false;
                }
            }
            CANVAS.with(|c| {
                if let Some(c) = c.borrow_mut().as_mut() {
                    redraw(c, hwnd);
                }
            });
            LRESULT(0)
        }
        WM_CAND_UPDATE => {
            let _ = KillTimer(Some(hwnd), CARET_TIMER);
            if SHARED.lock().map(|s| s.visible && s.view.as_ref().map(|v| v.flash).unwrap_or(false)).unwrap_or(false) {
                let _ = SetTimer(Some(hwnd), FLASH_TIMER, 1200, None);
            }
            CANVAS.with(|c| {
                if let Some(c) = c.borrow_mut().as_mut() {
                    redraw(c, hwnd);
                }
            });
            LRESULT(0)
        }
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        // taskkill (installer upgrade) and logoff: flush the user dictionary.
        WM_CLOSE => {
            if let Some(e) = ENGINE.get() {
                e.shutdown();
            }
            LRESULT(0)
        }
        WM_QUERYENDSESSION => LRESULT(1),
        WM_ENDSESSION if wp.0 != 0 => {
            if let Some(e) = ENGINE.get() {
                e.shutdown();
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let x = (lp.0 & 0xffff) as i16 as f32;
            let y = ((lp.0 >> 16) & 0xffff) as i16 as f32;
            CANVAS.with(|c| {
                if let Some(c) = c.borrow().as_ref() {
                    click(c, x, y);
                }
            });
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            // Wheel over the window flips pages.
            let delta = ((wp.0 >> 16) & 0xffff) as i16;
            let session = SHOWN_SESSION.load(Ordering::SeqCst);
            if let Some(e) = ENGINE.get() {
                let e = e.clone();
                std::thread::spawn(move || e.pick(session, None, Some(delta > 0)));
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

pub fn start(engine: Arc<Engine>) {
    let _ = ENGINE.set(engine);
    std::thread::Builder::new()
        .name("candidates".into())
        .spawn(|| unsafe {
            let inst = GetModuleHandleW(None).unwrap_or_default();
            let Ok(hwnd) = ime_candui::create_window(inst.into(), w!("EarthDeskIMECandidates"), Some(wndproc)) else {
                crate::log(&format!("candidate window: CreateWindowEx failed: {:?}", windows::core::Error::from_win32()));
                return;
            };
            match Canvas::new() {
                Ok(c) => CANVAS.with(|s| *s.borrow_mut() = Some(c)),
                Err(e) => crate::log(&format!("candidate window: Direct2D unavailable: {e}")),
            }
            HWND_.store(hwnd.0 as isize, Ordering::SeqCst);
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        })
        .ok();
}
