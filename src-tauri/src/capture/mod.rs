//! 截图 (F1), in the manner of Snipaste.
//!
//! When the hotkey fires:
//! 1. copy the whole virtual desktop (GDI, tens of ms at 4K);
//! 2. show that image in a native "freeze" window at once, so the screen
//!    stops exactly as it was without waiting for a WebView;
//! 3. list windows and their child windows for snapping;
//! 4. the `capture` page (created at start-up and kept hidden, so it is
//!    warm) fetches the pixels and draws them; it is then shown under the
//!    freeze window, and the freeze window goes away.
//!
//! Everything interactive -- selection, magnifier, annotation -- lives in
//! capture.js. On copy or save the page sends the finished pixels back
//! (raw RGBA in the request body) and this module encodes them.
//!
//! Coordinates are physical screen pixels throughout.

pub mod encode;
#[cfg(windows)]
pub mod win;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager};

pub const LABEL: &str = "capture";

struct Session {
    id: u64,
    w: i32,
    h: i32,
    bgra: Arc<Vec<u8>>,
    preset: Option<Preset>,
}

/// Annotating a pin: the capture editor opens with the pin's pixels already
/// laid over the frozen screen and selected, and "done" puts the result
/// back into that pin.
#[derive(Clone)]
pub struct Preset {
    pub pin: u64,
    /// Screen position (physical) of the image's top-left.
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub rgba: Arc<Vec<u8>>,
}

static SESSION: Mutex<Option<Session>> = Mutex::new(None);
static BUSY: AtomicBool = AtomicBool::new(false);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Region {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

fn history_path() -> std::path::PathBuf {
    crate::config::config_path().with_file_name("capture-history.json")
}

fn load_history() -> Vec<Region> {
    std::fs::read_to_string(history_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn push_history(r: Region) {
    let keep = crate::toolkit::current().capture.history.max(1);
    let mut h = load_history();
    h.retain(|o| *o != r);
    h.push(r);
    if h.len() > keep {
        let cut = h.len() - keep;
        h.drain(..cut);
    }
    if let Ok(t) = serde_json::to_string(&h) {
        let _ = std::fs::write(history_path(), t);
    }
}

/// Called once from `setup`.
pub fn init(_app: &AppHandle) {
    #[cfg(windows)]
    win::start_freeze_thread();
}

/// Ends the session; returns the pin that was being edited, if any.
fn end_session(app: &AppHandle) -> Option<u64> {
    let preset = SESSION.lock().ok().and_then(|mut s| s.take()).and_then(|s| s.preset).map(|p| p.pin);
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.hide();
    }
    #[cfg(windows)]
    win::freeze_hide();
    BUSY.store(false, Ordering::SeqCst);
    preset
}

/// The `capture` built-in (F1 by default). Runs on the main thread.
pub fn start(app: &AppHandle) {
    start_with(app, None);
}

pub fn start_with(app: &AppHandle, preset: Option<Preset>) {
    if BUSY.swap(true, Ordering::SeqCst) {
        if let Some(p) = preset {
            crate::pins::restore(app, p.pin);
        }
        // Already capturing: just make sure the page has the keyboard.
        #[cfg(windows)]
        if let Some(w) = app.get_webview_window(LABEL) {
            if let Ok(h) = w.hwnd() {
                win::force_foreground(h.0 as isize);
            }
        }
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        #[cfg(windows)]
        {
            if preset.is_some() {
                // Let the pin being edited disappear from the screen first.
                std::thread::sleep(std::time::Duration::from_millis(80));
            }
            let cfg = crate::toolkit::current();
            let Some(frame) = win::grab(cfg.capture.include_cursor) else {
                crate::toolkit::log("capture: could not copy the screen");
                BUSY.store(false, Ordering::SeqCst);
                return;
            };
            win::freeze_show(&frame);
            let windows = if cfg.capture.detect_elements { win::windows_front_to_back() } else { Vec::new() };
            let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
            let (x, y, w, h) = (frame.x, frame.y, frame.w, frame.h);
            if let Ok(mut s) = SESSION.lock() {
                *s = Some(Session { id, w, h, bgra: frame.bgra.clone(), preset: preset.clone() });
            }
            let monitors: Vec<Value> = app
                .available_monitors()
                .unwrap_or_default()
                .iter()
                .map(|m| {
                    let p = m.position();
                    let s = m.size();
                    json!({ "x": p.x, "y": p.y, "w": s.width, "h": s.height, "scale": m.scale_factor() })
                })
                .collect();
            let payload = json!({
                "id": id,
                "origin": [x, y],
                "size": [w, h],
                "monitors": monitors,
                "windows": windows,
                "history": load_history(),
                "settings": &cfg.capture,
                "preset": preset.as_ref().map(|p| json!({ "pin": p.pin, "rect": [p.x, p.y, p.w, p.h] })),
            });
            let app2 = app.clone();
            let _ = app.run_on_main_thread(move || {
                let Some(page) = app2.get_webview_window(LABEL) else {
                    end_session(&app2);
                    return;
                };
                if let Ok(hwnd) = page.hwnd() {
                    win::place_below_freeze(hwnd.0 as isize, x, y, w, h);
                }
                let _ = page.emit("capture:start", payload);
            });
        }
        #[cfg(not(windows))]
        {
            let _ = (&app, &preset);
            BUSY.store(false, Ordering::SeqCst);
        }
    });
}

// --- commands for capture.js ---------------------------------------------------------

/// The frozen screen as RGBA, for an ImageData.
#[tauri::command]
pub async fn capture_frame(id: u64) -> Result<tauri::ipc::Response, String> {
    let (bgra, w, h) = {
        let g = SESSION.lock().map_err(|e| e.to_string())?;
        let s = g.as_ref().filter(|s| s.id == id).ok_or("截图已结束")?;
        (s.bgra.clone(), s.w, s.h)
    };
    let mut out = Vec::with_capacity((w as usize) * (h as usize) * 4);
    for px in bgra.chunks_exact(4) {
        out.extend_from_slice(&[px[2], px[1], px[0], 255]);
    }
    Ok(tauri::ipc::Response::new(out))
}

/// The page has drawn the frame: show it and take the keyboard. The freeze
/// window stays on top until `capture_shown`.
#[tauri::command]
pub fn capture_ready(app: AppHandle) {
    // Safety net: if the page never reports that it has painted (a WebView
    // that decides it is covered stops painting), the freeze window must
    // not stay up forever.
    let id = SESSION.lock().ok().and_then(|s| s.as_ref().map(|s| s.id));
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(350));
        let still = SESSION.lock().ok().and_then(|s| s.as_ref().map(|s| s.id)) == id;
        if still {
            #[cfg(windows)]
            win::freeze_hide();
        }
    });
    if let Some(page) = app.get_webview_window(LABEL) {
        let _ = page.show();
        #[cfg(windows)]
        if let Ok(h) = page.hwnd() {
            win::force_foreground(h.0 as isize);
        }
        let _ = page.set_focus();
    }
}

/// The page is visible and painted: drop the freeze window.
#[tauri::command]
pub fn capture_shown() {
    #[cfg(windows)]
    win::freeze_hide();
}

/// Controls inside one window (UI Automation), fetched when the pointer
/// first enters that window.
#[tauri::command]
pub async fn capture_elements(hwnd: isize) -> Vec<[i32; 4]> {
    #[cfg(windows)]
    {
        tauri::async_runtime::spawn_blocking(move || win::ui_elements(hwnd, std::time::Duration::from_millis(450)))
            .await
            .unwrap_or_default()
    }
    #[cfg(not(windows))]
    {
        let _ = hwnd;
        Vec::new()
    }
}

#[tauri::command]
pub fn capture_cancel(app: AppHandle) {
    if let Some(pin) = end_session(&app) {
        crate::pins::restore(&app, pin);
    }
}

/// The pin being annotated, as RGBA.
#[tauri::command]
pub async fn capture_preset(id: u64) -> Result<tauri::ipc::Response, String> {
    let g = SESSION.lock().map_err(|e| e.to_string())?;
    let p = g.as_ref().filter(|s| s.id == id).and_then(|s| s.preset.clone()).ok_or("没有要标注的贴图")?;
    Ok(tauri::ipc::Response::new((*p.rgba).clone()))
}

#[tauri::command]
pub fn capture_copy_text(text: String) -> Result<(), String> {
    #[cfg(windows)]
    return crate::toolkit::win::clipboard::set_text(&text, false);
    #[cfg(not(windows))]
    {
        let _ = text;
        Ok(())
    }
}

fn header<'a>(req: &'a tauri::ipc::Request<'_>, name: &str) -> Option<&'a str> {
    req.headers().get(name).and_then(|v| v.to_str().ok())
}

/// Finish with the composed image. Headers: `x-op` = copy | save | save-as,
/// `x-width`, `x-height`, and the selection (`x-rx` ...) for the history.
/// The page is hidden at once; encoding happens in the background.
#[tauri::command]
pub fn capture_finish(app: AppHandle, request: tauri::ipc::Request<'_>) -> Result<(), String> {
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Err("没有图像数据".into());
    };
    let num = |k: &str| header(&request, k).and_then(|v| v.parse::<i32>().ok()).unwrap_or(0);
    let op = header(&request, "x-op").unwrap_or("copy").to_string();
    let (w, h) = (num("x-width").max(0) as u32, num("x-height").max(0) as u32);
    if w == 0 || h == 0 || bytes.len() < (w * h * 4) as usize {
        return Err("图像尺寸不对".into());
    }
    let region = Region { x: num("x-rx"), y: num("x-ry"), w: num("x-rw"), h: num("x-rh") };
    let rgba = bytes.clone();
    let editing = end_session(&app);
    if op == "pin" {
        // Pinned exactly where it was on screen, so it seems to lift off.
        match editing {
            Some(pin) => crate::pins::replace(&app, pin, w, h, rgba, (region.x, region.y)),
            None => {
                push_history(region.clone());
                crate::pins::create(&app, crate::pins::Source::Rgba(w, h, Arc::new(rgba)), Some((region.x, region.y)))?;
            }
        }
        return Ok(());
    }
    if let Some(pin) = editing {
        // Copy / save from a pin's editor: the pin itself stays as it was.
        crate::pins::restore(&app, pin);
    } else {
        push_history(region);
    }
    let app2 = app.clone();
    std::thread::spawn(move || {
        let result = encode::finish(&op, w, h, &rgba);
        match result {
            Ok(Some(path)) => {
                let _ = app2.emit("capture:saved", path);
            }
            Ok(None) => {}
            Err(e) => {
                crate::toolkit::log(&format!("capture {op} failed: {e}"));
                let _ = app2.emit("capture:error", e);
            }
        }
    });
    Ok(())
}

#[tauri::command]
pub async fn capture_pick_folder(start: String) -> Option<String> {
    #[cfg(windows)]
    {
        let start = if start.is_empty() { encode::default_dir().to_string_lossy().into_owned() } else { start };
        tauri::async_runtime::spawn_blocking(move || win::pick_folder(&start)).await.ok().flatten()
    }
    #[cfg(not(windows))]
    {
        let _ = start;
        None
    }
}

/// Arrow keys before a selection exists move the real pointer by a pixel.
#[tauri::command]
pub fn capture_nudge(dx: i32, dy: i32) {
    #[cfg(windows)]
    win::nudge_cursor(dx, dy);
    #[cfg(not(windows))]
    let _ = (dx, dy);
}

#[tauri::command]
pub fn capture_cursor() -> Option<(i32, i32)> {
    #[cfg(windows)]
    return win::cursor_pos();
    #[cfg(not(windows))]
    None
}

#[tauri::command]
pub fn capture_default_dir() -> String {
    encode::default_dir().to_string_lossy().into_owned()
}

pub fn busy() -> bool {
    BUSY.load(Ordering::SeqCst)
}

pub fn forward_pin(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.emit("capture:pin", ());
    }
}
