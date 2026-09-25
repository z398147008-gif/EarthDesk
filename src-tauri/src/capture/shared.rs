//! Handing the frozen screen to the capture page without copying it through
//! IPC: a WebView2 shared buffer (shared memory the page sees as an
//! ArrayBuffer). At 4K that is 33 MB the page would otherwise receive as an
//! IPC response -- the slowest step between F1 and a usable screen.
//!
//! The buffer is kept and reused while the desktop size stays the same.

use std::sync::Mutex;
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use windows::core::{Interface, HSTRING};

struct Cached {
    size: u64,
    buf: ICoreWebView2SharedBuffer,
}

// Only touched on the main thread (inside with_webview).
struct Slot(Option<Cached>);
unsafe impl Send for Slot {}
static CACHE: Mutex<Slot> = Mutex::new(Slot(None));

/// Copy `bgra` into a shared buffer as RGBA and post it to the page with
/// `json` as the additional data. Main thread, from `with_webview`.
pub fn post(controller: &ICoreWebView2Controller, env: &ICoreWebView2Environment, bgra: &[u8], json: &str) -> windows::core::Result<()> {
    unsafe {
        let core: ICoreWebView2_17 = controller.CoreWebView2()?.cast()?;
        let env12: ICoreWebView2Environment12 = env.cast()?;
        let size = bgra.len() as u64;
        let mut cache = CACHE.lock().unwrap_or_else(|p| p.into_inner());
        if cache.0.as_ref().map(|c| c.size) != Some(size) {
            cache.0 = Some(Cached { size, buf: env12.CreateSharedBuffer(size)? });
        }
        let buf = &cache.0.as_ref().unwrap().buf;
        let mut ptr: *mut u8 = std::ptr::null_mut();
        buf.Buffer(&mut ptr)?;
        if ptr.is_null() {
            return Err(windows::core::Error::from_hresult(windows::Win32::Foundation::E_POINTER));
        }
        let dst = std::slice::from_raw_parts_mut(ptr, bgra.len());
        for (d, s) in dst.chunks_exact_mut(4).zip(bgra.chunks_exact(4)) {
            d[0] = s[2];
            d[1] = s[1];
            d[2] = s[0];
            d[3] = 255;
        }
        let j = HSTRING::from(json);
        core.PostSharedBufferToScript(buf, COREWEBVIEW2_SHARED_BUFFER_ACCESS_READ_WRITE, &j)
    }
}
