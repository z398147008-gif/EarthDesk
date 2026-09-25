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
    let old = load_settings();
    let json = serde_json::to_vec_pretty(&s).map_err(|e| e.to_string())?;
    std::fs::write(settings_path(), json).map_err(|e| format!("保存失败：{e}"))?;
    // Only the look changed: the engine picks it up from the file by itself.
    if s.same_but_skin(&old) {
        return Ok(true);
    }
    Ok(matches!(ask(Request::Deploy), Some(Reply::Ok)))
}

/// The weather skin's data (the candidate window paints the coming hours):
/// this hour and the next ones from an Open-Meteo answer.
pub fn weather_for_ime(data: &serde_json::Value) -> Option<ime_proto::Weather> {
    let cur = data.get("current")?;
    let now = cur.get("time")?.as_str()?;
    let hour = now.get(..13)?; // "2026-09-25T13"
    let hourly = data.get("hourly")?;
    let times = hourly.get("time")?.as_array()?;
    let codes = hourly.get("weather_code")?.as_array()?;
    let days = hourly.get("is_day").and_then(|v| v.as_array());
    let start = times.iter().position(|t| t.as_str().map(|t| t.starts_with(hour)).unwrap_or(false))?;
    let mut hours = Vec::new();
    for i in start..(start + 12).min(times.len()) {
        let code = codes.get(i).and_then(|c| c.as_u64()).unwrap_or(3) as u8;
        let day = match days.and_then(|d| d.get(i)).and_then(|d| d.as_u64()) {
            Some(d) => d == 1,
            None => cur.get("is_day").and_then(|d| d.as_u64()).unwrap_or(1) == 1,
        };
        hours.push((code, day));
    }
    // What it is like right now beats the hourly guess for this hour.
    if let (Some(first), Some(code)) = (hours.first_mut(), cur.get("weather_code").and_then(|c| c.as_u64())) {
        first.0 = code as u8;
        first.1 = cur.get("is_day").and_then(|d| d.as_u64()).map(|d| d == 1).unwrap_or(first.1);
    }
    let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let temp = cur.get("temperature_2m").and_then(|t| t.as_f64()).unwrap_or(0.0) as f32;
    Some(ime_proto::Weather { at, temp, hours })
}

pub fn save_weather(data: &serde_json::Value) {
    let Some(w) = weather_for_ime(data) else { return };
    if std::fs::create_dir_all(base()).is_err() {
        return;
    }
    if let Ok(json) = serde_json::to_vec(&w) {
        let tmp = base().join("weather.json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, base().join("weather.json"));
        }
    }
}


/// Rebuild the dictionaries (after editing the quick phrases by hand, say).
#[tauri::command]
pub fn ime_deploy() -> bool {
    matches!(ask(Request::Deploy), Some(Reply::Ok))
}

/// Tray 「刷新所有组件」: the engine saves, quits and is started again (a
/// few seconds; programs reconnect by themselves). Programs that are
/// already open keep their input-method module until they are reopened.
pub fn restart_engine() {
    std::thread::spawn(|| {
        let was_running = matches!(ask(Request::Restart), Some(Reply::Ok));
        if was_running {
            std::thread::sleep(Duration::from_millis(1200));
        }
        let _ = ime_start();
    });
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

#[cfg(test)]
mod tests {
    #[test]
    fn weather_hours() {
        let data = serde_json::json!({
            "current": { "time": "2026-09-25T13:15", "weather_code": 61, "is_day": 1, "temperature_2m": 17.6 },
            "hourly": {
                "time": ["2026-09-25T12:00", "2026-09-25T13:00", "2026-09-25T14:00", "2026-09-25T19:00"],
                "weather_code": [3, 3, 0, 0],
                "is_day": [1, 1, 1, 0]
            }
        });
        let w = super::weather_for_ime(&data).unwrap();
        assert_eq!(w.hours, vec![(61, true), (0, true), (0, false)]);
        assert_eq!(w.label(3), "18° 小雨转晴");
    }
}
