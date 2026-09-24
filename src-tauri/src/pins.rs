//! 贴图: images (and text) pinned to the screen, Snipaste style.
//!
//! Each pin is its own small transparent, borderless, always-on-top WebView
//! window (`pin-<id>`, pin.html). Rust keeps the pin's source (pixels, an
//! encoded image, or text) and places the window; everything else -- zoom,
//! opacity, rotation, flipping, the menu, thumbnail mode -- happens in
//! pin.js, which also hands back the finished pixels for copy / save /
//! annotate so what you get is what you see.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

#[derive(Clone)]
pub enum Source {
    Rgba(u32, u32, Arc<Vec<u8>>),
    Encoded(Arc<Vec<u8>>, String),
    Text(String),
}

struct Pin {
    source: Source,
    /// Screen position (physical) of the image's top-left corner.
    anchor: (i32, i32),
}

static PINS: Mutex<Option<HashMap<u64, Pin>>> = Mutex::new(None);
static NEXT: AtomicU64 = AtomicU64::new(1);
static HIDDEN: AtomicBool = AtomicBool::new(false);

fn with_pins<T>(f: impl FnOnce(&mut HashMap<u64, Pin>) -> T) -> T {
    let mut g = PINS.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(HashMap::new))
}

pub fn label(id: u64) -> String {
    format!("pin-{id}")
}

fn id_of(label: &str) -> Option<u64> {
    label.strip_prefix("pin-")?.parse().ok()
}

/// Make a pin. `anchor` = where the image's top-left goes on screen; None
/// puts it at the pointer.
pub fn create(app: &AppHandle, source: Source, anchor: Option<(i32, i32)>) -> Result<u64, String> {
    let anchor = anchor.unwrap_or_else(cursor);
    let id = NEXT.fetch_add(1, Ordering::SeqCst);
    with_pins(|m| m.insert(id, Pin { source, anchor }));
    HIDDEN.store(false, Ordering::SeqCst);
    let app = app.clone();
    // Window creation must not happen inside a synchronous command on the
    // main thread (WebView2 deadlocks); a helper thread is always safe.
    std::thread::spawn(move || {
        let built = WebviewWindowBuilder::new(&app, label(id), WebviewUrl::App(format!("pin.html?id={id}").into()))
            .title("贴图")
            .decorations(false)
            .transparent(true)
            .shadow(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .resizable(false)
            .visible(false)
            .focused(true)
            .inner_size(64.0, 64.0)
            .build();
        if let Err(e) = built {
            crate::toolkit::log(&format!("pin window failed: {e}"));
            with_pins(|m| m.remove(&id));
        }
    });
    Ok(id)
}

fn cursor() -> (i32, i32) {
    #[cfg(windows)]
    if let Some(p) = crate::capture::win::cursor_pos() {
        return p;
    }
    (100, 100)
}

/// F3 / "把剪贴板贴到屏幕".
pub fn from_clipboard(app: &AppHandle) {
    #[cfg(windows)]
    {
        use crate::toolkit::win::clipboard::{read_for_pin, Content};
        let source = match read_for_pin() {
            Some(Content::Encoded(b, mime)) => Source::Encoded(Arc::new(b), mime.to_string()),
            Some(Content::Rgba(w, h, px)) => Source::Rgba(w, h, Arc::new(px)),
            Some(Content::Text(t)) => Source::Text(t),
            _ => {
                crate::toolkit::log("pin: the clipboard holds nothing that can be pinned");
                return;
            }
        };
        // Slightly up-left of the pointer, so it lands under the hand.
        let (x, y) = cursor();
        let _ = create(app, source, Some((x - 12, y - 12)));
    }
    #[cfg(not(windows))]
    let _ = app;
}

pub fn toggle_hidden(app: &AppHandle) {
    let hide = !HIDDEN.load(Ordering::SeqCst);
    HIDDEN.store(hide, Ordering::SeqCst);
    for id in with_pins(|m| m.keys().copied().collect::<Vec<_>>()) {
        if let Some(w) = app.get_webview_window(&label(id)) {
            let _ = if hide { w.hide() } else { w.show() };
        }
    }
}

pub fn close_all(app: &AppHandle) {
    for id in with_pins(|m| m.drain().map(|(k, _)| k).collect::<Vec<_>>()) {
        if let Some(w) = app.get_webview_window(&label(id)) {
            let _ = w.destroy();
        }
    }
}

/// Mouse click-through off for every pin (the only way back to a pin that
/// lets the mouse through, besides its own hotkey).
pub fn solid_all(app: &AppHandle) {
    for id in with_pins(|m| m.keys().copied().collect::<Vec<_>>()) {
        if let Some(w) = app.get_webview_window(&label(id)) {
            let _ = w.set_ignore_cursor_events(false);
            let _ = w.emit("pin:solid", ());
        }
    }
}

/// A capture that was editing pin `id` finished with new pixels.
pub fn replace(app: &AppHandle, id: u64, w: u32, h: u32, rgba: Vec<u8>, anchor: (i32, i32)) {
    let exists = with_pins(|m| {
        if let Some(p) = m.get_mut(&id) {
            p.source = Source::Rgba(w, h, Arc::new(rgba));
            p.anchor = anchor;
            true
        } else {
            false
        }
    });
    if !exists {
        let _ = with_pins(|m| m.remove(&id));
        return;
    }
    if let Some(win) = app.get_webview_window(&label(id)) {
        let _ = win.emit("pin:reload", ());
    }
}

/// Show a pin again after an edit was cancelled.
pub fn restore(app: &AppHandle, id: u64) {
    if let Some(win) = app.get_webview_window(&label(id)) {
        let _ = win.show();
    }
}

// --- commands for pin.js -----------------------------------------------------------

#[tauri::command]
pub fn pin_info(window: tauri::WebviewWindow) -> Result<Value, String> {
    let id = id_of(window.label()).ok_or("not a pin")?;
    with_pins(|m| {
        let p = m.get(&id).ok_or("贴图已关闭")?;
        Ok(match &p.source {
            Source::Rgba(w, h, _) => json!({ "id": id, "kind": "rgba", "w": w, "h": h, "anchor": p.anchor }),
            Source::Encoded(_, mime) => json!({ "id": id, "kind": "encoded", "mime": mime, "anchor": p.anchor }),
            Source::Text(t) => json!({ "id": id, "kind": "text", "text": t, "anchor": p.anchor }),
        })
    })
}

#[tauri::command]
pub async fn pin_bytes(window: tauri::WebviewWindow) -> Result<tauri::ipc::Response, String> {
    let id = id_of(window.label()).ok_or("not a pin")?;
    let src = with_pins(|m| m.get(&id).map(|p| p.source.clone())).ok_or("贴图已关闭")?;
    Ok(tauri::ipc::Response::new(match src {
        Source::Rgba(_, _, px) => (*px).clone(),
        Source::Encoded(b, _) => (*b).clone(),
        Source::Text(t) => t.into_bytes(),
    }))
}

/// Move and size the pin's window in one step (physical pixels), without
/// taking focus. Called while zooming, rotating, and once on load.
#[tauri::command]
pub fn pin_geometry(window: tauri::WebviewWindow, x: i32, y: i32, w: i32, h: i32) {
    #[cfg(windows)]
    if let Ok(hwnd) = window.hwnd() {
        crate::capture::win::set_rect(hwnd.0 as isize, x, y, w.max(1), h.max(1));
        return;
    }
    let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
    let _ = window.set_size(tauri::PhysicalSize::new(w.max(1) as u32, h.max(1) as u32));
}

#[tauri::command]
pub fn pin_show(window: tauri::WebviewWindow) {
    if HIDDEN.load(Ordering::SeqCst) {
        return;
    }
    let _ = window.show();
    let _ = window.set_focus();
}

#[tauri::command]
pub fn pin_close(window: tauri::WebviewWindow) {
    if let Some(id) = id_of(window.label()) {
        with_pins(|m| m.remove(&id));
    }
    let _ = window.destroy();
}

/// The page remembers where the image is when the window is dragged.
#[tauri::command]
pub fn pin_moved(window: tauri::WebviewWindow, x: i32, y: i32) {
    if let Some(id) = id_of(window.label()) {
        with_pins(|m| {
            if let Some(p) = m.get_mut(&id) {
                p.anchor = (x, y);
            }
        });
    }
}

fn body(request: &tauri::ipc::Request<'_>) -> Result<Vec<u8>, String> {
    match request.body() {
        tauri::ipc::InvokeBody::Raw(b) => Ok(b.clone()),
        _ => Err("没有图像数据".into()),
    }
}

fn header_num(request: &tauri::ipc::Request<'_>, k: &str) -> i32 {
    request.headers().get(k).and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok()).unwrap_or(0)
}

/// Copy / save / save-as the pin as it looks now (x-op header, RGBA body).
#[tauri::command]
pub fn pin_export(app: AppHandle, request: tauri::ipc::Request<'_>) -> Result<(), String> {
    let rgba = body(&request)?;
    let op = request.headers().get("x-op").and_then(|v| v.to_str().ok()).unwrap_or("copy").to_string();
    let (w, h) = (header_num(&request, "x-width") as u32, header_num(&request, "x-height") as u32);
    if w == 0 || h == 0 || rgba.len() < (w * h * 4) as usize {
        return Err("图像尺寸不对".into());
    }
    std::thread::spawn(move || match crate::capture::encode::finish(&op, w, h, &rgba) {
        Ok(Some(p)) => {
            let _ = app.emit("capture:saved", p);
        }
        Ok(None) => {}
        Err(e) => crate::toolkit::log(&format!("pin {op} failed: {e}")),
    });
    Ok(())
}

/// 标注: hide the pin and open the capture editor on its pixels, in place.
#[tauri::command]
pub fn pin_edit(app: AppHandle, window: tauri::WebviewWindow, request: tauri::ipc::Request<'_>) -> Result<(), String> {
    let id = id_of(window.label()).ok_or("not a pin")?;
    let rgba = body(&request)?;
    let (w, h) = (header_num(&request, "x-width") as u32, header_num(&request, "x-height") as u32);
    let (x, y) = (header_num(&request, "x-x"), header_num(&request, "x-y"));
    if w == 0 || h == 0 || rgba.len() < (w * h * 4) as usize {
        return Err("图像尺寸不对".into());
    }
    let _ = window.hide();
    crate::capture::start_with(
        &app,
        Some(crate::capture::Preset { pin: id, x, y, w, h, rgba: Arc::new(rgba) }),
    );
    Ok(())
}

#[tauri::command]
pub fn pins_toggle_hidden(app: AppHandle) {
    toggle_hidden(&app);
}

#[tauri::command]
pub fn pins_close_all(app: AppHandle) {
    close_all(&app);
}
