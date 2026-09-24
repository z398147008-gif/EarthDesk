//! Carrying out a rule. Runs on one worker thread (a COM apartment, for the
//! shell calls) so the hooks never wait on it.

use super::{apps, elevation, input};
use crate::toolkit::config::Action;
use crate::toolkit::keys::{self, Combo, ALT, WIN};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Mutex, OnceLock};
use windows::core::{BSTR, HSTRING, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

struct Job {
    action: Action,
    target: isize,
    name: String,
}

static QUEUE: OnceLock<Mutex<Sender<Job>>> = OnceLock::new();

pub fn submit(action: Action, target: isize, name: &str) {
    if let Some(q) = QUEUE.get() {
        if let Ok(tx) = q.lock() {
            let _ = tx.send(Job { action, target, name: name.to_string() });
        }
    }
}

pub fn start() {
    let (tx, rx) = channel::<Job>();
    let _ = QUEUE.set(Mutex::new(tx));
    std::thread::Builder::new()
        .name("toolkit-actions".into())
        .spawn(move || unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
            while let Ok(job) = rx.recv() {
                if let Err(e) = run(&job) {
                    crate::toolkit::log(&format!("「{}」failed: {e}", job.name));
                }
            }
        })
        .ok();
}

fn hwnd(v: isize) -> HWND {
    HWND(v as *mut _)
}

fn is_shell(h: HWND) -> bool {
    matches!(
        apps::class_of(h).as_str(),
        "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd" | "NotifyIconOverflowWindow"
    )
}

/// Bring the gesture's window to the front so keys reach it. The swallowed
/// right press would have activated it; we do the same by hand.
unsafe fn focus(target: HWND) {
    if target.0.is_null() || !IsWindow(Some(target)).as_bool() {
        return;
    }
    if apps::force_foreground(apps::root_of(target)) {
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

fn run(job: &Job) -> Result<(), String> {
    let target = hwnd(job.target);
    match &job.action {
        Action::None => Ok(()),
        Action::Keys { keys } => {
            let seq = keys::parse_sequence(keys).ok_or_else(|| format!("无法识别的按键 {keys}"))?;
            unsafe { focus(target) };
            wait_for_release();
            input::press_sequence(&seq);
            Ok(())
        }
        Action::Text { text } => {
            unsafe { focus(target) };
            wait_for_release();
            input::type_text(text);
            Ok(())
        }
        Action::Run { path, args, cwd, admin } => launch(path, args, cwd, *admin),
        Action::Open { target: t } => launch(t, "", "", false),
        Action::Builtin { name } => builtin(name, target),
    }
}

/// A hotkey fires while its keys are still down. Typed text must not mix
/// with them; give the user a moment to let go (at most 400 ms).
fn wait_for_release() {
    for _ in 0..20 {
        if input::physical_mods() & (keys::CTRL | ALT | WIN) == 0 {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

fn expand(s: &str) -> String {
    let wide = HSTRING::from(s);
    let mut buf = vec![0u16; 2048];
    let n = unsafe { windows::Win32::System::Environment::ExpandEnvironmentStringsW(&wide, Some(&mut buf)) };
    if n == 0 || n as usize > buf.len() {
        return s.to_string();
    }
    String::from_utf16_lossy(&buf[..(n as usize).saturating_sub(1)])
}

fn launch(path: &str, args: &str, cwd: &str, admin: bool) -> Result<(), String> {
    let path = expand(path.trim().trim_matches('"'));
    if path.is_empty() {
        return Err("没有填写程序".into());
    }
    let args = expand(args);
    let mut cwd = expand(cwd);
    if cwd.is_empty() {
        if let Some(parent) = std::path::Path::new(&path).parent() {
            if parent.is_absolute() {
                cwd = parent.to_string_lossy().into_owned();
            }
        }
    }
    // Elevated ourselves, a plain ShellExecute would hand our administrator
    // token to whatever we start. Ask Explorer to start it instead: it runs
    // with the user's ordinary rights, as if double-clicked.
    if !admin && elevation::is_elevated() {
        match unsafe { shell_execute_as_user(&path, &args, &cwd) } {
            Ok(()) => return Ok(()),
            Err(e) => crate::toolkit::log(&format!("unelevated start failed ({e}), falling back")),
        }
    }
    unsafe { shell_execute(&path, &args, &cwd, if admin { "runas" } else { "open" }) }
}

/// Open a link or document with the user's ordinary rights (from any
/// thread; COM is set up for the call).
pub fn open_as_user(target: &str) -> Result<(), String> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
        shell_execute_as_user(target, "", "").or_else(|_| shell_execute(target, "", "", "open"))
    }
}

unsafe fn shell_execute(path: &str, args: &str, cwd: &str, verb: &str) -> Result<(), String> {
    let (p, a, d, v) = (HSTRING::from(path), HSTRING::from(args), HSTRING::from(cwd), HSTRING::from(verb));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
        lpVerb: PCWSTR(v.as_ptr()),
        lpFile: PCWSTR(p.as_ptr()),
        lpParameters: if args.is_empty() { PCWSTR::null() } else { PCWSTR(a.as_ptr()) },
        lpDirectory: if cwd.is_empty() { PCWSTR::null() } else { PCWSTR(d.as_ptr()) },
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    ShellExecuteExW(&mut info).map_err(|e| e.message())
}

/// Raymond Chen's recipe: find the desktop's shell view and use its
/// IShellDispatch2::ShellExecute, which runs inside Explorer's process.
unsafe fn shell_execute_as_user(path: &str, args: &str, cwd: &str) -> windows::core::Result<()> {
    let windows: IShellWindows = CoCreateInstance(&ShellWindows, None, CLSCTX_LOCAL_SERVER)?;
    let loc = VARIANT::from(CSIDL_DESKTOP as i32);
    let empty = VARIANT::default();
    let mut hwnd = 0i32;
    let disp = windows.FindWindowSW(&loc, &empty, SWC_DESKTOP, &mut hwnd, SWFO_NEEDDISPATCH)?;
    let sp: windows::Win32::System::Com::IServiceProvider = windows::core::Interface::cast(&disp)?;
    let browser: IShellBrowser = sp.QueryService(&SID_STopLevelBrowser)?;
    let view = browser.QueryActiveShellView()?;
    let bg: windows::Win32::System::Com::IDispatch = view.GetItemObject(SVGIO_BACKGROUND)?;
    let folder: IShellFolderViewDual = windows::core::Interface::cast(&bg)?;
    let app = folder.Application()?;
    let shell: IShellDispatch2 = windows::core::Interface::cast(&app)?;
    shell.ShellExecute(
        &BSTR::from(path),
        &VARIANT::from(BSTR::from(args)),
        &VARIANT::from(BSTR::from(cwd)),
        &VARIANT::from(BSTR::from("open")),
        &VARIANT::from(SW_SHOWNORMAL.0),
    )
}

fn tap(k: &str) {
    if let Some(c) = Combo::parse(k) {
        input::press(c);
    }
}

fn builtin(name: &str, target: HWND) -> Result<(), String> {
    if crate::toolkit::APP_BUILTINS.contains(&name) {
        crate::toolkit::run_app_builtin(name, target.0 as isize);
        return Ok(());
    }
    let window = if target.0.is_null() { unsafe { GetForegroundWindow() } } else { apps::root_of(target) };
    let sys = |cmd: u32| unsafe {
        if window.0.is_null() || is_shell(window) {
            return;
        }
        let _ = PostMessageW(Some(window), WM_SYSCOMMAND, WPARAM(cmd as usize), LPARAM(0));
    };
    match name {
        "window_close" => sys(SC_CLOSE),
        "window_minimize" => sys(SC_MINIMIZE),
        "window_maximize" => unsafe {
            if IsZoomed(window).as_bool() {
                sys(SC_RESTORE)
            } else {
                sys(SC_MAXIMIZE)
            }
        },
        "window_topmost" => unsafe {
            if !window.0.is_null() && !is_shell(window) {
                let ex = GetWindowLongW(window, GWL_EXSTYLE) as u32;
                let after = if ex & WS_EX_TOPMOST.0 != 0 { HWND_NOTOPMOST } else { HWND_TOPMOST };
                let _ = SetWindowPos(window, Some(after), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
        },
        "show_desktop" => tap("Win+D"),
        "task_view" => tap("Win+Tab"),
        "lock" => unsafe {
            let _ = windows::Win32::System::Shutdown::LockWorkStation();
        },
        "media_play_pause" => tap("MediaPlayPause"),
        "media_next" => tap("MediaNext"),
        "media_prev" => tap("MediaPrev"),
        "volume_up" => tap("VolumeUp"),
        "volume_down" => tap("VolumeDown"),
        "volume_mute" => tap("VolumeMute"),
        other => return Err(format!("unknown built-in {other}")),
    }
    Ok(())
}
