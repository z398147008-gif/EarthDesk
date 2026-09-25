//! Running as administrator without a UAC prompt every time.
//!
//! Windows keeps low-level hooks and SendInput from reaching elevated windows
//! (Task Manager, installers, some launchers) unless the process doing it is
//! elevated too. The usual way to start elevated silently is a scheduled
//! task with "run with highest privileges": creating the task needs one UAC
//! consent (the installer already has it), after which running it does not.
//!
//! - The task is per user: `\EarthDesk\<user>`, action = this exe, and a
//!   logon trigger when "start with Windows" is on (it then replaces the
//!   HKCU Run value).
//! - A copy started without elevation (Start menu, the installer's "run now",
//!   the old Run value) runs the task and exits.
//! - Standard (non-admin) accounts have nothing to elevate to; for them all
//!   of this stays off.

use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use windows::core::{BSTR, HSTRING, PCWSTR};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
use windows::Win32::System::TaskScheduler::*;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::{GetTokenInformation, TokenElevation, TokenElevationType, TOKEN_ELEVATION, TOKEN_QUERY};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken, WaitForSingleObject, GetExitCodeProcess, INFINITE};
use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "EarthDesk";
pub const SETUP_ARG: &str = "--setup-task";
pub const AUTOSTART_ARG: &str = "--autostart";
pub const NO_AUTOSTART_ARG: &str = "--no-autostart";
pub const REMOVE_ARG: &str = "--remove-task";

fn quiet(cmd: &mut Command) -> &mut Command {
    cmd.creation_flags(CREATE_NO_WINDOW)
}

pub fn is_elevated() -> bool {
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut e = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut e as *mut _ as *mut _),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(token);
        ok && e.TokenIsElevated != 0
    }
}

/// True for an administrator running with a filtered (UAC) token -- the only
/// case in which there is anything to elevate to.
pub fn can_elevate() -> bool {
    if is_elevated() {
        return true;
    }
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut t = 0i32;
        let mut len = 0u32;
        let ok = GetTokenInformation(token, TokenElevationType, Some(&mut t as *mut _ as *mut _), 4, &mut len).is_ok();
        let _ = CloseHandle(token);
        // TokenElevationTypeLimited = 3
        ok && t == 3
    }
}

fn user() -> String {
    let name = std::env::var("USERNAME").unwrap_or_else(|_| "user".into());
    name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' }).collect()
}

/// In the root folder: a task in a folder created by an elevated process may
/// not be readable (and so not runnable) by the same user unelevated.
fn task_name() -> String {
    format!("EarthDesk ({})", user())
}

fn exe() -> Option<PathBuf> {
    std::env::current_exe().ok()
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The task definition registered through ITaskFolder::RegisterTask.
fn task_xml(exe: &str, logon: bool) -> String {
    let who = match (std::env::var("USERDOMAIN"), std::env::var("USERNAME")) {
        (Ok(d), Ok(u)) => format!("{d}\\{u}"),
        (_, Ok(u)) => u,
        _ => String::new(),
    };
    let who = xml_escape(&who);
    let trigger = if logon {
        format!(
            "<Triggers><LogonTrigger><Enabled>true</Enabled><UserId>{who}</UserId><Delay>PT3S</Delay></LogonTrigger></Triggers>"
        )
    } else {
        String::new()
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>地球桌面：以管理员身份启动，让手势、快捷键和截图在所有窗口上都能用。</Description></RegistrationInfo>
  {trigger}
  <Principals>
    <Principal id="Author">
      <UserId>{who}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>false</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings><StopOnIdleEnd>false</StopOnIdleEnd><RestartOnIdle>false</RestartOnIdle></IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <WakeToRun>false</WakeToRun>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>5</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec><Command>{exe}</Command></Exec>
  </Actions>
</Task>
"#,
        exe = xml_escape(exe)
    )
}

/// The Task Scheduler's root folder. COM is initialised on whatever thread
/// asks (the calls are short and made from blocking helper threads).
unsafe fn root_folder() -> windows::core::Result<ITaskFolder> {
    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    let svc: ITaskService = CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER)?;
    let none = VARIANT::default();
    svc.Connect(&none, &none, &none, &none)?;
    svc.GetFolder(&BSTR::from("\\"))
}

unsafe fn registered() -> Option<IRegisteredTask> {
    root_folder().ok()?.GetTask(&BSTR::from(task_name())).ok()
}

/// What the registered task runs, and whether it starts at logon.
fn query() -> Option<(String, bool)> {
    let xml = unsafe { registered()?.Xml().ok()?.to_string() };
    let cmd = xml
        .split("<Command>")
        .nth(1)
        .and_then(|s| s.split("</Command>").next())
        .unwrap_or("")
        .trim()
        .trim_matches('"')
        .replace("&amp;", "&");
    Some((cmd, xml.contains("<LogonTrigger>")))
}

/// The task exists and runs this very exe.
pub fn task_installed() -> bool {
    let (Some((cmd, _)), Some(me)) = (query(), exe()) else { return false };
    cmd.eq_ignore_ascii_case(&me.to_string_lossy())
}

pub fn task_autostart() -> bool {
    match (query(), exe()) {
        (Some((cmd, logon)), Some(me)) => logon && cmd.eq_ignore_ascii_case(&me.to_string_lossy()),
        _ => false,
    }
}

/// Create or replace the task. Must be elevated.
pub fn install_task(logon: bool) -> Result<(), String> {
    let exe = exe().ok_or("找不到程序路径")?;
    let xml = task_xml(&exe.to_string_lossy(), logon);
    unsafe {
        let folder = root_folder().map_err(|e| e.message())?;
        let none = VARIANT::default();
        folder
            .RegisterTask(
                &BSTR::from(task_name()),
                &BSTR::from(xml),
                TASK_CREATE_OR_UPDATE.0,
                &none,
                &none,
                TASK_LOGON_INTERACTIVE_TOKEN,
                &none,
            )
            .map_err(|e| format!("注册计划任务失败：{}", e.message()))?;
    }
    // The task owns starting with Windows now; the Run value would start a
    // second, unelevated copy.
    if logon {
        remove_run_value();
    }
    Ok(())
}

pub fn delete_task() {
    unsafe {
        if let Ok(f) = root_folder() {
            let _ = f.DeleteTask(&BSTR::from(task_name()), 0);
        }
    }
}

pub fn run_task() -> bool {
    unsafe {
        match registered() {
            Some(t) => t.Run(&VARIANT::default()).is_ok(),
            None => false,
        }
    }
}

fn run_value_present() -> bool {
    quiet(Command::new("reg").args(["query", RUN_KEY, "/v", RUN_VALUE]))
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn remove_run_value() {
    let _ = quiet(Command::new("reg").args(["delete", RUN_KEY, "/v", RUN_VALUE, "/f"])).status();
}

/// Start this exe elevated with `args` (one UAC prompt) and wait for it.
fn run_self_elevated(args: &str) -> Result<(), String> {
    let exe = exe().ok_or("找不到程序路径")?;
    let (file, params, verb) = (HSTRING::from(exe.as_os_str()), HSTRING::from(args), HSTRING::from("runas"));
    unsafe {
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS,
            lpVerb: PCWSTR(verb.as_ptr()),
            lpFile: PCWSTR(file.as_ptr()),
            lpParameters: PCWSTR(params.as_ptr()),
            nShow: SW_HIDE.0,
            ..Default::default()
        };
        ShellExecuteExW(&mut info).map_err(|_| "已取消".to_string())?;
        if info.hProcess.is_invalid() {
            return Ok(());
        }
        WaitForSingleObject(info.hProcess, INFINITE);
        let mut code = 1u32;
        let _ = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
        if code == 0 {
            Ok(())
        } else {
            Err(format!("设置计划任务失败（{code}）"))
        }
    }
}

/// `EarthDesk.exe --setup-task [--autostart | --no-autostart]`: create the
/// task and exit. Without either flag, starting with Windows stays as it
/// was (an upgrade must not switch it back on or off). Used by the
/// installer (already elevated) and by the settings page through UAC.
/// `--remove-task` is the uninstaller's.
pub fn setup_mode() -> Option<i32> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == REMOVE_ARG) {
        delete_task();
        remove_run_value();
        return Some(0);
    }
    if !args.iter().any(|a| a == SETUP_ARG) {
        return None;
    }
    let logon = if args.iter().any(|a| a == AUTOSTART_ARG) {
        true
    } else if args.iter().any(|a| a == NO_AUTOSTART_ARG) {
        false
    } else {
        task_autostart() || run_value_present()
    };
    Some(match install_task(logon) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{e}");
            2
        }
    })
}

/// Decide at start-up. Returns true when this copy should exit because an
/// elevated one is being started in its place.
pub fn relaunch_if_needed(elevate: bool, first_run: bool) -> bool {
    if !elevate || is_elevated() || !can_elevate() {
        return false;
    }
    if !task_installed() {
        // Once per installation: the installer normally creates the task,
        // so this only happens for copies that were not installed (or an
        // install made by another account). One UAC prompt.
        let autostart = first_run || run_value_present();
        let args = if autostart { format!("{SETUP_ARG} {AUTOSTART_ARG}") } else { SETUP_ARG.to_string() };
        if let Err(e) = run_self_elevated(&args) {
            crate::toolkit::log(&format!("elevation declined or failed: {e}"));
            let mut cfg = (*crate::toolkit::current()).clone();
            cfg.elevate = false;
            cfg.save();
            return false;
        }
    }
    run_task()
}

/// Once running elevated: keep the task pointing at this exe and make it,
/// not the Run value, the thing that starts us with Windows.
pub fn tidy_after_start() {
    if !is_elevated() {
        return;
    }
    remove_old_ime_files();
    let run = run_value_present();
    let installed = task_installed();
    let logon = task_autostart();
    if !installed || (run && !logon) {
        let _ = install_task(run || logon);
    } else if run {
        remove_run_value();
    }
}

/// An upgrade moves input-method files that were in use aside
/// (EarthDeskTSF.dll.old1 ...) instead of asking for a restart. Once the
/// programs that had them loaded are closed, they can go.
fn remove_old_ime_files() {
    let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("ime"))) else { return };
    let Ok(entries) = std::fs::read_dir(&dir) else { return };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.contains(".old") && (name.contains(".dll") || name.contains(".exe")) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Start with Windows, whichever way applies.
pub fn autostart_enabled() -> bool {
    task_autostart() || run_value_present()
}

pub fn set_autostart(on: bool, elevate: bool) -> Result<(), String> {
    if elevate && is_elevated() {
        install_task(on)?;
        if !on {
            remove_run_value();
        }
        return Ok(());
    }
    if task_installed() && !on {
        // Cannot rewrite the task without elevation; the Run value is off
        // already, so just make sure a logon trigger does not remain.
        if task_autostart() {
            run_self_elevated(&format!("{SETUP_ARG} {NO_AUTOSTART_ARG}"))?;
        }
    }
    crate::environment::set_autostart(on)
}

/// The settings page's "以管理员身份运行" switch.
pub fn set_elevate(on: bool) -> Result<(), String> {
    let autostart = autostart_enabled();
    if on {
        if !can_elevate() {
            return Err("当前 Windows 账户不是管理员，无法以管理员身份运行".into());
        }
        let args = if autostart { format!("{SETUP_ARG} {AUTOSTART_ARG}") } else { SETUP_ARG.to_string() };
        if is_elevated() {
            install_task(autostart)
        } else {
            run_self_elevated(&args)
        }
    } else {
        if task_installed() {
            // Deleting it may need elevation; if that fails the task simply
            // stays unused, because `elevate` is off.
            delete_task();
        }
        if autostart {
            crate::environment::set_autostart(true)?;
        }
        Ok(())
    }
}

/// One copy at a time. Dev builds use their own name so `cargo tauri dev`
/// can run next to an installed copy.
pub fn claim_instance() -> bool {
    use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS, WAIT_ABANDONED, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::CreateMutexW;
    let name = if cfg!(debug_assertions) { r"Local\EarthDesk.Dev" } else { r"Local\EarthDesk.Running" };
    unsafe {
        match CreateMutexW(None, true, &HSTRING::from(name)) {
            Ok(h) => {
                if GetLastError() == ERROR_ALREADY_EXISTS {
                    // Give a copy that is handing over to us time to exit.
                    let r = WaitForSingleObject(h, 6000);
                    r == WAIT_OBJECT_0 || r == WAIT_ABANDONED
                } else {
                    true
                }
                // The handle is kept for the life of the process on purpose.
            }
            // Access denied: an elevated copy owns it.
            Err(_) => false,
        }
    }
}
