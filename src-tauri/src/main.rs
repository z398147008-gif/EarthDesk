#![cfg_attr(all(not(debug_assertions), windows), windows_subsystem = "windows")]

mod config;
mod environment;
mod platform;
mod snap;
mod sysmon;
mod wallpaper;
mod weather;

use config::{Config, WidgetRect};
use serde_json::Value;
use snap::{Rect, SnapResult};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, State, WebviewWindow};

/// The draggable widgets. The wallpaper is deliberately not one of them.
const WIDGETS: [&str; 2] = ["weather", "perf"];
const MIN_W: u32 = 240;
const MIN_H: u32 = 120;

/// First-run geometry, in logical pixels. Applied through the monitor's own
/// scale factor, so the widgets come out the same apparent size on a 1080p
/// display at 100% and a 4K display at 150% -- which is what "follows the
/// system" has to mean for the window as well as for the text inside it.
///
/// The sizes must match DESIGN in widget.js: that is the box the layout was
/// drawn for, and it is the size at which 1rem is exactly 16 CSS pixels.
const DEFAULT_LAYOUT: [(&str, f64, f64, f64, f64); 2] = [
    ("weather", 72.0, 64.0, 440.0, 440.0),
    ("perf", 72.0, 528.0, 440.0, 440.0),
];

/// Bumped whenever the location changes, so the weather loop stops waiting
/// and fetches for the new place at once.
static WEATHER_GEN: AtomicU64 = AtomicU64::new(0);

struct AppState {
    config: Mutex<Config>,
    editing: Mutex<bool>,
    weather: Mutex<Option<Value>>,
    sysmon: Mutex<Option<Value>>,
}

fn widget_rect(win: &WebviewWindow) -> Option<Rect> {
    // GetWindowRect answers in screen coordinates whether or not the window is
    // parented into the desktop layer, so reads never need converting.
    let p = win.outer_position().ok()?;
    let s = win.outer_size().ok()?;
    Some(Rect { x: p.x, y: p.y, w: s.width as i32, h: s.height as i32 })
}

/// Writes are the asymmetric half: `SetWindowPos` on a child window takes
/// coordinates relative to its parent, and while a widget is parked in the
/// desktop layer its parent is the wallpaper window at the virtual screen
/// origin.
fn move_widget_to_screen(win: &WebviewWindow, x: i32, y: i32) {
    let (offset_x, offset_y) = if platform::is_detached(win) {
        (0, 0)
    } else {
        let (vx, vy, _, _) = platform::virtual_screen();
        (vx, vy)
    };
    let _ = win.set_position(PhysicalPosition::new(x - offset_x, y - offset_y));
}

fn monitor_rect(win: &WebviewWindow) -> Rect {
    match win.current_monitor() {
        Ok(Some(m)) => {
            let p = m.position();
            let s = m.size();
            Rect { x: p.x, y: p.y, w: s.width as i32, h: s.height as i32 }
        }
        _ => {
            let (x, y, w, h) = platform::virtual_screen();
            Rect { x, y, w, h }
        }
    }
}

/// Every widget except `exclude`, for widget-to-widget alignment.
fn peers(app: &AppHandle, exclude: &str) -> Vec<Rect> {
    WIDGETS
        .iter()
        .filter(|l| **l != exclude)
        .filter_map(|l| app.get_webview_window(l))
        .filter(|w| feature_on(app, w.label()))
        .filter_map(|w| widget_rect(&w))
        .collect()
}

fn show_guides(app: &AppHandle, guides: &[snap::Guide], monitor: Rect) {
    let Some(overlay) = app.get_webview_window("guides") else { return };
    if guides.is_empty() {
        let _ = overlay.emit("guides", serde_json::json!({ "lines": [] }));
        return;
    }
    let _ = overlay.set_position(PhysicalPosition::new(monitor.x, monitor.y));
    let _ = overlay.set_size(PhysicalSize::new(monitor.w as u32, monitor.h as u32));
    let _ = overlay.emit(
        "guides",
        serde_json::json!({ "origin": [monitor.x, monitor.y], "lines": guides }),
    );
    let _ = overlay.show();
}

#[tauri::command]
fn drag_widget(
    app: AppHandle,
    state: State<AppState>,
    label: String,
    x: i32,
    y: i32,
) -> Option<SnapResult> {
    let win = app.get_webview_window(&label)?;
    let mut r = widget_rect(&win)?;
    r.x = x;
    r.y = y;
    let monitor = monitor_rect(&win);
    let threshold = state.config.lock().ok()?.snap_threshold;
    let result = snap::snap(r, &peers(&app, &label), monitor, threshold);
    move_widget_to_screen(&win, result.x, result.y);
    show_guides(&app, &result.guides, monitor);
    Some(result)
}

#[tauri::command]
fn resize_widget(app: AppHandle, label: String, w: u32, h: u32) -> Option<()> {
    let win = app.get_webview_window(&label)?;
    win.set_size(PhysicalSize::new(w.max(MIN_W), h.max(MIN_H))).ok()
}

/// Called once when the pointer is released: persists whatever the widgets
/// ended up at, and clears the guide overlay.
#[tauri::command]
fn commit_layout(app: AppHandle, state: State<AppState>) {
    if let Ok(mut cfg) = state.config.lock() {
        for label in WIDGETS {
            if let Some(win) = app.get_webview_window(label) {
                if let Some(r) = widget_rect(&win) {
                    let visible = win.is_visible().unwrap_or(true);
                    cfg.widgets.insert(
                        label.to_string(),
                        WidgetRect { x: r.x, y: r.y, w: r.w as u32, h: r.h as u32, visible },
                    );
                }
            }
        }
        cfg.save();
    }
    if let Some(overlay) = app.get_webview_window("guides") {
        let _ = overlay.emit("guides", serde_json::json!({ "lines": [] }));
    }
}

#[tauri::command]
fn get_location(state: State<AppState>) -> Option<config::Location> {
    state.config.lock().ok().map(|c| c.location.clone())
}

#[tauri::command]
fn get_wallpaper_config(state: State<AppState>) -> Option<config::Wallpaper> {
    state.config.lock().ok().map(|c| c.wallpaper.clone())
}

/// Screen rectangles in physical pixels. The wallpaper canvas covers the
/// virtual desktop; `monitors` lists each screen inside it so the globe can be
/// drawn once per monitor, and `primary` is what single-scene framing uses.
#[tauri::command]
fn get_screen_layout(app: AppHandle) -> serde_json::Value {
    let (vx, vy, vw, vh) = platform::virtual_screen();
    let (px, py, pw, ph) = platform::primary_screen();
    let monitors: Vec<Value> = app
        .available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|m| {
            let p = m.position();
            let s = m.size();
            serde_json::json!({
                "x": p.x, "y": p.y, "w": s.width, "h": s.height,
                "primary": p.x == px && p.y == py,
                "name": m.name(),
            })
        })
        .collect();
    serde_json::json!({
        "virtual": { "x": vx, "y": vy, "w": vw, "h": vh },
        "primary": { "x": px, "y": py, "w": pw, "h": ph },
        "monitors": monitors,
    })
}

#[tauri::command]
fn get_ui_scale(state: State<AppState>) -> f64 {
    state.config.lock().map(|c| c.ui_scale).unwrap_or(1.0)
}

#[tauri::command]
fn get_weather(state: State<AppState>) -> Option<Value> {
    state.weather.lock().ok().and_then(|w| w.clone())
}

#[tauri::command]
fn get_sysmon(state: State<AppState>) -> Option<Value> {
    state.sysmon.lock().ok().and_then(|s| s.clone())
}

#[tauri::command]
fn is_editing(state: State<AppState>) -> bool {
    state.editing.lock().map(|e| *e).unwrap_or(false)
}

/// Save a place picked on the settings page and tell every page about it: the
/// globe re-aims, the weather card relabels, and the weather loop refetches.
#[tauri::command]
fn set_location(app: AppHandle, state: State<AppState>, location: config::Location) -> Result<(), String> {
    if !(-90.0..=90.0).contains(&location.lat) || !(-180.0..=180.0).contains(&location.lon) {
        return Err("坐标超出范围".into());
    }
    {
        let mut cfg = state.config.lock().map_err(|e| e.to_string())?;
        cfg.location = location.clone();
        cfg.save();
    }
    if let Ok(mut w) = state.weather.lock() {
        *w = None;
    }
    WEATHER_GEN.fetch_add(1, Ordering::SeqCst);
    let _ = app.emit("location:changed", location);
    Ok(())
}

/// Is the bundled hardware monitor there, and is it answering?
#[tauri::command]
async fn lhm_status(app: AppHandle) -> Value {
    let url = {
        let state = app.state::<AppState>();
        let u = state.config.lock().map(|c| c.sensors.lhm_url.clone()).unwrap_or_default();
        u
    };
    let bundled = environment::bundled_lhm(app.path().resource_dir().ok()).is_some();
    let running = tauri::async_runtime::spawn_blocking(move || environment::lhm_reachable(&url))
        .await
        .unwrap_or(false);
    serde_json::json!({ "bundled": bundled, "running": running })
}

/// One-click repair from the settings page (one UAC prompt).
#[tauri::command]
fn lhm_repair(app: AppHandle) -> Result<(), String> {
    match environment::bundled_lhm(app.path().resource_dir().ok()) {
        Some(exe) => environment::repair_lhm(&exe),
        None => Err("没有找到内置的硬件监控组件，请重新安装地球桌面".into()),
    }
}

#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    environment::open_url(&url)
}

#[tauri::command]
fn get_autostart() -> bool {
    environment::autostart_enabled()
}

#[tauri::command]
fn set_autostart(on: bool) -> Result<(), String> {
    environment::set_autostart(on)
}

/// Make sure the hardware monitor is up a few seconds after start. The
/// installer registered a logon task for it; this covers a first start right
/// after installing, or the monitor having been closed from its tray icon.
fn spawn_environment_check(app: AppHandle) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(4));
        let url = {
            let state = app.state::<AppState>();
            let u = state.config.lock().map(|c| c.sensors.lhm_url.clone()).unwrap_or_default();
            u
        };
        if environment::lhm_reachable(&url) {
            return;
        }
        if environment::bundled_lhm(app.path().resource_dir().ok()).is_some() {
            let ok = environment::start_lhm_task();
            watchdog_log(&format!("hardware monitor not answering; started logon task: {ok}"));
        }
    });
}

/// Everything the settings page shows, in one round trip.
#[tauri::command]
fn get_settings(state: State<AppState>) -> serde_json::Value {
    match state.config.lock() {
        Ok(cfg) => serde_json::json!({
            "features": cfg.features,
            "z_mode": cfg.z_mode,
            "ui_scale": cfg.ui_scale,
            "card_opacity": cfg.card_opacity,
            "fan_roles": cfg.sensors.fan_roles,
            "location": cfg.location,
        }),
        Err(_) => serde_json::json!({}),
    }
}

/// Flip one switch on the settings page. Whole pieces (the wallpaper, a
/// widget) are shown or hidden right here; parts inside them are applied by
/// the pages themselves when they hear `features:changed`.
#[tauri::command]
fn set_feature(app: AppHandle, state: State<AppState>, key: String, on: bool) -> Result<(), String> {
    let (features, z_mode, wall) = {
        let mut cfg = state.config.lock().map_err(|e| e.to_string())?;
        cfg.features.insert(key.clone(), on);
        cfg.save();
        let picked = (cfg.features.clone(), cfg.z_mode.clone(), cfg.wallpaper.clone());
        picked
    };

    match key.as_str() {
        "wallpaper" => {
            // Widgets in the desktop layer live *inside* the wallpaper window,
            // so hiding it would take them down too. Move them out first when
            // it goes away, and back in only once it is showing again.
            if on {
                wallpaper::setup(&app, &wall);
                repark_widgets(&app, &z_mode);
            } else {
                repark_widgets(&app, &z_mode);
                wallpaper::hide(&app);
            }
        }
        label if WIDGETS.contains(&label) => {
            if let Some(win) = app.get_webview_window(label) {
                if on {
                    let _ = win.show();
                    let rect = widget_rect(&win);
                    park_widget(&app, &win, &z_mode);
                    if let Some(r) = rect {
                        move_widget_to_screen(&win, r.x, r.y);
                    }
                } else {
                    let _ = win.hide();
                }
            }
        }
        _ => {}
    }

    let _ = app.emit("features:changed", &features);
    Ok(())
}

#[tauri::command]
fn set_z_mode(app: AppHandle, state: State<AppState>, mode: String) -> Result<(), String> {
    if !matches!(mode.as_str(), "desktop" | "bottom" | "topmost" | "normal") {
        return Err(format!("unknown z_mode {mode}"));
    }
    {
        let mut cfg = state.config.lock().map_err(|e| e.to_string())?;
        cfg.z_mode = mode.clone();
        cfg.save();
    }
    repark_widgets(&app, &mode);
    Ok(())
}

#[tauri::command]
fn set_ui_scale(app: AppHandle, state: State<AppState>, value: f64) -> Result<(), String> {
    let value = value.clamp(0.6, 2.5);
    {
        let mut cfg = state.config.lock().map_err(|e| e.to_string())?;
        cfg.ui_scale = value;
        cfg.save();
    }
    let _ = app.emit("ui-scale", value);
    Ok(())
}

#[tauri::command]
fn set_card_opacity(app: AppHandle, state: State<AppState>, value: f64) -> Result<(), String> {
    let value = value.clamp(0.0, 1.0);
    {
        let mut cfg = state.config.lock().map_err(|e| e.to_string())?;
        cfg.card_opacity = value;
        cfg.save();
    }
    let _ = app.emit("card-opacity", value);
    Ok(())
}

/// The performance widget reports which fan headers it worked out belong to
/// the CPU cooler; remember that across restarts.
#[tauri::command]
fn set_fan_roles(
    app: AppHandle,
    state: State<AppState>,
    roles: std::collections::BTreeMap<String, String>,
) -> Result<(), String> {
    {
        let mut cfg = state.config.lock().map_err(|e| e.to_string())?;
        cfg.sensors.fan_roles = roles.clone();
        cfg.save();
    }
    let _ = app.emit("fan-roles", &roles);
    Ok(())
}

#[tauri::command]
fn toggle_editing(app: AppHandle) {
    toggle_edit_mode(&app);
}

/// Re-seat every visible widget in the z-order the current mode asks for,
/// keeping each exactly where it is on screen.
fn feature_on(app: &AppHandle, key: &str) -> bool {
    app.state::<AppState>()
        .config
        .lock()
        .map(|c| c.feature(key))
        .unwrap_or(true)
}

fn repark_widgets(app: &AppHandle, z_mode: &str) {
    for label in WIDGETS {
        let Some(win) = app.get_webview_window(label) else { continue };
        if !feature_on(app, label) {
            continue;
        }
        let rect = widget_rect(&win);
        park_widget(app, &win, z_mode);
        let _ = win.show();
        if let Some(r) = rect {
            move_widget_to_screen(&win, r.x, r.y);
        }
    }
}

fn show_settings(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("settings") {
        let _ = win.unminimize();
        let _ = win.show();
        let _ = win.set_focus();
    }
}

#[tauri::command]
fn open_config_dir() {
    let path = config::config_path();
    if let Some(dir) = path.parent() {
        #[cfg(windows)]
        let _ = std::process::Command::new("explorer").arg(dir).spawn();
        #[cfg(not(windows))]
        let _ = dir;
    }
}

/// Put a widget where it belongs in the z-order.
///
/// "desktop" parks it inside the wallpaper window, so it rides in the desktop
/// layer: drawn over the globe, under the desktop icons, and under every
/// application window. That is the only mode in which a widget behaves like
/// part of the desktop rather than like a window that refuses to go away.
fn park_widget(app: &AppHandle, win: &WebviewWindow, z_mode: &str) {
    match z_mode {
        "topmost" => {
            platform::detach(win);
            platform::set_topmost(win, true);
        }
        "normal" => {
            platform::detach(win);
            platform::set_topmost(win, false);
        }
        // Fallback if parking inside the wallpaper misbehaves on a given
        // machine: bottom of the ordinary z-order. Still out of the way of
        // every application window, just not inside the desktop layer.
        "bottom" => {
            platform::detach(win);
            platform::set_topmost(win, false);
            platform::sink_to_bottom(win);
        }
        _ => {
            platform::set_topmost(win, false);
            let host = app.get_webview_window(wallpaper::LABEL);
            let parked = match &host {
                Some(h) if feature_on(app, "wallpaper") && h.is_visible().unwrap_or(false) => {
                    platform::attach_to(win, h)
                }
                _ => false,
            };
            if !parked {
                // No wallpaper to ride along with: bottom of the normal
                // z-order is the next best thing.
                platform::detach(win);
                platform::sink_to_bottom(win);
            }
        }
    }
}

fn set_edit_mode(app: &AppHandle, on: bool) {
    let z_mode = app
        .state::<AppState>()
        .config
        .lock()
        .map(|c| c.z_mode.clone())
        .unwrap_or_else(|_| "desktop".into());

    for label in WIDGETS {
        let Some(win) = app.get_webview_window(label) else { continue };
        let rect = widget_rect(&win);

        if on {
            // A child of the wallpaper layer sits behind the icon view and
            // never sees the mouse, so arranging has to happen as a normal
            // top-level window.
            platform::detach(&win);
            platform::set_topmost(&win, true);
        } else {
            park_widget(app, &win, &z_mode);
        }

        // Re-parenting changes what set_position means, so restore the screen
        // position the widget had a moment ago rather than trusting the number
        // the move left behind.
        if let Some(r) = rect {
            move_widget_to_screen(&win, r.x, r.y);
        }

        // Click-through is the whole point of a desktop widget: it is only
        // released while the user is actually arranging things.
        let _ = win.set_ignore_cursor_events(!on);
        let _ = win.emit("edit-mode", on);
    }

    if let Some(overlay) = app.get_webview_window("guides") {
        if on {
            let _ = overlay.set_ignore_cursor_events(true);
            platform::set_topmost(&overlay, true);
        } else {
            let _ = overlay.emit("guides", serde_json::json!({ "lines": [] }));
            let _ = overlay.hide();
        }
    }
}

fn toggle_edit_mode(app: &AppHandle) {
    let state = app.state::<AppState>();
    let now = {
        let Ok(mut editing) = state.editing.lock() else { return };
        *editing = !*editing;
        *editing
    };
    set_edit_mode(app, now);
}

/// Poll Open-Meteo and push the result at the weather widget. The widget keeps
/// rendering the last good payload while a refresh is failing.
fn spawn_weather_loop(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut failures: u32 = 0;
        loop {
            let (lat, lon, tz, minutes) = {
                let state = app.state::<AppState>();
                let Ok(cfg) = state.config.lock() else { return };
                let picked = (
                    cfg.location.lat,
                    cfg.location.lon,
                    cfg.location.timezone.clone(),
                    cfg.weather_refresh_minutes.max(1),
                );
                picked
            };

            let generation = WEATHER_GEN.load(Ordering::SeqCst);
            let wait = match weather::fetch(lat, lon, &tz).await {
                Ok(data) => {
                    failures = 0;
                    if let Ok(mut slot) = app.state::<AppState>().weather.lock() {
                        *slot = Some(data.clone());
                    }
                    let _ = app.emit("weather:update", data);
                    minutes * 60
                }
                Err(e) => {
                    eprintln!("weather fetch failed: {e}");
                    let _ = app.emit("weather:error", e);
                    // Networks can be flaky; try again soon, backing off to
                    // the normal interval: 30 s, 1, 2, 4, 8 min ...
                    failures = (failures + 1).min(8);
                    (30u64 << (failures - 1)).min(minutes * 60)
                }
            };

            // Sleep in short steps so a location change cuts the wait short.
            let mut slept = 0;
            while slept < wait && WEATHER_GEN.load(Ordering::SeqCst) == generation {
                tokio::time::sleep(Duration::from_secs(5)).await;
                slept += 5;
            }
        }
    });
}

fn spawn_sysmon_loop(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let (url, interval) = {
            let state = app.state::<AppState>();
            // Bound to a local so the MutexGuard temporary is dropped before
            // `state` goes out of scope at the end of the block.
            let picked = match state.config.lock() {
                Ok(cfg) => (cfg.sensors.lhm_url.clone(), cfg.sensors.sample_ms.max(500)),
                Err(_) => return,
            };
            picked
        };

        let mut sampler = sysmon::Sampler::new();
        // The first CPU reading is an average since boot; throw it away.
        let _ = sampler.sample();

        loop {
            tokio::time::sleep(Duration::from_millis(interval)).await;
            let mut snapshot = sampler.sample();
            sysmon::merge_lhm(&mut snapshot, &url).await;
            let Ok(value) = serde_json::to_value(&snapshot) else { continue };
            if let Ok(mut slot) = app.state::<AppState>().sysmon.lock() {
                *slot = Some(value.clone());
            }
            let _ = app.emit("sysmon:update", value);
        }
    });
}

/// explorer.exe restarts, a resolution change or "show desktop" can all knock
/// the desktop layer out from under us. Putting it back is cheap *as long as
/// nothing actually moved* -- see wallpaper::reassert.
fn spawn_layer_watchdog(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;

            let editing = app
                .state::<AppState>()
                .editing
                .lock()
                .map(|e| *e)
                .unwrap_or(false);
            if editing {
                continue;
            }

            let (wall, z_mode) = {
                let state = app.state::<AppState>();
                let picked = match state.config.lock() {
                    Ok(c) => (c.wallpaper.clone(), c.z_mode.clone()),
                    Err(_) => return,
                };
                picked
            };

            wallpaper::reassert(&app, &wall);

            if z_mode == "desktop" && feature_on(&app, "wallpaper") {
                for label in WIDGETS {
                    let Some(win) = app.get_webview_window(label) else { continue };
                    if feature_on(&app, label) && platform::is_detached(&win) {
                        let rect = widget_rect(&win);
                        park_widget(&app, &win, &z_mode);
                        if let Some(r) = rect {
                            move_widget_to_screen(&win, r.x, r.y);
                        }
                    }
                }
            }
        }
    });
}

/// Where `watchdog.log` goes.
///
/// A build made from the source folder knows where that folder is, and putting
/// the log beside the source (`osaka-desk\watchdog.log`) means it can be read
/// there directly. Anything else -- an installed copy -- falls back to
/// `%APPDATA%\EarthDesk`.
fn watchdog_path() -> Option<std::path::PathBuf> {
    let project = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    if project.join("src").join("index.html").is_file() {
        return Some(project.join("watchdog.log"));
    }
    config::config_path().parent().map(|d| d.join("watchdog.log"))
}

/// Append one line to the watchdog log. Past 256 KB the older half is
/// dropped, so it can be left on forever and the newest lines always survive.
fn watchdog_log(line: &str) {
    use std::io::Write;
    let Some(path) = watchdog_path() else { return };
    if std::fs::metadata(&path).map(|m| m.len() > 256 * 1024).unwrap_or(false) {
        if let Ok(text) = std::fs::read_to_string(&path) {
            let mut cut = text.len() / 2;
            while !text.is_char_boundary(cut) {
                cut += 1;
            }
            let rest = &text[cut..];
            let rest = rest.split_once('\n').map(|(_, r)| r).unwrap_or(rest);
            let _ = std::fs::write(&path, rest);
        } else {
            let _ = std::fs::remove_file(&path);
        }
    }
    let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        % 86_400;
    let _ = writeln!(file, "{:02}:{:02}:{:02}Z {line}", secs / 3600, secs % 3600 / 60, secs % 60);
}

/// A widget page tells us when the browser engine decides it is hidden. If a
/// widget is reported "healthy" by the window checks and still cannot be
/// seen, this is the other half of the picture.
#[tauri::command]
fn report_visibility(label: String, hidden: bool) {
    watchdog_log(&format!("page {label}: document.hidden={hidden}"));
}

/// The running app, for code that is called back from outside Tauri (the
/// shell-event guard in platform.rs).
static APP: std::sync::OnceLock<AppHandle> = std::sync::OnceLock::new();

/// Show Desktop hides the widgets and the guard then lifts them back. Rather
/// than have them pop into view, the pages are told to go transparent while
/// the desktop is on top (`true`) and to fade up once they are lifted
/// (`false`). See widget.js and shared.css.
fn veil_widgets(on: bool) {
    let Some(app) = APP.get() else { return };
    let _ = app.emit(if on { "widget:veil" } else { "widget:unveil" }, ());
}

/// What the widget watchdog remembers between passes.
struct Watch {
    /// The last state line written to the log, so only changes are written.
    last: String,
    announced: bool,
}

impl Watch {
    fn new() -> Self {
        Watch { last: String::new(), announced: false }
    }
}

/// One pass of the widget watchdog.
///
/// "Show desktop" (Win+D) destroys nothing, so the 60-second layer watchdog
/// cannot see what it does. Two arrangements need looking after, and which one
/// applies depends on whether the earth wallpaper is on:
///
/// * Wallpaper on: the widgets are children of the wallpaper window and can
///   be hidden, minimised, or stacked under the WebView2 host that paints the
///   globe. See `tick_parked`.
/// * Wallpaper off (or `z_mode` "bottom"): the widgets are plain top-level
///   windows at the bottom of the z-order. Show Desktop raises the shell's
///   desktop windows over everything, which leaves them underneath it. See
///   `tick_loose`.
///
/// Both only act when something is wrong -- an unconditional write here is a
/// flicker -- and both append every change in what they see to
/// `watchdog.log`, so a case they do not know how to repair leaves a record.
fn widget_tick(app: &AppHandle, w: &mut Watch) {
    let editing = app
        .state::<AppState>()
        .editing
        .lock()
        .map(|e| *e)
        .unwrap_or(false);
    if editing {
        platform::guard_set(&[], false);
        return;
    }
    let z_mode = match app.state::<AppState>().config.lock() {
        Ok(c) => c.z_mode.clone(),
        Err(_) => return,
    };
    // "topmost" and "normal" are ordinary windows that Show Desktop already
    // treats the way the user asked for.
    if z_mode != "desktop" && z_mode != "bottom" {
        platform::guard_set(&[], false);
        return;
    }

    let live: Vec<WebviewWindow> = WIDGETS
        .iter()
        .filter(|l| feature_on(app, **l))
        .filter_map(|l| app.get_webview_window(l))
        .collect();
    let refs: Vec<&WebviewWindow> = live.iter().collect();

    let host = app.get_webview_window(wallpaper::LABEL);
    match host {
        Some(host) if z_mode == "desktop" && feature_on(app, "wallpaper") => {
            // Inside the wallpaper window the widgets ride with the desktop
            // layer on their own; nothing to guard.
            platform::guard_set(&[], false);
            tick_parked(app, w, &z_mode, &host, &live, &refs)
        }
        _ => tick_loose(w, &live, &refs),
    }
}

fn tick_loose(w: &mut Watch, live: &[WebviewWindow], refs: &[&WebviewWindow]) {
    if !w.announced {
        w.announced = true;
        watchdog_log("start [watchdog v5]: widgets are loose top-level windows (earth off, or z_mode bottom)");
        for win in live {
            watchdog_log(&format!("start {}: {}", win.label(), platform::chain(win)));
        }
        watchdog_log(&format!("start z-order: {}", platform::zorder_dump(refs, 16)));
    }

    // The event hooks do the real work (see platform::guard_*); telling them
    // which windows to look after is all this needs to do, plus a
    // reconcile() as a safety net in case an event was missed.
    platform::guard_set(refs, true);
    platform::guard_reconcile_now();

    let mut states = Vec::new();
    let mut snap = String::from("loose");
    for win in live {
        let Some(st) = platform::loose_state(win) else { continue };
        snap.push_str(&format!(" {}[{}]", win.label(), st.describe()));
        states.push((win, st));
    }
    let showing = platform::desktop_showing(refs);
    let fg = platform::foreground();
    let (lifts, lowers) = platform::guard_counts();
    snap.push_str(&format!(" desktop_showing={showing} lifts={lifts} lowers={lowers} fg=[{fg}]"));

    if snap != w.last {
        let trouble = states.iter().any(|(_, st)| !st.healthy() || st.cloaked);
        watchdog_log(&snap);
        // The focus landing on the desktop is what Win+D looks like from
        // outside, so that is when to record what the stack looks like.
        if trouble || showing || fg.contains("icons=true") {
            for (win, st) in &states {
                if !st.healthy() || st.cloaked {
                    watchdog_log(&format!("    {}: {}", win.label(), platform::chain(win)));
                }
            }
            watchdog_log(&format!("    z-order: {}", platform::zorder_dump(refs, 16)));
        }
        w.last = snap;
    }

    // Hidden or minimised: bring it back, and put it back where it lives.
    for (win, st) in &states {
        if !st.visible || st.iconic {
            platform::show_noactivate(win);
            if !st.topmost {
                platform::sink_to_bottom(win);
            }
        }
    }
}

fn tick_parked(
    app: &AppHandle,
    w: &mut Watch,
    z_mode: &str,
    host: &WebviewWindow,
    live: &[WebviewWindow],
    refs: &[&WebviewWindow],
) {
    if !w.announced {
        w.announced = true;
        watchdog_log(&format!("start: widgets are children of the wallpaper: {}", platform::chain(host)));
        for win in live {
            watchdog_log(&format!("start {}: {}", win.label(), platform::chain(win)));
        }
    }

    let host_visible = host.is_visible().unwrap_or(true);
    let mut snap = format!("wallpaper[vis={host_visible}]");
    let mut states = Vec::new();
    for win in live {
        let Some(st) = platform::widget_state(win, host, refs) else { continue };
        snap.push_str(&format!(" {}[{}]", win.label(), st.describe()));
        states.push((win, st));
    }

    if snap != w.last {
        let trouble = !host_visible || states.iter().any(|(_, st)| !st.healthy() || st.cloaked);
        if trouble {
            watchdog_log(&format!("{snap}\n    wallpaper: {}", platform::chain(host)));
            for (win, st) in &states {
                if !st.healthy() || st.cloaked {
                    watchdog_log(&format!("    {}: {}", win.label(), platform::chain(win)));
                }
            }
        } else {
            watchdog_log(&snap);
        }
        w.last = snap;
    }

    if !host_visible {
        // Nothing inside a hidden wallpaper can be seen. Put back our own
        // visibility bit; if an ancestor is what got hidden, this is a no-op
        // and the log above is what tells us so.
        platform::show_noactivate(host);
        return;
    }

    for (win, st) in states {
        if st.healthy() {
            continue;
        }
        if !st.in_host {
            let rect = widget_rect(win);
            park_widget(app, win, z_mode);
            if let Some(r) = rect {
                move_widget_to_screen(win, r.x, r.y);
            }
            continue;
        }
        if !st.visible || st.iconic {
            platform::show_noactivate(win);
        }
        if st.covered {
            platform::raise(win);
        }
    }
}

fn spawn_widget_watchdog(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut watch = Watch::new();
        // Let setup finish parking everything before the first look.
        tokio::time::sleep(Duration::from_secs(3)).await;
        loop {
            widget_tick(&app, &mut watch);
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
    });
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
    use tauri::tray::TrayIconBuilder;

    let settings = MenuItem::with_id(app, "settings", "设置…", true, None::<&str>)?;
    let edit = MenuItem::with_id(app, "edit", "编辑模式", true, None::<&str>)?;
    let reload = MenuItem::with_id(app, "reload", "重新载入组件", true, None::<&str>)?;
    let folder = MenuItem::with_id(app, "config", "打开配置目录", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "退出地球桌面", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&settings, &edit, &reload, &folder, &sep, &quit])?;

    let mut builder = TrayIconBuilder::with_id("earth-desk")
        .tooltip("地球桌面")
        .menu(&menu);

    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }

    builder
        .on_menu_event(|app, event| match event.id().as_ref() {
            "settings" => show_settings(app),
            "edit" => toggle_edit_mode(app),
            "reload" => {
                for label in WIDGETS.iter().chain(std::iter::once(&wallpaper::LABEL)) {
                    if let Some(win) = app.get_webview_window(label) {
                        let _ = win.eval("window.location.reload()");
                    }
                }
            }
            "config" => open_config_dir(),
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;

    Ok(())
}

fn main() {
    // No config file yet means this is a fresh install: start with Windows
    // from now on (the settings page can turn it off).
    let first_run = !config::config_path().exists();
    let cfg = Config::load();
    if first_run && !cfg!(debug_assertions) {
        let _ = environment::set_autostart(true);
    }
    let hotkey = cfg.edit_hotkey.clone();

    tauri::Builder::default()
        .manage(AppState {
            config: Mutex::new(cfg),
            editing: Mutex::new(false),
            weather: Mutex::new(None),
            sysmon: Mutex::new(None),
        })
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |app, _shortcut, event| {
                    if event.state() != tauri_plugin_global_shortcut::ShortcutState::Pressed {
                        return;
                    }
                    toggle_edit_mode(app);
                })
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            drag_widget,
            resize_widget,
            commit_layout,
            get_location,
            get_wallpaper_config,
            get_screen_layout,
            get_settings,
            set_feature,
            set_z_mode,
            set_ui_scale,
            set_card_opacity,
            set_fan_roles,
            toggle_editing,
            get_ui_scale,
            get_weather,
            get_sysmon,
            is_editing,
            open_config_dir,
            report_visibility,
            set_location,
            lhm_status,
            lhm_repair,
            get_autostart,
            set_autostart,
            open_url
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            let (widgets, z_mode, wall, enabled) = {
                let state = handle.state::<AppState>();
                let cfg = state.config.lock().expect("config poisoned");
                let enabled: Vec<(String, bool)> = std::iter::once("wallpaper")
                    .chain(WIDGETS.iter().copied())
                    .map(|k| (k.to_string(), cfg.feature(k)))
                    .collect();
                let picked = (cfg.widgets.clone(), cfg.z_mode.clone(), cfg.wallpaper.clone(), enabled);
                picked
            };
            let is_on = |k: &str| enabled.iter().any(|(key, on)| key == k && *on);

            if is_on("wallpaper") {
                wallpaper::setup(&handle, &wall);
            }

            for label in WIDGETS {
                let Some(win) = handle.get_webview_window(label) else { continue };

                // Every widget is sized and placed even when it is switched
                // off, so turning it on later from the settings page just
                // shows it where it belongs.
                let rect = match widgets.get(label) {
                    Some(r) => Some((r.x, r.y, r.w, r.h)),
                    None => DEFAULT_LAYOUT.iter().find(|(l, ..)| *l == label).map(
                        |&(_, x, y, w, h)| {
                            let scale = win.scale_factor().unwrap_or(1.0);
                            let (ox, oy, ..) = platform::primary_screen();
                            (
                                ox + (x * scale).round() as i32,
                                oy + (y * scale).round() as i32,
                                (w * scale).round() as u32,
                                (h * scale).round() as u32,
                            )
                        },
                    ),
                };

                if let Some((_, _, w, h)) = rect {
                    let _ = win.set_size(PhysicalSize::new(w, h));
                }
                platform::mark_as_widget(&win);
                let _ = win.set_ignore_cursor_events(true);
                // Park first: it decides whether the position below is read as
                // a screen coordinate or as an offset inside the wallpaper.
                park_widget(&handle, &win, &z_mode);
                if let Some((x, y, ..)) = rect {
                    move_widget_to_screen(&win, x, y);
                }
                if is_on(label) {
                    let _ = win.show();
                }
            }

            if let Some(overlay) = handle.get_webview_window("guides") {
                platform::mark_as_widget(&overlay);
                let _ = overlay.set_ignore_cursor_events(true);
            }

            if let Err(e) = build_tray(&handle) {
                eprintln!("tray icon unavailable: {e}");
            }

            use tauri_plugin_global_shortcut::GlobalShortcutExt;
            if let Err(e) = handle.global_shortcut().register(hotkey.as_str()) {
                eprintln!("could not register {hotkey}: {e}");
            }

            spawn_weather_loop(handle.clone());
            spawn_sysmon_loop(handle.clone());
            spawn_layer_watchdog(handle.clone());
            // Listens for Show Desktop from the main thread, which is the one
            // that pumps the messages the shell's events arrive as.
            let _ = APP.set(handle.clone());
            platform::guard_on_lift(veil_widgets);
            if !platform::guard_install() {
                eprintln!("could not hook shell events; Show Desktop falls back to polling");
            }
            spawn_widget_watchdog(handle.clone());
            spawn_environment_check(handle.clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the settings window must not end the app -- the
            // wallpaper and widgets keep running and the tray brings the
            // window back.
            if window.label() == "settings" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("failed to start EarthDesk");
}
