//! Serving on Windows: single instance, Rime start-up, the pipe server and
//! the candidate window.

mod candwin;
pub mod keyconv;
mod pipe;
pub mod setup;

use crate::session::{Caret, Engine, Ui, View};
use std::sync::{Arc, OnceLock};
use windows::core::HSTRING;
use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS, HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

pub static ENGINE: OnceLock<Arc<Engine>> = OnceLock::new();

struct WinUi;

impl Ui for WinUi {
    fn show(&self, view: View, caret: Option<Caret>) {
        candwin::show(view, caret);
    }
    fn move_to(&self, caret: Caret) {
        candwin::move_to(caret);
    }
    fn hide(&self) {
        candwin::hide();
    }
    fn notify(&self, notify: u64) {
        if notify != 0 {
            unsafe {
                let _ = PostMessageW(Some(HWND(notify as *mut _)), ime_proto::WM_NOTIFY_OFFSET, WPARAM(0), LPARAM(0));
            }
        }
    }
}

fn session_id() -> u32 {
    let mut sid = 0u32;
    unsafe {
        let _ = windows::Win32::System::RemoteDesktop::ProcessIdToSessionId(std::process::id(), &mut sid);
    }
    sid
}

pub fn serve() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        // One engine per logon session.
        let name = HSTRING::from(format!(r"Local\EarthDeskIME.{}", session_id()));
        let _m = CreateMutexW(None, true, &name);
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }
        std::mem::forget(_m);
    }
    let rime = match crate::start_rime() {
        Ok(r) => r,
        Err(e) => {
            crate::log(&format!("cannot start: {e}"));
            return;
        }
    };
    // Build dictionaries in the background; keys pass through until done.
    rime.maintain(false, false);
    let engine = Arc::new(Engine::new(rime, crate::start_mozc(), Box::new(WinUi), crate::SCHEMA));
    let _ = ENGINE.set(engine.clone());
    candwin::start(engine.clone());
    pipe::run(&ime_proto::pipe_name(session_id()), engine);
}
