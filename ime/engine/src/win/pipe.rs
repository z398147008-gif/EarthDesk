//! The named-pipe server. One thread per connected DLL instance (each is a
//! UI thread in some program), blocking reads, one request -> one reply.
//!
//! Security: only this user may connect -- including this user's sandboxed
//! (AppContainer) apps and elevated programs -- and the pipe carries a low
//! integrity label so low-integrity processes (browsers' sandboxes around
//! text fields, protected-mode apps) can reach it too.

use crate::session::Engine;
use ime_proto::{read_frame, write_frame, Reply, Request};
use std::io::{Read, Write};
use std::sync::Arc;
use windows::core::{HSTRING, PWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::*;
use windows::Win32::Storage::FileSystem::{FlushFileBuffers, ReadFile, WriteFile, PIPE_ACCESS_DUPLEX};
use windows::Win32::System::Pipes::*;
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

struct Pipe(HANDLE);
unsafe impl Send for Pipe {}

impl Read for Pipe {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut n = 0u32;
        unsafe { ReadFile(self.0, Some(buf), Some(&mut n), None) }.map_err(|e| std::io::Error::other(e.message()))?;
        if n == 0 && !buf.is_empty() {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        Ok(n as usize)
    }
}

impl Write for Pipe {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut n = 0u32;
        unsafe { WriteFile(self.0, Some(buf), Some(&mut n), None) }.map_err(|e| std::io::Error::other(e.message()))?;
        Ok(n as usize)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Drop for Pipe {
    fn drop(&mut self) {
        unsafe {
            let _ = FlushFileBuffers(self.0);
            let _ = DisconnectNamedPipe(self.0);
            let _ = CloseHandle(self.0);
        }
    }
}

fn user_sid() -> Option<String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).ok()?;
        let mut len = 0u32;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut len);
        let mut buf = vec![0u8; len as usize];
        let ok = GetTokenInformation(token, TokenUser, Some(buf.as_mut_ptr() as *mut _), len, &mut len).is_ok();
        let _ = CloseHandle(token);
        if !ok {
            return None;
        }
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut s = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut s).ok()?;
        let out = s.to_string().ok();
        let _ = LocalFree(Some(HLOCAL(s.0 as *mut _)));
        out
    }
}

fn security() -> Option<(SECURITY_ATTRIBUTES, PSECURITY_DESCRIPTOR)> {
    let me = user_sid()?;
    // Generic read/write for: this user, all AppContainers, all restricted
    // AppContainers, SYSTEM. Low mandatory label (no-write-up) so lower
    // integrity levels are allowed in.
    let sddl = format!("D:(A;;GRGW;;;{me})(A;;GRGW;;;AC)(A;;GRGW;;;S-1-15-2-2)(A;;GA;;;SY)S:(ML;;NW;;;LW)");
    unsafe {
        let mut sd = PSECURITY_DESCRIPTOR::default();
        ConvertStringSecurityDescriptorToSecurityDescriptorW(&HSTRING::from(sddl), SDDL_REVISION_1, &mut sd, None).ok()?;
        Some((
            SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: sd.0,
                bInheritHandle: false.into(),
            },
            sd,
        ))
    }
}

fn serve_client(mut pipe: Pipe, engine: Arc<Engine>) {
    let mut sessions: Vec<u64> = Vec::new();
    loop {
        // A request we do not know (a newer DLL than this engine) gets an
        // error reply, not a closed pipe: the DLL then falls back.
        let raw: serde_json::Value = match read_frame(&mut pipe) {
            Ok(r) => r,
            Err(_) => break,
        };
        let req: Request = match serde_json::from_value(raw) {
            Ok(r) => r,
            Err(e) => {
                if write_frame(&mut pipe, &Reply::Error { message: format!("unknown request: {e}") }).is_err() {
                    break;
                }
                continue;
            }
        };
        let reply = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| engine.handle(req.clone())))
            .unwrap_or_else(|_| Reply::Error { message: "engine panicked".into() });
        match (&req, &reply) {
            (Request::Hello { .. }, Reply::Hello { session, .. }) => sessions.push(*session),
            (Request::Bye { session }, _) => sessions.retain(|s| s != session),
            _ => {}
        }
        if write_frame(&mut pipe, &reply).is_err() {
            break;
        }
    }
    // The program closed (or crashed): its sessions go too.
    for s in sessions {
        engine.handle(Request::Bye { session: s });
    }
}

pub fn run(name: &str, engine: Arc<Engine>) {
    let Some((sa, _sd)) = security() else {
        crate::log("pipe: could not build the security descriptor");
        return;
    };
    let wide = HSTRING::from(name);
    crate::log(&format!("listening on {name}"));
    loop {
        let h = unsafe {
            CreateNamedPipeW(
                &wide,
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                64 * 1024,
                64 * 1024,
                0,
                Some(&sa),
            )
        };
        if h.is_invalid() {
            crate::log("pipe: CreateNamedPipe failed");
            std::thread::sleep(std::time::Duration::from_secs(1));
            continue;
        }
        let connected = unsafe { ConnectNamedPipe(h, None) }.is_ok() || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
        if !connected {
            unsafe {
                let _ = CloseHandle(h);
            }
            continue;
        }
        let e = engine.clone();
        let pipe = Pipe(h);
        let _ = std::thread::Builder::new().name("ime-client".into()).spawn(move || serve_client(pipe, e));
    }
}
