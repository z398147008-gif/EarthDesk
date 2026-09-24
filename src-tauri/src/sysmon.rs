//! System counters.
//!
//! CPU / memory / disks come from `sysinfo`, which needs no privileges.
//! Fan RPM, board and GPU temperatures, GPU load and VRAM have no usable
//! public Windows API. They come from EarthDesk's own hardware monitor
//! service (sensors/EarthDeskSensors.cs, LibreHardwareMonitorLib inside),
//! read over the named pipe `\\.\pipe\EarthDeskSensors`. An external
//! LibreHardwareMonitor web server (`sensors.lhm_url`) is only a fallback for
//! when the service is not there. When neither answers, `lhm` is false and
//! the widget says so instead of inventing numbers.

use serde::Serialize;
use serde_json::Value;
use sysinfo::{DiskKind, Disks, System};

#[derive(Debug, Clone, Serialize)]
pub struct DiskInfo {
    /// Drive letter ("C:") or mount folder.
    pub name: String,
    pub total: u64,
    pub used: u64,
    /// Volume label as Explorer shows it; empty when the volume has none.
    pub label: String,
    /// Plugged in over USB / SD / FireWire, or removable media: a drive that
    /// can come and go while the widget is running.
    pub external: bool,
    /// "ssd" | "hdd" | "" when Windows won't say (common behind USB bridges).
    pub kind: String,
    /// Model from the device descriptor, e.g. "WD My Passport 25E2".
    pub model: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Reading {
    /// Leaf sensor name, e.g. "CPU Package" or "Fan #2".
    pub name: String,
    /// Owning hardware, e.g. "NVIDIA GeForce RTX 4070".
    pub owner: String,
    /// "cpu" | "gpu" | "mainboard" | "memory" | "storage" | "other"
    pub source: String,
    pub value: f64,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Snapshot {
    pub cpu: f32,
    pub cpu_per_core: Vec<f32>,
    pub cpu_name: String,
    pub mem_used: u64,
    pub mem_total: u64,
    pub swap_used: u64,
    pub swap_total: u64,
    pub uptime: u64,
    pub disks: Vec<DiskInfo>,

    /// Everything LibreHardwareMonitor reports, split by unit.
    pub temps: Vec<Reading>,
    pub fans: Vec<Reading>,
    pub loads: Vec<Reading>,
    pub powers: Vec<Reading>,
    pub clocks: Vec<Reading>,
    /// Megabyte-valued sensors -- VRAM, mostly.
    pub data: Vec<Reading>,

    pub gpu_name: Option<String>,
    /// Temperatures / fans / GPU arrived (from either source). Named for the
    /// LibreHardwareMonitor web server it originally meant; perf.js keys on it.
    pub lhm: bool,
    pub lhm_error: Option<String>,
    /// "service" | "lhm" (external web server) | "" (nothing answered).
    pub sensor_source: String,
    /// What the service found, "Cpu: Intel Core i7-13700KF" and so on, for
    /// the settings page.
    pub hardware: Vec<String>,
    /// Whether the PawnIO driver is installed, as the service reports it.
    pub pawnio: Option<bool>,
    /// The service answered the pipe this sample (even if only to say it is
    /// still starting). What the nurse in main.rs goes by: a live service
    /// that is busy opening must not be restarted.
    pub sensor_alive: bool,
    /// What is in the Recycle Bin (all drives); None until first read.
    pub recycle: Option<crate::recycle::RecycleBin>,
}

pub struct Sampler {
    sys: System,
    disks: Disks,
    /// (external, model) per mount point, filled in whenever the list is
    /// rebuilt -- asking the device is too slow to do every sample.
    disk_meta: std::collections::HashMap<String, (bool, String)>,
    /// Drive letters present at the last enumeration (bit 0 = A:).
    drive_mask: u32,
    /// Samples since the list was last rebuilt.
    since_list: u32,
    cpu_name: String,
    /// Latest Recycle Bin reading, kept current by its own thread.
    recycle: std::sync::Arc<std::sync::Mutex<Option<crate::recycle::RecycleBin>>>,
}

/// Rebuild the disk list at least this often (in samples, ~30 s at the default
/// 2 s) even when no drive letter came or went. That picks up volumes mounted
/// into a folder, and letters reassigned in Disk Management.
const RELIST_EVERY: u32 = 15;

fn mount_name(disk: &sysinfo::Disk) -> String {
    disk.mount_point().to_string_lossy().trim_end_matches('\\').to_string()
}

impl Sampler {
    pub fn new() -> Self {
        let mut sys = System::new_all();
        sys.refresh_cpu_all();
        sys.refresh_memory();
        let cpu_name = sys
            .cpus()
            .first()
            .map(|c| c.brand().trim().to_string())
            .unwrap_or_else(|| "CPU".to_string());
        let mut sampler = Sampler {
            sys,
            disks: Disks::new(),
            disk_meta: Default::default(),
            drive_mask: 0,
            since_list: 0,
            cpu_name,
            recycle: crate::recycle::watch(),
        };
        sampler.relist_disks(drives::letter_mask());
        sampler
    }

    /// Enumerate the volumes from scratch.
    ///
    /// `Disks::refresh()` only updates free space on the volumes found the
    /// first time round, so before this a drive plugged in after start-up
    /// never showed up, and one pulled out stayed on the list with stale
    /// numbers.
    fn relist_disks(&mut self, mask: u32) {
        self.disks.refresh_list();
        self.drive_mask = mask;
        self.since_list = 0;
        self.disk_meta = self
            .disks
            .iter()
            .map(|d| {
                let mount = mount_name(d);
                let (bus, model) = drives::describe(&mount);
                (mount, (d.is_removable() || drives::is_external_bus(bus), model))
            })
            .collect();
    }

    /// Cheap check every sample: did a drive letter appear or disappear, or
    /// did a known volume stop answering? Either one rebuilds the list, so a
    /// drive shows up (or goes away) within one sample.
    fn refresh_disks(&mut self) {
        let mask = drives::letter_mask();
        self.since_list += 1;
        let mut stale = mask != self.drive_mask || self.since_list >= RELIST_EVERY;
        if !stale {
            for disk in self.disks.list_mut() {
                if !disk.refresh() {
                    stale = true;
                }
            }
        }
        if stale {
            self.relist_disks(mask);
        }
    }

    /// One pass over the cheap counters. CPU percentages are meaningless on the
    /// first call -- the caller throws that sample away.
    pub fn sample(&mut self) -> Snapshot {
        self.sys.refresh_cpu_all();
        self.sys.refresh_memory();
        self.refresh_disks();

        let mut disks: Vec<DiskInfo> = self
            .disks
            .iter()
            .map(|d| {
                let total = d.total_space();
                let free = d.available_space();
                let name = mount_name(d);
                let (external, model) = self.disk_meta.get(&name).cloned().unwrap_or_default();
                DiskInfo {
                    total,
                    used: total.saturating_sub(free),
                    label: d.name().to_string_lossy().trim().to_string(),
                    external,
                    kind: match d.kind() {
                        DiskKind::SSD => "ssd",
                        DiskKind::HDD => "hdd",
                        _ => "",
                    }
                    .to_string(),
                    model,
                    name,
                }
            })
            .filter(|d| d.total > 0)
            .collect();
        // Internal drives first, then the ones that come and go; letters in
        // order within each. Enumeration order alone isn't stable.
        disks.sort_by(|a, b| a.external.cmp(&b.external).then_with(|| a.name.cmp(&b.name)));

        Snapshot {
            cpu: self.sys.global_cpu_usage(),
            cpu_per_core: self.sys.cpus().iter().map(|c| c.cpu_usage()).collect(),
            cpu_name: self.cpu_name.clone(),
            mem_used: self.sys.used_memory(),
            mem_total: self.sys.total_memory(),
            swap_used: self.sys.used_swap(),
            swap_total: self.sys.total_swap(),
            uptime: System::uptime(),
            disks,
            recycle: self.recycle.lock().ok().and_then(|r| r.clone()),
            ..Default::default()
        }
    }
}

/// The bits of Win32 storage the `sysinfo` crate doesn't expose: which drive
/// letters exist right now, and what bus a volume's disk hangs off. Reached
/// through hand-declared FFI, like platform.rs, rather than the `windows`
/// crate. Needs no administrator rights: the volume is opened with zero access,
/// which is enough for a property query.
#[cfg(windows)]
mod drives {
    use std::ffi::c_void;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetLogicalDrives() -> u32;
        fn CreateFileW(
            name: *const u16,
            access: u32,
            share: u32,
            security: *mut c_void,
            disposition: u32,
            flags: u32,
            template: isize,
        ) -> isize;
        fn DeviceIoControl(
            device: isize,
            code: u32,
            input: *const c_void,
            input_size: u32,
            output: *mut c_void,
            output_size: u32,
            returned: *mut u32,
            overlapped: *mut c_void,
        ) -> i32;
        fn CloseHandle(handle: isize) -> i32;
    }

    const INVALID_HANDLE: isize = -1;
    const FILE_SHARE_READ_WRITE: u32 = 0x1 | 0x2;
    const OPEN_EXISTING: u32 = 3;
    /// CTL_CODE(IOCTL_STORAGE_BASE, 0x500, METHOD_BUFFERED, FILE_ANY_ACCESS)
    const IOCTL_STORAGE_QUERY_PROPERTY: u32 = 0x002D_1400;

    /// STORAGE_PROPERTY_QUERY { StorageDeviceProperty, PropertyStandardQuery }
    #[repr(C)]
    struct Query {
        property_id: u32,
        query_type: u32,
        extra: [u8; 4],
    }

    pub fn letter_mask() -> u32 {
        unsafe { GetLogicalDrives() }
    }

    /// STORAGE_BUS_TYPE values that mean "a drive you plug in".
    pub fn is_external_bus(bus: u32) -> bool {
        matches!(bus, 4 /* 1394 */ | 7 /* USB */ | 12 /* SD */ | 13 /* MMC */)
    }

    /// (STORAGE_BUS_TYPE, "vendor product") for a drive letter like "E:".
    /// Anything that isn't a plain letter, or doesn't answer, gives (0, "").
    pub fn describe(mount: &str) -> (u32, String) {
        let bytes = mount.as_bytes();
        if bytes.len() != 2 || !bytes[0].is_ascii_alphabetic() || bytes[1] != b':' {
            return (0, String::new());
        }
        let path: Vec<u16> = format!(r"\\.\{mount}").encode_utf16().chain([0]).collect();
        unsafe {
            let handle = CreateFileW(
                path.as_ptr(),
                0,
                FILE_SHARE_READ_WRITE,
                std::ptr::null_mut(),
                OPEN_EXISTING,
                0,
                0,
            );
            if handle == INVALID_HANDLE || handle == 0 {
                return (0, String::new());
            }
            let query = Query { property_id: 0, query_type: 0, extra: [0; 4] };
            // STORAGE_DEVICE_DESCRIPTOR plus the strings it points into.
            let mut buffer = [0u64; 128];
            let mut returned = 0u32;
            let ok = DeviceIoControl(
                handle,
                IOCTL_STORAGE_QUERY_PROPERTY,
                &query as *const Query as *const c_void,
                std::mem::size_of::<Query>() as u32,
                buffer.as_mut_ptr() as *mut c_void,
                std::mem::size_of_val(&buffer) as u32,
                &mut returned,
                std::ptr::null_mut(),
            );
            CloseHandle(handle);
            if ok == 0 || returned < 32 {
                return (0, String::new());
            }
            let raw = std::slice::from_raw_parts(buffer.as_ptr() as *const u8, returned as usize);
            let u32_at = |at: usize| u32::from_le_bytes([raw[at], raw[at + 1], raw[at + 2], raw[at + 3]]);
            // Offsets: VendorIdOffset 12, ProductIdOffset 16, BusType 28.
            let text_at = |offset: u32| -> String {
                let start = offset as usize;
                if start == 0 || start >= raw.len() {
                    return String::new();
                }
                let end = raw[start..].iter().position(|&b| b == 0).map_or(raw.len(), |n| start + n);
                String::from_utf8_lossy(&raw[start..end]).trim().to_string()
            };
            let vendor = text_at(u32_at(12));
            let product = text_at(u32_at(16));
            let model = if vendor.is_empty() || product.starts_with(&vendor) {
                product
            } else {
                format!("{vendor} {product}")
            };
            (u32_at(28), model.split_whitespace().collect::<Vec<_>>().join(" "))
        }
    }
}

#[cfg(not(windows))]
mod drives {
    pub fn letter_mask() -> u32 {
        0
    }
    pub fn is_external_bus(_bus: u32) -> bool {
        false
    }
    pub fn describe(_mount: &str) -> (u32, String) {
        (0, String::new())
    }
}

/// The service's pipe. See sensors/EarthDeskSensors.cs for the other end.
#[cfg_attr(not(windows), allow(dead_code))]
pub const PIPE: &str = r"\\.\pipe\EarthDeskSensors";

/// One reading from the service: connect, read to the end, done.
#[cfg(windows)]
fn read_pipe() -> std::io::Result<String> {
    use std::io::Read;
    const ERROR_BROKEN_PIPE: i32 = 109;
    const ERROR_PIPE_BUSY: i32 = 231;
    let mut busy = None;
    for _ in 0..20 {
        match std::fs::OpenOptions::new().read(true).open(PIPE) {
            Ok(mut file) => {
                let mut bytes = Vec::with_capacity(32 * 1024);
                let mut chunk = [0u8; 16 * 1024];
                loop {
                    match file.read(&mut chunk) {
                        Ok(0) => break,
                        Ok(n) => bytes.extend_from_slice(&chunk[..n]),
                        // The service closing its end is the end of the reply.
                        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe
                            || e.raw_os_error() == Some(ERROR_BROKEN_PIPE) => break,
                        Err(e) => return Err(e),
                    }
                }
                return String::from_utf8(bytes)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e));
            }
            // Someone else is being answered right now; it takes milliseconds.
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                busy = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => return Err(e),
        }
    }
    Err(busy.unwrap_or_else(|| std::io::Error::other("pipe busy")))
}

#[cfg(not(windows))]
fn read_pipe() -> std::io::Result<String> {
    Err(std::io::Error::new(std::io::ErrorKind::NotFound, "no service on this platform"))
}

/// Ask the service, with a deadline: a service stuck inside a driver call
/// must not stall the whole widget.
async fn ask_service() -> Result<Value, String> {
    let job = tauri::async_runtime::spawn_blocking(read_pipe);
    let text = match tokio::time::timeout(std::time::Duration::from_secs(4), job).await {
        Err(_) => return Err("硬件监控服务没有响应".into()),
        Ok(Err(e)) => return Err(e.to_string()),
        Ok(Ok(Err(e))) => {
            return Err(if e.kind() == std::io::ErrorKind::NotFound {
                "硬件监控服务没有运行".to_string()
            } else {
                format!("读取硬件监控服务失败({e})")
            })
        }
        Ok(Ok(Ok(t))) => t,
    };
    serde_json::from_str::<Value>(&text).map_err(|e| format!("硬件监控服务返回的数据有误({e})"))
}

/// Pull sensors from the service -- or, failing that, from an external
/// LibreHardwareMonitor web server -- and fold them into the snapshot. Any
/// failure is recorded rather than propagated: the rest of the widget keeps
/// working without it.
pub async fn merge_sensors(snapshot: &mut Snapshot, fallback_url: &str) {
    let service_error = match ask_service().await {
        Ok(v) => {
            snapshot.sensor_alive = true;
            let usable = v.get("hardware").and_then(Value::as_array).map_or(false, |h| !h.is_empty());
            snapshot.pawnio = v.get("pawnio").and_then(Value::as_bool);
            if usable {
                fold_service(&v, snapshot);
                snapshot.sensor_source = "service".into();
                finish(snapshot);
                return;
            }
            let why = v.get("error").and_then(Value::as_str).unwrap_or("").trim().to_string();
            if why.is_empty() {
                "硬件监控服务正在启动".to_string()
            } else {
                format!("硬件监控服务出错:{why}")
            }
        }
        Err(e) => e,
    };

    let url = fallback_url.trim();
    if !url.is_empty() {
        if let Ok(tree) = fetch_lhm(url).await {
            walk(&tree, &mut Vec::new(), snapshot);
            snapshot.sensor_source = "lhm".into();
            finish(snapshot);
            return;
        }
    }
    snapshot.lhm_error = Some(service_error);
}

/// The service's JSON: a flat list of hardware (sub-hardware carries its
/// parent's name), each with typed sensors. Types are LibreHardwareMonitor's
/// own enums, so no guessing from icons or units as with the web server.
fn fold_service(v: &Value, out: &mut Snapshot) {
    let Some(list) = v.get("hardware").and_then(Value::as_array) else { return };
    for hw in list {
        let name = hw.get("name").and_then(Value::as_str).unwrap_or("").trim().to_string();
        let kind = hw.get("type").and_then(Value::as_str).unwrap_or("");
        let source = match kind {
            "Cpu" => "cpu",
            "GpuNvidia" | "GpuAmd" | "GpuIntel" => "gpu",
            "Motherboard" | "SuperIO" | "EmbeddedController" => "mainboard",
            "Memory" => "memory",
            "Storage" => "storage",
            _ => "other",
        };
        if hw.get("parent").map_or(true, Value::is_null) {
            out.hardware.push(format!("{kind}: {name}"));
        }
        let Some(sensors) = hw.get("sensors").and_then(Value::as_array) else { continue };
        for s in sensors {
            let Some(value) = s.get("value").and_then(Value::as_f64) else { continue };
            if !value.is_finite() {
                continue;
            }
            let reading = Reading {
                name: s.get("name").and_then(Value::as_str).unwrap_or("").trim().to_string(),
                owner: name.clone(),
                source: source.to_string(),
                value,
            };
            // The same grouping LibreHardwareMonitor's web page uses, which is
            // what perf.js was written against.
            match s.get("type").and_then(Value::as_str).unwrap_or("") {
                "Temperature" => out.temps.push(reading),
                "Fan" => out.fans.push(reading),
                "Power" => out.powers.push(reading),
                "Load" | "Control" | "Level" | "Humidity" => out.loads.push(reading),
                "Clock" => out.clocks.push(reading),
                "Data" => out.data.push(Reading { value: value * 1024.0, ..reading }),
                "SmallData" => out.data.push(reading),
                _ => {}
            }
        }
    }
}

/// Shared tail of both sources: mark success, name the GPU, drop duplicates.
fn finish(snapshot: &mut Snapshot) {
    snapshot.lhm = true;
    snapshot.lhm_error = None;
    snapshot.gpu_name = snapshot
        .loads
        .iter()
        .chain(snapshot.temps.iter())
        .find(|r| r.source == "gpu")
        .map(|r| r.owner.clone());

    for list in [
        &mut snapshot.temps,
        &mut snapshot.fans,
        &mut snapshot.loads,
        &mut snapshot.powers,
        &mut snapshot.clocks,
        &mut snapshot.data,
    ] {
        dedup(list);
    }
}

/// Where to look for an external LibreHardwareMonitor, most likely first.
///
/// The configured URL may be the page itself ("http://host:port/") rather
/// than its JSON feed, so a bare path gets `data.json` appended. And because
/// LHM listens on every interface, the same port on 127.0.0.1 is tried too:
/// it keeps working when the address that was typed in belongs to a VPN or
/// tailnet that happens to be down.
fn candidates(configured: &str) -> Vec<String> {
    let trimmed = configured.trim();
    let json = if trimmed.ends_with(".json") {
        trimmed.to_string()
    } else {
        format!("{}/data.json", trimmed.trim_end_matches('/'))
    };

    let mut list = vec![json.clone()];
    if let Some(port) = json
        .split("://")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .and_then(|host| host.rsplit_once(':'))
        .map(|(_, port)| port.to_string())
    {
        let local = format!("http://127.0.0.1:{port}/data.json");
        if !list.contains(&local) {
            list.push(local);
        }
    }
    list
}

/// The external web server's sensor tree, from the first address that answers.
async fn fetch_lhm(url: &str) -> Result<Value, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        // Without this, a system-wide proxy (a VPN client, usually) swallows
        // the request to 127.0.0.1.
        .no_proxy()
        .build()
        .map_err(|e| e.to_string())?;
    let mut last = String::new();
    for candidate in candidates(url) {
        match client.get(&candidate).send().await {
            Ok(resp) if resp.status().is_success() => match resp.json::<Value>().await {
                Ok(v) => return Ok(v),
                Err(e) => last = e.to_string(),
            },
            Ok(resp) => last = resp.status().to_string(),
            Err(e) => last = e.to_string(),
        }
    }
    Err(last)
}

fn dedup(list: &mut Vec<Reading>) {
    let mut seen = std::collections::HashSet::new();
    list.retain(|r| seen.insert(format!("{}|{}", r.owner, r.name)));
}

/// LHM tags each hardware node with a small icon; it is the only field that
/// reliably says *what kind* of device a sensor belongs to, so it beats
/// guessing from the name.
fn classify(node: &Value, name: &str) -> String {
    let icon = node
        .get("ImageURL")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    let lower = name.to_ascii_lowercase();

    let has = |needles: &[&str]| needles.iter().any(|n| icon.contains(n) || lower.contains(n));

    if has(&["nvidia", "geforce", "radeon", "ati.png", "amd.png", "gpu"]) {
        // "amd.png" also marks Ryzen CPUs, so make sure it is not a processor.
        if !lower.contains("ryzen") && !lower.contains("threadripper") {
            return "gpu".into();
        }
    }
    if has(&["cpu", "intel", "ryzen", "core i", "threadripper"]) {
        return "cpu".into();
    }
    if has(&["mainboard", "motherboard", "chip", "nuvoton", "ite", "asrock", "msi", "gigabyte", "asus"]) {
        return "mainboard".into();
    }
    if has(&["ram", "memory", "dimm"]) {
        return "memory".into();
    }
    if has(&["hdd", "ssd", "nvme", "storage"]) {
        return "storage".into();
    }
    "other".into()
}

/// LHM's JSON is a uniform tree of `{Text, Value, Children}`. Leaves carry a
/// formatted value like "45.0 °C" or "1234 RPM"; the unit is the only reliable
/// way to tell what kind of sensor we are looking at.
///
/// `trail` holds (name, source) for each ancestor. Depth 0 is the "Sensor"
/// root, depth 1 is the machine, depth 2 is the actual piece of hardware.
fn walk(node: &Value, trail: &mut Vec<(String, String)>, out: &mut Snapshot) {
    let text = node.get("Text").and_then(Value::as_str).unwrap_or("").trim().to_string();
    let raw = node.get("Value").and_then(Value::as_str).unwrap_or("").trim().to_string();

    if !raw.is_empty() && !text.is_empty() {
        if let Some((value, unit)) = split_value(&raw) {
            // The nearest ancestor that looks like real hardware owns this
            // sensor; the category folder ("Temperatures", "Fans") does not.
            let owner = trail
                .iter()
                .rev()
                .find(|(_, source)| source.as_str() != "other")
                .or(trail.last())
                .cloned()
                .unwrap_or_else(|| (String::new(), "other".into()));

            let reading = Reading {
                name: text.clone(),
                owner: owner.0,
                source: owner.1,
                value,
            };
            match unit.as_str() {
                "°C" | "C" => out.temps.push(reading),
                "RPM" => out.fans.push(reading),
                "W" => out.powers.push(reading),
                "%" => out.loads.push(reading),
                "MHz" => out.clocks.push(reading),
                // Normalise VRAM-style sensors to megabytes.
                "MB" => out.data.push(reading),
                "GB" => out.data.push(Reading { value: value * 1024.0, ..reading }),
                _ => {}
            }
        }
    }

    if let Some(children) = node.get("Children").and_then(Value::as_array) {
        if !children.is_empty() {
            trail.push((text.clone(), classify(node, &text)));
            for child in children {
                walk(child, trail, out);
            }
            trail.pop();
        }
    }
}

fn split_value(raw: &str) -> Option<(f64, String)> {
    let mut number = String::new();
    let mut rest = "";
    for (i, ch) in raw.char_indices() {
        if ch.is_ascii_digit() || ch == '.' || ch == '-' || ch == '+' {
            number.push(ch);
        } else {
            rest = raw[i..].trim();
            break;
        }
    }
    let parsed = number.parse::<f64>().ok()?;
    if !parsed.is_finite() {
        return None;
    }
    Some((parsed, rest.to_string()))
}
