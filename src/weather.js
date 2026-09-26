import { describe, iconUrl, cardTone, SUNRISE_ICON, SUNSET_ICON } from "./wmo.js";
import { createSky } from "./wxsky.js";

// Animated sky behind the card; null without WebGL2 (plain tint instead).
const sky = createSky(document.getElementById("sky"));
let skyPainted = false;

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const HOUR_SLOTS = 6;
const DAY_ROWS = 5;
const DOW = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"];

const el = (id) => document.getElementById(id);
let latest = null;
let failedSince = null;
let features = {};
const isOn = (key) => features[key] !== false;

// Write markup only when it differs from what is already there. The card
// re-renders every 30 seconds to keep "now" current; rebuilding the icon
// <img> elements each time blanks them for a frame while they re-decode,
// which reads as the widget flickering.
const lastHtml = new Map();
function setHtml(id, html) {
  if (lastHtml.get(id) === html) return;
  lastHtml.set(id, html);
  el(id).innerHTML = html;
}

/// Open-Meteo returns local wall-clock strings plus the location's UTC offset.
/// Anchoring on the offset instead of the PC's timezone keeps every time on
/// the card right even if the PC is set to another zone.
function epochOf(isoLocal, offsetSeconds) {
  const withSeconds = isoLocal.length === 16 ? `${isoLocal}:00` : isoLocal;
  return Date.parse(`${withSeconds}Z`) - offsetSeconds * 1000;
}

function partsIn(timezone, epoch) {
  const f = new Intl.DateTimeFormat("en-GB", {
    timeZone: timezone, hour: "2-digit", minute: "2-digit", weekday: "short", hour12: false,
  });
  const parts = Object.fromEntries(f.formatToParts(new Date(epoch)).map((p) => [p.type, p.value]));
  const idx = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"].indexOf(parts.weekday);
  return { hour: Number(parts.hour) % 24, minute: Number(parts.minute), dow: idx };
}

const round = (v) => (Number.isFinite(v) ? Math.round(v) : null);

/// Between that day's sunrise and sunset.
function isDaylight(t, sunrises, sunsets) {
  for (let i = 0; i < sunrises.length; i += 1) {
    if (t < sunrises[i]) return false;
    if (sunsets[i] !== undefined && t < sunsets[i]) return true;
  }
  return false;
}

// --- temperature colour scale ---------------------------------------------
// The same idea as the system widget: cold is blue, mild is green, warm is
// yellow, hot is orange-red, and a day's bar is a gradient across its range.
const STOPS = [
  [-10, [94, 92, 230]],
  [0, [100, 210, 255]],
  [10, [48, 209, 88]],
  [18, [255, 214, 10]],
  [25, [255, 159, 10]],
  [33, [255, 69, 58]],
];

function tempColor(t) {
  if (t <= STOPS[0][0]) return `rgb(${STOPS[0][1].join(",")})`;
  for (let i = 1; i < STOPS.length; i += 1) {
    const [t1, c1] = STOPS[i];
    if (t <= t1) {
      const [t0, c0] = STOPS[i - 1];
      const k = (t - t0) / (t1 - t0);
      return `rgb(${c0.map((v, j) => Math.round(v + (c1[j] - v) * k)).join(",")})`;
    }
  }
  return `rgb(${STOPS[STOPS.length - 1][1].join(",")})`;
}

// --- rendering ---------------------------------------------------------------

function renderHeader(data, now) {
  const c = data.current || {};
  const isDay = c.is_day !== 0;
  el("temp").textContent = round(c.temperature_2m) ?? "--";
  el("cond").textContent = describe(c.weather_code).label;
  const src = iconUrl(c.weather_code, isDay);
  if (el("icon").getAttribute("src") !== src) el("icon").src = src;
  el("hi").textContent = round(data.daily?.temperature_2m_max?.[0]) ?? "--";
  el("lo").textContent = round(data.daily?.temperature_2m_min?.[0]) ?? "--";
  const cls = `card tone-${cardTone(c.weather_code, isDay)}`;
  if (el("card").className !== cls) el("card").className = cls;
  if (sky) {
    sky.set(c.weather_code, isDay);
    if (!skyPainted) {
      sky.snap();
      skyPainted = true;
    }
  }
}

/// "Now", then the following hours -- with sunrise and sunset slotted in at
/// their real minute when they fall inside the strip, the way the system
/// widget does it. The strip always holds exactly HOUR_SLOTS entries.
function renderHours(data, now) {
  const tz = data.timezone;
  const offset = data.utc_offset_seconds ?? 0;
  const times = (data.hourly?.time || []).map((t) => epochOf(t, offset));
  const temps = data.hourly?.temperature_2m || [];
  const codes = data.hourly?.weather_code || [];
  const pops = data.hourly?.precipitation_probability || [];
  const sunrises = (data.daily?.sunrise || []).map((t) => epochOf(t, offset));
  const sunsets = (data.daily?.sunset || []).map((t) => epochOf(t, offset));
  const current = data.current || {};

  const entries = [{
    kind: "now",
    t: now,
    label: "现在",
    icon: iconUrl(current.weather_code, current.is_day !== 0),
    pop: pops[Math.max(0, times.findIndex((t) => t > now) - 1)],
    value: `${round(current.temperature_2m) ?? "--"}°`,
  }];

  const startHour = times.findIndex((t) => t > now);
  const windowEnd = now + HOUR_SLOTS * 3600000;
  for (let i = Math.max(0, startHour); i < times.length && times[i] <= windowEnd; i += 1) {
    const t = times[i];
    entries.push({
      kind: "hour",
      t,
      label: `${partsIn(tz, t).hour}时`,
      icon: iconUrl(codes[i], isDaylight(t, sunrises, sunsets)),
      value: `${round(temps[i]) ?? "--"}°`,
      pop: pops[i],
    });
  }

  const events = [];
  sunrises.forEach((t) => events.push({ t, icon: SUNRISE_ICON, word: "日出" }));
  sunsets.forEach((t) => events.push({ t, icon: SUNSET_ICON, word: "日落" }));
  for (const e of events) {
    if (e.t > now && e.t < windowEnd) {
      const p = partsIn(tz, e.t);
      entries.push({
        kind: "event",
        t: e.t,
        label: `${p.hour}:${String(p.minute).padStart(2, "0")}`,
        icon: e.icon,
        value: e.word,
      });
    }
  }

  entries.sort((a, b) => a.t - b.t);
  const shown = entries.slice(0, HOUR_SLOTS);

  setHtml("hours", shown.map((e) =>
    `<div class="hour ${e.kind === "now" ? "now" : ""} ${e.kind === "event" ? "event" : ""}">` +
    `<span class="t">${e.label}</span><span class="icon-cell"><img src="${e.icon}" alt="" />` +
    (Number.isFinite(e.pop) && e.pop >= 10 ? `<span class="pop">${e.pop}%</span>` : "") +
    `</span><span class="v">${e.value}</span></div>`
  ).join(""));
}

/// One row per day starting tomorrow. Every bar is drawn against the same
/// scale -- the lowest low and highest high across the rows -- so the bars
/// compare with each other at a glance.
function renderDays(data, now) {
  const daily = data.daily;
  if (!daily?.time?.length) return;
  const offset = data.utc_offset_seconds ?? 0;
  const rows = [];
  for (let i = 1; i < daily.time.length && rows.length < DAY_ROWS; i += 1) {
    rows.push({
      epoch: epochOf(`${daily.time[i]}T12:00`, offset),
      code: daily.weather_code?.[i],
      lo: daily.temperature_2m_min?.[i],
      hi: daily.temperature_2m_max?.[i],
      pop: daily.precipitation_probability_max?.[i],
    });
  }
  const min = Math.min(...rows.map((r) => r.lo));
  const max = Math.max(...rows.map((r) => r.hi));
  const span = Math.max(1, max - min);

  setHtml("days", rows.map((r) => {
    const left = ((r.lo - min) / span) * 100;
    const width = Math.max(4, ((r.hi - r.lo) / span) * 100);
    // Every day's chance of rain (JMA's official figure in Japan); faint
    // below 30%.
    const pop = Number.isFinite(r.pop) ? `<span class="pop${r.pop < 30 ? " low" : ""}">${r.pop}%</span>` : "";
    return `<div class="day">
      <span class="dow">${DOW[partsIn(data.timezone, r.epoch).dow]}</span>
      <span class="icon-cell"><img src="${iconUrl(r.code, true)}" alt="" />${pop}</span>
      <span class="lo">${round(r.lo)}°</span>
      <span class="range"><span class="fill" style="left:${left}%;width:${width}%;background:linear-gradient(90deg, ${tempColor(r.lo)}, ${tempColor(r.hi)})"></span></span>
      <span class="hi">${round(r.hi)}°</span></div>`;
  }).join(""));
}

function render() {
  el("hours").hidden = !isOn("weather.hourly");
  el("days").hidden = !isOn("weather.daily");
  if (!latest) return;
  const now = Date.now();
  renderHeader(latest, now);
  renderHours(latest, now);
  renderDays(latest, now);
  document.body.classList.toggle("stale", Boolean(failedSince));
  el("status").textContent = failedSince ? "天气数据暂未更新" : "";
}

listen("features:changed", (event) => {
  features = event.payload || {};
  render();
});

async function boot() {
  const settings = await invoke("get_settings").catch(() => null);
  features = (settings && settings.features) || {};
  const location = await invoke("get_location").catch(() => null);
  if (location) el("place").textContent = location.label.replace(/^大阪市/, "");
  listen("location:changed", (event) => {
    if (event.payload) el("place").textContent = event.payload.label;
  });
  const cached = await invoke("get_weather").catch(() => null);
  if (cached) {
    latest = cached;
    render();
  }
}

listen("weather:update", (event) => {
  latest = event.payload;
  failedSince = null;
  render();
});

listen("weather:error", () => {
  if (latest && !failedSince) failedSince = Date.now();
  render();
});

// The "now" column and the hour labels have to roll over on their own
// between the ten-minute refreshes.
setInterval(render, 30000);

boot();
