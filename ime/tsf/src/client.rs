//! The DLL's side of the pipe. Every call has a deadline: if the engine is
//! slow or gone, the key goes to the program untouched and we try to
//! reconnect later. A host program must never freeze because of us.

use ime_proto::{read_frame, write_frame, Reply, Request, State};
use std::io::{Read, Write};
use std::time::{Duration, Instant};
use windows::core::HSTRING;
use windows::Win32::Foundation::*;
use windows::Win32::Security::{GetTokenInformation, TokenElevation, TokenIsAppContainer, TOKEN_ELEVATION, TOKEN_QUERY};
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::Pipes::{SetNamedPipeHandleState, WaitNamedPipeW, PIPE_READMODE_BYTE};
use windows::Win32::System::Threading::*;
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};

const DEADLINE: Duration = Duration::from_millis(400);

struct Conn {
    pipe: HANDLE,
    event: HANDLE,
}

impl Conn {
    unsafe fn io(&self, write: bool, buf: *mut u8, len: usize) -> std::io::Result<usize> {
        let mut ov = OVERLAPPED { hEvent: self.event, ..Default::default() };
        let mut n = 0u32;
        let slice = std::slice::from_raw_parts_mut(buf, len);
        let r = if write {
            WriteFile(self.pipe, Some(slice), None, Some(&mut ov))
        } else {
            ReadFile(self.pipe, Some(slice), None, Some(&mut ov))
        };
        if let Err(e) = r {
            if e.code() != ERROR_IO_PENDING.to_hresult() {
                return Err(std::io::Error::other(e.message()));
            }
            if WaitForSingleObject(self.event, DEADLINE.as_millis() as u32) != WAIT_OBJECT_0 {
                let _ = CancelIoEx(self.pipe, Some(&ov));
                let _ = GetOverlappedResult(self.pipe, &ov, &mut n, true);
                return Err(std::io::ErrorKind::TimedOut.into());
            }
        }
        GetOverlappedResult(self.pipe, &ov, &mut n, false).map_err(|e| std::io::Error::other(e.message()))?;
        if n == 0 && len > 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        Ok(n as usize)
    }
}

impl Read for Conn {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        unsafe { self.io(false, buf.as_mut_ptr(), buf.len()) }
    }
}

impl Write for Conn {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        unsafe { self.io(true, buf.as_ptr() as *mut u8, buf.len()) }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.pipe);
            let _ = CloseHandle(self.event);
        }
    }
}

pub struct Client {
    conn: Option<Conn>,
    session: u64,
    notify: u64,
    /// We draw the candidate window ourselves (see candwin.rs).
    draws: bool,
    last_try: Option<Instant>,
    /// When we last started the engine (it may have crashed since).
    started_engine: Option<Instant>,
}

fn session_id() -> u32 {
    let mut sid = 0u32;
    unsafe {
        let _ = windows::Win32::System::RemoteDesktop::ProcessIdToSessionId(GetCurrentProcessId(), &mut sid);
    }
    sid
}

pub fn exe_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()))
        .unwrap_or_default()
}

/// Sandboxed or elevated hosts must not start the engine: it would inherit
/// their restrictions (or their administrator rights).
fn may_start_engine() -> bool {
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut ac = 0u32;
        let mut len = 0u32;
        let is_ac = GetTokenInformation(token, TokenIsAppContainer, Some(&mut ac as *mut _ as *mut _), 4, &mut len).is_ok() && ac != 0;
        let mut el = TOKEN_ELEVATION::default();
        let elevated = GetTokenInformation(token, TokenElevation, Some(&mut el as *mut _ as *mut _), std::mem::size_of::<TOKEN_ELEVATION>() as u32, &mut len)
            .is_ok()
            && el.TokenIsElevated != 0;
        let _ = CloseHandle(token);
        !is_ac && !elevated
    }
}

fn start_engine() {
    let dll = {
        let mut buf = [0u16; 1024];
        let n = unsafe { windows::Win32::System::LibraryLoader::GetModuleFileNameW(Some(crate::module().into()), &mut buf) } as usize;
        String::from_utf16_lossy(&buf[..n])
    };
    let Some(dir) = dll.rfind('\\').map(|i| &dll[..i]) else { return };
    let exe = format!("{dir}\\EarthDeskIME.exe");
    let mut cmd: Vec<u16> = format!("\"{exe}\"").encode_utf16().chain(std::iter::once(0)).collect();
    let si = STARTUPINFOW { cb: std::mem::size_of::<STARTUPINFOW>() as u32, ..Default::default() };
    let mut pi = PROCESS_INFORMATION::default();
    unsafe {
        if CreateProcessW(
            &HSTRING::from(exe),
            Some(windows::core::PWSTR(cmd.as_mut_ptr())),
            None,
            None,
            false,
            DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP,
            None,
            None,
            &si,
            &mut pi,
        )
        .is_ok()
        {
            let _ = CloseHandle(pi.hProcess);
            let _ = CloseHandle(pi.hThread);
        }
    }
}

impl Client {
    pub fn new(notify: u64, draws: bool) -> Client {
        Client { conn: None, session: 0, notify, draws, last_try: None, started_engine: None }
    }

    fn connect(&mut self) -> bool {
        if self.conn.is_some() {
            return true;
        }
        // Do not hammer a missing engine on every key.
        if let Some(t) = self.last_try {
            if t.elapsed() < Duration::from_millis(1500) {
                return false;
            }
        }
        self.last_try = Some(Instant::now());
        let name = HSTRING::from(ime_proto::pipe_name(session_id()));
        unsafe {
            let open = || {
                CreateFileW(
                    &name,
                    (GENERIC_READ | GENERIC_WRITE).0,
                    FILE_SHARE_NONE,
                    None,
                    OPEN_EXISTING,
                    FILE_FLAG_OVERLAPPED,
                    None,
                )
            };
            let mut h = open();
            if h.is_err() && GetLastError() == ERROR_PIPE_BUSY && WaitNamedPipeW(&name, 200).as_bool() {
                h = open();
            }
            let Ok(pipe) = h else {
                // Start it (again, if it went away), at most every 10 s.
                let due = self.started_engine.map(|t| t.elapsed() > Duration::from_secs(10)).unwrap_or(true);
                if due && may_start_engine() {
                    self.started_engine = Some(Instant::now());
                    start_engine();
                }
                return false;
            };
            let mode = PIPE_READMODE_BYTE;
            let _ = SetNamedPipeHandleState(pipe, Some(&mode), None, None);
            let Ok(event) = CreateEventW(None, true, false, None) else {
                let _ = CloseHandle(pipe);
                return false;
            };
            self.conn = Some(Conn { pipe, event });
        }
        let hello = Request::Hello { version: ime_proto::VERSION, pid: unsafe { GetCurrentProcessId() }, exe: exe_name(), notify: self.notify, draws: self.draws };
        match self.raw(&hello) {
            Some(Reply::Hello { session, .. }) => {
                self.session = session;
                true
            }
            _ => {
                self.conn = None;
                false
            }
        }
    }

    fn raw(&mut self, req: &Request) -> Option<Reply> {
        let conn = self.conn.as_mut()?;
        let r = write_frame(conn, req).and_then(|_| read_frame::<_, Reply>(conn));
        match r {
            Ok(reply) => Some(reply),
            Err(_) => {
                // Broken or too slow: drop the connection; a new one (and a
                // new session) is made on a later key.
                self.conn = None;
                None
            }
        }
    }

    /// Send a request that concerns our session. None = no engine.
    pub fn call(&mut self, make: impl Fn(u64) -> Request) -> Option<Reply> {
        if !self.connect() {
            return None;
        }
        let req = make(self.session);
        self.raw(&req)
    }

    /// The key as Windows reported it. Err(()) = the engine does not know
    /// RawKey (older version): convert here and use `key`.
    pub fn raw_key(&mut self, vk: u16, scan: u16, flags: u32, hkl: u64) -> Result<Option<State>, ()> {
        match self.call(|session| Request::RawKey { session, vk, scan, flags, hkl }) {
            Some(Reply::State(s)) => Ok(Some(s)),
            Some(Reply::Error { .. }) => Err(()),
            _ => Ok(None),
        }
    }

    /// A click in our own candidate window.
    pub fn pick(&mut self, index: Option<u32>, page: Option<bool>) -> Option<State> {
        match self.call(|session| Request::Pick { session, index, page })? {
            Reply::State(s) => Some(s),
            _ => None,
        }
    }

    pub fn key(&mut self, keycode: u32, mask: u32) -> Option<State> {
        match self.call(|session| Request::Key { session, keycode, mask })? {
            Reply::State(s) => Some(s),
            _ => None,
        }
    }

    /// Fire-and-forget requests only when already connected (no reason to
    /// start the engine for a caret move).
    pub fn tell(&mut self, make: impl Fn(u64) -> Request) {
        if self.conn.is_some() {
            let req = make(self.session);
            let _ = self.raw(&req);
        }
    }

    pub fn close(&mut self) {
        if self.conn.is_some() {
            let s = self.session;
            let _ = self.raw(&Request::Bye { session: s });
        }
        self.conn = None;
    }
}
