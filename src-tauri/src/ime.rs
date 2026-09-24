//! 设置页的「输入法」栏：地球桌面输入法本身是独立的程序（ime\ 目录：
//! EarthDeskIME.exe 引擎 + TSF 模块，见 ime/ 工作区），这里只负责
//!   - 看它装没装、引擎在不在跑（通过引擎的命名管道问 Status）；
//!   - 保存设置到 %APPDATA%\EarthDesk\ime\settings.json 并让引擎重新部署；
//!   - 打开系统的输入法设置、用户词库文件夹、日志。
//! 我们自己常以管理员身份运行，所以启动引擎一律经由 Explorer（普通权限）。

use ime_proto::{Reply, Request, Settings};
use serde::Serialize;
use std::path::PathBuf;
use std::time::Duration;

fn base() -> PathBuf {
    std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_default().join("EarthDesk").join("ime")
}

fn settings_path() -> PathBuf {
    base().join("settings.json")
}

fn load_settings() -> Settings {
    std::fs::read(settings_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

#[derive(Serialize)]
pub struct EngineStatus {
    librime: String,
    /// Japanese engine's data version; empty = not installed.
    mozc: String,
    maintaining: bool,
    clients: u32,
}

#[derive(Serialize)]
pub struct ImeStatus {
    /// Registered with Windows (the 64-bit DLL's path), None = not installed.
    dll: Option<String>,
    /// 32-bit programs covered too.
    dll32: bool,
    engine: Option<EngineStatus>,
    settings: Settings,
    user_dir: String,
}

#[cfg(windows)]
mod win {
    use windows::core::HSTRING;
    use windows::Win32::System::Registry::*;

    /// The DLL registered for our class id in the 64-bit (or 32-bit)
    /// registry view.
    pub fn registered_dll(wow64_32: bool) -> Option<String> {
        let key = HSTRING::from(format!(r"SOFTWARE\Classes\CLSID\{}\InprocServer32", ime_proto::CLSID_STR));
        let view = if wow64_32 { RRF_SUBKEY_WOW6432KEY } else { RRF_SUBKEY_WOW6464KEY };
        let mut buf = [0u16; 1024];
        let mut len = (buf.len() * 2) as u32;
        let r = unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, &key, None, RRF_RT_REG_SZ | view, None, Some(buf.as_mut_ptr() as *mut _), Some(&mut len)) };
        if r.is_err() {
            return None;
        }
        let n = (len as usize / 2).saturating_sub(1).min(buf.len());
        let s = String::from_utf16_lossy(&buf[..n]);
        (!s.is_empty()).then_some(s)
    }

    pub fn session_id() -> u32 {
        let mut sid = 0u32;
        unsafe {
            let _ = windows::Win32::System::RemoteDesktop::ProcessIdToSessionId(std::process::id(), &mut sid);
        }
        sid
    }
}

/// One request to the running engine; None if it is not running (or does
/// not answer within a second).
fn ask(req: Request) -> Option<Reply> {
    #[cfg(windows)]
    {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let r = (|| {
                let mut f = std::fs::OpenOptions::new().read(true).write(true).open(ime_proto::pipe_name(win::session_id())).ok()?;
                ime_proto::write_frame(&mut f, &req).ok()?;
                ime_proto::read_frame::<_, Reply>(&mut f).ok()
            })();
            let _ = tx.send(r);
        });
        rx.recv_timeout(Duration::from_secs(1)).ok().flatten()
    }
    #[cfg(not(windows))]
    {
        let _ = (req, Duration::ZERO);
        None
    }
}

fn engine_exe() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let dll = win::registered_dll(false)?;
        let exe = std::path::Path::new(&dll).parent()?.join("EarthDeskIME.exe");
        exe.exists().then_some(exe)
    }
    #[cfg(not(windows))]
    None
}

#[tauri::command]
pub fn ime_status() -> ImeStatus {
    #[cfg(windows)]
    let (dll, dll32) = (win::registered_dll(false), win::registered_dll(true).is_some());
    #[cfg(not(windows))]
    let (dll, dll32) = (None, false);
    let engine = match ask(Request::Status) {
        Some(Reply::Status { librime, mozc, maintaining, clients }) => Some(EngineStatus { librime, mozc, maintaining, clients }),
        _ => None,
    };
    ImeStatus { dll, dll32, engine, settings: load_settings(), user_dir: base().join("rime").to_string_lossy().into_owned() }
}

/// Save the settings; the engine rewrites its Rime patches and redeploys
/// (a few seconds; typing passes through meanwhile).
#[tauri::command]
pub fn ime_save(settings: Settings) -> Result<bool, String> {
    let mut s = settings;
    s.page_size = s.page_size.clamp(3, 9);
    s.ascii_apps = s.ascii_apps.into_iter().map(|a| a.trim().to_lowercase()).filter(|a| !a.is_empty()).collect();
    std::fs::create_dir_all(base()).map_err(|e| e.to_string())?;
    let json = serde_json::to_vec_pretty(&s).map_err(|e| e.to_string())?;
    std::fs::write(settings_path(), json).map_err(|e| format!("保存失败：{e}"))?;
    Ok(matches!(ask(Request::Deploy), Some(Reply::Ok)))
}

/// Rebuild the dictionaries (after editing the quick phrases by hand, say).
#[tauri::command]
pub fn ime_deploy() -> bool {
    matches!(ask(Request::Deploy), Some(Reply::Ok))
}

#[tauri::command]
pub fn ime_start() -> Result<(), String> {
    let exe = engine_exe().ok_or("输入法没有安装")?;
    #[cfg(windows)]
    return crate::toolkit::win::actions::open_as_user(&exe.to_string_lossy());
    #[cfg(not(windows))]
    {
        let _ = exe;
        Err("只支持 Windows".into())
    }
}

/// "keyboards": Windows 的 语言和区域 设置（在那里添加 / 删除键盘）;
/// "userdir": 用户词库和快捷短语; "phrases": 快捷短语文件; "log".
#[tauri::command]
pub fn ime_open(what: String) -> Result<(), String> {
    let target = match what.as_str() {
        "keyboards" => "ms-settings:regionlanguage".to_string(),
        "userdir" => base().join("rime").to_string_lossy().into_owned(),
        "phrases" => base().join("rime").join("custom_phrase_double.txt").to_string_lossy().into_owned(),
        "log" => base().join("ime.log").to_string_lossy().into_owned(),
        _ => return Err("unknown".into()),
    };
    #[cfg(windows)]
    return crate::toolkit::win::actions::open_as_user(&target);
    #[cfg(not(windows))]
    {
        let _ = target;
        Ok(())
    }
}
