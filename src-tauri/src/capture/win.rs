//! Windows side of the screenshot: grabbing the screen, the native "freeze"
//! window that shows the grabbed image the instant F1 is pressed, finding
//! windows and controls to snap to, and the file dialogs.

use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::HiDpi::{SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

/// A grabbed screen: the whole virtual desktop, top-down BGRA.
pub struct Frame {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub bgra: Arc<Vec<u8>>,
}

pub fn virtual_screen() -> (i32, i32, i32, i32) {
    unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    }
}

/// Copy the screen. GDI BitBlt from the screen DC returns the composited
/// desktop (DWM), layered windows included with CAPTUREBLT. Fast enough for
/// 4K (tens of milliseconds) and needs no per-adapter setup.
pub fn grab(include_cursor: bool) -> Option<Frame> {
    let (x, y, w, h) = virtual_screen();
    if w <= 0 || h <= 0 {
        return None;
    }
    unsafe {
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let bmp = CreateDIBSection(Some(mem), &bi, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
        let old = SelectObject(mem, bmp.into());
        let ok = BitBlt(mem, 0, 0, w, h, Some(screen), x, y, SRCCOPY | CAPTUREBLT).is_ok();
        if ok && include_cursor {
            let mut ci = CURSORINFO { cbSize: std::mem::size_of::<CURSORINFO>() as u32, ..Default::default() };
            if GetCursorInfo(&mut ci).is_ok() && ci.flags.0 & CURSOR_SHOWING.0 != 0 {
                let mut ii = ICONINFO::default();
                if GetIconInfo(ci.hCursor.into(), &mut ii).is_ok() {
                    let _ = DrawIconEx(
                        mem,
                        ci.ptScreenPos.x - x - ii.xHotspot as i32,
                        ci.ptScreenPos.y - y - ii.yHotspot as i32,
                        ci.hCursor.into(),
                        0,
                        0,
                        0,
                        None,
                        DI_NORMAL,
                    );
                    if !ii.hbmColor.is_invalid() {
                        let _ = DeleteObject(ii.hbmColor.into());
                    }
                    if !ii.hbmMask.is_invalid() {
                        let _ = DeleteObject(ii.hbmMask.into());
                    }
                }
            }
        }
        let len = (w as usize) * (h as usize) * 4;
        let data = if ok { std::slice::from_raw_parts(bits as *const u8, len).to_vec() } else { Vec::new() };
        SelectObject(mem, old);
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        if !ok {
            return None;
        }
        Some(Frame { x, y, w, h, bgra: Arc::new(data) })
    }
}

// --- freeze window --------------------------------------------------------------

struct Freeze {
    frame: Option<(i32, i32, i32, i32, Arc<Vec<u8>>)>,
}
static FREEZE: Mutex<Freeze> = Mutex::new(Freeze { frame: None });
static FREEZE_HWND: AtomicIsize = AtomicIsize::new(0);
const WM_FREEZE_SHOW: u32 = WM_APP + 30;
const WM_FREEZE_HIDE: u32 = WM_APP + 31;

unsafe extern "system" fn freeze_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_FREEZE_SHOW => {
            let rect = FREEZE.lock().ok().and_then(|f| f.frame.as_ref().map(|(x, y, w, h, _)| (*x, *y, *w, *h)));
            if let Some((x, y, w, h)) = rect {
                let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, w, h, SWP_NOACTIVATE | SWP_SHOWWINDOW);
                let _ = RedrawWindow(Some(hwnd), None, None, RDW_INVALIDATE | RDW_UPDATENOW);
            }
            LRESULT(0)
        }
        WM_FREEZE_HIDE => {
            let _ = ShowWindow(hwnd, SW_HIDE);
            if let Ok(mut f) = FREEZE.lock() {
                f.frame = None;
            }
            LRESULT(0)
        }
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let dc = BeginPaint(hwnd, &mut ps);
            if let Ok(f) = FREEZE.lock() {
                if let Some((_, _, w, h, bits)) = &f.frame {
                    let bi = BITMAPINFO {
                        bmiHeader: BITMAPINFOHEADER {
                            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                            biWidth: *w,
                            biHeight: -*h,
                            biPlanes: 1,
                            biBitCount: 32,
                            biCompression: BI_RGB.0,
                            ..Default::default()
                        },
                        ..Default::default()
                    };
                    SetDIBitsToDevice(dc, 0, 0, *w as u32, *h as u32, 0, 0, 0, *h as u32, bits.as_ptr() as *const _, &bi, DIB_RGB_COLORS);
                }
            }
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

pub fn start_freeze_thread() {
    std::thread::Builder::new()
        .name("capture-freeze".into())
        .spawn(|| unsafe {
            let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let inst = GetModuleHandleW(None).unwrap_or_default();
            let class = w!("EarthDeskCaptureFreeze");
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(freeze_proc),
                hInstance: inst.into(),
                lpszClassName: class,
                hCursor: LoadCursorW(None, IDC_CROSS).unwrap_or_default(),
                ..Default::default()
            };
            RegisterClassExW(&wc);
            let Ok(hwnd) = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
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
                return;
            };
            FREEZE_HWND.store(hwnd.0 as isize, Ordering::SeqCst);
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        })
        .ok();
}

fn post_freeze(msg: u32) {
    let h = FREEZE_HWND.load(Ordering::SeqCst);
    if h != 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND(h as *mut _)), msg, WPARAM(0), LPARAM(0));
        }
    }
}

pub fn freeze_show(f: &Frame) {
    if let Ok(mut s) = FREEZE.lock() {
        s.frame = Some((f.x, f.y, f.w, f.h, f.bgra.clone()));
    }
    post_freeze(WM_FREEZE_SHOW);
}

pub fn freeze_hide() {
    post_freeze(WM_FREEZE_HIDE);
}

pub fn freeze_hwnd() -> isize {
    FREEZE_HWND.load(Ordering::SeqCst)
}

// --- windows and controls to snap to ---------------------------------------------

#[derive(serde::Serialize)]
pub struct Win {
    pub hwnd: isize,
    pub rect: [i32; 4],
    pub title: String,
    pub children: Vec<[i32; 4]>,
}

fn rect4(r: RECT) -> [i32; 4] {
    [r.left, r.top, r.right - r.left, r.bottom - r.top]
}

unsafe fn frame_rect(h: HWND) -> Option<RECT> {
    let mut r = RECT::default();
    if DwmGetWindowAttribute(h, DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut _ as *mut _, std::mem::size_of::<RECT>() as u32)
        .is_err()
    {
        GetWindowRect(h, &mut r).ok()?;
    }
    (r.right > r.left && r.bottom > r.top).then_some(r)
}

unsafe fn cloaked(h: HWND) -> bool {
    let mut v = 0u32;
    DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut v as *mut u32 as *mut _, 4).is_ok() && v != 0
}

unsafe extern "system" fn collect_child(h: HWND, lp: LPARAM) -> windows::core::BOOL {
    let out = &mut *(lp.0 as *mut Vec<[i32; 4]>);
    if out.len() > 800 {
        return false.into();
    }
    if IsWindowVisible(h).as_bool() {
        let mut r = RECT::default();
        if GetWindowRect(h, &mut r).is_ok() && r.right - r.left >= 6 && r.bottom - r.top >= 6 {
            out.push(rect4(r));
        }
    }
    true.into()
}

/// Top-level windows from front to back, each with its visible child
/// windows. Menus, tooltips and pop-ups are included (Snipaste snaps to
/// them too); click-through overlays and our own windows are not.
pub fn windows_front_to_back() -> Vec<Win> {
    let me = unsafe { GetCurrentProcessId() };
    let mut out = Vec::new();
    unsafe {
        let mut h = GetTopWindow(None).unwrap_or_default();
        while !h.0.is_null() && out.len() < 400 {
            let next = GetWindow(h, GW_HWNDNEXT).unwrap_or_default();
            let mut pid = 0u32;
            GetWindowThreadProcessId(h, Some(&mut pid));
            let ex = GetWindowLongW(h, GWL_EXSTYLE) as u32;
            if pid != me
                && IsWindowVisible(h).as_bool()
                && !IsIconic(h).as_bool()
                && !cloaked(h)
                && ex & WS_EX_TRANSPARENT.0 == 0
            {
                if let Some(r) = frame_rect(h) {
                    let mut children: Vec<[i32; 4]> = Vec::new();
                    let _ = EnumChildWindows(Some(h), Some(collect_child), LPARAM(&mut children as *mut _ as isize));
                    let mut buf = [0u16; 256];
                    let n = GetWindowTextW(h, &mut buf);
                    out.push(Win {
                        hwnd: h.0 as isize,
                        rect: rect4(r),
                        title: String::from_utf16_lossy(&buf[..n.max(0) as usize]),
                        children,
                    });
                }
            }
            h = next;
        }
    }
    out
}

/// Controls inside one window, through UI Automation: buttons, fields, list
/// items, toolbar parts... Walks the control view depth-first until a time
/// budget runs out, so a huge document never holds the capture up.
pub fn ui_elements(hwnd: isize, budget: Duration) -> Vec<[i32; 4]> {
    let mut out = Vec::new();
    let deadline = Instant::now() + budget;
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let Ok(uia) = CoCreateInstance::<_, IUIAutomation>(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)
            .or_else(|_| CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER))
        else {
            return out;
        };
        if let Ok(u2) = windows::core::Interface::cast::<IUIAutomation2>(&uia) {
            let _ = u2.SetConnectionTimeout(400);
            let _ = u2.SetTransactionTimeout(400);
        }
        let Ok(cache) = uia.CreateCacheRequest() else { return out };
        let _ = cache.AddProperty(UIA_BoundingRectanglePropertyId);
        let _ = cache.AddProperty(UIA_IsOffscreenPropertyId);
        let Ok(walker) = uia.ControlViewWalker() else { return out };
        let Ok(root) = uia.ElementFromHandleBuildCache(HWND(hwnd as *mut _), &cache) else { return out };
        let clip = root.CachedBoundingRectangle().unwrap_or_default();
        let mut stack: Vec<(IUIAutomationElement, u32)> = vec![(root, 0)];
        while let Some((el, depth)) = stack.pop() {
            if Instant::now() > deadline || out.len() > 4000 {
                break;
            }
            if depth > 0 {
                let off = el.CachedIsOffscreen().map(|b| b.as_bool()).unwrap_or(false);
                if off {
                    continue;
                }
                if let Ok(r) = el.CachedBoundingRectangle() {
                    // Clip to the window: scrolled-away parts report rects
                    // outside it.
                    let r = RECT {
                        left: r.left.max(clip.left),
                        top: r.top.max(clip.top),
                        right: r.right.min(clip.right),
                        bottom: r.bottom.min(clip.bottom),
                    };
                    if r.right - r.left >= 4 && r.bottom - r.top >= 4 {
                        out.push(rect4(r));
                    }
                }
            }
            if depth >= 24 {
                continue;
            }
            // Children are pushed in reverse so they are visited in order.
            let mut kids = Vec::new();
            let mut c = walker.GetFirstChildElementBuildCache(&el, &cache).ok();
            while let Some(k) = c {
                if kids.len() > 400 || Instant::now() > deadline {
                    break;
                }
                c = walker.GetNextSiblingElementBuildCache(&k, &cache).ok();
                kids.push(k);
            }
            for k in kids.into_iter().rev() {
                stack.push((k, depth + 1));
            }
        }
    }
    out
}

// --- dialogs and misc -------------------------------------------------------------

/// Local wall-clock time: (year, month, day, hour, minute, second, ms).
pub fn local_time() -> (u16, u16, u16, u16, u16, u16, u16) {
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    (t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond, t.wMilliseconds)
}

pub fn save_dialog(dir: &str, name: &str, jpg_first: bool) -> Option<String> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
        let dlg: IFileSaveDialog = CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let png = COMDLG_FILTERSPEC { pszName: w!("PNG 图片"), pszSpec: w!("*.png") };
        let jpg = COMDLG_FILTERSPEC { pszName: w!("JPEG 图片"), pszSpec: w!("*.jpg;*.jpeg") };
        let bmp = COMDLG_FILTERSPEC { pszName: w!("BMP 图片"), pszSpec: w!("*.bmp") };
        let filters = if jpg_first { [jpg, png, bmp] } else { [png, jpg, bmp] };
        let _ = dlg.SetFileTypes(&filters);
        let _ = dlg.SetDefaultExtension(if jpg_first { w!("jpg") } else { w!("png") });
        let _ = dlg.SetFileName(&HSTRING::from(name));
        let _ = dlg.SetTitle(w!("保存截图"));
        if let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(dir), None) {
            let _ = dlg.SetFolder(&item);
        }
        dlg.Show(None).ok()?;
        let item = dlg.GetResult().ok()?;
        let p = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s
    }
}

pub fn pick_folder(start: &str) -> Option<String> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
        let dlg: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let opts = dlg.GetOptions().unwrap_or_default();
        let _ = dlg.SetOptions(opts | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM);
        let _ = dlg.SetTitle(w!("选择截图保存位置"));
        if let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(start), None) {
            let _ = dlg.SetFolder(&item);
        }
        dlg.Show(None).ok()?;
        let item = dlg.GetResult().ok()?;
        let p = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s
    }
}

/// Raise and focus one of our windows even though another program is in
/// front (the hotkey arrived through a hook, which does not make us the
/// foreground process).
pub fn force_foreground(hwnd: isize) {
    crate::toolkit::win::apps::force_foreground(HWND(hwnd as *mut _));
}

/// Put one of our windows exactly over a physical-pixel rectangle, below
/// the freeze window so the switch-over is invisible.
pub fn place_below_freeze(hwnd: isize, x: i32, y: i32, w: i32, h: i32) {
    unsafe {
        let f = freeze_hwnd();
        let after = if f != 0 { HWND(f as *mut _) } else { HWND_TOPMOST };
        let target = HWND(hwnd as *mut _);
        let ex = GetWindowLongW(target, GWL_EXSTYLE) as u32;
        if ex & WS_EX_TOPMOST.0 == 0 {
            let _ = SetWindowPos(target, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
        let _ = SetWindowPos(target, Some(after), x, y, w, h, SWP_NOACTIVATE);
    }
}

pub fn cursor_pos() -> Option<(i32, i32)> {
    let mut p = POINT::default();
    unsafe { GetCursorPos(&mut p).ok()? };
    Some((p.x, p.y))
}

pub fn nudge_cursor(dx: i32, dy: i32) {
    if let Some((x, y)) = cursor_pos() {
        unsafe {
            let _ = SetCursorPos(x + dx, y + dy);
        }
    }
}

/// Move and size one of our windows (physical pixels) without activating it.
pub fn set_rect(hwnd: isize, x: i32, y: i32, w: i32, h: i32) {
    unsafe {
        let _ = SetWindowPos(HWND(hwnd as *mut _), None, x, y, w, h, SWP_NOACTIVATE | SWP_NOZORDER);
    }
}
