//! The wallpaper layer: one opaque window stretched over the whole virtual
//! screen, holding the WebGL globe.
//!
//! It is never interactive and never in the z-order competition -- either it
//! lives inside the shell's own wallpaper window (so desktop icons draw on top
//! of it), or, if the shell would not hand that over, it sits at the bottom of
//! the normal z-order.

use crate::config::Wallpaper;
use crate::platform;
use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize};

pub const LABEL: &str = "wallpaper";

pub fn hide(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(LABEL) {
        let _ = win.hide();
    }
}

pub fn setup(app: &AppHandle, cfg: &Wallpaper) {
    let Some(win) = app.get_webview_window(LABEL) else { return };

    let (x, y, w, h) = platform::virtual_screen();
    let _ = win.set_position(PhysicalPosition::new(x, y));
    let _ = win.set_size(PhysicalSize::new(w as u32, h as u32));
    let _ = win.set_ignore_cursor_events(true);
    let _ = win.set_always_on_top(false);
    platform::mark_as_widget(&win);
    let _ = win.show();

    let attached = if cfg.layer == "behind_icons" {
        platform::attach_wallpaper(&win)
    } else {
        false
    };

    if !attached {
        if cfg.layer == "behind_icons" {
            eprintln!(
                "wallpaper: shell would not give up its wallpaper layer, \
                 falling back to bottom-of-z (desktop icons will be covered)"
            );
        }
        platform::sink_to_bottom(&win);
    }
}

/// Put the window back in its layer if something knocked it out -- explorer.exe
/// restarting, a resolution change, "show desktop".
///
/// This runs on a timer, so it has to be a no-op in the common case where
/// nothing moved. Re-parenting unconditionally means re-probing Progman, and
/// that makes the shell rebuild its wallpaper surface: a visible flash every
/// time the timer fires.
pub fn reassert(app: &AppHandle, cfg: &Wallpaper) {
    let Some(win) = app.get_webview_window(LABEL) else { return };
    if !win.is_visible().unwrap_or(false) {
        return;
    }

    if cfg.layer == "behind_icons" {
        if platform::is_in_wallpaper_layer(&win) {
            return;
        }
        if platform::attach_wallpaper(&win) {
            return;
        }
    }

    // Fallback layer: only touch the window if it has actually drifted. A
    // timer that repositions a full-screen window unconditionally is a
    // flicker generator even when every value it writes is unchanged.
    let (x, y, w, h) = platform::virtual_screen();
    let pos = win.outer_position().ok();
    let size = win.outer_size().ok();
    let drifted = pos.map(|p| (p.x, p.y)) != Some((x, y))
        || size.map(|s| (s.width, s.height)) != Some((w as u32, h as u32));
    if drifted {
        let _ = win.set_position(PhysicalPosition::new(x, y));
        let _ = win.set_size(PhysicalSize::new(w as u32, h as u32));
        platform::sink_to_bottom(&win);
    }
}
