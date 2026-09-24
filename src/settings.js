const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

/// What the page offers. A piece has one master switch; its parts (if any)
/// are indented under it and only mean something while the piece is on.
/// Keys match the `features` table in config.json.
const PIECES = [
  {
    key: "wallpaper",
    title: "地球壁纸",
    desc: "实时光照的地球壁纸,含星空、日月与当天云图",
    parts: [],
  },
  {
    key: "weather",
    title: "天气组件",
    desc: "当前天气、逐时与每日预报",
    parts: [
      { key: "weather.hourly", title: "逐时预报", desc: "从现在开始的几个小时,含日出日落" },
      { key: "weather.daily", title: "每日预报", desc: "未来几天的温度区间" },
    ],
  },
  {
    key: "perf",
    title: "性能组件",
    desc: "机器的实时状态",
    parts: [
      { key: "perf.cpu", title: "CPU", desc: "占用、各核心、功耗与频率" },
      { key: "perf.gpu", title: "显卡", desc: "占用、显存、功耗与频率" },
      { key: "perf.memory", title: "内存", desc: "内存与交换空间" },
      { key: "perf.disks", title: "磁盘", desc: "各分区的容量" },
      { key: "perf.recycle", title: "回收站", desc: "回收站里文件的大小,和它的上限(回收站属性里设置)" },
      { key: "perf.temps", title: "温度", desc: "显示在 CPU、显卡和每块硬盘后面" },
      { key: "perf.fans", title: "风扇", desc: "显卡风扇归显卡,其余为机箱风扇;没接的不显示" },
    ],
  },
];

let features = {};
const isOn = (key) => features[key] !== false;

function switchHtml(key) {
  return `<label class="switch"><input type="checkbox" data-key="${key}" ${isOn(key) ? "checked" : ""} /><span class="knob"></span></label>`;
}

function render() {
  const host = document.getElementById("groups");
  host.replaceChildren(
    ...PIECES.map((piece) => {
      const group = document.createElement("section");
      group.className = "group";
      const parentOn = isOn(piece.key);
      const rows = [
        `<div class="row parent">
           <div class="text"><div class="title">${piece.title}</div><div class="desc">${piece.desc}</div></div>
           ${switchHtml(piece.key)}
         </div>`,
        ...piece.parts.map(
          (part) => `<div class="row child ${parentOn ? "" : "disabled"}">
             <div class="text"><div class="title">${part.title}</div><div class="desc">${part.desc}</div></div>
             ${switchHtml(part.key)}
           </div>`
        ),
      ];
      group.innerHTML = `<div class="card">${rows.join("")}</div>`;
      return group;
    })
  );
}

document.addEventListener("change", async (event) => {
  const input = event.target.closest("input[data-key]");
  if (!input) return;
  const key = input.dataset.key;
  const on = input.checked;
  features[key] = on;
  render();
  try {
    await invoke("set_feature", { key, on });
  } catch (e) {
    console.error(e);
    features[key] = !on;
    render();
  }
});

// --- general -----------------------------------------------------------------

function renderZMode(mode) {
  for (const b of document.querySelectorAll("#zmode button")) {
    b.classList.toggle("on", b.dataset.mode === mode);
  }
}

document.getElementById("zmode").addEventListener("click", async (event) => {
  const button = event.target.closest("button[data-mode]");
  if (!button) return;
  renderZMode(button.dataset.mode);
  await invoke("set_z_mode", { mode: button.dataset.mode }).catch(console.error);
});

const scale = document.getElementById("scale");
const scaleOut = document.getElementById("scaleOut");
let scaleTimer = null;
scale.addEventListener("input", () => {
  scaleOut.textContent = `${Number(scale.value).toFixed(2)}×`;
  // Every step would rewrite config.json; wait for the drag to settle.
  clearTimeout(scaleTimer);
  scaleTimer = setTimeout(() => {
    invoke("set_ui_scale", { value: Number(scale.value) }).catch(console.error);
  }, 120);
});

const alpha = document.getElementById("alpha");
const alphaOut = document.getElementById("alphaOut");
let alphaTimer = null;
alpha.addEventListener("input", () => {
  alphaOut.textContent = `${Math.round(Number(alpha.value) * 100)}%`;
  clearTimeout(alphaTimer);
  alphaTimer = setTimeout(() => {
    invoke("set_card_opacity", { value: Number(alpha.value) }).catch(console.error);
  }, 60);
});

const editButton = document.getElementById("edit");
editButton.addEventListener("click", () => invoke("toggle_editing").catch(console.error));
listen("edit-mode", (event) => {
  const on = event.payload === true;
  editButton.classList.toggle("active", on);
  editButton.textContent = on ? "完成" : "编辑模式";
});

document.getElementById("folder").addEventListener("click", () => invoke("open_config_dir").catch(console.error));

// Stay in step if something else (the tray, a hand edit) changes the table.
listen("features:changed", (event) => {
  features = event.payload || {};
  render();
});

// --- location ----------------------------------------------------------------
//
// Three cascading lists over data/cities.json (GeoNames cities of 15 000+
// people, Chinese names for China, Japan, HK, Macau and Taiwan):
//   { tz: [...], countries: { ISO: { n, en, a: { admin1: name }, c: [[name, admin1, lat, lon, tzIndex]] } } }

const countrySel = document.getElementById("country");
const admin1Sel = document.getElementById("admin1");
const citySel = document.getElementById("city");
const locSave = document.getElementById("locSave");
const locNow = document.getElementById("locNow");
const OTHER = "__other";
let geo = null;
let current = null;

// China first, then the rest of Greater China, then everyone else by pinyin.
const PINNED = ["CN", "HK", "MO", "TW"];
const zh = new Intl.Collator("zh-Hans-CN");

function option(value, text) {
  const o = document.createElement("option");
  o.value = value;
  o.textContent = text;
  return o;
}

function fillCountries() {
  const codes = Object.keys(geo.countries);
  const rest = codes.filter((c) => !PINNED.includes(c)).sort((a, b) => zh.compare(geo.countries[a].n, geo.countries[b].n));
  countrySel.replaceChildren(
    ...PINNED.filter((c) => geo.countries[c]).map((c) => option(c, geo.countries[c].n)),
    option("", "──────────"),
    ...rest.map((c) => option(c, geo.countries[c].n))
  );
  countrySel.options[PINNED.length].disabled = true;
}

function citiesOf(country, admin1) {
  const c = geo.countries[country];
  if (!c) return [];
  if (admin1 === OTHER) return c.c.filter((x) => !c.a[x[1]]);
  return c.c.filter((x) => x[1] === admin1);
}

function fillAdmin1(country, want) {
  const c = geo.countries[country];
  const used = new Set(c.c.map((x) => x[1]));
  const codes = Object.keys(c.a).filter((k) => used.has(k));
  // Order provinces by their biggest city, so the likely picks come first.
  const rank = new Map();
  c.c.forEach((x, i) => { if (!rank.has(x[1])) rank.set(x[1], i); });
  codes.sort((a, b) => (country === "CN" ? zh.compare(c.a[a], c.a[b]) : rank.get(a) - rank.get(b)));
  const opts = codes.map((k) => option(k, c.a[k]));
  if (c.c.some((x) => !c.a[x[1]])) opts.push(option(OTHER, codes.length ? "其他" : c.n));
  admin1Sel.replaceChildren(...opts);
  admin1Sel.disabled = opts.length <= 1;
  admin1Sel.value = want && [...admin1Sel.options].some((o) => o.value === want) ? want : admin1Sel.options[0]?.value;
}

function fillCities(want) {
  const list = citiesOf(countrySel.value, admin1Sel.value);
  citySel.replaceChildren(...list.map((x, i) => option(String(i), x[0])));
  let idx = 0;
  if (want) {
    const found = list.findIndex((x) => x[0] === want.name || (Math.abs(x[2] - want.lat) < 0.02 && Math.abs(x[3] - want.lon) < 0.02));
    if (found >= 0) idx = found;
  }
  citySel.value = String(idx);
  updateSave();
}

function picked() {
  const list = citiesOf(countrySel.value, admin1Sel.value);
  const x = list[Number(citySel.value)];
  if (!x) return null;
  return {
    lat: x[2],
    lon: x[3],
    label: x[0],
    timezone: geo.tz[x[4]] || "auto",
    country: countrySel.value,
    admin1: admin1Sel.value === OTHER ? "" : admin1Sel.value,
  };
}

function updateSave() {
  const p = picked();
  const same = p && current && Math.abs(p.lat - current.lat) < 1e-4 && Math.abs(p.lon - current.lon) < 1e-4;
  locSave.disabled = !p || same;
  locSave.textContent = same ? "已保存" : "保存";
}

function describe(loc) {
  if (!loc) return "—";
  const parts = [];
  const c = geo?.countries[loc.country];
  if (c) {
    parts.push(c.n);
    const a = c.a[loc.admin1];
    if (a && a !== loc.label && !a.startsWith(loc.label)) parts.push(a);
  }
  parts.push(loc.label);
  return `${parts.join(" · ")}（${loc.lat.toFixed(2)}°, ${loc.lon.toFixed(2)}°）`;
}

// Older configs only have coordinates: find the nearest listed city.
function guessPlace(loc) {
  let best = null;
  for (const [code, c] of Object.entries(geo.countries)) {
    for (const x of c.c) {
      const d = (x[2] - loc.lat) ** 2 + ((x[3] - loc.lon) * Math.cos((loc.lat * Math.PI) / 180)) ** 2;
      if (!best || d < best.d) best = { d, code, admin1: c.a[x[1]] ? x[1] : OTHER };
    }
  }
  return best;
}

function showLocation(loc) {
  current = loc;
  locNow.textContent = describe(loc);
  if (!geo || !loc) return;
  let country = loc.country;
  let admin1 = loc.admin1 || OTHER;
  if (!geo.countries[country]) {
    const g = guessPlace(loc);
    country = g.code;
    admin1 = g.admin1;
  }
  countrySel.value = country;
  fillAdmin1(country, admin1);
  fillCities({ name: loc.label, lat: loc.lat, lon: loc.lon });
}

countrySel.addEventListener("change", () => {
  fillAdmin1(countrySel.value);
  fillCities();
});
admin1Sel.addEventListener("change", () => fillCities());
citySel.addEventListener("change", updateSave);

locSave.addEventListener("click", async () => {
  const p = picked();
  if (!p) return;
  locSave.disabled = true;
  try {
    await invoke("set_location", { location: p });
    showLocation(p);
  } catch (e) {
    console.error(e);
    locSave.textContent = "保存失败";
    locSave.disabled = false;
  }
});

listen("location:changed", (event) => event.payload && showLocation(event.payload));

async function loadGeo() {
  try {
    geo = await (await fetch("data/cities.json")).json();
  } catch (e) {
    console.error("city list unavailable", e);
    for (const s of [countrySel, admin1Sel, citySel]) s.disabled = true;
    return;
  }
  fillCountries();
}

// --- hardware monitor -------------------------------------------------------
//
// Everything here runs by itself (see SensorNurse in main.rs). The page only
// reports, and offers the one thing that needs the user: a repair, which
// asks Windows for administrator rights.

const hwBadge = document.getElementById("hwBadge");
const hwDesc = document.getElementById("hwDesc");
const hwRepair = document.getElementById("hwRepair");
const hwFoundRow = document.getElementById("hwFoundRow");
const hwFound = document.getElementById("hwFound");
const hwNoteRow = document.getElementById("hwNoteRow");
const hwNote = document.getElementById("hwNote");
let repairing = false;

const HW_NAMES = {
  Cpu: "处理器",
  GpuNvidia: "显卡",
  GpuAmd: "显卡",
  GpuIntel: "显卡",
  Motherboard: "主板",
  Memory: "内存",
  Storage: "硬盘",
  Cooler: "散热器",
  EmbeddedController: "EC",
  Psu: "电源",
};

/// ["Cpu: Intel Core i7-13700KF", "Storage: WD ...", ...] -> chips, one per
/// kind, "硬盘 ×2", the model names in the tooltip.
function renderHardware(list) {
  const groups = new Map();
  for (const entry of Array.isArray(list) ? list : []) {
    const at = entry.indexOf(": ");
    const kind = at > 0 ? entry.slice(0, at) : entry;
    const name = at > 0 ? entry.slice(at + 2) : "";
    const label = HW_NAMES[kind] || kind;
    if (!groups.has(label)) groups.set(label, []);
    groups.get(label).push(name);
  }
  hwFound.replaceChildren();
  for (const [label, names] of groups) {
    const chip = document.createElement("span");
    chip.className = "chip";
    // Several drives or graphics cards are worth counting; LibreHardwareMonitor's
    // "Generic Memory" + "Virtual Memory" are not two things to the user.
    const countable = label === "硬盘" || label === "显卡";
    chip.textContent = countable && names.length > 1 ? `${label} ×${names.length}` : label;
    chip.title = names.join("\n");
    hwFound.append(chip);
  }
  hwFoundRow.hidden = groups.size === 0;
  return groups;
}

function setBadge(text, ok) {
  hwBadge.textContent = text;
  hwBadge.classList.toggle("ok", ok === true);
  hwBadge.classList.toggle("bad", ok === false);
}

function note(text) {
  hwNote.textContent = text || "";
  hwNoteRow.hidden = !text;
}

async function refreshHw() {
  const st = await invoke("sensors_status").catch(() => null);
  if (!st || repairing) return st;
  const groups = renderHardware(st.source === "service" ? st.hardware : []);
  let needRepair = false;
  note("");

  if (!st.dotnet) {
    setBadge("缺少组件", false);
    hwDesc.innerHTML = "";
    hwDesc.append("这台电脑没有 .NET Framework 4.8，硬件监控无法运行。请先");
    const a = document.createElement("a");
    a.href = "#";
    a.dataset.url = "https://dotnet.microsoft.com/zh-cn/download/dotnet-framework/net48";
    a.textContent = "从微软官网下载安装";
    hwDesc.append(a, "，装好后回到这里点“修复”。");
    needRepair = st.can_repair;
  } else if (st.reading && st.source === "service") {
    setBadge("正常", true);
    hwDesc.textContent = "内置的硬件监控服务在后台运行，没有窗口，也不占托盘。";
    if (!st.pawnio) {
      note("读取主板温度、风扇和 CPU 温度用的驱动（PawnIO）没有装好，这几项会缺。点“修复”重新安装。");
      needRepair = st.can_repair;
    } else if (!groups.has("显卡")) {
      note("没有识别到显卡。刚开机时显卡驱动可能还没就绪，服务会在一分半钟后再找一次。");
    }
  } else if (st.reading && st.source === "lhm") {
    setBadge("正常", true);
    hwDesc.textContent = "内置服务没有响应，暂时在用你自己运行的 LibreHardwareMonitor。点“修复”可以换回内置服务。";
    needRepair = st.can_repair;
  } else if (st.alive) {
    setBadge("启动中…", null);
    hwDesc.textContent = "硬件监控服务在运行，正在识别硬件。通常几秒钟；有硬盘响应很慢时会久一些。";
  } else {
    switch (st.service) {
      case "not_installed":
        setBadge("未安装", false);
        hwDesc.textContent = "硬件监控服务还没有安装。点“修复”，在 Windows 弹窗里点“是”。";
        needRepair = true;
        break;
      case "starting":
        setBadge("启动中…", null);
        hwDesc.textContent = "硬件监控服务正在启动，通常几秒钟。";
        break;
      case "stopped":
      case "stopping":
        setBadge("已停止", false);
        hwDesc.textContent = "服务停止了，地球桌面正在把它重新启动。一直停着的话请点“修复”。";
        needRepair = true;
        break;
      default:
        setBadge("没有响应", false);
        hwDesc.textContent = st.error
          ? `${st.error}。地球桌面会自动重启它；一直这样的话请点“修复”。`
          : "硬件监控服务没有响应。地球桌面会自动重启它；一直这样的话请点“修复”。";
        needRepair = true;
    }
    if (!st.can_repair) {
      needRepair = false;
      note("没有找到内置的硬件监控组件，请重新安装地球桌面。");
    }
  }
  hwRepair.hidden = !needRepair;
  return st;
}

hwRepair.addEventListener("click", async () => {
  repairing = true;
  hwRepair.disabled = true;
  hwRepair.textContent = "修复中…";
  setBadge("修复中…", null);
  hwDesc.textContent = "请在 Windows 弹出的授权窗口里点“是”。安装驱动和服务需要十几秒。";
  note("");
  let error = null;
  try {
    await invoke("sensors_repair");
  } catch (e) {
    error = String(e);
  }
  repairing = false;
  hwRepair.disabled = false;
  hwRepair.textContent = "修复";
  if (error) {
    await refreshHw();
    note(error);
    return;
  }
  // Give the service a few samples to come up before judging.
  hwDesc.textContent = "已修复，正在等待服务响应…";
  for (let i = 0; i < 10; i++) {
    await new Promise((r) => setTimeout(r, 2000));
    const st = await refreshHw();
    if (st && st.reading && st.source === "service") break;
  }
});

// Keep the status current while the window is open.
setInterval(() => {
  if (!document.hidden && !repairing) refreshHw();
}, 4000);

document.addEventListener("click", (event) => {
  const a = event.target.closest("a[data-url]");
  if (!a) return;
  event.preventDefault();
  invoke("open_url", { url: a.dataset.url }).catch(console.error);
});

// Re-check whenever the window comes back into view.
document.addEventListener("visibilitychange", () => {
  if (!document.hidden) refreshHw();
});

// --- autostart --------------------------------------------------------------

const autostart = document.getElementById("autostart");
autostart.addEventListener("change", async () => {
  const on = autostart.checked;
  try {
    await invoke("set_autostart", { on });
  } catch (e) {
    console.error(e);
    autostart.checked = !on;
  }
});

async function boot() {
  const [settings] = await Promise.all([invoke("get_settings").catch(() => ({})), loadGeo()]);
  showLocation(settings.location || null);
  refreshHw();
  invoke("get_autostart").then((on) => (autostart.checked = on === true)).catch(() => {});
  features = settings.features || {};
  render();
  renderZMode(settings.z_mode || "desktop");
  const s = Number(settings.ui_scale) || 1;
  scale.value = String(s);
  scaleOut.textContent = `${s.toFixed(2)}×`;
  const a = Number.isFinite(Number(settings.card_opacity)) ? Number(settings.card_opacity) : 0.85;
  alpha.value = String(a);
  alphaOut.textContent = `${Math.round(a * 100)}%`;
  const editing = await invoke("is_editing").catch(() => false);
  editButton.classList.toggle("active", editing === true);
  editButton.textContent = editing ? "完成" : "编辑模式";
}

boot();
