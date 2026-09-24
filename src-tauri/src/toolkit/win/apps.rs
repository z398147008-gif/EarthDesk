//! Which program a window belongs to, and a few questions about windows the
//! gesture and hotkey code needs answered quickly (from inside a hook).

use std::collections::HashMap;
use std::sync::Mutex;
use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, HWND, POINT, RECT};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::*;

/// pid -> full image path. Processes come and go but a pid is not reused
/// while its windows exist, and an occasional stale entry only costs a wrong
/// scope for a program that has already exited.
static PATHS: Mutex<Option<HashMap<u32, String>>> = Mutex::new(None);

pub fn pid_of(hwnd: HWND) -> u32 {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid
}

pub fn path_of_pid(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    if let Ok(g) = PATHS.lock() {
        if let Some(p) = g.as_ref().and_then(|m| m.get(&pid)) {
            return Some(p.clone());
        }
    }
    let path = unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(h);
        if !ok {
            return None;
        }
        String::from_utf16_lossy(&buf[..len as usize])
    };
    if let Ok(mut g) = PATHS.lock() {
        let m = g.get_or_insert_with(HashMap::new);
        if m.len() > 512 {
            m.clear();
        }
        m.insert(pid, path.clone());
    }
    Some(path)
}

pub fn exe_name(path: &str) -> String {
    path.rsplit(['\\', '/']).next().unwrap_or(path).to_lowercase()
}

pub fn class_of(hwnd: HWND) -> String {
    let mut buf = [0u16; 128];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

pub fn title_of(hwnd: HWND) -> String {
    let mut buf = [0u16; 512];
    let n = unsafe { GetWindowTextW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

pub fn root_of(hwnd: HWND) -> HWND {
    let r = unsafe { GetAncestor(hwnd, GA_ROOT) };
    if r.0.is_null() {
        hwnd
    } else {
        r
    }
}

/// The program a window really belongs to. Store apps draw inside a frame
/// owned by ApplicationFrameHost.exe; the app itself is the child
/// CoreWindow's process.
pub fn exe_of_window(hwnd: HWND) -> String {
    let root = root_of(hwnd);
    let mut pid = pid_of(root);
    let mut path = path_of_pid(pid).unwrap_or_default();
    if exe_name(&path) == "applicationframehost.exe" {
        unsafe {
            let mut child = FindWindowExW(Some(root), None, windows::core::w!("Windows.UI.Core.CoreWindow"), None)
                .unwrap_or_default();
            if child.0.is_null() {
                child = hwnd;
            }
            let p = pid_of(child);
            if p != 0 && p != pid {
                pid = p;
                path = path_of_pid(pid).unwrap_or(path);
            }
        }
    }
    exe_name(&path)
}

pub fn window_at(p: POINT) -> HWND {
    unsafe { WindowFromPoint(p) }
}

pub fn foreground() -> HWND {
    unsafe { GetForegroundWindow() }
}

fn is_shell_class(c: &str) -> bool {
    matches!(c, "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd")
}

/// A window that covers its whole monitor and is not the desktop itself: a
/// game, a full-screen video, a presentation.
pub fn is_fullscreen(hwnd: HWND) -> bool {
    let root = root_of(hwnd);
    if root.0.is_null() || is_shell_class(&class_of(root)) {
        return false;
    }
    unsafe {
        let style = GetWindowLongW(root, GWL_STYLE) as u32;
        // A maximised ordinary window also fills the screen (minus the
        // taskbar); only borderless or caption-less windows count.
        if style & WS_CAPTION.0 == WS_CAPTION.0 {
            return false;
        }
        let mut r = RECT::default();
        if GetWindowRect(root, &mut r).is_err() {
            return false;
        }
        let mon = MonitorFromWindow(root, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(mon, &mut mi).as_bool() {
            return false;
        }
        let m = mi.rcMonitor;
        r.left <= m.left && r.top <= m.top && r.right >= m.right && r.bottom >= m.bottom
    }
}

/// DPI scale of the monitor a point is on (1.0 at 100%).
pub fn scale_at(p: POINT) -> f64 {
    unsafe {
        let mon = windows::Win32::Graphics::Gdi::MonitorFromPoint(p, MONITOR_DEFAULTTONEAREST);
        let (mut x, mut y) = (96u32, 96u32);
        if GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut x, &mut y).is_ok() {
            x as f64 / 96.0
        } else {
            1.0
        }
    }
}

fn cloaked(hwnd: HWND) -> bool {
    let mut v = 0u32;
    unsafe {
        DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut v as *mut u32 as *mut _, 4).is_ok() && v != 0
    }
}

/// A window the taskbar would show a button for.
pub fn is_app_window(hwnd: HWND) -> bool {
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() || cloaked(hwnd) {
            return false;
        }
        let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        if ex & WS_EX_TOOLWINDOW.0 != 0 {
            return false;
        }
        let owner = GetWindow(hwnd, GW_OWNER).unwrap_or_default();
        if !owner.0.is_null() && ex & WS_EX_APPWINDOW.0 == 0 {
            return false;
        }
        GetWindowTextLengthW(hwnd) > 0
    }
}

pub struct RunningApp {
    pub exe: String,
    pub path: String,
    pub title: String,
}

/// Programs with a window open, one entry per executable, front-most first.
pub fn running_apps() -> Vec<RunningApp> {
    let mut out: Vec<RunningApp> = Vec::new();
    unsafe {
        let mut h = GetTopWindow(None).unwrap_or_default();
        while !h.0.is_null() {
            if is_app_window(h) {
                let exe = exe_of_window(h);
                if !exe.is_empty() && !out.iter().any(|a| a.exe == exe) && exe != "earthdesk.exe" && exe != "earth-desk.exe" {
                    let path = path_of_pid(pid_of(h)).unwrap_or_default();
                    out.push(RunningApp { exe, path, title: title_of(h) });
                }
            }
            h = GetWindow(h, GW_HWNDNEXT).unwrap_or_default();
        }
    }
    out
}

/// Make `root` the foreground window even when another program is in front
/// and we were not the last to receive input (hooks do not count). Returns
/// true when it had to change anything.
pub fn force_foreground(root: HWND) -> bool {
    use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    unsafe {
        let fg = GetForegroundWindow();
        if fg == root {
            return false;
        }
        let me = GetCurrentThreadId();
        let theirs = if fg.0.is_null() { 0 } else { GetWindowThreadProcessId(fg, None) };
        let attached = theirs != 0 && theirs != me && AttachThreadInput(me, theirs, true).as_bool();
        let _ = SetForegroundWindow(root);
        let _ = BringWindowToTop(root);
        if attached {
            let _ = AttachThreadInput(me, theirs, false);
        }
        if GetForegroundWindow() != root {
            // Last resort: a keystroke of our own makes us "the last input".
            super::input::mask();
            let _ = SetForegroundWindow(root);
        }
        true
    }
}
