//! System counters.
//!
//! CPU / memory / disks come from `sysinfo`, which needs no privileges.
//! Fan RPM, board and GPU temperatures, GPU load and VRAM have no usable
//! public Windows API, so they come from LibreHardwareMonitor's built-in web
//! server -- LHM has to be running, as administrator, with "Remote Web Server"
//! enabled. When it isn't, `lhm` reports false and the widget says so instead
//! of inventing numbers.

use serde::Serialize;
use serde_json::Value;
use sysinfo::{Disks, System};

#[derive(Debug, Clone, Serialize)]
pub struct DiskInfo {
    pub name: String,
    pub total: u64,
    pub used: u64,
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
    pub lhm: bool,
    pub lhm_error: Option<String>,
}

pub struct Sampler {
    sys: System,
    disks: Disks,
    cpu_name: String,
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
        Sampler { sys, disks: Disks::new_with_refreshed_list(), cpu_name }
    }

    /// One pass over the cheap counters. CPU percentages are meaningless on the
    /// first call -- the caller throws that sample away.
    pub fn sample(&mut self) -> Snapshot {
        self.sys.refresh_cpu_all();
        self.sys.refresh_memory();
        self.disks.refresh();

        let disks = self
            .disks
            .iter()
            .map(|d| {
                let total = d.total_space();
                let free = d.available_space();
                DiskInfo {
                    name: d.mount_point().to_string_lossy().trim_end_matches('\\').to_string(),
                    total,
                    used: total.saturating_sub(free),
                }
            })
            .filter(|d| d.total > 0)
            .collect();

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
            ..Default::default()
        }
    }
}

/// Where to look for LibreHardwareMonitor, most likely first.
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

/// Pull the sensor tree out of LibreHardwareMonitor and fold it into the
/// snapshot. Any failure is recorded rather than propagated: the rest of the
/// widget keeps working without it.
pub async fn merge_lhm(snapshot: &mut Snapshot, url: &str) {
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        // Without this, a system-wide proxy (a VPN client, usually) swallows
        // the request to 127.0.0.1 and the sensors look permanently missing.
        .no_proxy()
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            snapshot.lhm_error = Some(e.to_string());
            return;
        }
    };

    let mut failures = Vec::new();
    let mut tree: Option<Value> = None;
    for candidate in candidates(url) {
        match client.get(&candidate).send().await {
            Ok(resp) if resp.status().is_success() => match resp.json::<Value>().await {
                Ok(v) => {
                    tree = Some(v);
                    break;
                }
                Err(e) => failures.push(format!("{candidate} 返回的不是 JSON({e})")),
            },
            Ok(resp) => failures.push(format!("{candidate} 返回 {}", resp.status())),
            Err(e) => {
                let why = if e.is_connect() {
                    "连不上"
                } else if e.is_timeout() {
                    "超时"
                } else {
                    "请求失败"
                };
                failures.push(format!("{candidate} {why}"));
            }
        }
    }

    let Some(tree) = tree else {
        snapshot.lhm_error = Some(format!(
            "LibreHardwareMonitor 读不到:{}",
            failures.join(";")
        ));
        return;
    };

    walk(&tree, &mut Vec::new(), snapshot);
    snapshot.lhm = true;

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
