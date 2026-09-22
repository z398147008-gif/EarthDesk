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

const lhmBadge = document.getElementById("lhmBadge");
const lhmDesc = document.getElementById("lhmDesc");
const lhmRepair = document.getElementById("lhmRepair");
let lhmTimer = null;

async function refreshLhm() {
  const st = await invoke("lhm_status").catch(() => null);
  if (!st) return null;
  lhmBadge.classList.toggle("ok", st.running);
  lhmBadge.classList.toggle("bad", !st.running);
  if (st.running) {
    lhmBadge.textContent = "运行中";
    lhmDesc.textContent = "由内置的 LibreHardwareMonitor 提供，开机自动在后台运行。";
  } else if (st.bundled) {
    lhmBadge.textContent = "未运行";
    lhmDesc.textContent = "点“一键修复”，在 Windows 弹窗里点“是”。详见下方说明。";
  } else {
    lhmBadge.textContent = "未安装";
    lhmDesc.textContent = "这个版本没有内置硬件监控；可自行运行 LibreHardwareMonitor 并开启 8085 端口的 Web 服务器。";
  }
  lhmRepair.hidden = st.running || !st.bundled;
  return st;
}

lhmRepair.addEventListener("click", async () => {
  lhmRepair.disabled = true;
  lhmRepair.textContent = "修复中…";
  try {
    await invoke("lhm_repair");
  } catch (e) {
    lhmDesc.textContent = String(e);
  }
  // Poll for up to a minute while the prompt is answered and it starts.
  clearInterval(lhmTimer);
  let tries = 0;
  lhmTimer = setInterval(async () => {
    const st = await refreshLhm();
    if ((st && st.running) || ++tries > 20) {
      clearInterval(lhmTimer);
      lhmRepair.disabled = false;
      lhmRepair.textContent = "一键修复";
    }
  }, 3000);
});

document.addEventListener("click", (event) => {
  const a = event.target.closest("a[data-url]");
  if (!a) return;
  event.preventDefault();
  invoke("open_url", { url: a.dataset.url }).catch(console.error);
});

// Re-check whenever the window comes back into view.
document.addEventListener("visibilitychange", () => {
  if (!document.hidden) refreshLhm();
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
  refreshLhm();
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
