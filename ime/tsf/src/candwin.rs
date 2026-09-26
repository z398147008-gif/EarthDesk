//! The candidate window drawn inside the host program — only in immersive
//! hosts: the Start menu's search box, other shell surfaces and Store apps.
//!
//! Windows puts those hosts in a window layer (a "band") above every
//! ordinary window, topmost ones included, so the engine's own candidate
//! window ends up hidden behind them. A window created by the host's own
//! process lands in the host's band, which is why Windows' own input
//! methods (and Weasel) draw their candidates in-process. Everywhere else
//! the engine keeps drawing (so fixes to it apply without reopening
//! programs).
//!
//! Everything here runs on the text service's UI thread; the state is
//! per thread, like the text service itself.

use crate::guard;
use ime_candui::{Canvas, Caret, Hit};
use ime_proto::{CandUi, Cands};
use std::cell::RefCell;
use std::sync::OnceLock;
use windows::core::w;
use windows::Win32::Foundation::*;
use windows::Win32::Security::{GetTokenInformation, TokenIsAppContainer, TOKEN_QUERY};
use windows::Win32::System::Threading::{GetCurrentProcess, IsImmersiveProcess, OpenProcessToken};
use windows::Win32::UI::HiDpi::{SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::WindowsAndMessaging::*;

/// Wait this long for the caret before showing the window at all (the
/// program may answer the edit session a moment later).
const CARET_TIMER: usize = 1;
const CARET_WAIT_MS: u32 = 40;
/// A notice (mode switched) hides by itself.
const FLASH_TIMER: usize = 2;
const FLASH_MS: u32 = 1200;

/// Shell hosts that are immersive even where the checks below disagree.
const SHELL_HOSTS: &[&str] = &[
    "searchhost.exe",
    "searchapp.exe",
    "searchui.exe",
    "startmenuexperiencehost.exe",
    "shellexperiencehost.exe",
    "textinputhost.exe",
];

/// Whether this process needs the in-process window.
pub fn wanted() -> bool {
    static WANTED: OnceLock<bool> = OnceLock::new();
    *WANTED.get_or_init(|| unsafe {
        let exe = crate::client::exe_name();
        if SHELL_HOSTS.contains(&exe.as_str()) {
            return true;
        }
        if IsImmersiveProcess(GetCurrentProcess()).is_ok() {
            return true;
        }
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut ac = 0u32;
        let mut len = 0u32;
        let is_ac = GetTokenInformation(token, TokenIsAppContainer, Some(&mut ac as *mut _ as *mut _), 4, &mut len).is_ok() && ac != 0;
        let _ = CloseHandle(token);
        is_ac
    })
}

struct Win {
    hwnd: HWND,
    canvas: Canvas,
    view: Option<Cands>,
    caret: Option<Caret>,
    /// The caret was reported since the view last changed.
    caret_fresh: bool,
    /// Waiting for CARET_TIMER before the first draw.
    waiting: bool,
    notes: Vec<String>,
}

thread_local! {
    static WIN: RefCell<Option<Win>> = const { RefCell::new(None) };
}

/// Run `f` as a per-monitor DPI aware thread: the window then works in
/// physical pixels, like the caret rectangles and the engine's window,
/// whatever the host program itself is.
fn per_monitor<T>(f: impl FnOnce() -> T) -> T {
    unsafe {
        let old: DPI_AWARENESS_CONTEXT = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let r = f();
        if !old.is_invalid() {
            SetThreadDpiAwarenessContext(old);
        }
        r
    }
}

/// Make this thread's window. False = cannot (the engine draws instead).
pub fn init() -> bool {
    if WIN.with(|w| w.borrow().is_some()) {
        return true;
    }
    per_monitor(|| unsafe {
        let Ok(hwnd) = ime_candui::create_window(crate::module(), w!("EarthDeskTSFCandidates"), Some(wndproc)) else { return false };
        match Canvas::new() {
            Ok(canvas) => {
                WIN.with(|w| {
                    *w.borrow_mut() = Some(Win { hwnd, canvas, view: None, caret: None, caret_fresh: false, waiting: false, notes: Vec::new() })
                });
                true
            }
            Err(_) => {
                let _ = DestroyWindow(hwnd);
                false
            }
        }
    })
}

pub fn destroy() {
    let win = WIN.with(|w| w.try_borrow_mut().ok().and_then(|mut w| w.take()));
    if let Some(win) = win {
        unsafe {
            let _ = DestroyWindow(win.hwnd);
        }
    }
}

/// Draw (or hide) according to the stored view and caret.
fn draw(win: &mut Win) {
    per_monitor(|| unsafe {
        let hwnd = win.hwnd;
        let Some(view) = win.view.as_ref() else {
            win.canvas.hide(hwnd);
            return;
        };
        let notes = RefCell::new(Vec::new());
        win.canvas.draw(hwnd, view, win.caret, &|m: &str| notes.borrow_mut().push(m.to_string()));
        win.notes.extend(notes.into_inner());
    })
}

fn with_win(f: impl FnOnce(&mut Win)) {
    WIN.with(|w| {
        if let Ok(mut w) = w.try_borrow_mut() {
            if let Some(win) = w.as_mut() {
                f(win);
            }
        }
    });
}

/// What the engine said the window should do.
pub fn update(ui: &CandUi) {
    with_win(|win| unsafe {
        match ui {
            CandUi::Hide => {
                win.view = None;
                win.waiting = false;
                let _ = KillTimer(Some(win.hwnd), CARET_TIMER);
                let _ = KillTimer(Some(win.hwnd), FLASH_TIMER);
                draw(win);
            }
            CandUi::Show { view } => {
                let flash = view.flash;
                win.view = Some(view.clone());
                let _ = KillTimer(Some(win.hwnd), FLASH_TIMER);
                if flash {
                    let _ = SetTimer(Some(win.hwnd), FLASH_TIMER, FLASH_MS, None);
                }
                if !win.canvas.shown() && !win.caret_fresh {
                    // Newly appearing, and the program has not told us
                    // where the text is yet: give it a moment.
                    win.waiting = true;
                    let _ = SetTimer(Some(win.hwnd), CARET_TIMER, CARET_WAIT_MS, None);
                } else if !win.waiting {
                    draw(win);
                }
                win.caret_fresh = false;
            }
        }
    });
}

/// Where the composition is (physical screen pixels).
pub fn caret(rc: RECT) {
    let c = Caret { x: rc.left, y: rc.top, h: (rc.bottom - rc.top).max(1) };
    with_win(|win| unsafe {
        let moved = win.caret != Some(c);
        win.caret = Some(c);
        win.caret_fresh = true;
        if win.waiting {
            win.waiting = false;
            let _ = KillTimer(Some(win.hwnd), CARET_TIMER);
            draw(win);
        } else if moved && win.canvas.shown() {
            draw(win);
        }
    });
}

pub fn hide() {
    update(&CandUi::Hide);
}

/// Problems while drawing, for the engine's log.
pub fn take_notes() -> Vec<String> {
    let mut out = Vec::new();
    with_win(|win| out = std::mem::take(&mut win.notes));
    out
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER if wp.0 == CARET_TIMER => {
            guard((), || {
                with_win(|win| {
                    let _ = KillTimer(Some(hwnd), CARET_TIMER);
                    if win.waiting {
                        win.waiting = false;
                        draw(win);
                    }
                })
            });
            LRESULT(0)
        }
        // The weather skin's next frame.
        WM_TIMER if wp.0 == ime_candui::ANIM_TIMER => {
            guard((), || {
                with_win(|win| {
                    let notes = RefCell::new(Vec::new());
                    per_monitor(|| win.canvas.tick(hwnd, &|m: &str| notes.borrow_mut().push(m.to_string())));
                    win.notes.extend(notes.into_inner());
                })
            });
            LRESULT(0)
        }
        WM_TIMER if wp.0 == FLASH_TIMER => {
            guard((), || {
                with_win(|win| {
                    let _ = KillTimer(Some(hwnd), FLASH_TIMER);
                    if win.view.as_ref().map(|v| v.flash).unwrap_or(false) {
                        win.view = None;
                        draw(win);
                    }
                })
            });
            LRESULT(0)
        }
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_LBUTTONUP => {
            let x = (lp.0 & 0xffff) as i16 as f32;
            let y = ((lp.0 >> 16) & 0xffff) as i16 as f32;
            // Look up what was hit, then let go of the window state before
            // the text service (which updates the window) takes over.
            let mut hit = None;
            let mut doomed = None;
            with_win(|win| {
                hit = win.canvas.hit(x, y);
                // A click on the one marked for deletion deletes it.
                if let (Some(Hit::Cand(i)), Some(a)) = (hit, win.canvas.armed()) {
                    if i == a {
                        doomed = Some(i);
                    }
                }
                if win.canvas.armed().is_some() {
                    win.canvas.arm(None);
                    draw(win);
                }
            });
            if let Some(i) = doomed {
                guard((), || crate::service::forget_from_window(i as u32));
                return LRESULT(0);
            }
            let pick = match hit {
                Some(Hit::Cand(i)) => Some((Some(i as u32), None)),
                Some(Hit::Prev) => Some((None, Some(true))),
                Some(Hit::Next) => Some((None, Some(false))),
                None => None,
            };
            if let Some((index, page)) = pick {
                guard((), || crate::service::pick_from_window(index, page));
            }
            LRESULT(0)
        }
        // Right click: mark the candidate for deletion; again: delete it.
        WM_RBUTTONUP => {
            let x = (lp.0 & 0xffff) as i16 as f32;
            let y = ((lp.0 >> 16) & 0xffff) as i16 as f32;
            let mut doomed = None;
            with_win(|win| {
                match (win.canvas.hit(x, y), win.canvas.armed()) {
                    (Some(Hit::Cand(i)), Some(a)) if i == a => {
                        win.canvas.arm(None);
                        doomed = Some(i);
                    }
                    (Some(Hit::Cand(i)), _) => win.canvas.arm(Some(i)),
                    _ => win.canvas.arm(None),
                }
                draw(win);
            });
            if let Some(i) = doomed {
                guard((), || crate::service::forget_from_window(i as u32));
            }
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let delta = ((wp.0 >> 16) & 0xffff) as i16;
            guard((), || crate::service::pick_from_window(None, Some(delta > 0)));
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}
