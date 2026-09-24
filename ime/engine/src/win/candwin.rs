//! The candidate window: a layered, topmost, never-activated window drawn
//! with Direct2D, following the text caret of whatever program is typing.
//!
//! Layout (horizontal, like 微信输入法):
//!
//!   ni hao                                 <- spelling, small and grey
//!   [1 你好]  2 拟好  3 你  4 尼 ...   < >  <- candidates, first highlighted
//!
//! Clicking a candidate picks it (the engine then notifies the DLL, which
//! commits); the arrows flip pages. Colours follow the system light / dark
//! app theme.

use crate::session::{Caret, Engine, View};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_numerics::Vector2;

const WM_CAND_UPDATE: u32 = WM_APP + 1;
const WM_CAND_SHOW: u32 = WM_APP + 2;
const CARET_TIMER: usize = 1;
const FLASH_TIMER: usize = 2;

#[derive(Default)]
struct Shared {
    view: Option<View>,
    caret: Option<Caret>,
    visible: bool,
    /// A new view arrived; its caret usually follows a millisecond later
    /// (the DLL measures it after applying the preedit). Wait briefly for
    /// it rather than drawing at the old place and jumping.
    awaiting_caret: bool,
}

static SHARED: Mutex<Shared> = Mutex::new(Shared { view: None, caret: None, visible: false, awaiting_caret: false });
static HWND_: AtomicIsize = AtomicIsize::new(0);
static ENGINE: OnceLock<Arc<Engine>> = OnceLock::new();

fn post() {
    let h = HWND_.load(Ordering::SeqCst);
    if h != 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND(h as *mut _)), WM_CAND_UPDATE, WPARAM(0), LPARAM(0));
        }
    }
}

fn post_msg(msg: u32) {
    let h = HWND_.load(Ordering::SeqCst);
    if h != 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND(h as *mut _)), msg, WPARAM(0), LPARAM(0));
        }
    }
}

pub fn show(view: View, caret: Option<Caret>) {
    let was_visible;
    if let Ok(mut s) = SHARED.lock() {
        was_visible = s.visible;
        s.view = Some(view);
        if caret.is_some() {
            s.caret = caret;
        }
        s.visible = true;
        s.awaiting_caret = !was_visible;
    } else {
        return;
    }
    // Already on screen: redraw now. Newly appearing: wait for the caret.
    post_msg(if was_visible { WM_CAND_UPDATE } else { WM_CAND_SHOW });
}

pub fn move_to(caret: Caret) {
    if let Ok(mut s) = SHARED.lock() {
        let waiting = s.awaiting_caret;
        s.awaiting_caret = false;
        if s.caret == Some(caret) && !waiting {
            return;
        }
        s.caret = Some(caret);
    }
    post();
}

pub fn hide() {
    if let Ok(mut s) = SHARED.lock() {
        if !s.visible {
            return;
        }
        s.visible = false;
    }
    post();
}

fn dark_theme() -> bool {
    let mut v = 1u32;
    let mut size = 4u32;
    unsafe {
        let _ = RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut v as *mut u32 as *mut _),
            Some(&mut size),
        );
    }
    v == 0
}

fn color(hex: u32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: ((hex >> 16) & 255) as f32 / 255.0,
        g: ((hex >> 8) & 255) as f32 / 255.0,
        b: (hex & 255) as f32 / 255.0,
        a,
    }
}

struct Palette {
    bg: D2D1_COLOR_F,
    border: D2D1_COLOR_F,
    text: D2D1_COLOR_F,
    dim: D2D1_COLOR_F,
    hl_bg: D2D1_COLOR_F,
    hl_text: D2D1_COLOR_F,
    label: D2D1_COLOR_F,
}

fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            bg: color(0x2b2b2e, 0.98),
            border: color(0xffffff, 0.10),
            text: color(0xf2f2f7, 1.0),
            dim: color(0x98989f, 1.0),
            hl_bg: color(0x0a84ff, 1.0),
            hl_text: color(0xffffff, 1.0),
            label: color(0x8e8e93, 1.0),
        }
    } else {
        Palette {
            bg: color(0xffffff, 0.98),
            border: color(0x000000, 0.10),
            text: color(0x1c1c1e, 1.0),
            dim: color(0x8e8e93, 1.0),
            hl_bg: color(0x0a84ff, 1.0),
            hl_text: color(0xffffff, 1.0),
            label: color(0x8e8e93, 1.0),
        }
    }
}

/// Where each clickable thing ended up, in window pixels.
#[derive(Default, Clone)]
struct HitMap {
    cands: Vec<(f32, f32, f32, f32)>,
    prev: Option<(f32, f32, f32, f32)>,
    next: Option<(f32, f32, f32, f32)>,
    session: u64,
}

struct Canvas {
    dwrite: IDWriteFactory,
    target: ID2D1DCRenderTarget,
    dc: HDC,
    bitmap: HBITMAP,
    cap: (i32, i32),
    formats: Option<(f32, IDWriteTextFormat, IDWriteTextFormat, IDWriteTextFormat)>,
    hits: HitMap,
    shown: bool,
}

fn inside(r: (f32, f32, f32, f32), x: f32, y: f32) -> bool {
    x >= r.0 && y >= r.1 && x < r.0 + r.2 && y < r.1 + r.3
}

impl Canvas {
    unsafe fn new() -> windows::core::Result<Canvas> {
        let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
        let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: 96.0,
            dpiY: 96.0,
            usage: D2D1_RENDER_TARGET_USAGE_NONE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        };
        let target = factory.CreateDCRenderTarget(&props)?;
        Ok(Canvas {
            dwrite,
            target,
            dc: CreateCompatibleDC(None),
            bitmap: HBITMAP::default(),
            cap: (0, 0),
            formats: None,
            hits: HitMap::default(),
            shown: false,
        })
    }

    unsafe fn reserve(&mut self, w: i32, h: i32) -> bool {
        if w <= self.cap.0 && h <= self.cap.1 && !self.bitmap.is_invalid() {
            return true;
        }
        let (nw, nh) = (w.max(self.cap.0).max(320), h.max(self.cap.1).max(80));
        let bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: nw,
                biHeight: -nh,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let Ok(bmp) = CreateDIBSection(Some(self.dc), &bi, DIB_RGB_COLORS, &mut bits, None, 0) else { return false };
        SelectObject(self.dc, bmp.into());
        if !self.bitmap.is_invalid() {
            let _ = DeleteObject(self.bitmap.into());
        }
        self.bitmap = bmp;
        self.cap = (nw, nh);
        true
    }

    unsafe fn fmt(&mut self, scale: f32) -> Option<(IDWriteTextFormat, IDWriteTextFormat, IDWriteTextFormat)> {
        if let Some((s, a, b, c)) = &self.formats {
            if (*s - scale).abs() < 0.01 {
                return Some((a.clone(), b.clone(), c.clone()));
            }
        }
        let make = |size: f32, weight: DWRITE_FONT_WEIGHT| {
            self.dwrite.CreateTextFormat(
                w!("Microsoft YaHei UI"),
                None,
                weight,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size * scale,
                w!("zh-cn"),
            )
        };
        let cand = make(17.0, DWRITE_FONT_WEIGHT_NORMAL).ok()?;
        let small = make(12.5, DWRITE_FONT_WEIGHT_NORMAL).ok()?;
        let label = make(12.0, DWRITE_FONT_WEIGHT_SEMI_BOLD).ok()?;
        for f in [&cand, &small, &label] {
            let _ = f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
        }
        self.formats = Some((scale, cand.clone(), small.clone(), label.clone()));
        Some((cand, small, label))
    }

    unsafe fn layout(&self, text: &str, f: &IDWriteTextFormat) -> Option<(IDWriteTextLayout, f32, f32)> {
        let wide: Vec<u16> = text.encode_utf16().collect();
        let l = self.dwrite.CreateTextLayout(&wide, f, 4000.0, 400.0).ok()?;
        let mut m = DWRITE_TEXT_METRICS::default();
        l.GetMetrics(&mut m).ok()?;
        Some((l, m.widthIncludingTrailingWhitespace, m.height))
    }

    unsafe fn draw(&mut self, hwnd: HWND) {
        let (view, caret, visible) = {
            let Ok(s) = SHARED.lock() else { return };
            (s.view.clone(), s.caret, s.visible)
        };
        let (Some(view), true) = (view, visible) else {
            if self.shown {
                let _ = ShowWindow(hwnd, SW_HIDE);
                self.shown = false;
            }
            return;
        };
        let caret = caret.unwrap_or_else(|| {
            let mut p = POINT::default();
            let _ = GetCursorPos(&mut p);
            Caret { x: p.x, y: p.y, h: 20 }
        });
        let mon = MonitorFromPoint(POINT { x: caret.x, y: caret.y }, MONITOR_DEFAULTTONEAREST);
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        let s = dx as f32 / 96.0;
        let Some((fcand, fsmall, flabel)) = self.fmt(s) else {
            crate::log("candidate window: no text format");
            return;
        };
        let pal = palette(dark_theme());

        // Measure.
        let pad = 8.0 * s;
        let gap = 14.0 * s;
        let mut items = Vec::new();
        let mut x = pad;
        let pre = self.layout(&view.preedit, &fsmall);
        let pre_h = pre.as_ref().map(|p| p.2).unwrap_or(0.0);
        let row_y = pad + pre_h + 3.0 * s;
        let mut row_h: f32 = 0.0;
        for (i, (text, comment)) in view.candidates.iter().enumerate() {
            let label = view.labels.get(i).cloned().unwrap_or_else(|| (i + 1).to_string());
            let (Some(l), Some(t)) = (self.layout(&label, &flabel), self.layout(text, &fcand)) else { continue };
            let c = if comment.is_empty() { None } else { self.layout(comment, &fsmall) };
            let w = l.1 + 4.0 * s + t.1 + c.as_ref().map(|c| c.1 + 4.0 * s).unwrap_or(0.0);
            row_h = row_h.max(t.2);
            items.push((x, w, l, t, c));
            x += w + gap;
        }
        let arrows_w = if view.flash { 0.0 } else { 36.0 * s };
        let content_w = (x - gap + arrows_w).max(pre.as_ref().map(|p| p.1 + pad).unwrap_or(0.0));
        let width = (content_w + pad * 2.0).ceil() as i32;
        let height = (row_y + row_h + pad * 1.5).ceil() as i32;
        let shadow = (8.0 * s) as i32;
        let (w, h) = (width + shadow * 2, height + shadow * 2);
        if w > 8000 || h > 2000 || !self.reserve(w, h) {
            crate::log(&format!("candidate window: no bitmap for {w}x{h}"));
            return;
        }

        // Place: under the caret line, above it if there is no room.
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut mi);
        let wa = mi.rcWork;
        let mut wx = caret.x - shadow;
        let mut wy = caret.y + caret.h + (2.0 * s) as i32 - shadow;
        if wy + h > wa.bottom {
            wy = caret.y - h - (2.0 * s) as i32 + shadow;
        }
        wx = wx.min(wa.right - w + shadow).max(wa.left - shadow);
        wy = wy.max(wa.top - shadow);

        let rect = RECT { left: 0, top: 0, right: w, bottom: h };
        if let Err(e) = self.target.BindDC(self.dc, &rect) {
            crate::log(&format!("candidate window: BindDC {e}"));
            return;
        }
        let t = &self.target;
        t.BeginDraw();
        t.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
        let ox = shadow as f32;
        let oy = shadow as f32;
        // Soft shadow: a few expanding translucent rounded rects.
        for i in 0..6 {
            let e = i as f32 * 1.3 * s;
            if let Ok(b) = t.CreateSolidColorBrush(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.035 }, None) {
                t.FillRoundedRectangle(
                    &D2D1_ROUNDED_RECT {
                        rect: D2D_RECT_F { left: ox - e, top: oy - e + 2.0 * s, right: ox + width as f32 + e, bottom: oy + height as f32 + e + 2.0 * s },
                        radiusX: 10.0 * s + e,
                        radiusY: 10.0 * s + e,
                    },
                    &b,
                );
            }
        }
        let card = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F { left: ox, top: oy, right: ox + width as f32, bottom: oy + height as f32 },
            radiusX: 10.0 * s,
            radiusY: 10.0 * s,
        };
        if let Ok(b) = t.CreateSolidColorBrush(&pal.bg, None) {
            t.FillRoundedRectangle(&card, &b);
        }
        if let Ok(b) = t.CreateSolidColorBrush(&pal.border, None) {
            t.DrawRoundedRectangle(&card, &b, 1.0, None);
        }
        let brush = |c: &D2D1_COLOR_F| t.CreateSolidColorBrush(c, None).ok();
        if let (Some((l, _, _)), Some(b)) = (&pre, brush(&pal.dim)) {
            t.DrawTextLayout(Vector2 { X: ox + pad, Y: oy + pad }, l, &b, D2D1_DRAW_TEXT_OPTIONS_NONE);
        }
        let mut hits = HitMap { session: view.session, ..Default::default() };
        for (i, (x, iw, l, tx, c)) in items.iter().enumerate() {
            let hl = i as i32 == view.highlighted;
            let cx = ox + pad + x - pad;
            let cy = oy + row_y;
            let cell = (cx - 5.0 * s, cy - 2.0 * s, iw + 10.0 * s, row_h + 4.0 * s);
            if hl {
                if let Some(b) = brush(&pal.hl_bg) {
                    t.FillRoundedRectangle(
                        &D2D1_ROUNDED_RECT {
                            rect: D2D_RECT_F { left: cell.0, top: cell.1, right: cell.0 + cell.2, bottom: cell.1 + cell.3 },
                            radiusX: 6.0 * s,
                            radiusY: 6.0 * s,
                        },
                        &b,
                    );
                }
            }
            let (lc, tc) = if hl { (pal.hl_text, pal.hl_text) } else { (pal.label, pal.text) };
            if let Some(b) = brush(&lc) {
                t.DrawTextLayout(Vector2 { X: cx, Y: cy + (row_h - l.2) * 0.62 }, &l.0, &b, D2D1_DRAW_TEXT_OPTIONS_NONE);
            }
            let tx0 = cx + l.1 + 4.0 * s;
            if let Some(b) = brush(&tc) {
                t.DrawTextLayout(Vector2 { X: tx0, Y: cy }, &tx.0, &b, D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT, );
            }
            if let (Some(c), Some(b)) = (c, brush(if hl { &pal.hl_text } else { &pal.dim })) {
                t.DrawTextLayout(Vector2 { X: tx0 + tx.1 + 4.0 * s, Y: cy + (row_h - c.2) * 0.62 }, &c.0, &b, D2D1_DRAW_TEXT_OPTIONS_NONE);
            }
            hits.cands.push(cell);
        }
        // Page arrows (dimmed when there is nothing that way).
        let ax = ox + width as f32 - pad - arrows_w;
        let ay = oy + row_y + row_h * 0.5;
        let arrows: &[(&str, bool)] = if view.flash { &[] } else { &[("‹", view.page_no > 0), ("›", !view.last_page)] };
        for (k, (sym, enabled)) in arrows.iter().enumerate() {
            if let (Some((l, lw, lh)), Some(b)) = (self.layout(sym, &fcand), brush(if *enabled { &pal.text } else { &pal.border })) {
                let x0 = ax + k as f32 * arrows_w * 0.5 + (arrows_w * 0.5 - lw) * 0.5;
                t.DrawTextLayout(Vector2 { X: x0, Y: ay - lh * 0.55 }, &l, &b, D2D1_DRAW_TEXT_OPTIONS_NONE);
                let r = (ax + k as f32 * arrows_w * 0.5, oy + row_y - 2.0 * s, arrows_w * 0.5, row_h + 4.0 * s);
                if *enabled {
                    if k == 0 {
                        hits.prev = Some(r);
                    } else {
                        hits.next = Some(r);
                    }
                }
            }
        }
        if let Err(e) = t.EndDraw(None, None) {
            crate::log(&format!("candidate window: EndDraw {e}"));
            return;
        }
        self.hits = hits;
        if std::env::var_os("EARTHDESK_IME_DEBUG").is_some() {
            crate::log(&format!("candidate window: {} items at {wx},{wy} {w}x{h}", items.len()));
        }
        let dst = POINT { x: wx, y: wy };
        let size = SIZE { cx: w, cy: h };
        let src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
        if let Err(e) = UpdateLayeredWindow(hwnd, None, Some(&dst), Some(&size), Some(self.dc), Some(&src), COLORREF(0), Some(&blend), ULW_ALPHA) {
            crate::log(&format!("candidate window: UpdateLayeredWindow {e}"));
        }
        if !self.shown {
            let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
            self.shown = true;
        } else {
            // Stay above the window being typed in, which may itself be
            // topmost.
            let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }

    fn click(&self, x: f32, y: f32) {
        let Some(engine) = ENGINE.get() else { return };
        let session = self.hits.session;
        if let Some(i) = self.hits.cands.iter().position(|r| inside(*r, x, y)) {
            let e = engine.clone();
            std::thread::spawn(move || e.pick(session, Some(i), None));
        } else if self.hits.prev.map(|r| inside(r, x, y)).unwrap_or(false) {
            let e = engine.clone();
            std::thread::spawn(move || e.pick(session, None, Some(true)));
        } else if self.hits.next.map(|r| inside(r, x, y)).unwrap_or(false) {
            let e = engine.clone();
            std::thread::spawn(move || e.pick(session, None, Some(false)));
        }
    }
}

thread_local! {
    static CANVAS: std::cell::RefCell<Option<Canvas>> = const { std::cell::RefCell::new(None) };
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_CAND_SHOW => {
            let _ = SetTimer(Some(hwnd), CARET_TIMER, 40, None);
            if SHARED.lock().map(|s| s.view.as_ref().map(|v| v.flash).unwrap_or(false)).unwrap_or(false) {
                let _ = SetTimer(Some(hwnd), FLASH_TIMER, 1200, None);
            }
            LRESULT(0)
        }
        WM_TIMER if wp.0 == CARET_TIMER => {
            let _ = KillTimer(Some(hwnd), CARET_TIMER);
            if let Ok(mut s) = SHARED.lock() {
                s.awaiting_caret = false;
            }
            CANVAS.with(|c| {
                if let Some(c) = c.borrow_mut().as_mut() {
                    c.draw(hwnd);
                }
            });
            LRESULT(0)
        }
        WM_TIMER if wp.0 == FLASH_TIMER => {
            let _ = KillTimer(Some(hwnd), FLASH_TIMER);
            if let Ok(mut s) = SHARED.lock() {
                if s.view.as_ref().map(|v| v.flash).unwrap_or(false) {
                    s.visible = false;
                }
            }
            CANVAS.with(|c| {
                if let Some(c) = c.borrow_mut().as_mut() {
                    c.draw(hwnd);
                }
            });
            LRESULT(0)
        }
        WM_CAND_UPDATE => {
            let _ = KillTimer(Some(hwnd), CARET_TIMER);
            if SHARED.lock().map(|s| s.visible && s.view.as_ref().map(|v| v.flash).unwrap_or(false)).unwrap_or(false) {
                let _ = SetTimer(Some(hwnd), FLASH_TIMER, 1200, None);
            }
            CANVAS.with(|c| {
                if let Some(c) = c.borrow_mut().as_mut() {
                    c.draw(hwnd);
                }
            });
            LRESULT(0)
        }
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        // taskkill (installer upgrade) and logoff: flush the user dictionary.
        WM_CLOSE => {
            if let Some(e) = ENGINE.get() {
                e.shutdown();
            }
            LRESULT(0)
        }
        WM_QUERYENDSESSION => LRESULT(1),
        WM_ENDSESSION if wp.0 != 0 => {
            if let Some(e) = ENGINE.get() {
                e.shutdown();
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let x = (lp.0 & 0xffff) as i16 as f32;
            let y = ((lp.0 >> 16) & 0xffff) as i16 as f32;
            CANVAS.with(|c| {
                if let Some(c) = c.borrow().as_ref() {
                    c.click(x, y);
                }
            });
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            // Wheel over the window flips pages.
            let delta = ((wp.0 >> 16) & 0xffff) as i16;
            let session = CANVAS.with(|c| c.borrow().as_ref().map(|c| c.hits.session).unwrap_or(0));
            if let Some(e) = ENGINE.get() {
                let e = e.clone();
                std::thread::spawn(move || e.pick(session, None, Some(delta > 0)));
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

pub fn start(engine: Arc<Engine>) {
    let _ = ENGINE.set(engine);
    std::thread::Builder::new()
        .name("candidates".into())
        .spawn(|| unsafe {
            let inst = GetModuleHandleW(None).unwrap_or_default();
            let class = w!("EarthDeskIMECandidates");
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(wndproc),
                hInstance: inst.into(),
                lpszClassName: class,
                hCursor: LoadCursorW(None, IDC_HAND).unwrap_or_default(),
                ..Default::default()
            };
            if RegisterClassExW(&wc) == 0 {
                crate::log(&format!("candidate window: RegisterClassEx failed: {:?}", windows::core::Error::from_win32()));
            }
            let Ok(hwnd) = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                class,
                PCWSTR::null(),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                Some(inst.into()),
                None,
            ) else {
                crate::log(&format!("candidate window: CreateWindowEx failed: {:?}", windows::core::Error::from_win32()));
                return;
            };
            match Canvas::new() {
                Ok(c) => CANVAS.with(|s| *s.borrow_mut() = Some(c)),
                Err(e) => crate::log(&format!("candidate window: Direct2D unavailable: {e}")),
            }
            HWND_.store(hwnd.0 as isize, Ordering::SeqCst);
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        })
        .ok();
}
