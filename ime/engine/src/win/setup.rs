//! Installing the text service: registering the two TSF DLLs (64-bit for
//! 64-bit programs, 32-bit for 32-bit ones such as older WeChat / QQ) and
//! adding it to the user's keyboard list. Called by the installer (and by
//! the development script) as
//!
//!   EarthDeskIME.exe --register     (administrator) both DLLs
//!   EarthDeskIME.exe --unregister   (administrator)
//!   EarthDeskIME.exe --enable       (the user) add to 设置 → 语言 → 键盘
//!   EarthDeskIME.exe --disable      (the user) remove it from there

use windows::core::{s, w, HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, FreeLibrary};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::SystemInformation::{GetSystemDirectoryW, GetSystemWow64DirectoryW};
use windows::Win32::System::Threading::*;

fn exe_dir() -> std::path::PathBuf {
    std::env::current_exe().ok().and_then(|p| p.parent().map(|p| p.to_path_buf())).unwrap_or_default()
}

fn dir(get: impl Fn(&mut [u16]) -> u32) -> Option<String> {
    let mut buf = [0u16; 520];
    let n = get(&mut buf) as usize;
    (n > 0 && n < buf.len()).then(|| String::from_utf16_lossy(&buf[..n]))
}

/// Run a program, wait, return its exit code.
fn run(exe: &str, args: &str) -> Option<u32> {
    let mut cmd: Vec<u16> = format!("\"{exe}\" {args}").encode_utf16().chain(std::iter::once(0)).collect();
    let si = STARTUPINFOW { cb: std::mem::size_of::<STARTUPINFOW>() as u32, ..Default::default() };
    let mut pi = PROCESS_INFORMATION::default();
    unsafe {
        CreateProcessW(&HSTRING::from(exe), Some(PWSTR(cmd.as_mut_ptr())), None, None, false, CREATE_NO_WINDOW, None, None, &si, &mut pi).ok()?;
        WaitForSingleObject(pi.hProcess, 60_000);
        let mut code = 1u32;
        let _ = GetExitCodeProcess(pi.hProcess, &mut code);
        let _ = CloseHandle(pi.hProcess);
        let _ = CloseHandle(pi.hThread);
        Some(code)
    }
}

/// regsvr32 of the matching bitness for each DLL. Returns false if the
/// 64-bit one failed (the 32-bit one is optional: 32-bit Windows programs
/// just keep their current input method if it is missing).
fn regsvr(unregister: bool) -> bool {
    let d = exe_dir();
    let flag = if unregister { "/s /u" } else { "/s" };
    let mut ok = true;
    let pairs = [
        (dir(|b| unsafe { GetSystemDirectoryW(Some(b)) }), d.join("EarthDeskTSF.dll"), true),
        (dir(|b| unsafe { GetSystemWow64DirectoryW(Some(b)) }), d.join("EarthDeskTSF32.dll"), false),
    ];
    for (sys, dll, required) in pairs {
        let Some(sys) = sys else { continue };
        if !dll.exists() {
            if required {
                crate::log(&format!("{} missing", dll.display()));
                ok = false;
            }
            continue;
        }
        let code = run(&format!("{sys}\\regsvr32.exe"), &format!("{flag} \"{}\"", dll.display()));
        crate::log(&format!("regsvr32 {flag} {} -> {code:?}", dll.display()));
        if required && code != Some(0) {
            ok = false;
        }
    }
    ok
}

pub fn register() -> bool {
    regsvr(false)
}

pub fn unregister() -> bool {
    regsvr(true)
}

/// InstallLayoutOrTip (input.dll): the documented way to add a text service
/// to the current user's language list, or remove it (ILOT_UNINSTALL).
fn install_tip(remove: bool) -> bool {
    type Ilot = unsafe extern "system" fn(PCWSTR, u32) -> i32;
    unsafe {
        let Ok(lib) = LoadLibraryW(w!("input.dll")) else {
            crate::log("InstallLayoutOrTip: input.dll not found");
            return false;
        };
        let ok = match GetProcAddress(lib, s!("InstallLayoutOrTip")) {
            Some(f) => {
                let f: Ilot = std::mem::transmute(f);
                let tip = HSTRING::from(ime_proto::tip_string());
                f(PCWSTR(tip.as_ptr()), if remove { 1 } else { 0 }) != 0
            }
            None => false,
        };
        let _ = FreeLibrary(lib);
        crate::log(&format!("InstallLayoutOrTip remove={remove} -> {ok}"));
        ok
    }
}

pub fn enable() -> bool {
    install_tip(false)
}

pub fn disable() -> bool {
    install_tip(true)
}
