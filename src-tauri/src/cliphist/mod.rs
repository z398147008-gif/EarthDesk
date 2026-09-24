//! 剪贴板历史: every copy is recorded (text, images, files) and can be
//! pasted again from a pop-up panel (Alt+V) or managed in a window.
//!
//! - `store`: SQLite + image files (plain Rust).
//! - the watcher lives in toolkit/win/clipboard.rs (`start_monitor`,
//!   `read_snapshot`); each change is read in one short open/close and
//!   processed (PNG encode, thumbnail) on a worker so the program that
//!   copied is never held up.
//! - `clippanel` is a warm hidden window placed at the caret / pointer;
//!   the manager window is created the first time it is opened.

pub mod store;

use serde_json::{json, Value};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Mutex;
use store::{New, Store};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

pub const PANEL: &str = "clippanel";
pub const MANAGER: &str = "cliphistory";

static STORE: Mutex<Option<Store>> = Mutex::new(None);
/// The window that was in front when the panel opened: where a pick pastes.
static TARGET: AtomicIsize = AtomicIsize::new(0);

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn dir() -> std::path::PathBuf {
    crate::config::config_path().with_file_name("clipboard")
}

fn with_store<T>(f: impl FnOnce(&Store) -> Result<T, String>) -> Result<T, String> {
    let mut g = STORE.lock().map_err(|e| e.to_string())?;
    if g.is_none() {
        *g = Some(Store::open(&dir()).map_err(|e| format!("剪贴板历史数据库打不开：{e}"))?);
    }
    f(g.as_ref().unwrap())
}

pub fn init(app: &AppHandle) {
    let _ = APP.set(app.clone());
    #[cfg(windows)]
    crate::toolkit::win::clipboard::start_monitor(on_clipboard_change);
    // Prune once at start-up (limits may have changed while we were off).
    std::thread::spawn(|| {
        let c = crate::toolkit::current().clipboard.clone();
        let _ = with_store(|s| s.prune(c.max_items, c.max_days, now_ms()).map_err(|e| e.to_string()));
    });
}

static APP: std::sync::OnceLock<AppHandle> = std::sync::OnceLock::new();

#[cfg(windows)]
fn on_clipboard_change() {
    use crate::toolkit::win::clipboard::{png_size, read_snapshot, Image};
    let cfg = crate::toolkit::current().clipboard.clone();
    if !cfg.enabled {
        return;
    }
    let max_px = cfg.max_image_mb as u64 * 1024 * 1024 / 4;
    let Some(snap) = read_snapshot(max_px, cfg.images) else { return };
    if !snap.owner.is_empty() && cfg.exclude_apps.iter().any(|a| *a == snap.owner) {
        return;
    }
    // Encoding and the database happen off the clipboard thread.
    std::thread::spawn(move || {
        let item = if let Some(files) = snap.files {
            Some(New::Files(files))
        } else if let Some(text) = snap.text {
            Some(New::Text { text, html: snap.html })
        } else {
            match snap.image {
                Some(Image::Png(png)) => {
                    let (w, h) = png_size(&png).unwrap_or((0, 0));
                    let thumb = store::decode_png(&png).and_then(|(w, h, px)| {
                        let (tw, th, t) = store::thumbnail(w, h, &px, 320);
                        crate::capture::encode::png(tw, th, &t, true).ok()
                    });
                    thumb.map(|thumb| New::Image { png, thumb, width: w, height: h })
                }
                Some(Image::Rgba(w, h, px)) => {
                    let png = crate::capture::encode::png(w, h, &px, true).ok();
                    let (tw, th, t) = store::thumbnail(w, h, &px, 320);
                    let thumb = crate::capture::encode::png(tw, th, &t, true).ok();
                    match (png, thumb) {
                        (Some(png), Some(thumb)) => Some(New::Image { png, thumb, width: w, height: h }),
                        _ => None,
                    }
                }
                None => None,
            }
        };
        let Some(item) = item else { return };
        let r = with_store(|s| {
            let r = s.add(item, &snap.owner, now_ms()).map_err(|e| e.to_string())?;
            if r.1 {
                s.prune(cfg.max_items, cfg.max_days, now_ms()).map_err(|e| e.to_string())?;
            }
            Ok(r)
        });
        match r {
            Ok(_) => {
                if let Some(app) = APP.get() {
                    let _ = app.emit("clip:changed", ());
                }
            }
            Err(e) => crate::toolkit::log(&format!("clipboard history: {e}")),
        }
    });
}

// --- opening the panel and the manager ---------------------------------------------------

/// The `clipboard_panel` built-in (Alt+V). `target` = window in front.
pub fn open_panel(app: &AppHandle, target: isize) {
    let Some(panel) = app.get_webview_window(PANEL) else { return };
    #[cfg(windows)]
    {
        let fg = if target != 0 { target } else { crate::toolkit::win::apps::foreground().0 as isize };
        TARGET.store(fg, Ordering::SeqCst);
        if let Ok(h) = panel.hwnd() {
            let (x, y, w, h2) = panel_rect();
            crate::capture::win::set_rect(h.0 as isize, x, y, w, h2);
            let _ = panel.show();
            crate::capture::win::force_foreground(h.0 as isize);
            let _ = panel.set_focus();
        }
    }
    #[cfg(not(windows))]
    {
        let _ = target;
        let _ = panel.show();
    }
    let _ = panel.emit("clip:open", ());
}

/// Next to the text caret when the program in front reports one, else
/// at the pointer; kept inside that monitor's work area.
#[cfg(windows)]
fn panel_rect() -> (i32, i32, i32, i32) {
    use windows::Win32::Foundation::{POINT, RECT};
    use windows::Win32::Graphics::Gdi::{ClientToScreen, GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
    use windows::Win32::UI::WindowsAndMessaging::{GetGUIThreadInfo, GUITHREADINFO};
    unsafe {
        let mut anchor = crate::capture::win::cursor_pos().map(|(x, y)| POINT { x, y }).unwrap_or_default();
        let mut gi = GUITHREADINFO { cbSize: std::mem::size_of::<GUITHREADINFO>() as u32, ..Default::default() };
        if GetGUIThreadInfo(0, &mut gi).is_ok() && !gi.hwndCaret.0.is_null() {
            let r: RECT = gi.rcCaret;
            let mut p = POINT { x: r.left, y: r.bottom };
            if ClientToScreen(gi.hwndCaret, &mut p).as_bool() && (r.right - r.left) < 50 {
                anchor = p;
            }
        }
        let mon = MonitorFromPoint(anchor, MONITOR_DEFAULTTONEAREST);
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        let s = dx as f64 / 96.0;
        let (w, h) = ((380.0 * s) as i32, (520.0 * s) as i32);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut mi);
        let wa = mi.rcWork;
        let mut x = anchor.x + (8.0 * s) as i32;
        let mut y = anchor.y + (8.0 * s) as i32;
        if x + w > wa.right {
            x = (anchor.x - w).max(wa.left);
        }
        if y + h > wa.bottom {
            y = (anchor.y - h - (16.0 * s) as i32).max(wa.top);
        }
        (x.max(wa.left), y.max(wa.top), w, h)
    }
}

pub fn open_manager(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(MANAGER) {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        let _ = w.emit("clip:open", ());
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let r = WebviewWindowBuilder::new(&app, MANAGER, WebviewUrl::App("clip.html?mode=manager".into()))
            .title("剪贴板历史")
            .inner_size(920.0, 640.0)
            .min_inner_size(560.0, 400.0)
            .center()
            .visible(true)
            .build();
        if let Err(e) = r {
            crate::toolkit::log(&format!("clipboard manager window failed: {e}"));
        }
    });
}

fn hide_panel(app: &AppHandle) {
    if let Some(p) = app.get_webview_window(PANEL) {
        let _ = p.hide();
    }
}

/// Put entry `id` on the clipboard (marked as ours, so it is not recorded
/// again -- it just moves to the top).
fn put_back(id: i64, plain: bool) -> Result<(), String> {
    let full = with_store(|s| s.get(id).map_err(|e| e.to_string()))?.ok_or("这条记录已经不在了")?;
    #[cfg(windows)]
    {
        use crate::toolkit::win::clipboard as cb;
        match full.kind.as_str() {
            "text" => {
                let html = if plain { None } else { full.html.as_deref() };
                cb::set_text_html(full.text.as_deref().unwrap_or(""), html, true)?;
            }
            "files" => cb::set_files(&full.files.unwrap_or_default(), true)?,
            _ => {
                let path = with_store(|s| Ok(s.image_path(&full.hash)))?;
                let png = std::fs::read(&path).map_err(|_| "图片文件丢失了".to_string())?;
                let px = store::decode_png(&png);
                cb::set_png(&png, px.as_ref().map(|(w, h, p)| (*w, *h, p.as_slice())), true)?;
            }
        }
    }
    #[cfg(not(windows))]
    let _ = (full, plain);
    with_store(|s| s.touch(id, now_ms()).map_err(|e| e.to_string()))
}

// --- commands --------------------------------------------------------------------------

#[tauri::command]
pub async fn clip_list(query: String, kind: String, offset: i64, limit: i64) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        // Most recent first: the entry you want is almost always one of the
        // last few. Starred entries have their own filter.
        let items = with_store(|s| s.list(&query, &kind, offset, limit.clamp(1, 500), false).map_err(|e| e.to_string()))?;
        let total = with_store(|s| Ok(s.count()))?;
        Ok(json!({ "items": items, "total": total }))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn clip_get(id: i64) -> Result<Value, String> {
    let f = with_store(|s| s.get(id).map_err(|e| e.to_string()))?.ok_or("这条记录已经不在了")?;
    Ok(json!({ "kind": f.kind, "text": f.text, "files": f.files, "width": f.width, "height": f.height, "has_html": f.html.is_some() }))
}

#[tauri::command]
pub async fn clip_thumb(id: i64, full: bool) -> Result<tauri::ipc::Response, String> {
    let f = with_store(|s| s.get(id).map_err(|e| e.to_string()))?.ok_or("这条记录已经不在了")?;
    let p = with_store(|s| Ok(if full { s.image_path(&f.hash) } else { s.thumb_path(&f.hash) }))?;
    let bytes = std::fs::read(p).map_err(|_| "图片文件丢失了".to_string())?;
    Ok(tauri::ipc::Response::new(bytes))
}

/// Pick in the panel: put it on the clipboard, go back to the window that
/// was in front, and paste there (unless turned off in settings).
#[tauri::command]
pub async fn clip_paste(app: AppHandle, id: i64, plain: bool) -> Result<(), String> {
    hide_panel(&app);
    tauri::async_runtime::spawn_blocking(move || {
        put_back(id, plain)?;
        #[cfg(windows)]
        {
            let target = TARGET.load(Ordering::SeqCst);
            if target != 0 {
                crate::toolkit::win::apps::force_foreground(windows::Win32::Foundation::HWND(target as *mut _));
            }
            if crate::toolkit::current().clipboard.paste_on_pick {
                std::thread::sleep(std::time::Duration::from_millis(80));
                if let Some(c) = crate::toolkit::keys::Combo::parse("Ctrl+V") {
                    crate::toolkit::win::input::press(c);
                }
            }
        }
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn clip_copy(id: i64, plain: bool) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || put_back(id, plain)).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn clip_delete(app: AppHandle, id: i64) -> Result<(), String> {
    with_store(|s| s.delete(id).map_err(|e| e.to_string()))?;
    let _ = app.emit("clip:changed", ());
    Ok(())
}

#[tauri::command]
pub fn clip_set_pinned(app: AppHandle, id: i64, on: bool) -> Result<(), String> {
    with_store(|s| s.set_pinned(id, on).map_err(|e| e.to_string()))?;
    let _ = app.emit("clip:changed", ());
    Ok(())
}

#[tauri::command]
pub fn clip_clear(app: AppHandle) -> Result<usize, String> {
    let n = with_store(|s| s.clear().map_err(|e| e.to_string()))?;
    let _ = app.emit("clip:changed", ());
    Ok(n)
}

/// 贴到屏幕: an entry as a pin (images as images, text as a text card).
#[tauri::command]
pub fn clip_pin(app: AppHandle, id: i64) -> Result<(), String> {
    let f = with_store(|s| s.get(id).map_err(|e| e.to_string()))?.ok_or("这条记录已经不在了")?;
    let src = match f.kind.as_str() {
        "image" => {
            let p = with_store(|s| Ok(s.image_path(&f.hash)))?;
            let png = std::fs::read(p).map_err(|_| "图片文件丢失了".to_string())?;
            crate::pins::Source::Encoded(std::sync::Arc::new(png), "image/png".into())
        }
        "files" => crate::pins::Source::Text(f.files.unwrap_or_default().join("\n")),
        _ => crate::pins::Source::Text(f.text.unwrap_or_default()),
    };
    hide_panel(&app);
    crate::pins::create(&app, src, None).map(|_| ())
}

#[tauri::command]
pub fn clip_hide_panel(app: AppHandle) {
    hide_panel(&app);
}

#[tauri::command]
pub fn clip_open_manager(app: AppHandle) {
    hide_panel(&app);
    open_manager(&app);
}

#[tauri::command]
pub fn clip_open_folder() {
    let d = dir();
    let _ = std::fs::create_dir_all(&d);
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(&d).spawn();
}
