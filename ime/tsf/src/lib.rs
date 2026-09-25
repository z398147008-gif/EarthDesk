//! EarthDeskTSF.dll — the TSF text service of 地球桌面输入法.
//!
//! This DLL is loaded into every program the user types in, so it does as
//! little as possible: it turns key events into Rime key codes, asks the
//! engine process (EarthDeskIME.exe) over a named pipe what to do, and
//! applies the answer (underlined preedit, committed text) through TSF edit
//! sessions. Pinyin, dictionaries and the candidate window all live in the
//! engine, so a problem there cannot take the host program down. The one
//! exception: in immersive hosts (the Start menu's search) the DLL draws
//! the candidate window itself, see candwin.rs.
//!
//! Every COM entry point catches panics (see `guard`): unwinding into a
//! foreign program is undefined behaviour, and killing it is worse.

#![cfg(windows)]
#![allow(non_snake_case)]

mod candwin;
mod client;
mod edit;
mod keys;
mod register;
mod service;

use std::ffi::c_void;
use std::sync::atomic::{AtomicIsize, Ordering};
use windows::core::{implement, IUnknown, Interface, BOOL, GUID, HRESULT};
use windows::Win32::Foundation::{CLASS_E_CLASSNOTAVAILABLE, E_FAIL, E_UNEXPECTED, HINSTANCE, S_FALSE, S_OK};
use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl};
use windows::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

/// The text service's COM class.
pub const CLSID: GUID = GUID::from_u128(0x6f1a7c52_3b8e_4d0a_9e61_2c5d8b7e4a13);
/// Its language profile (Simplified Chinese).
pub const PROFILE: GUID = GUID::from_u128(0x0b9d3e27_71c4_4f5e_8a2d_9c6e1f4b7d20);
/// The underline style of the preedit.
pub const DISPLAY_ATTR: GUID = GUID::from_u128(0xd4e8a1f3_5c27_4b69_b03e_7a1c9f2d6e58);
pub const NAME: &str = "地球桌面输入法";

static MODULE: AtomicIsize = AtomicIsize::new(0);

pub fn module() -> HINSTANCE {
    HINSTANCE(MODULE.load(Ordering::SeqCst) as *mut _)
}

/// Run `f`, turning a panic into `fallback`.
pub fn guard<T>(fallback: T, f: impl FnOnce() -> T) -> T {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or(fallback)
}

#[no_mangle]
extern "system" fn DllMain(module: HINSTANCE, reason: u32, _reserved: *mut c_void) -> BOOL {
    if reason == DLL_PROCESS_ATTACH {
        MODULE.store(module.0 as isize, Ordering::SeqCst);
    }
    true.into()
}

#[no_mangle]
extern "system" fn DllRegisterServer() -> HRESULT {
    guard(E_FAIL, || match register::register() {
        Ok(()) => S_OK,
        Err(e) => e.code(),
    })
}

#[no_mangle]
extern "system" fn DllUnregisterServer() -> HRESULT {
    guard(E_FAIL, || match register::unregister() {
        Ok(()) => S_OK,
        Err(e) => e.code(),
    })
}

#[no_mangle]
extern "system" fn DllGetClassObject(clsid: *const GUID, iid: *const GUID, out: *mut *mut c_void) -> HRESULT {
    guard(E_UNEXPECTED, || unsafe {
        if out.is_null() || clsid.is_null() || iid.is_null() {
            return E_UNEXPECTED;
        }
        *out = std::ptr::null_mut();
        if *clsid != CLSID {
            return CLASS_E_CLASSNOTAVAILABLE;
        }
        let factory: IUnknown = Factory.into();
        factory.query(iid, out)
    })
}

#[no_mangle]
extern "system" fn DllCanUnloadNow() -> HRESULT {
    // Keep it simple and never unload while the process lives: TSF itself
    // holds text services for the thread's lifetime.
    S_FALSE
}

#[implement(IClassFactory)]
struct Factory;

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(
        &self,
        outer: windows_core::Ref<'_, IUnknown>,
        iid: *const GUID,
        out: *mut *mut c_void,
    ) -> windows::core::Result<()> {
        guard(Err(E_FAIL.into()), || unsafe {
            if !outer.is_null() {
                return Err(windows::Win32::Foundation::CLASS_E_NOAGGREGATION.into());
            }
            let unknown = service::TextService::create();
            unknown.query(iid, out).ok()
        })
    }

    fn LockServer(&self, _lock: BOOL) -> windows::core::Result<()> {
        Ok(())
    }
}
