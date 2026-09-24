//! Low-level mouse and keyboard hooks: right-button gestures and hotkeys.
//!
//! Both hooks live on one dedicated thread whose only job is to pump their
//! messages. The callbacks decide quickly and hand anything slow (running an
//! action, drawing) to other threads.
//!
//! Right button, in order:
//! - pressed  -> swallowed; we wait to see what the user does.
//! - moved past the threshold -> a gesture: the trail starts drawing.
//! - released without moving  -> we replay a normal right click, so context
//!   menus work exactly as before.
//! - held still for `hold_ms`  -> we replay the press and step aside, so
//!   right-drag in programs that use it (file managers, games, CAD) works.
//! - left button or Esc during a gesture -> cancelled.

use super::{actions, apps, input, trail};
use crate::toolkit::config::ToolkitConfig;
use crate::toolkit::gesture::Recognizer;
use crate::toolkit::keys::{self, Combo, ALT, WIN};
use crate::toolkit::{RECORDING, SUSPENDED};
use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, RwLock};
use windows::Win32::Foundation::*;
use windows::Win32::System::Threading::{GetCurrentProcessId, GetCurrentThreadId};
use windows::Win32::UI::HiDpi::{SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::WindowsAndMessaging::*;

const WM_HOOK_REPLAY_CLICK: u32 = WM_APP + 10;
const WM_HOOK_REPLAY_DOWN: u32 = WM_APP + 11;
const WM_HOOK_MASK: u32 = WM_APP + 12;
const WM_HOOK_REINSTALL: u32 = WM_APP + 13;

/// Config as the hooks need it: hotkeys pre-parsed.
struct Compiled {
    cfg: Arc<ToolkitConfig>,
    hotkeys: Vec<(Combo, usize)>,
}

static COMPILED: RwLock<Option<Arc<Compiled>>> = RwLock::new(None);
static THREAD: AtomicU32 = AtomicU32::new(0);

pub fn apply(cfg: &Arc<ToolkitConfig>) {
    let hotkeys = cfg
        .hotkeys
        .rules
        .iter()
        .enumerate()
        .filter(|(_, r)| r.enabled)
        .filter_map(|(i, r)| Combo::parse(&r.keys).map(|c| (c, i)))
        .collect();
    if let Ok(mut g) = COMPILED.write() {
        *g = Some(Arc::new(Compiled { cfg: cfg.clone(), hotkeys }));
    }
}

fn compiled() -> Option<Arc<Compiled>> {
    COMPILED.read().ok().and_then(|g| g.clone())
}

enum Right {
    Idle,
    /// Pressed, not yet moved far enough to be a gesture.
    Pending { start: POINT, target: HWND, exe: String, scale: f64, timer: usize },
    Active { rec: Recognizer, target: HWND, exe: String, last: String },
    /// Handed back to the program; let everything through until release.
    Passthrough,
    /// Cancelled; swallow the release.
    Cancelled,
}

struct HookState {
    mouse: HHOOK,
    keyboard: HHOOK,
    right: Right,
    /// Keys whose press fired a hotkey: their repeats and release are ours.
    swallowed: HashSet<u32>,
    /// Swallow the left button's release after it cancelled a gesture.
    eat_left_up: bool,
}

thread_local! {
    static STATE: RefCell<HookState> = RefCell::new(HookState {
        mouse: HHOOK::default(),
        keyboard: HHOOK::default(),
        right: Right::Idle,
        swallowed: HashSet::new(),
        eat_left_up: false,
    });
}

fn post_self(msg: u32) {
    let t = THREAD.load(Ordering::SeqCst);
    if t != 0 {
        unsafe {
            let _ = PostThreadMessageW(t, msg, WPARAM(0), LPARAM(0));
        }
    }
}

fn describe(cfg: &ToolkitConfig, gesture: &str, exe: &str) -> (String, bool) {
    match cfg.gesture_rule(gesture, exe) {
        Some(r) => (format!("{gesture}  {}", r.name), true),
        None => (gesture.to_string(), false),
    }
}

fn own_process(hwnd: HWND) -> bool {
    apps::pid_of(hwnd) == unsafe { GetCurrentProcessId() }
}

/// Decide whether a right press at `pt` may become a gesture.
fn gesture_target(cfg: &ToolkitConfig, pt: POINT) -> Option<(HWND, String)> {
    let g = &cfg.gestures;
    if !g.enabled || g.rules.iter().all(|r| !r.enabled) {
        return None;
    }
    let hwnd = apps::window_at(pt);
    if hwnd.0.is_null() || own_process(hwnd) {
        return None;
    }
    let exe = apps::exe_of_window(hwnd);
    if g.exclude_apps.iter().any(|a| *a == exe) {
        return None;
    }
    if g.skip_fullscreen && apps::is_fullscreen(hwnd) {
        return None;
    }
    Some((apps::root_of(hwnd), exe))
}

unsafe extern "system" fn mouse_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code < 0 {
        return CallNextHookEx(None, code, wp, lp);
    }
    let info = &*(lp.0 as *const MSLLHOOKSTRUCT);
    if info.dwExtraInfo == input::MAGIC {
        return CallNextHookEx(None, code, wp, lp);
    }
    let msg = wp.0 as u32;
    let pt = info.pt;
    let swallow = STATE.with(|s| {
        let mut s = s.borrow_mut();
        match msg {
            WM_RBUTTONDOWN => {
                if RECORDING.load(Ordering::SeqCst) || SUSPENDED.load(Ordering::SeqCst) {
                    s.right = Right::Idle;
                    return false;
                }
                let Some(c) = compiled() else { return false };
                match gesture_target(&c.cfg, pt) {
                    Some((target, exe)) => {
                        let timer = SetTimer(None, 0, c.cfg.gestures.hold_ms as u32, None);
                        s.right = Right::Pending { start: pt, target, exe, scale: apps::scale_at(pt), timer };
                        true
                    }
                    None => {
                        s.right = Right::Idle;
                        false
                    }
                }
            }
            WM_MOUSEMOVE => {
                let promote = match &s.right {
                    Right::Pending { start, scale, .. } => {
                        let min = compiled().map(|c| c.cfg.gestures.min_distance).unwrap_or(10.0) * scale;
                        let (dx, dy) = ((pt.x - start.x) as f64, (pt.y - start.y) as f64);
                        dx * dx + dy * dy >= min * min
                    }
                    _ => false,
                };
                if promote {
                    if let Right::Pending { start, target, exe, scale, timer } =
                        std::mem::replace(&mut s.right, Right::Idle)
                    {
                        let _ = KillTimer(None, timer);
                        let Some(c) = compiled() else { return false };
                        let g = &c.cfg.gestures;
                        let mut rec = Recognizer::new((start.x, start.y), scale, g.diagonals);
                        rec.push((pt.x, pt.y));
                        trail::begin((start.x, start.y), scale, &g.trail_color, g.trail_width, g.trail);
                        trail::point((pt.x, pt.y));
                        s.right = Right::Active { rec, target, exe, last: String::new() };
                    }
                } else if let Right::Active { rec, exe, last, .. } = &mut s.right {
                    rec.push((pt.x, pt.y));
                    trail::point((pt.x, pt.y));
                    let now = rec.gesture();
                    if now != *last {
                        *last = now.clone();
                        if let Some(c) = compiled() {
                            if c.cfg.gestures.hint {
                                if rec.too_long() {
                                    trail::hint(Some(("取消".into(), false)));
                                } else {
                                    trail::hint(Some(describe(&c.cfg, &now, exe)));
                                }
                            }
                        }
                    }
                }
                false
            }
            WM_RBUTTONUP => match std::mem::replace(&mut s.right, Right::Idle) {
                Right::Pending { timer, .. } => {
                    let _ = KillTimer(None, timer);
                    post_self(WM_HOOK_REPLAY_CLICK);
                    true
                }
                Right::Active { rec, target, exe, .. } => {
                    trail::end();
                    if !rec.too_long() {
                        let gesture = rec.gesture();
                        if let Some(c) = compiled() {
                            if let Some(rule) = c.cfg.gesture_rule(&gesture, &exe) {
                                actions::submit(rule.action.clone(), target.0 as isize, &rule.name);
                            }
                        }
                    }
                    true
                }
                Right::Cancelled => true,
                Right::Passthrough | Right::Idle => false,
            },
            WM_LBUTTONDOWN => match &s.right {
                Right::Pending { timer, .. } => {
                    let _ = KillTimer(None, *timer);
                    s.right = Right::Cancelled;
                    s.eat_left_up = true;
                    true
                }
                Right::Active { .. } => {
                    trail::end();
                    s.right = Right::Cancelled;
                    s.eat_left_up = true;
                    true
                }
                _ => false,
            },
            WM_LBUTTONUP if s.eat_left_up => {
                s.eat_left_up = false;
                true
            }
            _ => false,
        }
    });
    if swallow {
        LRESULT(1)
    } else {
        CallNextHookEx(None, code, wp, lp)
    }
}

/// Held still: give the press back.
fn on_timer(id: usize) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        if let Right::Pending { timer, .. } = &s.right {
            if *timer == id {
                unsafe {
                    let _ = KillTimer(None, id);
                }
                s.right = Right::Passthrough;
                post_self(WM_HOOK_REPLAY_DOWN);
            }
        } else {
            unsafe {
                let _ = KillTimer(None, id);
            }
        }
    });
}

fn hotkey_for(c: &Compiled, combo: Combo) -> Option<usize> {
    let candidates: Vec<usize> = c.hotkeys.iter().filter(|(k, _)| *k == combo).map(|(_, i)| *i).collect();
    if candidates.is_empty() {
        return None;
    }
    let fg = apps::foreground();
    let exe = if fg.0.is_null() { String::new() } else { apps::exe_of_window(fg) };
    if c.cfg.hotkeys.exclude_apps.iter().any(|a| *a == exe) {
        return None;
    }
    let rules = &c.cfg.hotkeys.rules;
    candidates
        .into_iter()
        .filter_map(|i| rules[i].scope.rank(&exe).map(|k| (k, i)))
        .fold(None::<(u8, usize)>, |best, (k, i)| match best {
            Some((bk, _)) if bk >= k => best,
            _ => Some((k, i)),
        })
        .map(|(_, i)| i)
}

unsafe extern "system" fn keyboard_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code < 0 {
        return CallNextHookEx(None, code, wp, lp);
    }
    let info = &*(lp.0 as *const KBDLLHOOKSTRUCT);
    if info.dwExtraInfo == input::MAGIC {
        return CallNextHookEx(None, code, wp, lp);
    }
    let msg = wp.0 as u32;
    let down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
    let vk = info.vkCode;

    if keys::is_modifier(vk as u16) {
        input::note_physical(vk as u16, down);
        return CallNextHookEx(None, code, wp, lp);
    }

    let swallow = STATE.with(|s| {
        let mut s = s.borrow_mut();
        // Esc cancels a gesture in progress.
        if down && vk == 0x1B {
            match &s.right {
                Right::Pending { timer, .. } => {
                    let _ = KillTimer(None, *timer);
                    s.right = Right::Cancelled;
                    return true;
                }
                Right::Active { .. } => {
                    trail::end();
                    s.right = Right::Cancelled;
                    return true;
                }
                _ => {}
            }
        }
        if !down {
            return s.swallowed.remove(&vk);
        }
        if s.swallowed.contains(&vk) {
            return true; // auto-repeat of a key that fired a hotkey
        }
        input::reconcile();
        if SUSPENDED.load(Ordering::SeqCst) && !RECORDING.load(Ordering::SeqCst) {
            return false;
        }
        if RECORDING.load(Ordering::SeqCst) {
            let mods = input::physical_mods();
            let text = if mods == 0 && vk == 0x1B { String::new() } else { Combo { mods, vk: vk as u16 }.format() };
            s.swallowed.insert(vk);
            if mods & (ALT | WIN) != 0 {
                post_self(WM_HOOK_MASK);
            }
            crate::toolkit::report_recorded(text);
            return true;
        }
        let Some(c) = compiled() else { return false };
        if !c.cfg.hotkeys.enabled {
            return false;
        }
        let mods = input::physical_mods();
        let combo = Combo { mods, vk: vk as u16 };
        let Some(i) = hotkey_for(&c, combo) else { return false };
        let rule = &c.cfg.hotkeys.rules[i];
        s.swallowed.insert(vk);
        if mods & (ALT | WIN) != 0 {
            // Without this, letting go of Win opens Start and letting go of
            // Alt focuses the menu bar: they saw no other key.
            post_self(WM_HOOK_MASK);
        }
        let fg = apps::foreground();
        actions::submit(rule.action.clone(), fg.0 as isize, &rule.name);
        true
    });
    if swallow {
        LRESULT(1)
    } else {
        CallNextHookEx(None, code, wp, lp)
    }
}

unsafe fn install() {
    let (m, k) = (
        SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), None, 0),
        SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), None, 0),
    );
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.mouse = m.unwrap_or_default();
        s.keyboard = k.unwrap_or_default();
    });
}

unsafe fn uninstall() {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        if !s.mouse.is_invalid() {
            let _ = UnhookWindowsHookEx(s.mouse);
        }
        if !s.keyboard.is_invalid() {
            let _ = UnhookWindowsHookEx(s.keyboard);
        }
        s.mouse = HHOOK::default();
        s.keyboard = HHOOK::default();
    });
}

pub fn start() {
    std::thread::Builder::new()
        .name("input-hooks".into())
        .spawn(|| unsafe {
            let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            // Make sure the thread has a message queue before anyone posts.
            let mut msg = MSG::default();
            let _ = PeekMessageW(&mut msg, None, WM_USER, WM_USER, PM_NOREMOVE);
            THREAD.store(GetCurrentThreadId(), Ordering::SeqCst);
            install();
            crate::toolkit::log("hooks installed");
            // Windows removes a low-level hook without telling anyone if it
            // ever stalls (and after some sleep/resume cycles). Re-installing
            // now and then is cheap and makes that self-healing.
            let refresh = SetTimer(None, 0, 60_000, None);
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                match msg.message {
                    WM_TIMER if msg.hwnd.0.is_null() => {
                        if msg.wParam.0 == refresh {
                            post_self(WM_HOOK_REINSTALL);
                        } else {
                            on_timer(msg.wParam.0);
                        }
                    }
                    WM_HOOK_REPLAY_CLICK => input::right_click(),
                    WM_HOOK_REPLAY_DOWN => input::right_down(),
                    WM_HOOK_MASK => input::mask(),
                    WM_HOOK_REINSTALL => {
                        let idle = STATE.with(|s| matches!(s.borrow().right, Right::Idle));
                        if idle {
                            uninstall();
                            install();
                        }
                    }
                    _ => {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
            }
            uninstall();
        })
        .ok();
}
