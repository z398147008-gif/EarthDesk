#![allow(unused_unsafe)]
//! A safe wrapper over librime, loaded at run time (rime.dll next to the exe
//! on Windows, librime.so on Linux test machines).
//!
//! librime is not documented as thread-safe for concurrent calls, so every
//! call goes through one mutex (`Engine` in session.rs holds it).

use crate::rime_sys::*;
use std::ffi::{c_char, c_int, CStr, CString};
use std::path::Path;

pub struct Rime {
    api: *mut RimeApi,
    _lib: libloading::Library,
}

// The API table is plain function pointers into the loaded library.
unsafe impl Send for Rime {}
unsafe impl Sync for Rime {}

#[derive(Debug, Clone, Default)]
pub struct Candidate {
    pub text: String,
    pub comment: String,
}

#[derive(Debug, Clone, Default)]
pub struct Menu {
    pub page_no: i32,
    pub is_last_page: bool,
    pub highlighted: i32,
    pub candidates: Vec<Candidate>,
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub commit: Option<String>,
    /// Preedit text and caret (in bytes of the UTF-8 preedit, as Rime gives it).
    pub preedit: Option<(String, usize)>,
    pub menu: Option<Menu>,
    pub input: String,
    pub ascii: bool,
    pub composing: bool,
}

/// A path as librime can open it. librime (and glog, lua) hand char* paths
/// to the C runtime, which reads them in the process's ANSI code page. Our
/// manifest makes that UTF-8; on Windows versions that ignore it, fall back
/// to the short 8.3 name (ASCII) or the ANSI spelling.
fn native_path(p: &Path) -> CString {
    let utf8 = p.to_string_lossy().into_owned();
    #[cfg(windows)]
    {
        use windows::core::HSTRING;
        use windows::Win32::Globalization::{GetACP, WideCharToMultiByte, CP_ACP};
        use windows::Win32::Storage::FileSystem::GetShortPathNameW;
        if !utf8.is_ascii() && unsafe { GetACP() } != 65001 {
            let wide = HSTRING::from(utf8.as_str());
            let mut buf = vec![0u16; 1024];
            let n = unsafe { GetShortPathNameW(&wide, Some(&mut buf)) } as usize;
            if n > 0 && n < buf.len() {
                let short = String::from_utf16_lossy(&buf[..n]);
                if short.is_ascii() {
                    crate::log(&format!("using short path {short} for {utf8}"));
                    return CString::new(short).unwrap_or_default();
                }
            }
            let w: Vec<u16> = utf8.encode_utf16().collect();
            let mut out = vec![0u8; w.len() * 4 + 4];
            let mut lossy = windows::core::BOOL(0);
            let n = unsafe { WideCharToMultiByte(CP_ACP, 0, &w, Some(&mut out), None, Some(&mut lossy)) } as usize;
            if n > 0 && !lossy.as_bool() {
                out.truncate(n);
                return CString::new(out).unwrap_or_default();
            }
            crate::log(&format!("path {utf8} cannot be spelt in code page {}", unsafe { GetACP() }));
        }
    }
    CString::new(utf8).unwrap_or_default()
}

fn cstr(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }
}

macro_rules! call {
    ($self:ident . $f:ident ( $($a:expr),* )) => {
        unsafe { ((*$self.api).$f.expect(concat!("librime has no ", stringify!($f))))($($a),*) }
    };
}

/// Notifications (deploy start/success/failure) are reported here.
pub static DEPLOY_STATE: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

unsafe extern "C" fn on_notification(_ctx: *mut std::ffi::c_void, _sid: RimeSessionId, kind: *const c_char, value: *const c_char) {
    let (k, v) = (cstr(kind), cstr(value));
    if k == "deploy" {
        if let Ok(mut s) = DEPLOY_STATE.lock() {
            *s = v.clone();
        }
    }
    crate::log(&format!("rime: {k} {v}"));
}

impl Rime {
    pub fn load(lib: &Path) -> Result<Rime, String> {
        unsafe {
            let lib = libloading::Library::new(lib).map_err(|e| format!("无法加载 {}: {e}", lib.display()))?;
            let get: libloading::Symbol<unsafe extern "C" fn() -> *mut RimeApi> =
                lib.get(b"rime_get_api\0").map_err(|e| e.to_string())?;
            let api = get();
            if api.is_null() {
                return Err("rime_get_api returned null".into());
            }
            Ok(Rime { api, _lib: lib })
        }
    }

    /// True if this librime has the field at `offset` (bytes into RimeApi):
    /// RIME_STRUCT_HAS_MEMBER from rime_api.h.
    fn has(&self, offset: usize) -> bool {
        let size = unsafe { (*self.api).data_size } as usize;
        std::mem::size_of::<c_int>() + size > offset
    }

    pub fn version(&self) -> String {
        cstr(call!(self.get_version()))
    }

    /// Set up and initialise. `shared`: the read-only data shipped with us;
    /// `user`: the user's dictionaries, customisations and build output.
    pub fn start(&self, shared: &Path, user: &Path, log_dir: &Path) {
        let shared = native_path(shared);
        let user = native_path(user);
        let log = native_path(log_dir);
        let name = CString::new("EarthDesk").unwrap();
        let code = CString::new("earthdesk").unwrap();
        let ver = CString::new(env!("CARGO_PKG_VERSION")).unwrap();
        let app = CString::new("rime.earthdesk").unwrap();
        // Leaked on purpose: librime keeps the pointers for its lifetime.
        let (shared, user, log, name, code, ver, app) = (
            shared.into_raw(),
            user.into_raw(),
            log.into_raw(),
            name.into_raw(),
            code.into_raw(),
            ver.into_raw(),
            app.into_raw(),
        );
        let mut traits = RimeTraits {
            data_size: (std::mem::size_of::<RimeTraits>() - std::mem::size_of::<c_int>()) as c_int,
            shared_data_dir: shared,
            user_data_dir: user,
            distribution_name: name,
            distribution_code_name: code,
            distribution_version: ver,
            app_name: app,
            modules: std::ptr::null_mut(),
            min_log_level: 2,
            log_dir: log,
            prebuilt_data_dir: std::ptr::null(),
            staging_dir: std::ptr::null(),
        };
        call!(self.setup(&mut traits));
        call!(self.set_notification_handler(Some(on_notification), std::ptr::null_mut()));
        call!(self.initialize(std::ptr::null_mut()));
    }

    /// Build dictionaries if anything changed. `wait` blocks until done
    /// (tens of seconds the first time).
    pub fn maintain(&self, full_check: bool, wait: bool) -> bool {
        let started = call!(self.start_maintenance(full_check as c_int)) != 0;
        if started && wait {
            call!(self.join_maintenance_thread());
        }
        started
    }

    /// Flush the user dictionaries and stop. Nothing may use Rime after.
    pub fn finalize(&self) {
        call!(self.finalize());
    }

    pub fn is_maintaining(&self) -> bool {
        call!(self.is_maintenance_mode()) != 0
    }

    pub fn create_session(&self) -> RimeSessionId {
        call!(self.create_session())
    }

    pub fn destroy_session(&self, s: RimeSessionId) {
        call!(self.destroy_session(s));
    }

    pub fn find_session(&self, s: RimeSessionId) -> bool {
        call!(self.find_session(s)) != 0
    }

    pub fn process_key(&self, s: RimeSessionId, keycode: u32, mask: u32) -> bool {
        call!(self.process_key(s, keycode as c_int, mask as c_int)) != 0
    }

    /// The keys typed into the composition, as typed ("nihk").
    pub fn raw_input(&self, s: RimeSessionId) -> String {
        cstr(call!(self.get_input(s)))
    }

    pub fn clear(&self, s: RimeSessionId) {
        call!(self.clear_composition(s));
    }

    #[allow(dead_code)]
    pub fn commit_composition(&self, s: RimeSessionId) -> bool {
        call!(self.commit_composition(s)) != 0
    }

    pub fn select_on_page(&self, s: RimeSessionId, index: usize) -> bool {
        call!(self.select_candidate_on_current_page(s, index)) != 0
    }

    pub fn change_page(&self, s: RimeSessionId, backward: bool) -> bool {
        let off = std::mem::offset_of!(RimeApi, change_page);
        if !self.has(off) {
            let key = if backward { 0xff55 } else { 0xff56 };
            return self.process_key(s, key, 0);
        }
        call!(self.change_page(s, backward as c_int)) != 0
    }

    #[allow(dead_code)]
    pub fn set_option(&self, s: RimeSessionId, name: &str, on: bool) {
        let n = CString::new(name).unwrap();
        call!(self.set_option(s, n.as_ptr(), on as c_int));
    }

    #[allow(dead_code)]
    pub fn get_option(&self, s: RimeSessionId, name: &str) -> bool {
        let n = CString::new(name).unwrap();
        call!(self.get_option(s, n.as_ptr())) != 0
    }

    pub fn select_schema(&self, s: RimeSessionId, id: &str) -> bool {
        let n = CString::new(id).unwrap();
        call!(self.select_schema(s, n.as_ptr())) != 0
    }

    /// The whole candidate list (not just the current page), up to `max`.
    pub fn candidates(&self, s: RimeSessionId, max: usize) -> Vec<Candidate> {
        let mut out = Vec::new();
        unsafe {
            let mut it: RimeCandidateListIterator = std::mem::zeroed();
            if call!(self.candidate_list_begin(s, &mut it)) == 0 {
                return out;
            }
            while out.len() < max && call!(self.candidate_list_next(&mut it)) != 0 {
                out.push(Candidate { text: cstr(it.candidate.text), comment: cstr(it.candidate.comment) });
            }
            call!(self.candidate_list_end(&mut it));
        }
        out
    }

    /// Pick by index in the whole list.
    pub fn select_candidate(&self, s: RimeSessionId, index: usize) -> bool {
        call!(self.select_candidate(s, index)) != 0
    }

    /// Forget a word the user dictionary learned (the menu's `index`).
    /// Words of the shipped dictionaries stay (Rime cannot delete them).
    pub fn delete_candidate(&self, s: RimeSessionId, index: usize) -> bool {
        call!(self.delete_candidate(s, index)) != 0
    }

    pub fn delete_on_page(&self, s: RimeSessionId, index: usize) -> bool {
        call!(self.delete_candidate_on_current_page(s, index)) != 0
    }

    /// Everything the UI needs after a key: pending commit, preedit, menu.
    pub fn snapshot(&self, s: RimeSessionId) -> Snapshot {
        let mut out = Snapshot::default();
        unsafe {
            let mut commit: RimeCommit = std::mem::zeroed();
            commit.data_size = (std::mem::size_of::<RimeCommit>() - std::mem::size_of::<c_int>()) as c_int;
            if call!(self.get_commit(s, &mut commit)) != 0 {
                out.commit = Some(cstr(commit.text));
                call!(self.free_commit(&mut commit));
            }
            let mut status: RimeStatus = std::mem::zeroed();
            status.data_size = (std::mem::size_of::<RimeStatus>() - std::mem::size_of::<c_int>()) as c_int;
            if call!(self.get_status(s, &mut status)) != 0 {
                out.ascii = status.is_ascii_mode != 0;
                out.composing = status.is_composing != 0;
                call!(self.free_status(&mut status));
            }
            let mut ctx: RimeContext = std::mem::zeroed();
            ctx.data_size = (std::mem::size_of::<RimeContext>() - std::mem::size_of::<c_int>()) as c_int;
            if call!(self.get_context(s, &mut ctx)) != 0 {
                let c = &ctx.composition;
                if c.length > 0 && !c.preedit.is_null() {
                    out.preedit = Some((cstr(c.preedit), c.cursor_pos.max(0) as usize));
                }
                let m = &ctx.menu;
                if m.num_candidates > 0 && !m.candidates.is_null() {
                    let cands = std::slice::from_raw_parts(m.candidates, m.num_candidates as usize);
                    let mut labels = Vec::new();
                    let keys = cstr(m.select_keys);
                    for i in 0..cands.len() {
                        let l = if !ctx.select_labels.is_null() {
                            cstr(*ctx.select_labels.add(i))
                        } else if let Some(ch) = keys.chars().nth(i) {
                            ch.to_string()
                        } else {
                            ((i + 1) % 10).to_string()
                        };
                        labels.push(l);
                    }
                    out.menu = Some(Menu {
                        page_no: m.page_no,
                        is_last_page: m.is_last_page != 0,
                        highlighted: m.highlighted_candidate_index,
                        candidates: cands.iter().map(|c| Candidate { text: cstr(c.text), comment: cstr(c.comment) }).collect(),
                        labels,
                    });
                }
                call!(self.free_context(&mut ctx));
            }
            out.input = cstr(call!(self.get_input(s)));
        }
        out
    }
}
