//! The Recycle Bin: how much is in it, and how much it may hold.
//!
//! Size and item count come from `SHQueryRecycleBinW`, once per drive letter
//! so the tooltip can break it down. The limit is the per-volume "Maximum
//! size" from the Recycle Bin's Properties dialog, which Explorer stores under
//! `HKCU\...\Explorer\BitBucket\Volume\{volume GUID}\MaxCapacity` (MB) next to
//! `NukeOnDelete` ("don't move files to the Recycle Bin").
//!
//! Asking the shell walks the `$Recycle.Bin` folders, which is cheap for a
//! normal bin but not free for one holding tens of thousands of files, so it
//! runs on its own thread every few seconds and the performance widget reads
//! the last answer.

use serde::Serialize;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Serialize, Default)]
pub struct RecycleDrive {
    /// "C:"
    pub name: String,
    pub size: u64,
    pub items: u64,
    /// Bytes this drive's bin may hold; 0 when unknown.
    pub capacity: u64,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct RecycleBin {
    pub size: u64,
    pub items: u64,
    /// Sum of the per-drive limits; 0 when none is known.
    pub capacity: u64,
    /// Some limit was not recorded and was taken as Windows' default (5% of
    /// the volume), so `capacity` is approximate.
    pub approx: bool,
    pub drives: Vec<RecycleDrive>,
}

/// How often the bin is looked at. Deleting a file shows up within this.
#[cfg_attr(not(windows), allow(dead_code))]
const EVERY: std::time::Duration = std::time::Duration::from_secs(4);

/// Start the watcher thread; the returned slot always holds the latest
/// reading (None until the first one, and always None off Windows).
pub fn watch() -> Arc<Mutex<Option<RecycleBin>>> {
    let slot = Arc::new(Mutex::new(None));
    #[cfg(windows)]
    {
        let out = slot.clone();
        let _ = std::thread::Builder::new().name("recycle-bin".into()).spawn(move || {
            imp::init_thread();
            loop {
                let reading = imp::query();
                if let Ok(mut s) = out.lock() {
                    *s = reading;
                }
                std::thread::sleep(EVERY);
            }
        });
    }
    slot
}

#[cfg(windows)]
mod imp {
    use super::{RecycleBin, RecycleDrive};

    /// SHQUERYRBINFO. shellapi.h packs to 1 byte only on 32-bit Windows; on
    /// x64 it is naturally aligned (4 bytes padding after cbSize, 24 total).
    #[repr(C)]
    struct QueryInfo {
        cb_size: u32,
        size: i64,
        items: i64,
    }

    #[link(name = "shell32")]
    extern "system" {
        fn SHQueryRecycleBinW(root: *const u16, info: *mut QueryInfo) -> i32;
    }
    #[link(name = "ole32")]
    extern "system" {
        fn CoInitializeEx(reserved: *mut core::ffi::c_void, flags: u32) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetLogicalDrives() -> u32;
        fn GetDriveTypeW(root: *const u16) -> u32;
        fn GetVolumeNameForVolumeMountPointW(mount: *const u16, name: *mut u16, len: u32) -> i32;
        fn GetDiskFreeSpaceExW(root: *const u16, avail: *mut u64, total: *mut u64, free: *mut u64) -> i32;
    }
    #[link(name = "advapi32")]
    extern "system" {
        fn RegGetValueW(
            key: isize,
            sub_key: *const u16,
            value: *const u16,
            flags: u32,
            kind: *mut u32,
            data: *mut u8,
            size: *mut u32,
        ) -> i32;
    }

    const COINIT_APARTMENTTHREADED: u32 = 0x2;
    const DRIVE_REMOVABLE: u32 = 2;
    const DRIVE_FIXED: u32 = 3;
    const HKEY_CURRENT_USER: isize = 0x8000_0001u32 as i32 as isize;
    const RRF_RT_REG_DWORD: u32 = 0x0000_0010;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn init_thread() {
        // The shell's folder code expects COM on the calling thread.
        unsafe {
            CoInitializeEx(std::ptr::null_mut(), COINIT_APARTMENTTHREADED);
        }
    }

    fn reg_dword(sub_key: &str, value: &str) -> Option<u32> {
        let sub = wide(sub_key);
        let val = wide(value);
        let mut data = 0u32;
        let mut size = 4u32;
        let mut kind = 0u32;
        let rc = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                sub.as_ptr(),
                val.as_ptr(),
                RRF_RT_REG_DWORD,
                &mut kind,
                &mut data as *mut u32 as *mut u8,
                &mut size,
            )
        };
        (rc == 0).then_some(data)
    }

    /// "{xxxxxxxx-...}" for a root like "C:\".
    fn volume_guid(root: &[u16]) -> Option<String> {
        let mut buf = [0u16; 64];
        if unsafe { GetVolumeNameForVolumeMountPointW(root.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) } == 0 {
            return None;
        }
        let name = String::from_utf16_lossy(&buf[..buf.iter().position(|&c| c == 0).unwrap_or(buf.len())]);
        let start = name.find('{')?;
        let end = name[start..].find('}')? + start;
        Some(name[start..=end].to_string())
    }

    pub fn query() -> Option<RecycleBin> {
        let mask = unsafe { GetLogicalDrives() };
        let mut bin = RecycleBin::default();
        let mut any = false;
        for i in 0..26u32 {
            if mask & (1 << i) == 0 {
                continue;
            }
            let letter = (b'A' + i as u8) as char;
            let root = wide(&format!("{letter}:\\"));
            let kind = unsafe { GetDriveTypeW(root.as_ptr()) };
            // Network shares and optical drives have no bin; USB sticks
            // usually don't either, but USB hard drives often do.
            if kind != DRIVE_FIXED && kind != DRIVE_REMOVABLE {
                continue;
            }
            let mut info = QueryInfo { cb_size: std::mem::size_of::<QueryInfo>() as u32, size: 0, items: 0 };
            if unsafe { SHQueryRecycleBinW(root.as_ptr(), &mut info) } != 0 {
                continue;
            }
            any = true;

            let key = volume_guid(&root)
                .map(|g| format!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\BitBucket\Volume\{g}"));
            let nuke = key.as_deref().and_then(|k| reg_dword(k, "NukeOnDelete")).unwrap_or(0) != 0;
            let mut capacity = 0u64;
            if !nuke {
                match key.as_deref().and_then(|k| reg_dword(k, "MaxCapacity")) {
                    Some(mb) => capacity = mb as u64 * 1024 * 1024,
                    None => {
                        let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
                        if unsafe { GetDiskFreeSpaceExW(root.as_ptr(), &mut avail, &mut total, &mut free) } != 0 {
                            capacity = total / 20;
                            bin.approx = true;
                        }
                    }
                }
            }

            let size = info.size.max(0) as u64;
            let items = info.items.max(0) as u64;
            // A drive with no bin (removable stick, "delete immediately") and
            // nothing in it is not worth a line in the tooltip.
            if size == 0 && items == 0 && capacity == 0 {
                continue;
            }
            bin.size += size;
            bin.items += items;
            bin.capacity += capacity;
            bin.drives.push(RecycleDrive { name: format!("{letter}:"), size, items, capacity });
        }
        any.then_some(bin)
    }
}
