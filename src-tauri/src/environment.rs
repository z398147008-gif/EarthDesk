//! Things a friend's freshly installed copy needs so it works with nothing
//! else set up: the bundled hardware monitor (LibreHardwareMonitor, for
//! temperatures, fans and GPU load) and starting with Windows.
//!
//! The installer (windows/hooks.nsh) does the privileged part once: installs
//! the PawnIO driver silently, registers a logon task that starts the monitor
//! elevated -- so there is no UAC prompt at every boot -- and blocks it in the
//! firewall so its local web server is not reachable from the network. This
//! file only has to make sure it is running and offer a repair path.

use std::path::PathBuf;
#[cfg(windows)]
use std::os::windows::process::CommandExt;

pub const LHM_TASK: &str = "EarthDesk-LHM";
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "EarthDesk";
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn quiet(cmd: &mut std::process::Command) -> &mut std::process::Command {
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

/// Where the installer put LibreHardwareMonitor, if it did.
pub fn bundled_lhm(resource_dir: Option<PathBuf>) -> Option<PathBuf> {
    let exe = resource_dir?.join("lhm").join("LibreHardwareMonitor.exe");
    exe.exists().then_some(exe)
}

/// Is something answering on the monitor's port?
pub fn lhm_reachable(url: &str) -> bool {
    let hostport = url
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .split('/')
        .next()
        .unwrap_or("127.0.0.1:8085")
        .to_string();
    use std::net::ToSocketAddrs;
    let Ok(mut addrs) = hostport.to_socket_addrs() else { return false };
    addrs.any(|a| std::net::TcpStream::connect_timeout(&a, std::time::Duration::from_millis(400)).is_ok())
}

/// Start the monitor through the logon task (no prompt). Returns whether the
/// task exists and was asked to run.
pub fn start_lhm_task() -> bool {
    quiet(std::process::Command::new("schtasks").args(["/Run", "/TN", LHM_TASK]))
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Repair: run the same setup script the installer ran (driver, logon task,
/// firewall, start). Needs administrator rights, so this is one UAC prompt.
pub fn repair_lhm(exe: &PathBuf) -> Result<(), String> {
    let script = exe.with_file_name("setup-lhm.ps1");
    if !script.exists() {
        return Err("没有找到 setup-lhm.ps1，请重新安装地球桌面".into());
    }
    let path = script.to_string_lossy().replace('\'', "''");
    let outer = format!(
        "Start-Process powershell -Verb RunAs -WindowStyle Hidden -ArgumentList \
         '-NoProfile','-ExecutionPolicy','Bypass','-WindowStyle','Hidden','-File','\"{path}\"','install'"
    );
    quiet(std::process::Command::new("powershell").args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &outer]))
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Open a web page in the default browser (tutorial links on the settings
/// page). Only http(s), so a page can never make this run anything else.
pub fn open_url(url: &str) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("only web links".into());
    }
    #[cfg(windows)]
    {
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
