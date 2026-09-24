//! The gesture trail and the hint next to the pointer.
//!
//! A native layered window rather than a WebView: it has to appear on the
//! first pixel of movement and follow the pointer without lag. The window is
//! only as large as the stroke's bounding box (plus the hint), so each update
//! pushes a small bitmap, not the whole virtual screen. Drawing is Direct2D
//! into a DIB section, handed to UpdateLayeredWindow.
//!
//! Runs on its own thread with its own message loop; the hook thread only
//! appends points and posts a message, so drawing never delays input.

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::Mutex;
use windows::core::{w, PCWSTR};
use windows_numerics::Vector2;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

const WM_TRAIL_DRAW: u32 = WM_APP + 1;
const WM_TRAIL_END: u32 = WM_APP + 2;
const FADE_TIMER: usize = 7;

struct State {
    points: Vec<(i32, i32)>,
    hint: Option<(String, bool)>,
    color: (f32, f32, f32),
    width: f32,
    scale: f32,
    show_trail: bool,
    active: bool,
}

static STATE: Mutex<State> = Mutex::new(State {
    points: Vec::new(),
    hint: None,
    color: (0.24, 0.55, 1.0),
    width: 4.0,
    scale: 1.0,
    show_trail: true,
    active: false,
});
static HWND_: AtomicIsize = AtomicIsize::new(0);
static QUEUED: AtomicBool = AtomicBool::new(false);

fn post(msg: u32) {
    let h = HWND_.load(Ordering::SeqCst);
    if h != 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND(h as *mut _)), msg, WPARAM(0), LPARAM(0));
        }
    }
}

fn queue_draw() {
    if !QUEUED.swap(true, Ordering::SeqCst) {
        post(WM_TRAIL_DRAW);
    }
}

fn parse_color(s: &str) -> (f32, f32, f32) {
    let h = s.trim().trim_start_matches('#');
    if h.len() == 6 {
        if let Ok(v) = u32::from_str_radix(h, 16) {
            return (
                ((v >> 16) & 255) as f32 / 255.0,
                ((v >> 8) & 255) as f32 / 255.0,
                (v & 255) as f32 / 255.0,
            );
        }
    }
    (0.24, 0.55, 1.0)
}

pub fn begin(start: (i32, i32), scale: f64, color: &str, width: f64, show_trail: bool) {
    if let Ok(mut s) = STATE.lock() {
        s.points.clear();
        s.points.push(start);
        s.hint = None;
        s.color = parse_color(color);
        s.width = width as f32;
        s.scale = scale as f32;
        s.show_trail = show_trail;
        s.active = true;
    }
}

pub fn point(p: (i32, i32)) {
    if let Ok(mut s) = STATE.lock() {
        if !s.active {
            return;
        }
        // A very long scribble does not need every sample.
        if s.points.len() > 4000 {
            let keep: Vec<_> = s.points.iter().copied().step_by(2).collect();
            s.points = keep;
        }
        s.points.push(p);
    }
    queue_draw();
}

pub fn hint(text: Option<(String, bool)>) {
    if let Ok(mut s) = STATE.lock() {
        s.hint = text;
    }
    queue_draw();
}

pub fn end() {
    if let Ok(mut s) = STATE.lock() {
        s.active = false;
    }
    post(WM_TRAIL_END);
}

struct Canvas {
    factory: ID2D1Factory,
    dwrite: IDWriteFactory,
    format: Option<(f32, IDWriteTextFormat)>,
    target: ID2D1DCRenderTarget,
    stroke: ID2D1StrokeStyle,
    dc: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
    cap: (i32, i32),
    shown: bool,
    alpha: u8,
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
        let stroke = factory.CreateStrokeStyle(
            &D2D1_STROKE_STYLE_PROPERTIES {
                startCap: D2D1_CAP_STYLE_ROUND,
                endCap: D2D1_CAP_STYLE_ROUND,
                dashCap: D2D1_CAP_STYLE_ROUND,
                lineJoin: D2D1_LINE_JOIN_ROUND,
                miterLimit: 10.0,
                dashStyle: D2D1_DASH_STYLE_SOLID,
                dashOffset: 0.0,
            },
            None,
        )?;
        let dc = CreateCompatibleDC(None);
        Ok(Canvas {
            factory,
            dwrite,
            format: None,
            target,
            stroke,
            dc,
            bitmap: HBITMAP::default(),
            old: HGDIOBJ::default(),
            cap: (0, 0),
            shown: false,
            alpha: 255,
        })
    }

    /// Make sure the DIB is at least w x h.
    unsafe fn reserve(&mut self, w: i32, h: i32) -> bool {
        if w <= self.cap.0 && h <= self.cap.1 && !self.bitmap.is_invalid() {
            return true;
        }
        let (nw, nh) = (w.max(self.cap.0).max(256), h.max(self.cap.1).max(128));
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
        let Ok(bmp) = CreateDIBSection(Some(self.dc), &bi, DIB_RGB_COLORS, &mut bits, None, 0) else {
            return false;
        };
        let prev = SelectObject(self.dc, bmp.into());
        if self.bitmap.is_invalid() {
            self.old = prev;
        } else {
            let _ = DeleteObject(self.bitmap.into());
        }
        self.bitmap = bmp;
        self.cap = (nw, nh);
        true
    }

    unsafe fn text_format(&mut self, size: f32) -> Option<IDWriteTextFormat> {
        if let Some((s, f)) = &self.format {
            if (*s - size).abs() < 0.1 {
                return Some(f.clone());
            }
        }
        let f = self
            .dwrite
            .CreateTextFormat(
                w!("Microsoft YaHei UI"),
                None,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size,
                w!("zh-cn"),
            )
            .ok()?;
        self.format = Some((size, f.clone()));
        Some(f)
    }

    unsafe fn draw(&mut self, hwnd: HWND) {
        let (points, hint, color, width, scale, show_trail) = {
            let Ok(s) = STATE.lock() else { return };
            if !s.active {
                return;
            }
            (s.points.clone(), s.hint.clone(), s.color, s.width, s.scale, s.show_trail)
        };
        if points.is_empty() {
            return;
        }
        let stroke_w = (width * scale).max(1.0);
        let pad = (stroke_w * 2.0 + 4.0) as i32;
        let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        if show_trail {
            for &(x, y) in &points {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
        let last = *points.last().unwrap();

        // The hint sits just below-right of the pointer.
        let mut layout = None;
        if let Some((text, _)) = &hint {
            if let Some(fmt) = self.text_format(15.0 * scale) {
                let wide: Vec<u16> = text.encode_utf16().collect();
                if let Ok(l) = self.dwrite.CreateTextLayout(&wide, &fmt, 800.0 * scale, 100.0 * scale) {
                    let mut m = DWRITE_TEXT_METRICS::default();
                    let _ = l.GetMetrics(&mut m);
                    let (pw, ph) = (12.0 * scale, 7.0 * scale);
                    let bw = (m.width + pw * 2.0).ceil() as i32;
                    let bh = (m.height + ph * 2.0).ceil() as i32;
                    let bx = last.0 + (20.0 * scale) as i32;
                    let by = last.1 + (22.0 * scale) as i32;
                    x0 = x0.min(bx);
                    y0 = y0.min(by);
                    x1 = x1.max(bx + bw);
                    y1 = y1.max(by + bh);
                    layout = Some((l, bx, by, bw, bh, pw, ph));
                }
            }
        }
        if x0 == i32::MAX {
            return;
        }
        let (ox, oy) = (x0 - pad, y0 - pad);
        let (w, h) = ((x1 - x0) + pad * 2, (y1 - y0) + pad * 2);
        if w <= 0 || h <= 0 || w > 16384 || h > 16384 || !self.reserve(w, h) {
            return;
        }
        let rect = RECT { left: 0, top: 0, right: w, bottom: h };
        if self.target.BindDC(self.dc, &rect).is_err() {
            return;
        }
        let t = &self.target;
        t.BeginDraw();
        t.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
        let fx = |x: i32| (x - ox) as f32;
        let fy = |y: i32| (y - oy) as f32;

        if show_trail && points.len() > 1 {
            if let Ok(geom) = self.factory.CreatePathGeometry() {
                if let Ok(sink) = geom.Open() {
                    sink.BeginFigure(Vector2 { X: fx(points[0].0), Y: fy(points[0].1) }, D2D1_FIGURE_BEGIN_HOLLOW);
                    let rest: Vec<Vector2> =
                        points[1..].iter().map(|&(x, y)| Vector2 { X: fx(x), Y: fy(y) }).collect();
                    sink.AddLines(&rest);
                    sink.EndFigure(D2D1_FIGURE_END_OPEN);
                    let _ = sink.Close();
                    // A soft white halo under the stroke keeps it visible on
                    // dark and busy backgrounds alike.
                    if let Ok(halo) =
                        t.CreateSolidColorBrush(&D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 0.55 }, None)
                    {
                        t.DrawGeometry(&geom, &halo, stroke_w + 3.0 * scale, &self.stroke);
                    }
                    if let Ok(b) =
                        t.CreateSolidColorBrush(&D2D1_COLOR_F { r: color.0, g: color.1, b: color.2, a: 0.95 }, None)
                    {
                        t.DrawGeometry(&geom, &b, stroke_w, &self.stroke);
                    }
                }
            }
        }

        if let Some((l, bx, by, bw, bh, pw, ph)) = layout {
            let matched = hint.as_ref().map(|h| h.1).unwrap_or(false);
            let bg = D2D1_COLOR_F { r: 0.08, g: 0.09, b: 0.11, a: 0.86 };
            if let Ok(b) = t.CreateSolidColorBrush(&bg, None) {
                let rr = D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left: fx(bx),
                        top: fy(by),
                        right: fx(bx + bw),
                        bottom: fy(by + bh),
                    },
                    radiusX: 8.0 * scale,
                    radiusY: 8.0 * scale,
                };
                t.FillRoundedRectangle(&rr, &b);
            }
            let fg = if matched {
                D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 1.0 }
            } else {
                D2D1_COLOR_F { r: 0.62, g: 0.64, b: 0.68, a: 1.0 }
            };
            if let Ok(b) = t.CreateSolidColorBrush(&fg, None) {
                t.DrawTextLayout(
                    Vector2 { X: fx(bx) + pw, Y: fy(by) + ph },
                    &l,
                    &b,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                );
            }
        }
        if t.EndDraw(None, None).is_err() {
            return;
        }

        self.alpha = 255;
        let dst = POINT { x: ox, y: oy };
        let size = SIZE { cx: w, cy: h };
        let src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let _ = UpdateLayeredWindow(
            hwnd,
            None,
            Some(&dst),
            Some(&size),
            Some(self.dc),
            Some(&src),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        );
        // A new gesture can start while the last one is still fading out.
        let _ = KillTimer(Some(hwnd), FADE_TIMER);
        if !self.shown {
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            self.shown = true;
        }
    }

    unsafe fn fade_step(&mut self, hwnd: HWND) {
        self.alpha = self.alpha.saturating_sub(48);
        if self.alpha == 0 {
            let _ = KillTimer(Some(hwnd), FADE_TIMER);
            let _ = ShowWindow(hwnd, SW_HIDE);
            self.shown = false;
            return;
        }
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: self.alpha,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let _ = UpdateLayeredWindow(hwnd, None, None, None, None, None, COLORREF(0), Some(&blend), ULW_ALPHA);
    }
}

thread_local! {
    static CANVAS: std::cell::RefCell<Option<Canvas>> = const { std::cell::RefCell::new(None) };
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_TRAIL_DRAW => {
            QUEUED.store(false, Ordering::SeqCst);
            CANVAS.with(|c| {
                if let Some(c) = c.borrow_mut().as_mut() {
                    c.draw(hwnd);
                }
            });
            LRESULT(0)
        }
        WM_TRAIL_END => {
            CANVAS.with(|c| {
                if let Some(c) = c.borrow_mut().as_mut() {
                    if c.shown {
                        let _ = SetTimer(Some(hwnd), FADE_TIMER, 15, None);
                    }
                }
            });
            LRESULT(0)
        }
        WM_TIMER if wp.0 == FADE_TIMER => {
            CANVAS.with(|c| {
                if let Some(c) = c.borrow_mut().as_mut() {
                    c.fade_step(hwnd);
                }
            });
            LRESULT(0)
        }
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

pub fn start() {
    std::thread::Builder::new()
        .name("gesture-trail".into())
        .spawn(|| unsafe {
            let _ = windows::Win32::UI::HiDpi::SetThreadDpiAwarenessContext(
                windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            );
            let inst = GetModuleHandleW(None).unwrap_or_default();
            let class = w!("EarthDeskGestureTrail");
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(wndproc),
                hInstance: inst.into(),
                lpszClassName: class,
                ..Default::default()
            };
            RegisterClassExW(&wc);
            let Ok(hwnd) = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
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
                eprintln!("gesture trail window unavailable");
                return;
            };
            match Canvas::new() {
                Ok(c) => CANVAS.with(|slot| *slot.borrow_mut() = Some(c)),
                Err(e) => eprintln!("gesture trail: Direct2D unavailable: {e}"),
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
