//! Things a freshly installed copy needs so it works with nothing else set
//! up: the built-in hardware monitor and starting with Windows.
//!
//! The hardware monitor is our own Windows service, `EarthDeskSensors`
//! (sensors/EarthDeskSensors.cs, built on LibreHardwareMonitorLib). The
//! installer registers it once, elevated (windows/hooks.nsh ->
//! sensors/setup-sensors.ps1): PawnIO driver, service, crash recovery, and a
//! permission that lets the signed-in user start and stop it. From then on
//! the app needs no administrator rights: it reads the service over a named
//! pipe, starts it if it is stopped, and restarts it if it stops answering.
//! Only a broken or missing installation needs the user (one UAC prompt, from
//! the settings page).

use std::path::PathBuf;
#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg_attr(not(windows), allow(dead_code))]
pub const SERVICE: &str = "EarthDeskSensors";
const SETUP_SCRIPT: &str = "setup-sensors.ps1";
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "EarthDesk";
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn quiet(cmd: &mut std::process::Command) -> &mut std::process::Command {
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

/// What the service manager says about the hardware monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(windows), allow(dead_code))]
pub enum ServiceState {
    NotInstalled,
    Stopped,
    Starting,
    Stopping,
    Running,
    /// Paused or some other state we never put it in.
    Other,
    /// The service manager would not tell us.
    Unknown,
}

impl ServiceState {
    pub fn as_str(self) -> &'static str {
        match self {
            ServiceState::NotInstalled => "not_installed",
            ServiceState::Stopped => "stopped",
            ServiceState::Starting => "starting",
            ServiceState::Stopping => "stopping",
            ServiceState::Running => "running",
            ServiceState::Other => "other",
            ServiceState::Unknown => "unknown",
        }
    }
}

#[cfg(windows)]
mod scm {
    use super::ServiceState;

    #[repr(C)]
    #[derive(Default)]
    struct ServiceStatus {
        service_type: u32,
        current_state: u32,
        controls_accepted: u32,
        win32_exit_code: u32,
        service_specific_exit_code: u32,
        check_point: u32,
        wait_hint: u32,
    }

    #[link(name = "advapi32")]
    extern "system" {
        fn OpenSCManagerW(machine: *const u16, database: *const u16, access: u32) -> isize;
        fn OpenServiceW(scm: isize, name: *const u16, access: u32) -> isize;
        fn QueryServiceStatus(service: isize, status: *mut ServiceStatus) -> i32;
        fn StartServiceW(service: isize, argc: u32, argv: *const *const u16) -> i32;
        fn ControlService(service: isize, control: u32, status: *mut ServiceStatus) -> i32;
        fn CloseServiceHandle(handle: isize) -> i32;
        fn RegGetValueW(
            key: isize,
            sub_key: *const u16,
            value: *const u16,
            flags: u32,
            kind: *mut u32,
            data: *mut u8,
            size: *mut u32,
        ) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetLastError() -> u32;
    }

    const SC_MANAGER_CONNECT: u32 = 0x0001;
    const SERVICE_QUERY_STATUS: u32 = 0x0004;
    const SERVICE_START: u32 = 0x0010;
    const SERVICE_STOP: u32 = 0x0020;
    const SERVICE_CONTROL_STOP: u32 = 1;
    const ERROR_SERVICE_DOES_NOT_EXIST: u32 = 1060;
    const ERROR_SERVICE_ALREADY_RUNNING: u32 = 1056;

    const HKEY_LOCAL_MACHINE: isize = 0x8000_0002u32 as i32 as isize;
    const RRF_RT_REG_SZ: u32 = 0x0000_0002;
    const RRF_RT_REG_EXPAND_SZ: u32 = 0x0000_0004;
    const RRF_RT_REG_DWORD: u32 = 0x0000_0010;
    const RRF_NOEXPAND: u32 = 0x1000_0000;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Open the service with `access`, run `f`, close everything.
    fn with_service<T>(access: u32, f: impl FnOnce(isize) -> T) -> Result<T, u32> {
        unsafe {
            let scm = OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT);
            if scm == 0 {
                return Err(GetLastError());
            }
            let name = wide(super::SERVICE);
            let svc = OpenServiceW(scm, name.as_ptr(), access);
            if svc == 0 {
                let e = GetLastError();
                CloseServiceHandle(scm);
                return Err(e);
            }
            let out = f(svc);
            CloseServiceHandle(svc);
            CloseServiceHandle(scm);
            Ok(out)
        }
    }

    pub fn state() -> ServiceState {
        let r = with_service(SERVICE_QUERY_STATUS, |svc| unsafe {
            let mut st = ServiceStatus::default();
            if QueryServiceStatus(svc, &mut st) == 0 {
                return ServiceState::Unknown;
            }
            match st.current_state {
                1 => ServiceState::Stopped,
                2 => ServiceState::Starting,
                3 => ServiceState::Stopping,
                4 => ServiceState::Running,
                _ => ServiceState::Other,
            }
        });
        match r {
            Ok(s) => s,
            Err(ERROR_SERVICE_DOES_NOT_EXIST) => ServiceState::NotInstalled,
            Err(_) => ServiceState::Unknown,
        }
    }

    pub fn start() -> Result<(), String> {
        let r = with_service(SERVICE_START, |svc| unsafe {
            if StartServiceW(svc, 0, std::ptr::null()) != 0 {
                return Ok(());
            }
            match GetLastError() {
                ERROR_SERVICE_ALREADY_RUNNING => Ok(()),
                e => Err(e),
            }
        });
        match r {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) | Err(e) => Err(format!("启动服务失败(错误 {e})")),
        }
    }

    pub fn stop() -> Result<(), String> {
        let r = with_service(SERVICE_STOP, |svc| unsafe {
            let mut st = ServiceStatus::default();
            ControlService(svc, SERVICE_CONTROL_STOP, &mut st) != 0
        });
        match r {
            Ok(_) => Ok(()),
            Err(e) => Err(format!("停止服务失败(错误 {e})")),
        }
    }

    fn reg_string(sub_key: &str, value: &str) -> Option<String> {
        let sub = wide(sub_key);
        let val = wide(value);
        let mut buf = vec![0u16; 1024];
        let mut size = (buf.len() * 2) as u32;
        let mut kind = 0u32;
        let rc = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                sub.as_ptr(),
                val.as_ptr(),
                RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND,
                &mut kind,
                buf.as_mut_ptr() as *mut u8,
                &mut size,
            )
        };
        if rc != 0 {
            return None;
        }
        let n = (size as usize / 2).min(buf.len());
        let s = String::from_utf16_lossy(&buf[..n]);
        Some(s.trim_end_matches('\0').to_string())
    }

    pub fn reg_dword(sub_key: &str, value: &str) -> Option<u32> {
        let sub = wide(sub_key);
        let val = wide(value);
        let mut data = 0u32;
        let mut size = 4u32;
        let mut kind = 0u32;
        let rc = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                sub.as_ptr(),
                val.as_ptr(),
                RRF_RT_REG_DWORD,
                &mut kind,
                &mut data as *mut u32 as *mut u8,
                &mut size,
            )
        };
        (rc == 0).then_some(data)
    }

    /// The executable the service is registered with, quotes stripped.
    pub fn image_path() -> Option<String> {
        let raw = reg_string(&format!(r"SYSTEM\CurrentControlSet\Services\{}", super::SERVICE), "ImagePath")?;
        let raw = raw.trim();
        let path = if let Some(rest) = raw.strip_prefix('"') {
            rest.split('"').next().unwrap_or("").to_string()
        } else {
            raw.split(" --").next().unwrap_or(raw).to_string()
        };
        (!path.is_empty()).then_some(path)
    }

    pub fn pawnio_installed() -> bool {
        const KEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\PawnIO";
        // The installer's entry can be missing while the driver is installed
        // and working; the kernel service key is the real thing.
        reg_string(KEY, "DisplayVersion").is_some()
            || reg_string(r"SYSTEM\CurrentControlSet\Services\PawnIO", "ImagePath").is_some()
    }

    // ---- elevated repair -------------------------------------------------

    #[repr(C)]
    struct ShellExecuteInfoW {
        cb_size: u32,
        f_mask: u32,
        hwnd: isize,
        verb: *const u16,
        file: *const u16,
        parameters: *const u16,
        directory: *const u16,
        show: i32,
        inst_app: isize,
        id_list: *mut core::ffi::c_void,
        class: *const u16,
        hkey_class: isize,
        hot_key: u32,
        icon_or_monitor: isize,
        process: isize,
    }

    #[link(name = "shell32")]
    extern "system" {
        fn ShellExecuteExW(info: *mut ShellExecuteInfoW) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn WaitForSingleObject(handle: isize, ms: u32) -> u32;
        fn GetExitCodeProcess(handle: isize, code: *mut u32) -> i32;
        fn CloseHandle(handle: isize) -> i32;
    }

    const SEE_MASK_NOCLOSEPROCESS: u32 = 0x0000_0040;
    const SEE_MASK_NOASYNC: u32 = 0x0000_0100;
    const SW_HIDE: i32 = 0;
    const ERROR_CANCELLED: u32 = 1223;
    const WAIT_TIMEOUT: u32 = 258;

    /// Run the setup script elevated -- the one UAC prompt -- and wait for it.
    pub fn run_elevated_setup(script: &std::path::Path) -> Result<(), String> {
        let verb = wide("runas");
        let file = wide("powershell.exe");
        let params = wide(&format!(
            "-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File \"{}\" install",
            script.display()
        ));
        let mut info = ShellExecuteInfoW {
            cb_size: std::mem::size_of::<ShellExecuteInfoW>() as u32,
            f_mask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
            hwnd: 0,
            verb: verb.as_ptr(),
            file: file.as_ptr(),
            parameters: params.as_ptr(),
            directory: std::ptr::null(),
            show: SW_HIDE,
            inst_app: 0,
            id_list: std::ptr::null_mut(),
            class: std::ptr::null(),
            hkey_class: 0,
            hot_key: 0,
            icon_or_monitor: 0,
            process: 0,
        };
        unsafe {
            if ShellExecuteExW(&mut info) == 0 {
                return Err(match GetLastError() {
                    ERROR_CANCELLED => "已取消(Windows 授权弹窗里需要点“是”)".to_string(),
                    e => format!("无法以管理员身份运行(错误 {e})"),
                });
            }
            if info.process == 0 {
                return Ok(());
            }
            // Installing the driver can take a while on a slow machine.
            let waited = WaitForSingleObject(info.process, 180_000);
            let mut code = 0u32;
            GetExitCodeProcess(info.process, &mut code);
            CloseHandle(info.process);
            if waited == WAIT_TIMEOUT {
                return Err("修复超时(3 分钟没有完成)".into());
            }
            if code != 0 {
                return Err(format!("修复脚本出错(退出码 {code})"));
            }
        }
        Ok(())
    }
}

#[cfg(windows)]
pub fn service_state() -> ServiceState {
    scm::state()
}
#[cfg(not(windows))]
pub fn service_state() -> ServiceState {
    ServiceState::NotInstalled
}

/// Start the service. Allowed without elevation: the installer grants the
/// signed-in user start/stop rights on this one service.
#[cfg(windows)]
pub fn start_service() -> Result<(), String> {
    scm::start()
}
#[cfg(not(windows))]
pub fn start_service() -> Result<(), String> {
    Err("not supported".into())
}

/// Stop, wait for it to stop, start again. For a service that is running
/// but has stopped answering.
#[cfg(windows)]
pub fn restart_service() -> Result<(), String> {
    scm::stop()?;
    for _ in 0..40 {
        if matches!(scm::state(), ServiceState::Stopped | ServiceState::NotInstalled) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    scm::start()
}
#[cfg(not(windows))]
pub fn restart_service() -> Result<(), String> {
    Err("not supported".into())
}

#[cfg(windows)]
pub fn pawnio_installed() -> bool {
    scm::pawnio_installed()
}
#[cfg(not(windows))]
pub fn pawnio_installed() -> bool {
    false
}

/// .NET Framework 4.7.2 or later (release key 461808), which the service
/// runs on. Built into Windows 10 1803+ and every Windows 11.
#[cfg(windows)]
pub fn dotnet_ok() -> bool {
    scm::reg_dword(r"SOFTWARE\Microsoft\NET Framework Setup\NDP\v4\Full", "Release")
        .map(|r| r >= 461_808)
        .unwrap_or(false)
}
#[cfg(not(windows))]
pub fn dotnet_ok() -> bool {
    true
}

/// The setup script to repair with.
///
/// The directory the service is registered from comes first: a development
/// build (`cargo tauri dev`) carries its own copy in target\debug, and
/// "repairing" from there would point the service at build output that the
/// next compile tries to overwrite. The copy next to this executable is the
/// fallback -- a fresh install, or a service that was removed.
pub fn setup_script(resource_dir: Option<PathBuf>) -> Option<PathBuf> {
    #[cfg(windows)]
    if let Some(exe) = scm::image_path() {
        let script = PathBuf::from(exe).with_file_name(SETUP_SCRIPT);
        if script.exists() {
            return Some(script);
        }
    }
    let script = resource_dir?.join("sensors").join(SETUP_SCRIPT);
    script.exists().then_some(script)
}

/// Repair: run the setup script the installer ran (driver, service, recovery,
/// permissions, start). One UAC prompt; blocks until it has finished.
pub fn repair(script: &std::path::Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        scm::run_elevated_setup(script)
    }
    #[cfg(not(windows))]
    {
        let _ = script;
        Err("not supported".into())
    }
}

/// Open a web page in the default browser (tutorial links on the settings
/// page). Only http(s), so a page can never make this run anything else.
pub fn open_url(url: &str) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("only web links".into());
    }
    #[cfg(windows)]
    {
        // Elevated, a browser started directly would run as administrator
        // too; hand the link to Explorer instead.
        if crate::toolkit::win::elevation::is_elevated() {
            return crate::toolkit::win::actions::open_as_user(url);
        }
        quiet(std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", url]))
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(not(windows))]
    {
        let _ = url;
        Ok(())
    }
}

#[cfg_attr(windows, allow(dead_code))]
pub fn autostart_enabled() -> bool {
    quiet(std::process::Command::new("reg").args(["query", RUN_KEY, "/v", RUN_VALUE]))
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn set_autostart(on: bool) -> Result<(), String> {
    let status = if on {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let value = format!("\"{}\"", exe.to_string_lossy());
        quiet(std::process::Command::new("reg").args(["add", RUN_KEY, "/v", RUN_VALUE, "/t", "REG_SZ", "/d", &value, "/f"]))
            .status()
    } else {
        quiet(std::process::Command::new("reg").args(["delete", RUN_KEY, "/v", RUN_VALUE, "/f"])).status()
    };
    match status {
        Ok(s) if s.success() || !on => Ok(()),
        Ok(s) => Err(format!("reg exited with {s}")),
        Err(e) => Err(e.to_string()),
    }
}
