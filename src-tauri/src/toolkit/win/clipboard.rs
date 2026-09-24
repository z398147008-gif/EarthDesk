//! Writing to the clipboard: text, and images as both a DIB (what every
//! Windows program understands) and PNG (what browsers, Office and chat apps
//! prefer, and which keeps exact pixels).
//!
//! `mark_self` adds a private format, `EarthDesk.Self`, which the clipboard
//! history reads as "we put this back ourselves, do not record it again"
//! (pasting an old entry). Screenshots are not marked: a copied screenshot
//! belongs in the history like any other copy.

use windows::core::w;
use windows::Win32::Foundation::{GlobalFree, HANDLE, HWND};
use windows::Win32::Graphics::Gdi::{BITMAPINFOHEADER, BI_RGB};
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::*;
use windows::Win32::System::Ole::{CF_DIB, CF_UNICODETEXT};

pub fn self_format() -> u32 {
    unsafe { RegisterClipboardFormatW(w!("EarthDesk.Self")) }
}

pub fn png_format() -> u32 {
    unsafe { RegisterClipboardFormatW(w!("PNG")) }
}

/// Open the clipboard, retrying for a moment: another program may hold it.
unsafe fn open() -> bool {
    for _ in 0..20 {
        if OpenClipboard(Some(HWND::default())).is_ok() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    false
}

unsafe fn put(format: u32, bytes: &[u8]) -> bool {
    let Ok(h) = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1)) else { return false };
    let p = GlobalLock(h) as *mut u8;
    if p.is_null() {
        let _ = GlobalFree(Some(h));
        return false;
    }
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
    let _ = GlobalUnlock(h);
    if SetClipboardData(format, Some(HANDLE(h.0))).is_err() {
        let _ = GlobalFree(Some(h));
        return false;
    }
    true
}

fn mark() -> Vec<u8> {
    vec![1]
}

pub fn set_text(text: &str, mark_self: bool) -> Result<(), String> {
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    wide.push(0);
    let bytes = unsafe { std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2) };
    unsafe {
        if !open() {
            return Err("剪贴板被其他程序占用".into());
        }
        let _ = EmptyClipboard();
        let ok = put(CF_UNICODETEXT.0 as u32, bytes);
        if mark_self {
            put(self_format(), &mark());
        }
        let _ = CloseClipboard();
        if ok {
            Ok(())
        } else {
            Err("写入剪贴板失败".into())
        }
    }
}

/// `rgba`: top-down, 4 bytes per pixel. `png`: the same image already
/// encoded (optional).
pub fn set_image(width: u32, height: u32, rgba: &[u8], png: Option<&[u8]>, mark_self: bool) -> Result<(), String> {
    let stride = width as usize * 4;
    if rgba.len() < stride * height as usize {
        return Err("图像数据不完整".into());
    }
    // CF_DIB: header + bottom-up BGRA rows.
    let header = BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: width as i32,
        biHeight: height as i32,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB.0,
        biSizeImage: (stride * height as usize) as u32,
        ..Default::default()
    };
    let mut dib = Vec::with_capacity(std::mem::size_of::<BITMAPINFOHEADER>() + stride * height as usize);
    dib.extend_from_slice(unsafe {
        std::slice::from_raw_parts(&header as *const _ as *const u8, std::mem::size_of::<BITMAPINFOHEADER>())
    });
    for y in (0..height as usize).rev() {
        let row = &rgba[y * stride..(y + 1) * stride];
        for px in row.chunks_exact(4) {
            dib.extend_from_slice(&[px[2], px[1], px[0], 255]);
        }
    }
    unsafe {
        if !open() {
            return Err("剪贴板被其他程序占用".into());
        }
        let _ = EmptyClipboard();
        let ok = put(CF_DIB.0 as u32, &dib);
        if let Some(p) = png {
            put(png_format(), p);
        }
        if mark_self {
            put(self_format(), &mark());
        }
        let _ = CloseClipboard();
        if ok {
            Ok(())
        } else {
            Err("写入剪贴板失败".into())
        }
    }
}

// --- reading ---------------------------------------------------------------------

/// What F3 (pin the clipboard) and the clipboard history can do with the
/// clipboard's current content, best representation first.
pub enum Content {
    /// An encoded image (the "PNG" format, or an image file that was copied).
    Encoded(Vec<u8>, &'static str),
    /// Decoded pixels, top-down RGBA.
    Rgba(u32, u32, Vec<u8>),
    Files(Vec<String>),
    Text(String),
}

unsafe fn data(format: u32) -> Option<Vec<u8>> {
    if !IsClipboardFormatAvailable(format).is_ok() {
        return None;
    }
    let h = GetClipboardData(format).ok()?;
    let g = windows::Win32::Foundation::HGLOBAL(h.0);
    let size = GlobalSize(g);
    let p = GlobalLock(g) as *const u8;
    if p.is_null() || size == 0 {
        return None;
    }
    let v = std::slice::from_raw_parts(p, size).to_vec();
    let _ = GlobalUnlock(g);
    Some(v)
}

/// A packed DIB (CF_DIB / CF_DIBV5) to top-down RGBA. 24- and 32-bit only,
/// which is what every screenshot and browser copy produces.
pub fn dib_to_rgba(d: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    if d.len() < 40 {
        return None;
    }
    let u32at = |o: usize| u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]]);
    let size = u32at(0) as usize;
    let w = i32::from_le_bytes([d[4], d[5], d[6], d[7]]);
    let h = i32::from_le_bytes([d[8], d[9], d[10], d[11]]);
    let bpp = u16::from_le_bytes([d[14], d[15]]);
    let compression = u32at(16);
    let used = u32at(32) as usize;
    if w <= 0 || h == 0 || !(bpp == 24 || bpp == 32) || !(compression == 0 || compression == 3) {
        return None;
    }
    let (w, top_down, hh) = (w as usize, h < 0, h.unsigned_abs() as usize);
    let mut masks = (0x00FF_0000u32, 0x0000_FF00u32, 0x0000_00FFu32, 0xFF00_0000u32);
    let mut offset = size;
    if compression == 3 {
        if size == 40 {
            if d.len() < 52 {
                return None;
            }
            masks = (u32at(40), u32at(44), u32at(48), 0);
            offset += 12;
        } else if d.len() >= 56 {
            masks = (u32at(40), u32at(44), u32at(48), u32at(52));
        }
    }
    offset += used * 4; // colour table, normally empty for these depths
    let stride = (w * bpp as usize / 8 + 3) & !3;
    if d.len() < offset + stride * hh {
        return None;
    }
    let shift = |m: u32| if m == 0 { 0 } else { m.trailing_zeros() };
    let pick = |v: u32, m: u32| if m == 0 { 0u8 } else { (((v & m) >> shift(m)) * 255 / (m >> shift(m))) as u8 };
    let mut out = vec![0u8; w * hh * 4];
    let mut any_alpha = false;
    for y in 0..hh {
        let src_row = if top_down { y } else { hh - 1 - y };
        let row = &d[offset + src_row * stride..];
        for x in 0..w {
            let o = (y * w + x) * 4;
            if bpp == 24 {
                let p = &row[x * 3..x * 3 + 3];
                out[o..o + 4].copy_from_slice(&[p[2], p[1], p[0], 255]);
            } else {
                let v = u32::from_le_bytes([row[x * 4], row[x * 4 + 1], row[x * 4 + 2], row[x * 4 + 3]]);
                let a = pick(v, masks.3);
                any_alpha |= a != 0;
                out[o..o + 4].copy_from_slice(&[pick(v, masks.0), pick(v, masks.1), pick(v, masks.2), a]);
            }
        }
    }
    // Most programs leave the fourth byte at zero: that means opaque, not
    // invisible.
    if bpp == 32 && !any_alpha {
        for px in out.chunks_exact_mut(4) {
            px[3] = 255;
        }
    }
    Some((w as u32, hh as u32, out))
}

unsafe fn files() -> Option<Vec<String>> {
    use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
    let cf_hdrop = 15u32;
    if !IsClipboardFormatAvailable(cf_hdrop).is_ok() {
        return None;
    }
    let h = GetClipboardData(cf_hdrop).ok()?;
    let drop = HDROP(h.0);
    let n = DragQueryFileW(drop, u32::MAX, None);
    let mut out = Vec::new();
    for i in 0..n.min(500) {
        let len = DragQueryFileW(drop, i, None) as usize;
        let mut buf = vec![0u16; len + 1];
        DragQueryFileW(drop, i, Some(&mut buf));
        out.push(String::from_utf16_lossy(&buf[..len]));
    }
    Some(out)
}

fn image_mime(path: &str) -> Option<&'static str> {
    let p = path.to_ascii_lowercase();
    [(".png", "image/png"), (".jpg", "image/jpeg"), (".jpeg", "image/jpeg"), (".bmp", "image/bmp"), (".gif", "image/gif"), (".webp", "image/webp")]
        .iter()
        .find(|(e, _)| p.ends_with(e))
        .map(|(_, m)| *m)
}

/// The clipboard's content in the form that pins best. An image file that
/// was copied in Explorer counts as that image.
pub fn read_for_pin() -> Option<Content> {
    unsafe {
        if !open() {
            return None;
        }
        let result = (|| {
            if let Some(png) = data(png_format()) {
                return Some(Content::Encoded(png, "image/png"));
            }
            for f in [17u32 /* CF_DIBV5 */, CF_DIB.0 as u32] {
                if let Some(d) = data(f) {
                    if let Some((w, h, px)) = dib_to_rgba(&d) {
                        return Some(Content::Rgba(w, h, px));
                    }
                }
            }
            if let Some(list) = files() {
                if list.len() == 1 && image_mime(&list[0]).is_some() {
                    return Some(Content::Files(list));
                }
            }
            if let Some(t) = data(CF_UNICODETEXT.0 as u32) {
                let units: Vec<u16> = t.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|u| *u != 0).collect();
                let s = String::from_utf16_lossy(&units);
                if !s.trim().is_empty() {
                    return Some(Content::Text(s));
                }
            }
            None
        })();
        let _ = CloseClipboard();
        // A copied image file is read after the clipboard is closed.
        if let Some(Content::Files(list)) = &result {
            let mime = image_mime(&list[0])?;
            return std::fs::read(&list[0]).ok().map(|b| Content::Encoded(b, mime));
        }
        result
    }
}

// --- history: reading everything at once, writing back, watching -------------------

pub enum Image {
    Png(Vec<u8>),
    Rgba(u32, u32, Vec<u8>),
}

/// One clipboard change, read in a single short open/close so the program
/// that copied is never kept waiting.
pub struct Snapshot {
    pub files: Option<Vec<String>>,
    pub text: Option<String>,
    /// The raw "HTML Format" data (CF_HTML, with its header), for pasting
    /// back with formatting.
    pub html: Option<Vec<u8>>,
    pub image: Option<Image>,
    /// Program that owns the clipboard (lower-case exe), if known.
    pub owner: String,
}

fn reg(name: &str) -> u32 {
    unsafe { RegisterClipboardFormatW(&windows::core::HSTRING::from(name)) }
}

pub fn html_format() -> u32 {
    reg("HTML Format")
}

/// Formats by which other programs ask clipboard managers to look away
/// (password managers use these), plus our own mark.
unsafe fn asks_to_be_skipped() -> bool {
    for name in ["ExcludeClipboardContentFromMonitorProcessing", "Clipboard Viewer Ignore", "EarthDesk.Self"] {
        if IsClipboardFormatAvailable(reg(name)).is_ok() {
            return true;
        }
    }
    // Windows' own history flag: a DWORD 0 means "do not keep".
    if let Some(d) = data(reg("CanIncludeInClipboardHistory")) {
        if d.len() >= 4 && u32::from_le_bytes([d[0], d[1], d[2], d[3]]) == 0 {
            return true;
        }
    }
    false
}

fn utf16z(d: &[u8]) -> String {
    let units: Vec<u16> = d.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|u| *u != 0).collect();
    String::from_utf16_lossy(&units)
}

/// PNG width and height from its header.
pub fn png_size(b: &[u8]) -> Option<(u32, u32)> {
    if b.len() < 24 || &b[1..4] != b"PNG" {
        return None;
    }
    Some((u32::from_be_bytes([b[16], b[17], b[18], b[19]]), u32::from_be_bytes([b[20], b[21], b[22], b[23]])))
}

/// Read the clipboard for the history. None = nothing to record (empty, a
/// program asked us not to, or the clipboard could not be opened).
pub fn read_snapshot(max_image_pixels: u64, want_images: bool) -> Option<Snapshot> {
    unsafe {
        if !open() {
            return None;
        }
        let snap = (|| {
            if asks_to_be_skipped() {
                return None;
            }
            let owner_hwnd = GetClipboardOwner().unwrap_or_default();
            let owner = if owner_hwnd.0.is_null() { String::new() } else { super::apps::exe_of_window(owner_hwnd) };
            let files = files().filter(|f| !f.is_empty());
            let text = data(CF_UNICODETEXT.0 as u32).map(|d| utf16z(&d)).filter(|t| !t.is_empty());
            let html = if text.is_some() { data(html_format()) } else { None };
            let mut image = None;
            if want_images && files.is_none() && text.is_none() {
                if let Some(p) = data(png_format()) {
                    if png_size(&p).map(|(w, h)| (w as u64) * (h as u64) <= max_image_pixels).unwrap_or(false) {
                        image = Some(Image::Png(p));
                    }
                }
                if image.is_none() {
                    for f in [17u32, CF_DIB.0 as u32] {
                        if let Some(d) = data(f) {
                            if let Some((w, h, px)) = dib_to_rgba(&d) {
                                if (w as u64) * (h as u64) <= max_image_pixels {
                                    image = Some(Image::Rgba(w, h, px));
                                }
                                break;
                            }
                        }
                    }
                }
            }
            if files.is_none() && text.is_none() && image.is_none() {
                return None;
            }
            Some(Snapshot { files, text, html, image, owner })
        })();
        let _ = CloseClipboard();
        snap
    }
}

/// Put text back (with its HTML, unless `plain`).
pub fn set_text_html(text: &str, html: Option<&[u8]>, mark_self: bool) -> Result<(), String> {
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    wide.push(0);
    let bytes = unsafe { std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2) };
    unsafe {
        if !open() {
            return Err("剪贴板被其他程序占用".into());
        }
        let _ = EmptyClipboard();
        let ok = put(CF_UNICODETEXT.0 as u32, bytes);
        if let Some(h) = html {
            put(html_format(), h);
        }
        if mark_self {
            put(self_format(), &mark());
        }
        let _ = CloseClipboard();
        if ok {
            Ok(())
        } else {
            Err("写入剪贴板失败".into())
        }
    }
}

/// Put a list of files back (as Explorer's copy does).
pub fn set_files(paths: &[String], mark_self: bool) -> Result<(), String> {
    // DROPFILES header (20 bytes): offset of the list, pt, fNC, fWide.
    let mut v: Vec<u8> = Vec::new();
    v.extend_from_slice(&20u32.to_le_bytes());
    v.extend_from_slice(&[0u8; 12]);
    v.extend_from_slice(&1u32.to_le_bytes());
    for p in paths {
        for u in p.encode_utf16() {
            v.extend_from_slice(&u.to_le_bytes());
        }
        v.extend_from_slice(&[0, 0]);
    }
    v.extend_from_slice(&[0, 0]);
    unsafe {
        if !open() {
            return Err("剪贴板被其他程序占用".into());
        }
        let _ = EmptyClipboard();
        let ok = put(15, &v);
        // 1 = copy (not move) when pasted in Explorer.
        put(reg("Preferred DropEffect"), &1u32.to_le_bytes());
        if mark_self {
            put(self_format(), &mark());
        }
        let _ = CloseClipboard();
        if ok {
            Ok(())
        } else {
            Err("写入剪贴板失败".into())
        }
    }
}

/// Put an encoded PNG back, as PNG and as a DIB.
pub fn set_png(png_bytes: &[u8], rgba: Option<(u32, u32, &[u8])>, mark_self: bool) -> Result<(), String> {
    match rgba {
        Some((w, h, px)) => set_image(w, h, px, Some(png_bytes), mark_self),
        None => unsafe {
            if !open() {
                return Err("剪贴板被其他程序占用".into());
            }
            let _ = EmptyClipboard();
            let ok = put(png_format(), png_bytes);
            if mark_self {
                put(self_format(), &mark());
            }
            let _ = CloseClipboard();
            if ok {
                Ok(())
            } else {
                Err("写入剪贴板失败".into())
            }
        },
    }
}

static MONITOR_CB: std::sync::OnceLock<fn()> = std::sync::OnceLock::new();
const WM_CLIP_DEBOUNCED: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 40;

unsafe extern "system" fn monitor_proc(
    hwnd: HWND,
    msg: u32,
    wp: windows::Win32::Foundation::WPARAM,
    lp: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::*;
    const WM_CLIPBOARDUPDATE: u32 = 0x031D;
    match msg {
        WM_CLIPBOARDUPDATE => {
            // Programs set several formats one after another; wait for the
            // burst to settle, then read once.
            let _ = SetTimer(Some(hwnd), 1, 120, None);
            windows::Win32::Foundation::LRESULT(0)
        }
        WM_TIMER if wp.0 == 1 => {
            let _ = KillTimer(Some(hwnd), 1);
            let _ = PostMessageW(Some(hwnd), WM_CLIP_DEBOUNCED, wp, lp);
            windows::Win32::Foundation::LRESULT(0)
        }
        WM_CLIP_DEBOUNCED => {
            if let Some(f) = MONITOR_CB.get() {
                f();
            }
            windows::Win32::Foundation::LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// Watch the clipboard on its own thread; `on_change` runs there after each
/// (debounced) change.
pub fn start_monitor(on_change: fn()) {
    let _ = MONITOR_CB.set(on_change);
    std::thread::Builder::new()
        .name("clipboard-watch".into())
        .spawn(|| unsafe {
            use windows::Win32::UI::WindowsAndMessaging::*;
            let inst = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).unwrap_or_default();
            let class = w!("EarthDeskClipboardWatch");
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(monitor_proc),
                hInstance: inst.into(),
                lpszClassName: class,
                ..Default::default()
            };
            RegisterClassExW(&wc);
            let Ok(hwnd) = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class,
                windows::core::PCWSTR::null(),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                Some(inst.into()),
                None,
            ) else {
                return;
            };
            if AddClipboardFormatListener(hwnd).is_err() {
                crate::toolkit::log("clipboard: could not start listening");
                return;
            }
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        })
        .ok();
}
