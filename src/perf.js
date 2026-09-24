const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const el = (id) => document.getElementById(id);
const RING = 2 * Math.PI * 40;

let features = {};
const isOn = (key) => features[key] !== false;
let lastSnapshot = null;

// --- formatting ------------------------------------------------------------

function bytes(value) {
  const gb = value / 1024 ** 3;
  if (gb >= 1024) return `${(gb / 1024).toFixed(1)} TB`;
  if (gb >= 100) return `${gb.toFixed(0)} GB`;
  return `${gb.toFixed(1)} GB`;
}

function duration(seconds) {
  const d = Math.floor(seconds / 86400);
  const h = Math.floor((seconds % 86400) / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  if (d) return `已运行 ${d} 天 ${h} 小时`;
  if (h) return `已运行 ${h} 小时 ${m} 分`;
  return `已运行 ${m} 分`;
}

/// Green while there is headroom, amber when busy, red when pegged -- the
/// same palette the system uses for battery state, read the other way round.
function loadColor(pct) {
  if (pct >= 85) return "#ff453a";
  if (pct >= 60) return "#ffd60a";
  return "#30d158";
}

function tempColor(c) {
  if (c >= 85) return "#ff453a";
  if (c >= 70) return "#ff9f0a";
  return null;
}

// --- picking sensors out of the LHM tree -----------------------------------

const pick = (list, source, ...needles) => {
  const pool = (list || []).filter((r) => r.source === source);
  for (const needle of needles) {
    const hit = pool.find((r) => r.name.toLowerCase().includes(needle));
    if (hit) return hit;
  }
  return null;
};

const hottest = (list, source) =>
  (list || [])
    .filter((r) => r.source === source)
    .reduce((best, r) => (!best || r.value > best.value ? r : best), null);

function cpuTemperature(s) {
  return pick(s.temps, "cpu", "package", "core average", "tctl") || hottest(s.temps, "cpu");
}

const exact = (list, source, name) =>
  (list || []).find((r) => r.source === source && r.name.toLowerCase() === name) || null;

function gpuState(s) {
  return {
    load: pick(s.loads, "gpu", "gpu core", "core", "d3d 3d"),
    temp: pick(s.temps, "gpu", "gpu core", "core") || hottest(s.temps, "gpu"),
    hotspot: pick(s.temps, "gpu", "hot spot"),
    used: exact(s.data, "gpu", "gpu memory used") || pick(s.data, "gpu", "memory used", "used"),
    total: exact(s.data, "gpu", "gpu memory total") || pick(s.data, "gpu", "memory total", "total"),
    power: pick(s.powers, "gpu", "package", "board", "power"),
    clock: exact(s.clocks, "gpu", "gpu core") || pick(s.clocks, "gpu", "core"),
    fans: (s.fans || []).filter((f) => f.source === "gpu"),
  };
}

function cpuState(s) {
  const cores = (s.clocks || []).filter((c) => c.source === "cpu" && /core/i.test(c.name));
  return {
    temp: cpuTemperature(s),
    power: pick(s.powers, "cpu", "package"),
    clock: cores.reduce((best, c) => (!best || c.value > best.value ? c : best), null),
  };
}

/// Mainboard fan headers are only named "Fan #n" on most boards. Which ones
/// cool the CPU is learned (see learnFans) or taken from a name that says
/// CPU / pump / AIO; the rest are case fans. Headers reading 0 RPM have
/// nothing plugged in and are dropped.
let fanRoles = {};
function mainboardFans(s) {
  const cpu = [];
  const chassis = [];
  for (const f of (s.fans || []).filter((f) => f.source !== "gpu")) {
    if (!(f.value > 0)) continue;
    const role = fanRoles[f.name] || (/cpu|pump|aio|opt/i.test(f.name) ? "cpu" : null);
    (role === "cpu" ? cpu : chassis).push(f);
  }
  return { cpu, chassis };
}

// --- learning which fans belong to the CPU ----------------------------------
//
// The board's fan curve for the CPU header follows the CPU temperature, so
// the moment the CPU heats up that fan speeds up and the case fans do not.
// Watch both for a while; once the temperature has swung by 10 °C or more,
// a fan whose speed moved with it (correlation >= 0.6, and by >= 150 rpm) is
// the CPU cooler, one that did not (< 0.3) is a case fan. The answer is saved
// in config.json, so this only has to happen once.

const LEARN_WINDOW_MS = 10 * 60 * 1000;
const history = [];

function pearson(xs, ys) {
  const n = xs.length;
  const mx = xs.reduce((a, b) => a + b, 0) / n;
  const my = ys.reduce((a, b) => a + b, 0) / n;
  let sxy = 0, sxx = 0, syy = 0;
  for (let i = 0; i < n; i++) {
    const dx = xs[i] - mx, dy = ys[i] - my;
    sxy += dx * dy; sxx += dx * dx; syy += dy * dy;
  }
  return sxx > 0 && syy > 0 ? sxy / Math.sqrt(sxx * syy) : 0;
}

function learnFans(s) {
  const temp = cpuTemperature(s);
  const fans = {};
  for (const f of s.fans || []) if (f.source !== "gpu" && f.value > 0) fans[f.name] = f.value;
  const names = Object.keys(fans).filter((n) => !fanRoles[n]);
  if (!temp || !names.length) return;

  const now = Date.now();
  history.push({ t: now, temp: temp.value, fans });
  while (history.length && now - history[0].t > LEARN_WINDOW_MS) history.shift();
  if (history.length < 20) return;
  const temps = history.map((h) => h.temp);
  if (Math.max(...temps) - Math.min(...temps) < 10) return;

  const next = { ...fanRoles };
  let changed = false;
  for (const name of names) {
    const pts = history.filter((h) => h.fans[name] !== undefined);
    if (pts.length < 20) continue;
    const rpm = pts.map((p) => p.fans[name]);
    // A fan answers the temperature a few seconds late, so compare it with
    // the recent average temperature rather than the instantaneous one.
    const smooth = pts.map((p, i) => {
      const w = pts.slice(Math.max(0, i - 3), i + 1);
      return w.reduce((a, b) => a + b.temp, 0) / w.length;
    });
    const r = pearson(smooth, rpm);
    const span = Math.max(...rpm) - Math.min(...rpm);
    if (r >= 0.6 && span >= 150) next[name] = "cpu";
    else if (r < 0.3) next[name] = "case";
    else continue;
    changed = true;
  }
  if (changed) {
    fanRoles = next;
    invoke("set_fan_roles", { roles: next }).catch(() => {});
  }
}

/// LibreHardwareMonitor lists physical drives by model, sysinfo lists volumes
/// by drive letter. Pair them by model name when the backend could read one
/// from the drive, otherwise by each LHM drive's "Used Space" percentage,
/// which is a fingerprint good enough to tell drives apart. Pairs are handed
/// out closest first, so plugging in a drive can't steal another's match just
/// because its letter comes earlier.
const squash = (text) => String(text || "").toLowerCase().replace(/[^a-z0-9]/g, "");

function diskTemps(s) {
  const drives = new Map();
  for (const r of s.loads || []) {
    if (r.source === "storage" && /used space/i.test(r.name)) drives.set(r.owner, { used: r.value });
  }
  for (const [owner, d] of drives) {
    const temps = (s.temps || []).filter((t) => t.owner === owner && !/warning|critical|limit|threshold/i.test(t.name));
    d.temp = temps.find((t) => /composite/i.test(t.name)) || temps.find((t) => /^temperature$/i.test(t.name)) || temps[0] || null;
  }

  // USB drives the sensor service read a temperature from directly (their
  // enclosure hides it from LHM): already keyed by drive letter.
  const out = new Map();
  for (const t of s.temps || []) {
    const m = /^USB disk ([A-Z]:)$/.exec(t.owner || "");
    if (m) out.set(m[1], { model: null, temp: t });
  }

  const candidates = [];
  for (const disk of s.disks || []) {
    if (!disk.total) continue;
    const pct = (disk.used / disk.total) * 100;
    const model = squash(disk.model);
    for (const [owner, d] of drives) {
      const name = squash(owner);
      const sameModel = model.length >= 4 && (name.includes(model) || model.includes(name));
      const gap = Math.abs(d.used - pct);
      // A model match still has to agree on usage roughly: one physical drive
      // can carry several volumes with different fill levels, and LHM's
      // figure is for the whole drive.
      if (sameModel ? gap < 25 : gap < 1.5) candidates.push({ disk: disk.name, owner, score: sameModel ? gap - 100 : gap, d });
    }
  }
  candidates.sort((a, b) => a.score - b.score);

  const taken = new Set();
  for (const c of candidates) {
    if (out.has(c.disk) || taken.has(c.owner)) continue;
    taken.add(c.owner);
    out.set(c.disk, { model: c.owner, temp: c.d.temp });
  }
  return out;
}

function shortCpuName(name) {
  if (!name) return null;
  return name
    .replace(/\s*\d+th Gen\s*/i, "")
    .replace(/\(R\)|\(TM\)/g, "")
    .replace(/^Intel\s+Core\s+/i, "")
    .replace(/^AMD\s+/i, "")
    .replace(/\s+/g, " ")
    .trim();
}

function shortGpuName(name) {
  if (!name) return null;
  return name
    .replace(/^NVIDIA\s+/i, "")
    .replace(/^GeForce\s+/i, "")
    .replace(/^AMD\s+/i, "")
    .replace(/\s+Graphics$/i, "");
}

// --- markup ------------------------------------------------------------------

// Rewriting identical markup every two seconds would restart the ring
// transitions and make the card shimmer; only touch what actually changed.
const lastHtml = new Map();
function setHtml(id, html) {
  if (lastHtml.get(id) === html) return;
  lastHtml.set(id, html);
  el(id).innerHTML = html;
}

function stat(label, value, cls = "") {
  return `<div class="stat ${cls}"><span>${label}</span><b>${value}</b></div>`;
}

function tempStat(label, reading) {
  if (!reading) return "";
  const hot = tempColor(reading.value);
  return stat(label, `${reading.value.toFixed(0)}°`, hot ? "hot" : "");
}

function fanValue(f) {
  return f.value > 0 ? `${f.value.toFixed(0)} rpm` : "停转";
}

/// One column: the ring, the name, then everything that belongs to it.
function group(name, pct, stats) {
  const known = Number.isFinite(pct);
  const clamped = known ? Math.max(0, Math.min(100, pct)) : 0;
  const offset = RING * (1 - clamped / 100);
  return `<div class="group">
    <div class="dial">
      <svg viewBox="0 0 100 100">
        <circle class="track" cx="50" cy="50" r="40" />
        ${known ? `<circle class="fill" cx="50" cy="50" r="40" stroke="${loadColor(clamped)}"
            stroke-dasharray="${RING.toFixed(2)}" stroke-dashoffset="${offset.toFixed(2)}"
            ${clamped < 1 ? 'stroke-linecap="butt"' : ""} />` : ""}
      </svg>
      <div class="pct">${known ? Math.round(clamped) : "--"}<small>%</small></div>
    </div>
    <div class="name">${name}</div>
    <div class="stats">${stats.filter(Boolean).join("")}</div>
  </div>`;
}

// Small glyphs in front of each drive: solid-state, spinning, or plugged in.
const DISK_ICONS = {
  usb: `<svg viewBox="0 0 16 16"><path d="M5.5 1.5h5v4h-5z" fill="none" stroke="currentColor" stroke-width="1.3"/><path d="M4 5.5h8v6.2a2.8 2.8 0 0 1-2.8 2.8H6.8A2.8 2.8 0 0 1 4 11.7z" fill="currentColor"/><path d="M7 3.2h.01M9 3.2h.01" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/></svg>`,
  hdd: `<svg viewBox="0 0 16 16"><rect x="2" y="1.5" width="12" height="13" rx="2" fill="none" stroke="currentColor" stroke-width="1.3"/><circle cx="8" cy="7" r="3.2" fill="none" stroke="currentColor" stroke-width="1.2"/><circle cx="8" cy="7" r="0.9" fill="currentColor"/><path d="M4.5 12.2h2" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/></svg>`,
  ssd: `<svg viewBox="0 0 16 16"><rect x="1.5" y="4" width="13" height="8" rx="1.6" fill="none" stroke="currentColor" stroke-width="1.3"/><rect x="4" y="6.3" width="3.2" height="3.4" rx="0.5" fill="currentColor"/><rect x="8.6" y="6.3" width="3.2" height="3.4" rx="0.5" fill="currentColor"/></svg>`,
};

// When each drive letter was first seen, so a newly plugged-in drive can fade
// in -- and keep fading in smoothly if the row is rebuilt mid-animation.
const firstSeen = new Map();
let disksSeeded = false;
const FRESH_MS = 1600;

function freshness(disks) {
  const now = performance.now();
  const present = new Set(disks.map((d) => d.name));
  for (const name of firstSeen.keys()) if (!present.has(name)) firstSeen.delete(name);
  for (const name of present) if (!firstSeen.has(name)) firstSeen.set(name, disksSeeded ? now : -Infinity);
  if (disks.length) disksSeeded = true;
  return (name) => now - firstSeen.get(name);
}

const escapeHtml = (text) =>
  String(text).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);

function diskRow(disk, info, withTemp, age) {
  const p = disk.total ? Math.max(0, Math.min(100, (disk.used / disk.total) * 100)) : 0;
  // 0 °C is how some drives (and USB bridges) say "no reading"; show nothing.
  const t = withTemp && info && info.temp && info.temp.value > 0 ? info.temp : null;
  // Spinning drives wear out fast above ~55 °C, long before CPU-style limits.
  const hot = t && (disk.kind === "ssd" ? tempColor(t.value) : t.value >= 55);
  const icon = disk.external ? "usb" : disk.kind === "hdd" ? "hdd" : "ssd";
  const title = [disk.label, (info && info.model) || disk.model, disk.external ? "外置" : ""].filter(Boolean).join(" · ");
  const fresh = age < FRESH_MS ? ` fresh" style="animation-delay:-${Math.round(age)}ms` : "";
  return `<div class="disk${disk.external ? " external" : ""}${fresh}" title="${escapeHtml(title)}">
    <span class="i">${DISK_ICONS[icon]}</span>
    <span class="n">${escapeHtml(disk.name || "磁盘")}</span>
    <span class="track"><span style="width:${p.toFixed(1)}%;background:${loadColor(p)}"></span></span>
    <span class="v"><b>${bytes(disk.used)}</b> / ${bytes(disk.total)}</span>
    <span class="t ${hot ? "hot" : ""}">${t ? `${t.value.toFixed(0)}°` : ""}</span>
  </div>`;
}

/// Sizes that can be tiny (a recycle bin with one file in it): GB like the
/// drives once it is that big, MB / KB below.
function smallBytes(value) {
  if (!value) return "0";
  if (value >= 1024 ** 3) return bytes(value);
  const mb = value / 1024 ** 2;
  if (mb >= 100) return `${mb.toFixed(0)} MB`;
  if (mb >= 1) return `${mb.toFixed(1)} MB`;
  return `${Math.max(1, Math.round(value / 1024))} KB`;
}

const TRASH_ICON = `<svg viewBox="0 0 16 16"><path d="M2.5 4h11M6.2 4V2.6h3.6V4" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"/><path d="M3.8 4l.7 9.2c.05.7.6 1.3 1.3 1.3h4.4c.7 0 1.25-.6 1.3-1.3l.7-9.2" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round"/><path d="M6.6 6.8v5M9.4 6.8v5" stroke="currentColor" stroke-width="1.1" stroke-linecap="round"/></svg>`;

/// The Recycle Bin, on the drives' grid so its bar lines up with theirs:
/// size against the limit set in its Properties (all drives added up).
function recycleRow(bin) {
  const cap = bin.capacity || 0;
  const p = cap ? Math.max(0, Math.min(100, (bin.size / cap) * 100)) : 0;
  const perDrive = (bin.drives || [])
    .map((d) => `${d.name} ${smallBytes(d.size)}${d.capacity ? ` / ${bytes(d.capacity)}` : ""} · ${d.items} 项`);
  const title = [`回收站:${bin.items} 个项目`, ...perDrive, bin.approx ? "上限未设置的盘按 Windows 默认 5% 估算" : ""]
    .filter(Boolean)
    .join("\n");
  const value = bin.items === 0
    ? `<span class="empty">空</span>${cap ? ` / ${bin.approx ? "约 " : ""}${bytes(cap)}` : ""}`
    : `<b>${smallBytes(bin.size)}</b>${cap ? ` / ${bin.approx ? "约 " : ""}${bytes(cap)}` : ""}`;
  return `<div class="disk recycle${bin.items === 0 ? " is-empty" : ""}" title="${escapeHtml(title)}">
    <span class="i">${TRASH_ICON}</span>
    <span class="nt"><span class="n">回收站</span><span class="track"><span style="width:${p.toFixed(1)}%;background:${loadColor(p)}"></span></span></span>
    <span class="v">${value}</span>
    <span class="t"></span>
  </div>`;
}

// --- rendering ---------------------------------------------------------------

function render(s) {
  if (!s) return;
  lastSnapshot = s;

  const cpuName = shortCpuName(s.cpu_name);
  const gpuName = shortGpuName(s.gpu_name);
  el("machine").textContent = [cpuName, isOn("perf.gpu") ? gpuName : null].filter(Boolean).join(" · ") || "本机";
  el("uptime").textContent = duration(s.uptime || 0);

  const temps = isOn("perf.temps");
  const fansOn = isOn("perf.fans");
  const gpu = gpuState(s);
  const cpu = cpuState(s);
  const boardFans = mainboardFans(s);

  // One column per piece of hardware, with its own readings underneath.
  const groups = [];
  if (isOn("perf.cpu")) {
    groups.push(group("CPU", s.cpu, [
      temps ? tempStat("温度", cpu.temp) : "",
      cpu.power ? stat("功耗", `${cpu.power.value.toFixed(0)} W`) : "",
      cpu.clock ? stat("频率", `${(cpu.clock.value / 1000).toFixed(1)} GHz`) : "",
      ...(fansOn
        ? boardFans.cpu.map((f) =>
            stat(boardFans.cpu.length > 1 ? `风扇 ${f.name.replace(/^Fan\s*#?/i, "#")}` : "风扇", fanValue(f)))
        : []),
    ]));
  }
  if (isOn("perf.gpu")) {
    groups.push(group("显卡", gpu.load ? gpu.load.value : NaN, [
      temps ? tempStat("温度", gpu.temp) : "",
      gpu.used && gpu.total ? stat("显存", `${(gpu.used.value / 1024).toFixed(1)}/${(gpu.total.value / 1024).toFixed(0)} GB`) : "",
      gpu.power ? stat("功耗", `${gpu.power.value.toFixed(0)} W`) : "",
      ...(fansOn ? gpu.fans.slice(0, 2).map((f) => stat("风扇", fanValue(f), f.value > 0 ? "" : "idle")) : []),
    ]));
  }
  if (isOn("perf.memory")) {
    const pct = s.mem_total ? (s.mem_used / s.mem_total) * 100 : NaN;
    groups.push(group("内存", pct, [
      stat("已用", bytes(s.mem_used)),
      stat("总计", bytes(s.mem_total)),
      s.swap_total ? stat("虚拟", bytes(s.swap_used)) : "",
    ]));
  }
  el("rings").hidden = groups.length === 0;
  el("rings").style.gridTemplateColumns = `repeat(${Math.max(1, groups.length)}, 1fr)`;
  setHtml("rings", groups.join(""));

  // Disks, each with its own drive temperature at the end of the row.
  const matched = diskTemps(s);
  const age = freshness(s.disks || []);
  const disks = isOn("perf.disks")
    ? (s.disks || []).map((d) => diskRow(d, matched.get(d.name), temps, age(d.name)))
    : [];
  if (isOn("perf.recycle") && s.recycle) disks.push(recycleRow(s.recycle));
  el("disks").hidden = disks.length === 0;
  setHtml("disks", disks.join(""));

  // Whatever is left over: case fans that are actually spinning.
  const lines = [];
  if (fansOn && boardFans.chassis.length) {
    lines.push(`<div class="line"><span>机箱风扇</span><span class="items">${boardFans.chassis
      .map((f) => `<span class="item"><span>${f.name.replace(/^Fan\s*#?/i, "#")}</span><b>${f.value.toFixed(0)}</b></span>`)
      .join("")}</span></div>`);
  }
  el("sensors").hidden = lines.length === 0;
  setHtml("sensors", lines.join(""));

  // Only worth a word while something that needs the sensors is switched on.
  const needsSensors = isOn("perf.gpu") || isOn("perf.temps") || isOn("perf.fans");
  el("note").textContent = !s.lhm && needsSensors
    ? s.lhm_error || "暂时读不到温度、风扇和显卡（设置 → 硬件监控）"
    : "";

  refitIfLayoutChanged();
}

// --- fitting -----------------------------------------------------------------

/// Every extra drive adds a row. The rings give up their spare height first;
/// past that the whole card scales down a little instead of cutting off the
/// bottom rows. Done synchronously, before paint, so nothing flickers.
let fitKey = "";

function refit() {
  const card = el("card");
  if (!window.__widgetFit || !card.clientHeight) return;
  let fit = 1;
  window.__widgetFit(1);
  for (let i = 0; i < 4; i++) {
    const over = card.scrollHeight / card.clientHeight;
    if (over <= 1.002) break;
    fit = Math.max(0.5, (fit / over) * 0.99);
    window.__widgetFit(fit);
  }
}

function refitIfLayoutChanged() {
  const key = ["rings", "disks", "sensors"]
    .map((id) => `${el(id).hidden ? 0 : el(id).children.length}`)
    .concat(el("note").textContent ? "n" : "")
    .join("|");
  if (key === fitKey) return;
  fitKey = key;
  refit();
}

new ResizeObserver(() => refit()).observe(el("card"));

// --- wiring ------------------------------------------------------------------

listen("sysmon:update", (event) => {
  if (event.payload) learnFans(event.payload);
  render(event.payload);
});
listen("fan-roles", (event) => {
  fanRoles = event.payload || {};
  lastHtml.clear();
  render(lastSnapshot);
});
listen("features:changed", (event) => {
  features = event.payload || {};
  lastHtml.clear();
  render(lastSnapshot);
});

(async () => {
  const settings = await invoke("get_settings").catch(() => null);
  features = (settings && settings.features) || {};
  fanRoles = (settings && settings.fan_roles) || {};
  render(await invoke("get_sysmon").catch(() => null));
})();
