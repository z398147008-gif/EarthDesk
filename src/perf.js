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
/// by drive letter. Each LHM drive reports the "Used Space" percentage of its
/// volumes, which is a fingerprint good enough to pair the two.
function diskTemps(s) {
  const drives = new Map();
  for (const r of s.loads || []) {
    if (r.source === "storage" && /used space/i.test(r.name)) drives.set(r.owner, { used: r.value });
  }
  for (const [owner, d] of drives) {
    const temps = (s.temps || []).filter((t) => t.owner === owner && !/warning|critical|limit|threshold/i.test(t.name));
    d.temp = temps.find((t) => /composite/i.test(t.name)) || temps.find((t) => /^temperature$/i.test(t.name)) || temps[0] || null;
  }
  const out = new Map();
  const taken = new Set();
  for (const disk of s.disks || []) {
    if (!disk.total) continue;
    const pct = (disk.used / disk.total) * 100;
    let best = null;
    for (const [owner, d] of drives) {
      if (taken.has(owner)) continue;
      const gap = Math.abs(d.used - pct);
      if (gap < 1.5 && (!best || gap < best.gap)) best = { owner, gap, d };
    }
    if (best) {
      taken.add(best.owner);
      out.set(disk.name, { model: best.owner, temp: best.d.temp });
    }
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

function diskRow(disk, info, withTemp) {
  const p = disk.total ? Math.max(0, Math.min(100, (disk.used / disk.total) * 100)) : 0;
  const t = withTemp && info && info.temp ? info.temp : null;
  const hot = t && tempColor(t.value);
  return `<div class="disk" title="${info ? info.model : ""}">
    <span class="n">${disk.name || "磁盘"}</span>
    <span class="track"><span style="width:${p.toFixed(1)}%;background:${loadColor(p)}"></span></span>
    <span class="v"><b>${bytes(disk.used)}</b> / ${bytes(disk.total)}</span>
    <span class="t ${hot ? "hot" : ""}">${t ? `${t.value.toFixed(0)}°` : ""}</span>
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
  const disks = isOn("perf.disks")
    ? (s.disks || []).map((d) => diskRow(d, matched.get(d.name), temps))
    : [];
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
    ? s.lhm_error || "温度、风扇和显卡需要 LibreHardwareMonitor"
    : "";
}

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
