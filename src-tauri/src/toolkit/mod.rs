//! 效率工具: mouse gestures, global hotkeys, and (in later parts) screenshots,
//! pins and the clipboard history.
//!
//! Layout:
//! - `config`   toolkit.json and rule lookup (plain Rust, unit-tested)
//! - `gesture`  pointer path -> "↓→" (plain Rust, unit-tested)
//! - `keys`     "Ctrl+Alt+T" <-> virtual-key codes (plain Rust, unit-tested)
//! - `win::*`   everything that talks to Windows:
//!     - `hook`      low-level mouse/keyboard hooks on their own thread
//!     - `trail`     the native layered window that draws the gesture
//!     - `actions`   carries out a rule on a worker thread (COM, SendInput)
//!     - `input`     SendInput helpers
//!     - `apps`      window -> program, full-screen test, running programs
//!     - `elevation` running as administrator through a scheduled task
//!     - `dialog`    the "choose a program" file dialog
//!
//! The hooks never block: they look the rule up and hand it to the worker
//! thread, because Windows silently removes a low-level hook that keeps the
//! system waiting.

pub mod config;
pub mod gesture;
pub mod keys;
#[cfg(windows)]
pub mod win;

use config::{Action, ToolkitConfig};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, RwLock};
use tauri::{AppHandle, Emitter};

static CONFIG: RwLock<Option<Arc<ToolkitConfig>>> = RwLock::new(None);
static APP: OnceLock<AppHandle> = OnceLock::new();
/// Set while the settings page is recording a key combination: the next
/// combination pressed is reported to the page (`toolkit:recorded`) and
/// swallowed, instead of firing a hotkey or reaching Windows. Recording in the
/// hook rather than in the page is what lets it capture Win+… and F1.
pub static RECORDING: AtomicBool = AtomicBool::new(false);
/// "暂停手势和快捷键" in the tray: everything passes straight through until
/// it is unticked (or the app restarts).
pub static SUSPENDED: AtomicBool = AtomicBool::new(false);

/// EarthDesk's own functions a rule can call, with the names the settings
/// page shows. Window functions act on the window the gesture started over
/// (or, for a hotkey, the window in front).
pub const BUILTINS: &[(&str, &str, &str)] = &[
    ("capture", "截图", "截图"),
    ("pin_clipboard", "截图", "把剪贴板贴到屏幕"),
    ("hide_pins", "截图", "隐藏 / 显示所有贴图"),
    ("clipboard_panel", "剪贴板", "剪贴板历史（弹出面板）"),
    ("clipboard_history", "剪贴板", "剪贴板历史（管理窗口）"),
    ("window_close", "窗口", "关闭窗口"),
    ("window_minimize", "窗口", "最小化"),
    ("window_maximize", "窗口", "最大化 / 还原"),
    ("window_topmost", "窗口", "置顶 / 取消置顶"),
    ("show_desktop", "系统", "显示桌面"),
    ("task_view", "系统", "任务视图"),
    ("lock", "系统", "锁定电脑"),
    ("media_play_pause", "媒体", "播放 / 暂停"),
    ("media_next", "媒体", "下一首"),
    ("media_prev", "媒体", "上一首"),
    ("volume_up", "媒体", "音量加"),
    ("volume_down", "媒体", "音量减"),
    ("volume_mute", "媒体", "静音"),
    ("edit_mode", "地球桌面", "组件编辑模式"),
    ("settings", "地球桌面", "打开设置"),
];

/// Built-ins whose part of the toolkit is not in this build yet; the settings
/// page does not offer them.
pub const NOT_YET: &[&str] = &[];

/// Built-ins that need the app (windows, pages) rather than Win32 alone.
pub const APP_BUILTINS: &[&str] =
    &["capture", "pin_clipboard", "hide_pins", "clipboard_panel", "clipboard_history", "edit_mode", "settings"];

pub fn current() -> Arc<ToolkitConfig> {
    if let Ok(g) = CONFIG.read() {
        if let Some(c) = g.as_ref() {
            return c.clone();
        }
    }
    Arc::new(ToolkitConfig::default())
}

fn store(cfg: ToolkitConfig) {
    let cfg = Arc::new(cfg);
    if let Ok(mut g) = CONFIG.write() {
        *g = Some(cfg.clone());
    }
    #[cfg(windows)]
    win::hook::apply(&cfg);
}

/// Load toolkit.json and start the hooks. Call from `setup`.
pub fn init(app: &AppHandle) {
    let _ = APP.set(app.clone());
    let cfg = if let Ok(g) = CONFIG.read() {
        g.clone()
    } else {
        None
    };
    // `preload` may already have read the file (it decides on elevation
    // before Tauri starts).
    let cfg = cfg.map(|c| (*c).clone()).unwrap_or_else(ToolkitConfig::load);
    store(cfg);
    #[cfg(windows)]
    {
        win::trail::start();
        win::actions::start();
        win::hook::start();
    }
}

/// Read toolkit.json before Tauri starts (main uses `elevate`).
pub fn preload() -> Arc<ToolkitConfig> {
    let cfg = ToolkitConfig::load();
    store(cfg);
    current()
}

/// Called by the worker for built-ins in APP_BUILTINS. main.rs registers
/// the handler, because those need its windows.
static APP_HANDLER: OnceLock<fn(&AppHandle, &str, isize)> = OnceLock::new();

pub fn on_app_builtin(f: fn(&AppHandle, &str, isize)) {
    let _ = APP_HANDLER.set(f);
}

pub fn run_app_builtin(name: &str, target: isize) {
    let (Some(app), Some(f)) = (APP.get(), APP_HANDLER.get()) else { return };
    let app2 = app.clone();
    let name = name.to_string();
    let f = *f;
    let _ = app.run_on_main_thread(move || f(&app2, &name, target));
}

pub fn log(line: &str) {
    crate::watchdog_log(&format!("[toolkit] {line}"));
}

// --- commands for the settings page ------------------------------------------

#[tauri::command]
pub fn toolkit_get() -> Value {
    let cfg = current();
    #[cfg(windows)]
    let (elevated, task) = (win::elevation::is_elevated(), win::elevation::task_installed());
    #[cfg(not(windows))]
    let (elevated, task) = (false, false);
    json!({
        "config": &*cfg,
        "elevated": elevated,
        "task": task,
        "dev": cfg!(debug_assertions),
        "builtins": BUILTINS.iter().filter(|(k, ..)| !NOT_YET.contains(k)).map(|(k, g, n)| json!({ "key": k, "group": g, "name": n })).collect::<Vec<_>>(),
    })
}

#[tauri::command]
pub fn toolkit_set(app: AppHandle, config: ToolkitConfig) -> Result<(), String> {
    let mut config = config;
    config.normalise();
    // `elevate` has its own command: flipping it has side effects (task).
    config.elevate = current().elevate;
    config.defaults_level = current().defaults_level.max(config.defaults_level);
    for r in &mut config.hotkeys.rules {
        match keys::Combo::parse(&r.keys) {
            Some(c) => r.keys = c.format(),
            None if r.enabled => return Err(format!("快捷键「{}」无法识别", r.keys)),
            None => {}
        }
    }
    config.save();
    store(config);
    let _ = app.emit("toolkit:changed", ());
    Ok(())
}

#[tauri::command]
pub fn toolkit_record(on: bool) {
    RECORDING.store(on, Ordering::SeqCst);
}

/// From the hook thread: a combination was recorded.
pub fn report_recorded(keys: String) {
    RECORDING.store(false, Ordering::SeqCst);
    if let Some(app) = APP.get() {
        let app = app.clone();
        std::thread::spawn(move || {
            let _ = app.emit("toolkit:recorded", keys);
        });
    }
}

#[tauri::command]
pub fn toolkit_running_apps() -> Value {
    #[cfg(windows)]
    {
        Value::Array(
            win::apps::running_apps()
                .into_iter()
                .map(|a| json!({ "exe": a.exe, "path": a.path, "title": a.title }))
                .collect(),
        )
    }
    #[cfg(not(windows))]
    Value::Array(vec![])
}

#[tauri::command]
pub async fn toolkit_pick_file() -> Option<String> {
    #[cfg(windows)]
    {
        tauri::async_runtime::spawn_blocking(win::dialog::pick_program).await.ok().flatten()
    }
    #[cfg(not(windows))]
    None
}

/// "试一下" on the rule editor: run an action now, against whatever window
/// was in front before the settings window.
#[tauri::command]
pub fn toolkit_try(action: Action) {
    #[cfg(windows)]
    win::actions::submit(action, 0, "试一下");
    #[cfg(not(windows))]
    let _ = action;
}

#[tauri::command]
pub async fn toolkit_set_elevate(on: bool) -> Result<Value, String> {
    let mut cfg = (*current()).clone();
    cfg.elevate = on;
    #[cfg(windows)]
    {
        let r = tauri::async_runtime::spawn_blocking(move || win::elevation::set_elevate(on))
            .await
            .map_err(|e| e.to_string())?;
        r?;
    }
    cfg.save();
    store(cfg);
    Ok(toolkit_get())
}
