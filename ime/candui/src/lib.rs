//! Drawing the candidate window: a layered, topmost, never-activated window
//! painted with Direct2D next to the text caret.
//!
//! Shared by the engine process (which shows the window for ordinary
//! programs) and the TSF DLL (which shows it inside immersive hosts such as
//! the Start menu's search, where only a window of the host's own process
//! can appear above the host).
//!
//! Layout (horizontal, like 微信输入法):
//!
//!   ni hao                                 <- spelling, small and grey
//!   [1 你好]  2 拟好  3 你  4 尼 ...   < >  <- candidates, first highlighted
//!
//! Colours follow the system light / dark app theme, or a skin (see
//! `ime_proto::Skin`): "time" tints the window by the time of day (always
//! light), "weather" paints the coming hours' sky along the bar, now at the
//! left — the longer the bar, the further ahead it looks.

#![cfg(windows)]

pub use ime_proto::Cands;
use windows::core::{w, Interface, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_numerics::Vector2;

/// Where the composition starts, in physical screen pixels (top-left of the
/// line, plus the line height).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Caret {
    pub x: i32,
    pub y: i32,
    pub h: i32,
}

/// What a click landed on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Hit {
    /// A candidate on the current page.
    Cand(usize),
    /// The page arrows.
    Prev,
    Next,
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

#[derive(Clone, Copy)]
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

fn mix(a: D2D1_COLOR_F, b: D2D1_COLOR_F, t: f32) -> D2D1_COLOR_F {
    let t = t.clamp(0.0, 1.0);
    D2D1_COLOR_F { r: a.r + (b.r - a.r) * t, g: a.g + (b.g - a.g) * t, b: a.b + (b.b - a.b) * t, a: a.a + (b.a - a.a) * t }
}

/// A light palette over a tinted background (the skins never go dark).
fn skinned(accent: D2D1_COLOR_F) -> Palette {
    Palette {
        bg: color(0xffffff, 0.98),
        border: color(0x000000, 0.08),
        text: color(0x1c1c1e, 1.0),
        dim: color(0x5c626d, 1.0),
        hl_bg: accent,
        hl_text: color(0xffffff, 1.0),
        label: color(0x6b717c, 1.0),
    }
}

/// The local time of day in hours (13.5 = half past one).
fn local_hour() -> f32 {
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    t.wHour as f32 + t.wMinute as f32 / 60.0
}

/// The time skin through the day: (hour, left, right, accent). Dawn is
/// peach, the morning fresh blue, noon a bright warm white, the afternoon
/// amber, dusk coral into lavender, the night a pale periwinkle.
const DAY: &[(f32, u32, u32, u32)] = &[
    (0.0, 0xe4e6f6, 0xdde2f3, 0x5a67d0),
    (4.5, 0xe6e4f4, 0xe9e0ef, 0x6c62c8),
    (6.0, 0xfbe3e6, 0xfdeedd, 0xdb6b86),
    (8.0, 0xfff4e2, 0xe3f1fc, 0x2f8fe0),
    (10.5, 0xe4f3ff, 0xe8f8ef, 0x1f8be6),
    (13.0, 0xfffae6, 0xeef6ff, 0xe39419),
    (15.5, 0xfff0d9, 0xfde6d2, 0xe07f22),
    (18.0, 0xfde0d6, 0xebdef5, 0xd9604f),
    (19.5, 0xebdef6, 0xe0e3f8, 0x8565d2),
    (22.0, 0xe4e5f7, 0xdde2f3, 0x5a67d0),
    (24.0, 0xe4e6f6, 0xdde2f3, 0x5a67d0),
];

fn time_colors(hour: f32) -> (D2D1_COLOR_F, D2D1_COLOR_F, D2D1_COLOR_F) {
    let h = hour.rem_euclid(24.0);
    let i = DAY.windows(2).position(|w| h >= w[0].0 && h < w[1].0).unwrap_or(0);
    let (a, b) = (DAY[i], DAY[i + 1]);
    let t = (h - a.0) / (b.0 - a.0);
    // Ease, so each colour holds a while and moves on gently.
    let t = t * t * (3.0 - 2.0 * t);
    (mix(color(a.1, 0.98), color(b.1, 0.98), t), mix(color(a.2, 0.98), color(b.2, 0.98), t), mix(color(a.3, 1.0), color(b.3, 1.0), t))
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Sky {
    Clear,
    Partly,
    Cloudy,
    Fog,
    Drizzle,
    Rain,
    Snow,
    Thunder,
}

fn sky(code: u8) -> Sky {
    match code {
        0 | 1 => Sky::Clear,
        2 => Sky::Partly,
        3 => Sky::Cloudy,
        45 | 48 => Sky::Fog,
        51..=57 => Sky::Drizzle,
        61..=67 | 80..=82 => Sky::Rain,
        71..=77 | 85 | 86 => Sky::Snow,
        95..=99 => Sky::Thunder,
        _ => Sky::Cloudy,
    }
}

/// Background and highlight for a kind of sky (by day / by night).
fn sky_colors(k: Sky, day: bool) -> (D2D1_COLOR_F, D2D1_COLOR_F) {
    let (bg, accent) = match (k, day) {
        (Sky::Clear, true) => (0xd5ebff, 0x2d89e3),
        (Sky::Clear, false) => (0xe2e0f6, 0x6760d2),
        (Sky::Partly, true) => (0xdcebf9, 0x3a84cf),
        (Sky::Partly, false) => (0xe0e0f1, 0x6461c4),
        (Sky::Cloudy, true) => (0xe2e7ee, 0x5b7897),
        (Sky::Cloudy, false) => (0xdddfea, 0x5f6690),
        (Sky::Fog, _) => (0xe8eaed, 0x6f7d8a),
        (Sky::Drizzle, _) => (0xdbe5ef, 0x3f74aa),
        (Sky::Rain, _) => (0xd2ddea, 0x376aa3),
        (Sky::Snow, _) => (0xebf1f8, 0x5a8bc0),
        (Sky::Thunder, _) => (0xdbd8e8, 0x6556b0),
    };
    (color(bg, 0.98), color(accent, 1.0))
}

/// The same scatter every time (so the rain does not jump as you type).
fn hash(i: u32) -> f32 {
    let mut x = i.wrapping_mul(0x9E37_79B9) ^ 0x85EB_CA6B;
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    (x & 0xffff) as f32 / 65535.0
}

/// How the window is painted.
struct Look {
    pal: Palette,
    /// Background colours left to right (position 0..1); empty = plain.
    stops: Vec<(f32, D2D1_COLOR_F)>,
    /// Weather along the bar, now first.
    hours: Vec<(Sky, bool)>,
    corner: String,
}

fn look(skin: &ime_proto::Skin) -> Look {
    match skin.kind.as_str() {
        "time" => {
            let (l, r, accent) = time_colors(local_hour());
            Look { pal: skinned(accent), stops: vec![(0.0, l), (1.0, r)], hours: Vec::new(), corner: String::new() }
        }
        "weather" if !skin.hours.is_empty() => {
            let hours: Vec<(Sky, bool)> = skin.hours.iter().map(|&(c, d)| (sky(c), d)).collect();
            let (_, accent) = sky_colors(hours[0].0, hours[0].1);
            Look { pal: skinned(accent), stops: Vec::new(), hours, corner: skin.label.clone() }
        }
        // No weather known yet: the time of day stands in.
        "weather" => look(&ime_proto::Skin { kind: "time".into(), ..Default::default() }),
        _ => Look { pal: palette(dark_theme()), stops: Vec::new(), hours: Vec::new(), corner: String::new() },
    }
}

/// Where each clickable thing ended up, in window pixels.
#[derive(Default, Clone)]
struct HitMap {
    cands: Vec<(f32, f32, f32, f32)>,
    prev: Option<(f32, f32, f32, f32)>,
    next: Option<(f32, f32, f32, f32)>,
}

pub struct Canvas {
    factory: ID2D1Factory,
    dwrite: IDWriteFactory,
    target: ID2D1DCRenderTarget,
    dc: HDC,
    bitmap: HBITMAP,
    cap: (i32, i32),
    formats: Option<(f32, IDWriteTextFormat, IDWriteTextFormat, IDWriteTextFormat)>,
    hits: HitMap,
    shown: bool,
    retrying: bool,
}

fn dc_props() -> D2D1_RENDER_TARGET_PROPERTIES {
    D2D1_RENDER_TARGET_PROPERTIES {
        r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
        pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
        dpiX: 96.0,
        dpiY: 96.0,
        usage: D2D1_RENDER_TARGET_USAGE_NONE,
        minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
    }
}

fn inside(r: (f32, f32, f32, f32), x: f32, y: f32) -> bool {
    x >= r.0 && y >= r.1 && x < r.0 + r.2 && y < r.1 + r.3
}

/// Register the window class (once per module) and make the window: a
/// layered, topmost, never-activated tool window.
///
/// # Safety
/// `inst` must be the module that owns `wndproc`.
pub unsafe fn create_window(inst: HINSTANCE, class: PCWSTR, wndproc: WNDPROC) -> windows::core::Result<HWND> {
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: wndproc,
        hInstance: inst,
        lpszClassName: class,
        hCursor: LoadCursorW(None, IDC_HAND).unwrap_or_default(),
        ..Default::default()
    };
    // Fails harmlessly when the class exists already (a second thread).
    RegisterClassExW(&wc);
    CreateWindowExW(
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
        Some(inst),
        None,
    )
}

impl Canvas {
    /// # Safety
    /// Call on the thread that owns the window it will draw.
    pub unsafe fn new() -> windows::core::Result<Canvas> {
        let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
        let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
        let target = factory.CreateDCRenderTarget(&dc_props())?;
        Ok(Canvas {
            factory,
            dwrite,
            target,
            dc: CreateCompatibleDC(None),
            bitmap: HBITMAP::default(),
            cap: (0, 0),
            formats: None,
            hits: HitMap::default(),
            shown: false,
            retrying: false,
        })
    }

    pub fn shown(&self) -> bool {
        self.shown
    }

    /// # Safety
    /// `hwnd` is the window this canvas draws.
    pub unsafe fn hide(&mut self, hwnd: HWND) {
        if self.shown {
            let _ = ShowWindow(hwnd, SW_HIDE);
            self.shown = false;
        }
    }

    /// What is at window pixel (x, y).
    pub fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        if let Some(i) = self.hits.cands.iter().position(|r| inside(*r, x, y)) {
            Some(Hit::Cand(i))
        } else if self.hits.prev.map(|r| inside(r, x, y)).unwrap_or(false) {
            Some(Hit::Prev)
        } else if self.hits.next.map(|r| inside(r, x, y)).unwrap_or(false) {
            Some(Hit::Next)
        } else {
            None
        }
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

    /// Draw `view` under the caret (above it if there is no room) and show
    /// the window. `caret` None = near the mouse pointer. Coordinates are
    /// physical pixels: the calling thread must be per-monitor DPI aware.
    ///
    /// # Safety
    /// `hwnd` is the window this canvas draws, owned by this thread.
    pub unsafe fn draw(&mut self, hwnd: HWND, view: &Cands, caret: Option<Caret>, log: &dyn Fn(&str)) {
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
            log("candidate window: no text format");
            return;
        };
        let lk = look(&view.skin);
        let pal = lk.pal;

        // Measure. The first line: the letters as typed (when they read
        // differently), the reading, and the weather in the corner.
        let pad = 8.0 * s;
        let gap = 14.0 * s;
        let vgap = 6.0 * s;
        let typed = if view.typed.is_empty() { None } else { self.layout(&view.typed, &flabel) };
        let typed_w = typed.as_ref().map(|t| t.1 + 7.0 * s).unwrap_or(0.0);
        let pre = self.layout(&view.preedit, &fsmall);
        let corner = if lk.corner.is_empty() { None } else { self.layout(&lk.corner, &fsmall) };
        let corner_w = corner.as_ref().map(|c| c.1 + 16.0 * s).unwrap_or(0.0);
        let head_w = typed_w + pre.as_ref().map(|p| p.1).unwrap_or(0.0) + corner_w;
        let pre_h = pre.as_ref().map(|p| p.2).unwrap_or(0.0).max(typed.as_ref().map(|t| t.2).unwrap_or(0.0));
        let row_y = pad + pre_h + 3.0 * s;
        let mut row_h: f32 = 0.0;
        let mut measured = Vec::new();
        for (i, (text, comment)) in view.candidates.iter().enumerate() {
            let label = view.labels.get(i).cloned().unwrap_or_else(|| (i + 1).to_string());
            let (Some(l), Some(t)) = (self.layout(&label, &flabel), self.layout(text, &fcand)) else { continue };
            let c = if comment.is_empty() { None } else { self.layout(comment, &fsmall) };
            let w = l.1 + 4.0 * s + t.1 + c.as_ref().map(|c| c.1 + 4.0 * s).unwrap_or(0.0);
            row_h = row_h.max(t.2);
            measured.push((w, l, t, c));
        }
        let arrows_w = if view.flash { 0.0 } else { 36.0 * s };
        // (x, y, cell width, label, text, comment); a row, or a column for
        // a word's other spellings (the page arrows then go below it).
        let mut items = Vec::new();
        let (content_w, bottom, arrows_y) = if view.vertical {
            let col_w = measured.iter().map(|m| m.0).fold(0.0f32, f32::max);
            let mut y = row_y;
            for (_, l, t, c) in measured {
                items.push((pad, y, col_w, l, t, c));
                y += row_h + vgap;
            }
            let arrows_y = y;
            (col_w.max(arrows_w).max(head_w), arrows_y + row_h, arrows_y)
        } else {
            let mut x = pad;
            for (w, l, t, c) in measured {
                items.push((x, row_y, w, l, t, c));
                x += w + gap;
            }
            ((x - gap + arrows_w).max(head_w + pad), row_y + row_h, row_y)
        };
        let width = (content_w + pad * 2.0).ceil() as i32;
        let height = (bottom + pad * 1.5).ceil() as i32;
        let shadow = (8.0 * s) as i32;
        let (w, h) = (width + shadow * 2, height + shadow * 2);
        if w > 8000 || h > 2000 || !self.reserve(w, h) {
            log(&format!("candidate window: no bitmap for {w}x{h}"));
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
            log(&format!("candidate window: BindDC {e}"));
            return;
        }
        self.target.BeginDraw();
        let t = &self.target;
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
        self.paint_skin(&lk, &card.rect, corner_w, s);
        let t = &self.target;
        if let Ok(b) = t.CreateSolidColorBrush(&pal.border, None) {
            t.DrawRoundedRectangle(&card, &b, 1.0, None);
        }
        let brush = |c: &D2D1_COLOR_F| t.CreateSolidColorBrush(c, None).ok();
        if let (Some((l, _, lh)), Some(b)) = (&typed, brush(&pal.text)) {
            t.DrawTextLayout(Vector2 { X: ox + pad, Y: oy + pad + (pre_h - lh) * 0.6 }, l, &b, D2D1_DRAW_TEXT_OPTIONS_NONE);
        }
        if let (Some((l, _, lh)), Some(b)) = (&pre, brush(&pal.dim)) {
            t.DrawTextLayout(Vector2 { X: ox + pad + typed_w, Y: oy + pad + (pre_h - lh) * 0.6 }, l, &b, D2D1_DRAW_TEXT_OPTIONS_NONE);
        }
        if let (Some((l, lw, lh)), Some(b)) = (&corner, brush(&pal.dim)) {
            t.DrawTextLayout(Vector2 { X: ox + width as f32 - pad - lw, Y: oy + pad + (pre_h - lh) * 0.6 }, l, &b, D2D1_DRAW_TEXT_OPTIONS_NONE);
        }
        let mut hits = HitMap::default();
        for (i, (x, y, iw, l, tx, c)) in items.iter().enumerate() {
            let hl = i as i32 == view.highlighted;
            let cx = ox + x;
            let cy = oy + y;
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
                t.DrawTextLayout(Vector2 { X: tx0, Y: cy }, &tx.0, &b, D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT);
            }
            if let (Some(c), Some(b)) = (c, brush(if hl { &pal.hl_text } else { &pal.dim })) {
                t.DrawTextLayout(Vector2 { X: tx0 + tx.1 + 4.0 * s, Y: cy + (row_h - c.2) * 0.62 }, &c.0, &b, D2D1_DRAW_TEXT_OPTIONS_NONE);
            }
            hits.cands.push(cell);
        }
        // Page arrows (dimmed when there is nothing that way).
        let ax = ox + width as f32 - pad - arrows_w;
        let ay = oy + arrows_y + row_h * 0.5;
        let arrows: &[(&str, bool)] = if view.flash { &[] } else { &[("‹", view.page_no > 0), ("›", !view.last_page)] };
        for (k, (sym, enabled)) in arrows.iter().enumerate() {
            if let (Some((l, lw, lh)), Some(b)) = (self.layout(sym, &fcand), brush(if *enabled { &pal.text } else { &pal.border })) {
                let x0 = ax + k as f32 * arrows_w * 0.5 + (arrows_w * 0.5 - lw) * 0.5;
                t.DrawTextLayout(Vector2 { X: x0, Y: ay - lh * 0.55 }, &l, &b, D2D1_DRAW_TEXT_OPTIONS_NONE);
                let r = (ax + k as f32 * arrows_w * 0.5, oy + arrows_y - 2.0 * s, arrows_w * 0.5, row_h + 4.0 * s);
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
            // After sleep, a display change or a driver reset the render
            // target is lost (D2DERR_RECREATE_TARGET): make a new one and
            // draw again, or the window would stay blank for good.
            log(&format!("candidate window: EndDraw {e}, recreating"));
            if let Ok(nt) = self.factory.CreateDCRenderTarget(&dc_props()) {
                self.target = nt;
                if !self.retrying {
                    self.retrying = true;
                    self.draw(hwnd, view, Some(caret), log);
                    self.retrying = false;
                }
            }
            return;
        }
        self.hits = hits;
        if std::env::var_os("EARTHDESK_IME_DEBUG").is_some() {
            log(&format!("candidate window: {} items at {wx},{wy} {w}x{h}", items.len()));
        }
        let dst = POINT { x: wx, y: wy };
        let size = SIZE { cx: w, cy: h };
        let src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
        if let Err(e) = UpdateLayeredWindow(hwnd, None, Some(&dst), Some(&size), Some(self.dc), Some(&src), COLORREF(0), Some(&blend), ULW_ALPHA) {
            log(&format!("candidate window: UpdateLayeredWindow {e}"));
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
}

impl Canvas {
    /// The skin's background and weather, clipped to the card.
    /// `corner`: the width the weather label takes at the top right.
    unsafe fn paint_skin(&self, lk: &Look, card: &D2D_RECT_F, corner: f32, s: f32) {
        if lk.stops.is_empty() && lk.hours.is_empty() {
            return;
        }
        let t = &self.target;
        let w = card.right - card.left;
        let h = card.bottom - card.top;
        let r = D2D1_ROUNDED_RECT { rect: *card, radiusX: 10.0 * s, radiusY: 10.0 * s };
        let Ok(geo) = self.factory.CreateRoundedRectangleGeometry(&r) else { return };
        let Ok(geo) = geo.cast::<ID2D1Geometry>() else { return };
        let Ok(layer) = t.CreateLayer(None) else { return };
        let params = D2D1_LAYER_PARAMETERS {
            contentBounds: *card,
            geometricMask: std::mem::ManuallyDrop::new(Some(geo)),
            maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            maskTransform: windows_numerics::Matrix3x2::identity(),
            opacity: 1.0,
            opacityBrush: std::mem::ManuallyDrop::new(None),
            layerOptions: D2D1_LAYER_OPTIONS_NONE,
        };
        t.PushLayer(&params, &layer);
        let _ = std::mem::ManuallyDrop::into_inner(params.geometricMask);

        // As many hours as fit, about 120 px each.
        let n = if lk.hours.is_empty() { 0 } else { ((w / (120.0 * s)).floor() as usize).clamp(1, lk.hours.len()) };
        let seg = if n > 0 { w / n as f32 } else { w };
        let mut stops = lk.stops.clone();
        if n > 0 {
            stops.clear();
            for (i, &(k, d)) in lk.hours.iter().take(n).enumerate() {
                let c = sky_colors(k, d).0;
                // Each hour holds its colour over most of its stretch and
                // blends into the next at the border.
                stops.push(((i as f32 + 0.2) / n as f32, c));
                stops.push(((i as f32 + 0.8) / n as f32, c));
            }
        }
        let gs: Vec<D2D1_GRADIENT_STOP> = stops.iter().map(|&(p, c)| D2D1_GRADIENT_STOP { position: p, color: c }).collect();
        if let Ok(coll) = t.CreateGradientStopCollection(&gs, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP) {
            let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES { startPoint: Vector2 { X: card.left, Y: 0.0 }, endPoint: Vector2 { X: card.right, Y: 0.0 } };
            if let Ok(b) = t.CreateLinearGradientBrush(&props, None, &coll) {
                t.FillRectangle(card, &b);
            }
        }
        // A soft sheen from the top, so the tint reads as sky, not paint.
        let sheen = [D2D1_GRADIENT_STOP { position: 0.0, color: color(0xffffff, 0.45) }, D2D1_GRADIENT_STOP { position: 1.0, color: color(0xffffff, 0.0) }];
        if let Ok(coll) = t.CreateGradientStopCollection(&sheen, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP) {
            let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES { startPoint: Vector2 { X: 0.0, Y: card.top }, endPoint: Vector2 { X: 0.0, Y: card.top + h * 0.7 } };
            if let Ok(b) = t.CreateLinearGradientBrush(&props, None, &coll) {
                t.FillRectangle(card, &b);
            }
        }
        for (i, &(k, d)) in lk.hours.iter().take(n).enumerate() {
            let x0 = card.left + seg * i as f32;
            // The sun / moon keeps clear of the label.
            let keep = (x0 + seg - (card.right - corner)).max(0.0);
            self.paint_hour(k, d, D2D_RECT_F { left: x0, top: card.top, right: x0 + seg, bottom: card.bottom }, keep, i as u32, s);
        }
        t.PopLayer();
    }

    /// One hour's weather in its stretch of the bar. Kept faint: the
    /// candidates are read over it.
    unsafe fn paint_hour(&self, k: Sky, day: bool, r: D2D_RECT_F, keep: f32, seed: u32, s: f32) {
        let t = &self.target;
        let w = r.right - r.left;
        let h = r.bottom - r.top;
        let fill = |c: D2D1_COLOR_F, x: f32, y: f32, rx: f32, ry: f32| {
            if let Ok(b) = t.CreateSolidColorBrush(&c, None) {
                t.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: x, Y: y }, radiusX: rx, radiusY: ry }, &b);
            }
        };
        let line = |c: D2D1_COLOR_F, a: (f32, f32), b: (f32, f32), width: f32| {
            if let Ok(br) = t.CreateSolidColorBrush(&c, None) {
                t.DrawLine(Vector2 { X: a.0, Y: a.1 }, Vector2 { X: b.0, Y: b.1 }, &br, width, None);
            }
        };
        let bg = sky_colors(k, day).0;
        // Sun or moon, top right of the stretch.
        let (cx, cy) = ((r.right - keep - 22.0 * s).max(r.left + 16.0 * s), r.top + 16.0 * s);
        let body = matches!(k, Sky::Clear | Sky::Partly);
        if body && day {
            fill(color(0xffc53d, 0.12), cx, cy, 17.0 * s, 17.0 * s);
            fill(color(0xffc53d, 0.18), cx, cy, 12.0 * s, 12.0 * s);
            fill(color(0xffc53d, 0.55), cx, cy, 7.5 * s, 7.5 * s);
        } else if body {
            fill(color(0xf4d27a, 0.25), cx, cy, 12.0 * s, 12.0 * s);
            fill(color(0xf1cf6e, 0.75), cx, cy, 7.0 * s, 7.0 * s);
            fill(bg, cx + 3.6 * s, cy - 2.4 * s, 6.2 * s, 6.2 * s);
            for j in 0..4u32 {
                let sx = r.left + w * (0.12 + 0.7 * hash(seed * 31 + j * 7));
                let sy = r.top + h * (0.15 + 0.7 * hash(seed * 17 + j * 13 + 1));
                fill(color(0x8d8fd8, 0.35), sx, sy, 1.3 * s, 1.3 * s);
            }
        }
        // Clouds.
        let cloud = |x: f32, y: f32, z: f32, c: D2D1_COLOR_F| {
            fill(color(0x8796a8, 0.10), x + 1.0 * s, y + 3.0 * s, 17.0 * z, 6.0 * z);
            fill(c, x - 8.0 * z, y, 8.0 * z, 6.0 * z);
            fill(c, x + 1.0 * z, y - 4.0 * z, 9.0 * z, 8.0 * z);
            fill(c, x + 10.0 * z, y + 0.5 * z, 7.0 * z, 5.5 * z);
            fill(c, x + 1.0 * z, y + 2.5 * z, 15.0 * z, 4.0 * z);
        };
        let white = color(0xffffff, 0.9);
        let grey = color(0xf7f8fb, 0.85);
        let mid = r.top + h * 0.46;
        match k {
            Sky::Partly => cloud(cx - 10.0 * s, cy + 9.0 * s, 1.1 * s, white),
            Sky::Cloudy | Sky::Drizzle | Sky::Rain | Sky::Snow | Sky::Thunder => {
                cloud(r.left + w * 0.28, mid, 1.35 * s, grey);
                cloud(r.left + w * 0.74, mid - 5.0 * s, 1.1 * s, grey);
            }
            _ => {}
        }
        match k {
            Sky::Fog => {
                for j in 0..3 {
                    let y = r.top + h * (0.35 + 0.22 * j as f32);
                    let off = hash(seed * 5 + j) * w * 0.2;
                    line(color(0xffffff, 0.7), (r.left + off, y), (r.right - w * 0.15 + off, y), 3.0 * s);
                }
            }
            Sky::Drizzle | Sky::Rain | Sky::Thunder => {
                let (count, len, a) = if k == Sky::Drizzle { (9, 4.0, 0.25) } else { (15, 7.0, 0.32) };
                for j in 0..count {
                    let x = r.left + w * hash(seed * 101 + j);
                    let y = mid + 4.0 * s + (r.bottom - mid - 8.0 * s) * hash(seed * 67 + j * 3 + 2);
                    line(color(0x4a73a3, a), (x, y), (x - 2.0 * s, y + len * s), 1.1 * s);
                }
                if k == Sky::Thunder {
                    let (bx, by) = (r.left + w * 0.52, mid + 2.0 * s);
                    let bolt = color(0xf2b705, 0.6);
                    line(bolt, (bx, by), (bx - 4.0 * s, by + 9.0 * s), 1.8 * s);
                    line(bolt, (bx - 4.0 * s, by + 9.0 * s), (bx + 1.0 * s, by + 9.0 * s), 1.8 * s);
                    line(bolt, (bx + 1.0 * s, by + 9.0 * s), (bx - 3.0 * s, by + 18.0 * s), 1.8 * s);
                }
            }
            Sky::Snow => {
                for j in 0..12 {
                    let x = r.left + w * hash(seed * 211 + j);
                    let y = mid + 3.0 * s + (r.bottom - mid - 5.0 * s) * hash(seed * 89 + j * 5 + 4);
                    let z = (1.1 + 0.9 * hash(seed * 7 + j)) * s;
                    fill(color(0x8fa9c8, 0.4), x, y, z, z);
                }
            }
            _ => {}
        }
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        unsafe {
            // The DC first: a bitmap still selected into one cannot go.
            if !self.dc.is_invalid() {
                let _ = DeleteDC(self.dc);
            }
            if !self.bitmap.is_invalid() {
                let _ = DeleteObject(self.bitmap.into());
            }
        }
    }
}
