// 截图界面：冻结的桌面 + 选区 + 放大镜 + 标注。交互照 Snipaste。
//
// Units: every coordinate here is a *physical* pixel relative to the
// window's top-left (= the virtual desktop's origin). The DOM is in CSS
// pixels, so anything shown goes through css() (÷ devicePixelRatio). The
// base canvas holds the frozen screen at full resolution; annotations are
// vectors (`shapes`) re-rendered into the ann canvas whenever they change.

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (s) => document.querySelector(s);

const base = $("#base");
const ann = $("#ann");
const bctx = base.getContext("2d", { willReadFrequently: false });
const actx = ann.getContext("2d");
const selEl = $("#sel");
const hlEl = $("#hl");
const sizeEl = $("#size");
const magEl = $("#mag");
const magCanvas = magEl.querySelector("canvas");
const magCtx = magCanvas.getContext("2d");
const bar = $("#bar");
const sub = $("#sub");
const editEl = $("#edit");
const hintEl = $("#hint");

let S = null; // the session payload from Rust
let W = 0, H = 0, OX = 0, OY = 0;
let dpr = 1;
let pix = null; // RGBA of the frozen screen
let monitors = [];
let wins = [];
const uia = new Map(); // hwnd -> rects | "pending"
let detectControls = true;

let mode = "idle"; // idle | detect | selecting | selected
let sel = null; // {x, y, w, h}
let hl = null;
let level = 0; // 0 = smallest element at the pointer; wheel walks outwards
let lastCands = [];
let cursor = { x: 0, y: 0 };
let drag = null;
let tool = null;
let shapes = [];
let undoStack = [];
let redoStack = [];
let active = -1; // index of the shape being edited
let editing = null; // { index | -1, x, y }
let histIdx = -1;
let colorFmt = "hex";
let pixC = null, blurC = null;

const css = (v) => `${v / dpr}px`;
const clamp = (v, a, b) => Math.max(a, Math.min(b, v));

// --- styles ------------------------------------------------------------------------

const COLORS = ["#ff3b30", "#ff9500", "#ffcc00", "#34c759", "#0a84ff", "#af52de", "#ff2d92", "#ffffff", "#8e8e93", "#000000"];
const SIZES = [2, 4, 8]; // stroke widths at 100% scaling
const FONT_SIZES = [14, 20, 28, 40];
const styles = {
  rect: { color: COLORS[0], size: 1, fill: false },
  ellipse: { color: COLORS[0], size: 1, fill: false },
  line: { color: COLORS[0], size: 1 },
  arrow: { color: COLORS[0], size: 1 },
  pen: { color: COLORS[0], size: 1 },
  marker: { color: COLORS[2], size: 2 },
  mosaic: { size: 1, kind: "pixel", shape: "brush" },
  text: { color: COLORS[0], size: 1 },
  number: { color: COLORS[0], size: 1 },
  eraser: { size: 1 },
};
try {
  const saved = JSON.parse(localStorage.getItem("capture.styles") || "{}");
  for (const k of Object.keys(styles)) Object.assign(styles[k], saved[k] || {});
} catch {}
function saveStyles() {
  try { localStorage.setItem("capture.styles", JSON.stringify(styles)); } catch {}
}
const strokeW = (s) => SIZES[s.size ?? 1] * dpr;

// --- geometry ----------------------------------------------------------------------

const norm = (a, b) => ({ x: Math.min(a.x, b.x), y: Math.min(a.y, b.y), w: Math.abs(a.x - b.x), h: Math.abs(a.y - b.y) });
const inside = (r, x, y) => r && x >= r.x && y >= r.y && x < r.x + r.w && y < r.y + r.h;
const area = (r) => r.w * r.h;
const same = (a, b) => a && b && a.x === b.x && a.y === b.y && a.w === b.w && a.h === b.h;
function clip(r) {
  const x = clamp(r.x, 0, W), y = clamp(r.y, 0, H);
  return { x, y, w: clamp(r.x + r.w, 0, W) - x, h: clamp(r.y + r.h, 0, H) - y };
}
const rel = (a) => ({ x: a[0] - OX, y: a[1] - OY, w: a[2], h: a[3] });

function monitorAt(x, y) {
  return monitors.find((m) => inside(m, x, y)) || monitors[0] || { x: 0, y: 0, w: W, h: H };
}

function point(e) {
  return { x: clamp(Math.round(e.clientX * dpr), 0, W - 1), y: clamp(Math.round(e.clientY * dpr), 0, H - 1) };
}

// --- element detection -------------------------------------------------------------

function requestUia(w) {
  if (uia.has(w.hwnd)) return;
  uia.set(w.hwnd, "pending");
  const id = S.id;
  invoke("capture_elements", { hwnd: w.hwnd })
    .then((rects) => {
      if (!S || S.id !== id) return;
      uia.set(w.hwnd, rects.map(rel).map(clip).filter((r) => r.w >= 4 && r.h >= 4));
      if (mode === "detect") updateHover();
    })
    .catch(() => uia.set(w.hwnd, []));
}

/// Everything under the point that could be selected with one click, from
/// the whole screen down to the smallest control.
function candidates(x, y) {
  const out = [clip(monitorAt(x, y))];
  if (S?.settings?.detect_elements !== false) {
    for (const w of wins) {
      if (!inside(w.r, x, y)) continue;
      out.push(clip(w.r));
      if (detectControls) {
        for (const c of w.children) if (inside(c, x, y)) out.push(clip(c));
        const u = uia.get(w.hwnd);
        if (Array.isArray(u)) {
          for (const r of u) if (inside(r, x, y)) out.push(r);
        } else if (u === undefined) {
          requestUia(w);
        }
      }
      break;
    }
  }
  const uniq = [];
  for (const r of out) if (r.w > 0 && r.h > 0 && !uniq.some((q) => same(q, r))) uniq.push(r);
  return uniq.sort((a, b) => area(b) - area(a));
}

function updateHover() {
  const c = candidates(cursor.x, cursor.y);
  if (!same(c[c.length - 1], lastCands[lastCands.length - 1])) level = 0;
  lastCands = c;
  hl = c[Math.max(0, c.length - 1 - level)] || null;
  drawChrome();
}

// --- rendering the chrome ------------------------------------------------------------

function place(el, r) {
  el.style.left = css(r.x);
  el.style.top = css(r.y);
  el.style.width = css(r.w);
  el.style.height = css(r.h);
}

function drawChrome() {
  const box = mode === "selecting" || mode === "selected" ? sel : mode === "detect" ? hl : null;
  document.body.classList.toggle("has-box", !!box);
  document.body.classList.toggle("selecting", mode === "selecting");
  document.body.classList.toggle("selected", mode === "selected");
  document.body.classList.toggle("locked", locked());
  hlEl.hidden = mode !== "detect" || !hl;
  if (hl && mode === "detect") place(hlEl, hl);
  selEl.hidden = !(mode === "selecting" || mode === "selected") || !sel;
  if (sel && !selEl.hidden) place(selEl, sel);

  const r = box;
  sizeEl.hidden = !r;
  if (r) {
    sizeEl.textContent = `${r.w} × ${r.h}`;
    const ty = r.y - 24 * dpr >= 0 ? r.y - 24 * dpr : r.y + 4 * dpr;
    sizeEl.style.left = css(clamp(r.x, 0, W - 90 * dpr));
    sizeEl.style.top = css(ty);
  }
  const showBar = mode === "selected" && !drag;
  bar.hidden = !showBar;
  if (showBar) {
    renderBar();
    placeBars();
  } else sub.hidden = true;
  drawMag();
}

function placeBars() {
  const m = monitorAt(sel.x + sel.w / 2, sel.y + sel.h / 2);
  bar.style.left = "0px";
  bar.style.top = "0px";
  const bw = bar.offsetWidth * dpr, bh = bar.offsetHeight * dpr;
  const subShown = !!tool && !sub.hidden;
  const sh = subShown ? sub.offsetHeight * dpr + 6 * dpr : 0;
  const gap = 8 * dpr;
  let x = sel.x + sel.w - bw;
  let y = sel.y + sel.h + gap;
  if (y + bh + sh > m.y + m.h) {
    y = sel.y - gap - bh - sh;
    if (y < m.y) y = sel.y + sel.h - bh - sh - gap; // no room either side: inside
  }
  x = clamp(x, m.x + 4 * dpr, m.x + m.w - bw - 4 * dpr);
  y = clamp(y, m.y + 4 * dpr, m.y + m.h - bh - 4 * dpr);
  bar.style.left = css(x);
  bar.style.top = css(y);
  if (subShown) {
    const above = y < sel.y;
    const sx = clamp(x, m.x + 4 * dpr, m.x + m.w - sub.offsetWidth * dpr - 4 * dpr);
    sub.style.left = css(sx);
    sub.style.top = css(above ? y - sub.offsetHeight * dpr - 6 * dpr : y + bh + 6 * dpr);
  }
}

// --- magnifier ------------------------------------------------------------------------

const MAG_N = 21;
const magTmp = document.createElement("canvas");
magTmp.width = magTmp.height = MAG_N;
const magTmpCtx = magTmp.getContext("2d");

function pixelAt(x, y) {
  const i = (y * W + x) * 4;
  return [pix[i], pix[i + 1], pix[i + 2]];
}
const hex = (c) => "#" + c.map((v) => v.toString(16).padStart(2, "0")).join("").toUpperCase();
const rgbText = (c) => `${c[0]}, ${c[1]}, ${c[2]}`;

function drawMag() {
  const want = pix && S?.settings?.magnifier !== false && (mode === "detect" || mode === "selecting" || drag?.kind === "resize" || drag?.kind === "new");
  magEl.hidden = !want;
  if (!want) return;
  const half = (MAG_N - 1) / 2;
  const img = magTmpCtx.createImageData(MAG_N, MAG_N);
  for (let j = 0; j < MAG_N; j++) {
    for (let i = 0; i < MAG_N; i++) {
      const x = cursor.x + i - half, y = cursor.y + j - half;
      const o = (j * MAG_N + i) * 4;
      if (x < 0 || y < 0 || x >= W || y >= H) {
        img.data[o + 3] = 255;
        continue;
      }
      const s = (y * W + x) * 4;
      img.data[o] = pix[s];
      img.data[o + 1] = pix[s + 1];
      img.data[o + 2] = pix[s + 2];
      img.data[o + 3] = 255;
    }
  }
  magTmpCtx.putImageData(img, 0, 0);
  magCtx.imageSmoothingEnabled = false;
  magCtx.drawImage(magTmp, 0, 0, 126, 126);
  const cell = 126 / MAG_N;
  magCtx.strokeStyle = "rgba(30,144,255,0.55)";
  magCtx.lineWidth = cell;
  magCtx.beginPath();
  magCtx.moveTo(63, 0); magCtx.lineTo(63, 126 * (half / MAG_N));
  magCtx.moveTo(63, 126 * ((half + 1) / MAG_N)); magCtx.lineTo(63, 126);
  magCtx.moveTo(0, 63); magCtx.lineTo(126 * (half / MAG_N), 63);
  magCtx.moveTo(126 * ((half + 1) / MAG_N), 63); magCtx.lineTo(126, 63);
  magCtx.stroke();
  magCtx.strokeStyle = "#fff";
  magCtx.lineWidth = 1;
  magCtx.strokeRect(half * cell + 0.5, half * cell + 0.5, cell - 1, cell - 1);
  const c = pixelAt(cursor.x, cursor.y);
  $("#magPos").textContent = `(${cursor.x + OX}, ${cursor.y + OY})`;
  $("#magColor i").style.background = hex(c);
  $("#magColor span").textContent = colorFmt === "hex" ? hex(c) : `RGB(${rgbText(c)})`;
  // Beside the pointer, flipped away from screen edges.
  const m = monitorAt(cursor.x, cursor.y);
  const mw = 130 * dpr, mh = magEl.offsetHeight * dpr || 190 * dpr;
  let x = cursor.x + 22 * dpr, y = cursor.y + 22 * dpr;
  if (x + mw > m.x + m.w) x = cursor.x - 22 * dpr - mw;
  if (y + mh > m.y + m.h) y = cursor.y - 22 * dpr - mh;
  magEl.style.left = css(x);
  magEl.style.top = css(y);
}

// --- shapes ----------------------------------------------------------------------------

function mosaicSource(kind) {
  if (kind === "blur") {
    if (!blurC) {
      blurC = document.createElement("canvas");
      blurC.width = W;
      blurC.height = H;
      const c = blurC.getContext("2d");
      c.filter = `blur(${Math.round(9 * dpr)}px)`;
      c.drawImage(base, 0, 0);
    }
    return blurC;
  }
  if (!pixC) {
    const cell = Math.max(6, Math.round(9 * dpr));
    const small = document.createElement("canvas");
    small.width = Math.ceil(W / cell);
    small.height = Math.ceil(H / cell);
    const sc = small.getContext("2d");
    sc.imageSmoothingQuality = "medium";
    sc.drawImage(base, 0, 0, small.width, small.height);
    pixC = document.createElement("canvas");
    pixC.width = W;
    pixC.height = H;
    const pc = pixC.getContext("2d");
    pc.imageSmoothingEnabled = false;
    pc.drawImage(small, 0, 0, small.width * cell, small.height * cell);
  }
  return pixC;
}

function strokePath(ctx, pts) {
  ctx.beginPath();
  if (pts.length === 1) {
    ctx.moveTo(pts[0].x, pts[0].y);
    ctx.lineTo(pts[0].x + 0.01, pts[0].y);
    return;
  }
  ctx.moveTo(pts[0].x, pts[0].y);
  for (let i = 1; i < pts.length - 1; i++) {
    const mx = (pts[i].x + pts[i + 1].x) / 2, my = (pts[i].y + pts[i + 1].y) / 2;
    ctx.quadraticCurveTo(pts[i].x, pts[i].y, mx, my);
  }
  const last = pts[pts.length - 1];
  ctx.lineTo(last.x, last.y);
}

function arrowPolygon(x1, y1, x2, y2, w) {
  const dx = x2 - x1, dy = y2 - y1, L = Math.hypot(dx, dy);
  if (L < 1) return null;
  const ux = dx / L, uy = dy / L, nx = -uy, ny = ux;
  const head = Math.min(L * 0.75, w * 3.6 + 9 * dpr);
  const hw = head * 0.5;
  const neck = Math.max(w * 0.55, 1.2 * dpr);
  const tail = Math.max(w * 0.18, 0.6 * dpr);
  const bx = x2 - ux * head, by = y2 - uy * head;
  const kx = x2 - ux * head * 0.82, ky = y2 - uy * head * 0.82; // slightly swept-back barbs
  return [
    [x1 + nx * tail, y1 + ny * tail],
    [kx + nx * neck, ky + ny * neck],
    [bx + nx * hw, by + ny * hw],
    [x2, y2],
    [bx - nx * hw, by - ny * hw],
    [kx - nx * neck, ky - ny * neck],
    [x1 - nx * tail, y1 - ny * tail],
  ];
}

const FONT = '"Microsoft YaHei UI", "Segoe UI", sans-serif';

function textBox(s) {
  actx.font = `${s.size}px ${FONT}`;
  const lines = s.text.split("\n");
  const w = Math.max(...lines.map((l) => actx.measureText(l).width), s.size * 0.5);
  return { x: s.x, y: s.y, w: Math.ceil(w), h: Math.ceil(lines.length * s.size * 1.25) };
}

function numberRadius(s) {
  return (9 + s.size * 3) * dpr;
}

function drawShape(ctx, s) {
  ctx.save();
  ctx.lineCap = "round";
  ctx.lineJoin = "round";
  ctx.strokeStyle = ctx.fillStyle = s.color || "#f00";
  ctx.lineWidth = s.width;
  switch (s.type) {
    case "rect": {
      const r = norm(s.a, s.b);
      ctx.lineJoin = "miter";
      if (s.fill) ctx.fillRect(r.x, r.y, r.w, r.h);
      else ctx.strokeRect(r.x, r.y, r.w, r.h);
      break;
    }
    case "ellipse": {
      const r = norm(s.a, s.b);
      ctx.beginPath();
      ctx.ellipse(r.x + r.w / 2, r.y + r.h / 2, Math.max(r.w / 2, 0.5), Math.max(r.h / 2, 0.5), 0, 0, Math.PI * 2);
      s.fill ? ctx.fill() : ctx.stroke();
      break;
    }
    case "line":
      ctx.beginPath();
      ctx.moveTo(s.a.x, s.a.y);
      ctx.lineTo(s.b.x, s.b.y);
      ctx.stroke();
      break;
    case "arrow": {
      const p = arrowPolygon(s.a.x, s.a.y, s.b.x, s.b.y, s.width);
      if (!p) break;
      ctx.beginPath();
      ctx.moveTo(...p[0]);
      for (const q of p.slice(1)) ctx.lineTo(...q);
      ctx.closePath();
      ctx.fill();
      break;
    }
    case "pen":
      strokePath(ctx, s.pts);
      ctx.stroke();
      break;
    case "marker":
      ctx.globalAlpha = 0.42;
      ctx.lineCap = "square";
      strokePath(ctx, s.pts);
      ctx.stroke();
      break;
    case "eraser":
      ctx.globalCompositeOperation = "destination-out";
      strokePath(ctx, s.pts);
      ctx.stroke();
      break;
    case "mosaic": {
      const src = mosaicSource(s.kind);
      if (s.shape === "rect") {
        const r = norm(s.a, s.b);
        if (r.w && r.h) ctx.drawImage(src, r.x, r.y, r.w, r.h, r.x, r.y, r.w, r.h);
        break;
      }
      const xs = s.pts.map((p) => p.x), ys = s.pts.map((p) => p.y);
      const pad = s.width;
      const bx = Math.floor(Math.min(...xs) - pad), by = Math.floor(Math.min(...ys) - pad);
      const bw = Math.ceil(Math.max(...xs) + pad) - bx, bh = Math.ceil(Math.max(...ys) + pad) - by;
      if (bw <= 0 || bh <= 0) break;
      const t = document.createElement("canvas");
      t.width = bw;
      t.height = bh;
      const tc = t.getContext("2d");
      tc.translate(-bx, -by);
      tc.lineCap = tc.lineJoin = "round";
      tc.lineWidth = s.width;
      tc.strokeStyle = "#fff";
      strokePath(tc, s.pts);
      tc.stroke();
      tc.setTransform(1, 0, 0, 1, 0, 0);
      tc.globalCompositeOperation = "source-in";
      tc.drawImage(src, bx, by, bw, bh, 0, 0, bw, bh);
      ctx.drawImage(t, bx, by);
      break;
    }
    case "text": {
      ctx.font = `${s.size}px ${FONT}`;
      ctx.textBaseline = "top";
      s.text.split("\n").forEach((l, i) => ctx.fillText(l, s.x, s.y + i * s.size * 1.25));
      break;
    }
    case "number": {
      const r = numberRadius(s);
      ctx.beginPath();
      ctx.arc(s.x, s.y, r, 0, Math.PI * 2);
      ctx.fill();
      ctx.lineWidth = Math.max(1.5, dpr * 1.5);
      ctx.strokeStyle = "rgba(255,255,255,0.9)";
      ctx.stroke();
      ctx.fillStyle = light(s.color) ? "#000" : "#fff";
      ctx.font = `bold ${Math.round(r * 1.15)}px "Segoe UI", sans-serif`;
      ctx.textAlign = "center";
      ctx.textBaseline = "middle";
      ctx.fillText(String(s.n), s.x, s.y + r * 0.04);
      break;
    }
  }
  ctx.restore();
}

function light(c) {
  const v = parseInt(String(c).slice(1), 16);
  const r = (v >> 16) & 255, g = (v >> 8) & 255, b = v & 255;
  return r * 0.299 + g * 0.587 + b * 0.114 > 186;
}

function render(extra) {
  actx.clearRect(0, 0, W, H);
  if (!sel) return;
  actx.save();
  actx.beginPath();
  actx.rect(sel.x, sel.y, sel.w, sel.h);
  actx.clip();
  shapes.forEach((s, i) => {
    if (editing && editing.index === i) return; // shown by the text editor
    drawShape(actx, s);
  });
  if (extra) drawShape(actx, extra);
  actx.restore();
  drawShapeBox();
}

// --- selecting existing shapes -----------------------------------------------------------

function bbox(s) {
  switch (s.type) {
    case "rect":
    case "ellipse":
    case "line":
    case "arrow":
      return norm(s.a, s.b);
    case "text":
      return textBox(s);
    case "number": {
      const r = numberRadius(s);
      return { x: s.x - r, y: s.y - r, w: r * 2, h: r * 2 };
    }
    default:
      return null;
  }
}

function distSeg(p, a, b) {
  const dx = b.x - a.x, dy = b.y - a.y;
  const L = dx * dx + dy * dy;
  const t = L ? clamp(((p.x - a.x) * dx + (p.y - a.y) * dy) / L, 0, 1) : 0;
  return Math.hypot(p.x - (a.x + t * dx), p.y - (a.y + t * dy));
}

/// The topmost movable shape under the point.
function hitShape(p) {
  const tol = 6 * dpr;
  for (let i = shapes.length - 1; i >= 0; i--) {
    const s = shapes[i];
    const t = tol + (s.width || 0) / 2;
    switch (s.type) {
      case "rect": {
        const r = norm(s.a, s.b);
        if (s.fill ? inside({ x: r.x - t, y: r.y - t, w: r.w + 2 * t, h: r.h + 2 * t }, p.x, p.y)
          : inside({ x: r.x - t, y: r.y - t, w: r.w + 2 * t, h: r.h + 2 * t }, p.x, p.y) && !inside({ x: r.x + t, y: r.y + t, w: r.w - 2 * t, h: r.h - 2 * t }, p.x, p.y)) return i;
        break;
      }
      case "ellipse": {
        const r = norm(s.a, s.b);
        const rx = r.w / 2, ry = r.h / 2;
        if (rx < 1 || ry < 1) break;
        const d = Math.hypot((p.x - r.x - rx) / rx, (p.y - r.y - ry) / ry);
        const band = t / Math.min(rx, ry);
        if (s.fill ? d <= 1 + band : Math.abs(d - 1) <= band) return i;
        break;
      }
      case "line":
      case "arrow":
        if (distSeg(p, s.a, s.b) <= t + (s.type === "arrow" ? s.width : 0)) return i;
        break;
      case "text":
      case "number": {
        const b = bbox(s);
        if (inside({ x: b.x - tol, y: b.y - tol, w: b.w + 2 * tol, h: b.h + 2 * tol }, p.x, p.y)) return i;
        break;
      }
    }
  }
  return -1;
}

/// Handles of the active shape: corners for boxes, ends for lines.
function shapeHandles(s) {
  if (!s) return [];
  if (s.type === "line" || s.type === "arrow") return [{ k: "a", x: s.a.x, y: s.a.y }, { k: "b", x: s.b.x, y: s.b.y }];
  if (s.type === "rect" || s.type === "ellipse") {
    const r = norm(s.a, s.b);
    return [
      { k: "nw", x: r.x, y: r.y },
      { k: "ne", x: r.x + r.w, y: r.y },
      { k: "se", x: r.x + r.w, y: r.y + r.h },
      { k: "sw", x: r.x, y: r.y + r.h },
    ];
  }
  return [];
}

let sboxEls = [];
function drawShapeBox() {
  for (const e of sboxEls) e.remove();
  sboxEls = [];
  const s = shapes[active];
  if (!s || mode !== "selected") return;
  const b = bbox(s);
  if (b && s.type !== "line" && s.type !== "arrow") {
    const d = document.createElement("div");
    d.style.cssText = `position:absolute;pointer-events:none;outline:1px dashed rgba(30,144,255,.9);outline-offset:${css(3 * dpr)};`;
    place(d, b);
    document.body.append(d);
    sboxEls.push(d);
  }
  for (const h of shapeHandles(s)) {
    const d = document.createElement("div");
    d.style.cssText = `position:absolute;width:8px;height:8px;margin:-4px 0 0 -4px;border-radius:50%;background:#fff;border:1.5px solid #1e90ff;pointer-events:none;left:${css(h.x)};top:${css(h.y)}`;
    document.body.append(d);
    sboxEls.push(d);
  }
}

function handleAt(p) {
  const s = shapes[active];
  for (const h of shapeHandles(s)) if (Math.hypot(p.x - h.x, p.y - h.y) <= 7 * dpr) return h.k;
  return null;
}

// --- undo ---------------------------------------------------------------------------

function snapshot() {
  undoStack.push(JSON.stringify(shapes));
  if (undoStack.length > 200) undoStack.shift();
  redoStack = [];
}
function undo() {
  if (editing) return;
  if (!undoStack.length) return;
  redoStack.push(JSON.stringify(shapes));
  shapes = JSON.parse(undoStack.pop());
  active = -1;
  render();
  drawChrome();
}
function redo() {
  if (!redoStack.length) return;
  undoStack.push(JSON.stringify(shapes));
  shapes = JSON.parse(redoStack.pop());
  active = -1;
  render();
  drawChrome();
}

const locked = () => shapes.length > 0 || !!tool || !!S?.preset;

// --- text editing ---------------------------------------------------------------------

function startText(x, y, index = -1) {
  const st = styles.text;
  const s = index >= 0 ? shapes[index] : { type: "text", x, y, text: "", size: FONT_SIZES[st.size] * dpr, color: st.color };
  editing = { index, shape: s };
  editEl.hidden = false;
  editEl.textContent = s.text;
  editEl.style.left = css(s.x);
  editEl.style.top = css(s.y);
  editEl.style.fontSize = css(s.size);
  editEl.style.color = s.color;
  render();
  editEl.focus();
  requestAnimationFrame(() => {
    editEl.focus();
    const r = document.createRange();
    r.selectNodeContents(editEl);
    r.collapse(false);
    const sl = getSelection();
    sl.removeAllRanges();
    sl.addRange(r);
  });
}

function commitText() {
  if (!editing) return;
  const text = editEl.innerText.replace(/\n$/, "");
  const { index, shape } = editing;
  editing = null;
  editEl.hidden = true;
  editEl.blur();
  if (index >= 0) {
    if (text !== shape.text) {
      snapshot();
      if (text.trim()) shapes[index] = { ...shape, text, color: editEl.style.color ? shape.color : shape.color };
      else shapes.splice(index, 1);
    }
  } else if (text.trim()) {
    snapshot();
    shapes.push({ ...shape, text });
  }
  active = -1;
  render();
  drawChrome();
}

editEl.addEventListener("keydown", (e) => {
  e.stopPropagation();
  if (e.key === "Escape" || (e.key === "Enter" && e.ctrlKey)) {
    e.preventDefault();
    commitText();
  }
});
editEl.addEventListener("mousedown", (e) => e.stopPropagation());

// --- the toolbar ----------------------------------------------------------------------

const I = (d, extra = "") => `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" ${extra}>${d}</svg>`;
const ICONS = {
  rect: I('<rect x="4" y="6" width="16" height="12" rx="1"/>'),
  ellipse: I('<ellipse cx="12" cy="12" rx="8.5" ry="6.5"/>'),
  line: I('<path d="M5 19 19 5"/>'),
  arrow: I('<path d="M5 19 18 6"/><path d="M10 6h8v8"/>'),
  pen: I('<path d="M4 20c3-1 4-4 7-7l6-6a2 2 0 0 1 3 3l-6 6c-3 3-6 4-7 7"/><path d="M4 20l1.5-4"/>'),
  marker: I('<path d="m14 5 5 5-7 7H7v-5z"/><path d="M4 21h9"/>'),
  mosaic: I('<rect x="4" y="4" width="5" height="5"/><rect x="14" y="4" width="5" height="5" fill="currentColor"/><rect x="9" y="9" width="5" height="5" fill="currentColor"/><rect x="4" y="14" width="5" height="5" fill="currentColor"/><rect x="14" y="14" width="5" height="5"/>'),
  text: I('<path d="M5 6V4h14v2"/><path d="M12 4v16"/><path d="M9 20h6"/>'),
  number: I('<circle cx="12" cy="12" r="8.5"/><path d="M10.5 9.5 12.5 8v8"/>'),
  eraser: I('<path d="m8 20-4-4 10-10 6 6-8 8z"/><path d="M8 20h12"/><path d="m9 11 6 6"/>'),
  undo: I('<path d="M9 7 4 12l5 5"/><path d="M4 12h10a6 6 0 0 1 0 12"/>', 'style="transform:translateY(-3px)"'),
  redo: I('<path d="m15 7 5 5-5 5"/><path d="M20 12H10a6 6 0 0 0 0 12"/>', 'style="transform:translateY(-3px)"'),
  pin: I('<path d="M9 4h6l-1 5 3 3H7l3-3z"/><path d="M12 12v8"/>'),
  save: I('<path d="M5 4h11l3 3v13H5z"/><path d="M8 4v5h7V4"/><path d="M8 20v-6h8v6"/>'),
  copy: I('<path d="m5 12 5 5L20 7"/>', 'stroke-width="2.4"'),
  cancel: I('<path d="M6 6l12 12M18 6 6 18"/>', 'stroke-width="2.2"'),
};
const TOOLS = [
  ["rect", "矩形 (R)"],
  ["ellipse", "椭圆 (E)"],
  ["line", "直线 (L)"],
  ["arrow", "箭头 (A)"],
  ["pen", "画笔 (P)"],
  ["marker", "马克笔 (M)"],
  ["mosaic", "马赛克 / 模糊 (X)"],
  ["text", "文字 (T)"],
  ["number", "序号 (N)"],
  ["eraser", "橡皮擦 (Q)"],
];
const TOOL_KEYS = { r: "rect", e: "ellipse", l: "line", a: "arrow", p: "pen", m: "marker", x: "mosaic", t: "text", n: "number", q: "eraser" };

function btn(icon, title, onClick, cls = "") {
  const b = document.createElement("button");
  b.type = "button";
  b.innerHTML = ICONS[icon];
  b.title = title;
  b.className = cls;
  b.addEventListener("mousedown", (e) => e.stopPropagation());
  b.addEventListener("click", (e) => {
    e.stopPropagation();
    onClick();
  });
  return b;
}
function sep(host) {
  const s = document.createElement("div");
  s.className = "sep";
  host.append(s);
}

function renderBar() {
  bar.replaceChildren();
  for (const [t, title] of TOOLS) bar.append(btn(t, title, () => setTool(tool === t ? null : t), tool === t ? "on" : ""));
  sep(bar);
  const u = btn("undo", "撤销 (Ctrl+Z)", undo);
  u.disabled = !undoStack.length;
  const r = btn("redo", "重做 (Ctrl+Y)", redo);
  r.disabled = !redoStack.length;
  bar.append(u, r);
  sep(bar);
  bar.append(btn("pin", S?.preset ? "放回贴图 (Enter / F3)" : "贴到屏幕 (F3)", () => finish("pin")));
  bar.append(btn("save", "保存 (Ctrl+S)；另存为 (Ctrl+Shift+S)", () => finish("save")));
  bar.append(btn("cancel", "取消 (Esc)", cancel, "cancel"));
  bar.append(btn("copy", "复制到剪贴板 (Enter / Ctrl+C / 双击)", () => finish("copy"), "ok"));
  renderSub();
}

function renderSub() {
  sub.replaceChildren();
  if (!tool) {
    sub.hidden = true;
    return;
  }
  sub.hidden = false;
  const st = styles[tool];
  const cur = active >= 0 ? shapes[active] : null;
  const apply = (fn) => {
    fn(st);
    if (cur && cur.type === tool) {
      snapshot();
      fn(cur, true);
      render();
    }
    saveStyles();
    renderSub();
    placeBars();
  };
  const addSizes = (list, label) => {
    const wrap = document.createElement("div");
    wrap.className = "sizes";
    list.forEach((v, i) => {
      const b = document.createElement("button");
      b.type = "button";
      b.title = label(i);
      const d = tool === "text" ? 6 + i * 2 : 4 + i * 4;
      b.innerHTML = `<i style="width:${d}px;height:${d}px"></i>`;
      if (st.size === i) b.classList.add("on");
      b.addEventListener("mousedown", (e) => e.stopPropagation());
      b.addEventListener("click", (e) => {
        e.stopPropagation();
        apply((o, isShape) => {
          if (!isShape) o.size = i;
          else if (o.type === "text") o.size = FONT_SIZES[i] * dpr;
          else if (o.type === "number") o.size = i;
          else o.width = SIZES[i] * dpr * (o.type === "marker" ? 3.5 : 1);
        });
      });
      wrap.append(b);
    });
    sub.append(wrap);
  };
  if (tool === "text") addSizes(FONT_SIZES, (i) => `${FONT_SIZES[i]} 号`);
  else if (tool === "mosaic" || tool === "eraser") addSizes([0, 1, 2], (i) => ["细", "中", "粗"][i]);
  else addSizes(SIZES, (i) => ["细", "中", "粗"][i]);

  const toggle = (text, on, fn, title = "") => {
    const b = document.createElement("button");
    b.type = "button";
    b.className = `toggle ${on ? "on" : ""}`;
    b.textContent = text;
    b.title = title;
    b.addEventListener("mousedown", (e) => e.stopPropagation());
    b.addEventListener("click", (e) => {
      e.stopPropagation();
      fn();
    });
    sub.append(b);
  };
  if (tool === "rect" || tool === "ellipse") {
    const next = !st.fill;
    toggle("填充", st.fill, () => apply((o) => (o.fill = next)));
  }
  if (tool === "mosaic") {
    const s1 = document.createElement("div");
    s1.className = "sep";
    sub.append(s1);
    toggle("马赛克", st.kind === "pixel", () => { st.kind = "pixel"; saveStyles(); renderSub(); });
    toggle("模糊", st.kind === "blur", () => { st.kind = "blur"; saveStyles(); renderSub(); });
    const s2 = document.createElement("div");
    s2.className = "sep";
    sub.append(s2);
    toggle("涂抹", st.shape === "brush", () => { st.shape = "brush"; saveStyles(); renderSub(); });
    toggle("矩形", st.shape === "rect", () => { st.shape = "rect"; saveStyles(); renderSub(); });
  }
  if (tool !== "mosaic" && tool !== "eraser") {
    const s = document.createElement("div");
    s.className = "sep";
    sub.append(s);
    const current = document.createElement("label");
    current.className = "current";
    current.style.background = st.color;
    current.title = "自定义颜色";
    current.innerHTML = `<input type="color" value="${st.color}">`;
    current.addEventListener("mousedown", (e) => e.stopPropagation());
    current.firstChild.addEventListener("input", (e) => apply((o) => (o.color = e.target.value)));
    sub.append(current);
    const grid = document.createElement("div");
    grid.className = "colors";
    for (const c of COLORS) {
      const b = document.createElement("button");
      b.type = "button";
      b.style.background = c;
      if (c.toLowerCase() === st.color.toLowerCase()) b.classList.add("on");
      b.addEventListener("mousedown", (e) => e.stopPropagation());
      b.addEventListener("click", (e) => {
        e.stopPropagation();
        apply((o) => (o.color = c));
      });
      grid.append(b);
    }
    sub.append(grid);
  }
}

function setTool(t) {
  commitText();
  tool = t;
  active = active >= 0 && shapes[active]?.type === t ? active : -1;
  render();
  drawChrome();
}

// --- pointer ----------------------------------------------------------------------------

function newShapeAt(p) {
  const st = styles[tool];
  const color = st.color;
  switch (tool) {
    case "rect":
    case "ellipse":
      return { type: tool, a: p, b: p, color, width: strokeW(st), fill: !!st.fill };
    case "line":
    case "arrow":
      return { type: tool, a: p, b: p, color, width: strokeW(st) };
    case "pen":
      return { type: "pen", pts: [p], color, width: strokeW(st) };
    case "marker":
      return { type: "marker", pts: [p], color, width: strokeW(st) * 3.5 };
    case "eraser":
      return { type: "eraser", pts: [p], width: (8 + st.size * 10) * dpr };
    case "mosaic":
      return st.shape === "rect"
        ? { type: "mosaic", shape: "rect", kind: st.kind, a: p, b: p }
        : { type: "mosaic", shape: "brush", kind: st.kind, pts: [p], width: (12 + st.size * 12) * dpr };
  }
  return null;
}

const HANDLE_CURSORS = { nw: "nwse-resize", se: "nwse-resize", ne: "nesw-resize", sw: "nesw-resize", n: "ns-resize", s: "ns-resize", e: "ew-resize", w: "ew-resize" };

function selEdgeAt(p) {
  if (!sel || locked()) return null;
  const t = 6 * dpr;
  const l = Math.abs(p.x - sel.x) <= t, r = Math.abs(p.x - (sel.x + sel.w)) <= t;
  const tp = Math.abs(p.y - sel.y) <= t, b = Math.abs(p.y - (sel.y + sel.h)) <= t;
  const inX = p.x >= sel.x - t && p.x <= sel.x + sel.w + t;
  const inY = p.y >= sel.y - t && p.y <= sel.y + sel.h + t;
  if (!inX || !inY) return null;
  if (tp && l) return "nw";
  if (tp && r) return "ne";
  if (b && l) return "sw";
  if (b && r) return "se";
  if (tp) return "n";
  if (b) return "s";
  if (l) return "w";
  if (r) return "e";
  return null;
}

function setCursor(c) {
  document.body.style.cursor = c;
}

function hoverCursor(p) {
  if (mode !== "selected") return setCursor("crosshair");
  if (tool) {
    if (active >= 0 && handleAt(p)) return setCursor("pointer");
    if (hitShape(p) >= 0) return setCursor("move");
    if (tool === "text") return setCursor(inside(sel, p.x, p.y) ? "text" : "default");
    return setCursor(inside(sel, p.x, p.y) ? "crosshair" : "default");
  }
  const edge = selEdgeAt(p);
  if (edge) return setCursor(HANDLE_CURSORS[edge]);
  if (inside(sel, p.x, p.y)) return setCursor(locked() ? "default" : "move");
  return setCursor(locked() ? "default" : "crosshair");
}

document.addEventListener("contextmenu", (e) => e.preventDefault());

document.addEventListener("mousedown", (e) => {
  if (!S) return;
  const p = point(e);
  cursor = p;
  if (e.button === 2) return back();
  if (e.button !== 0) return;
  if (editing) {
    commitText();
    if (tool === "text") return;
  }
  if (mode === "detect") {
    drag = { kind: "pending", start: p };
    return;
  }
  if (mode !== "selected") return;

  if (tool) {
    const h = active >= 0 ? handleAt(p) : null;
    if (h) {
      snapshot();
      const s = shapes[active];
      if (h === "a" || h === "b") {
        drag = { kind: "shape-handle", end: h };
      } else {
        // Re-anchor the box on the opposite corner; `b` is then the corner
        // being dragged, whichever one it was.
        const r = norm(s.a, s.b);
        const corner = { nw: [r.x, r.y], ne: [r.x + r.w, r.y], se: [r.x + r.w, r.y + r.h], sw: [r.x, r.y + r.h] }[h];
        const fixed = { nw: [r.x + r.w, r.y + r.h], ne: [r.x, r.y + r.h], se: [r.x, r.y], sw: [r.x + r.w, r.y] }[h];
        s.a = { x: fixed[0], y: fixed[1] };
        s.b = { x: corner[0], y: corner[1] };
        drag = { kind: "shape-handle", end: "b" };
      }
      return;
    }
    const hit = hitShape(p);
    if (hit >= 0) {
      const s = shapes[hit];
      if (s.type !== tool) {
        tool = s.type === "mosaic" || s.type === "eraser" ? tool : s.type;
      }
      active = hit;
      snapshot();
      drag = { kind: "shape-move", last: p, moved: false };
      render();
      drawChrome();
      return;
    }
    active = -1;
    if (!inside(sel, p.x, p.y)) return;
    if (tool === "text") {
      startText(p.x, p.y);
      return;
    }
    if (tool === "number") {
      snapshot();
      const st = styles.number;
      const n = shapes.filter((s) => s.type === "number").reduce((m, s) => Math.max(m, s.n), 0) + 1;
      shapes.push({ type: "number", x: p.x, y: p.y, n, color: st.color, size: st.size });
      active = shapes.length - 1;
      render();
      drawChrome();
      return;
    }
    drag = { kind: "draw", shape: newShapeAt(p) };
    render(drag.shape);
    drawChrome();
    return;
  }

  const edge = selEdgeAt(p);
  if (edge) {
    drag = { kind: "resize", edge, start: { ...sel } };
    drawChrome();
    return;
  }
  if (inside(sel, p.x, p.y)) {
    if (!locked()) drag = { kind: "move", off: { x: p.x - sel.x, y: p.y - sel.y } };
    return;
  }
  if (!locked()) {
    // A new selection from scratch.
    mode = "selecting";
    sel = { x: p.x, y: p.y, w: 0, h: 0 };
    drag = { kind: "new", start: p };
    drawChrome();
  }
});

document.addEventListener("mousemove", (e) => {
  if (!S) return;
  const p = point(e);
  cursor = p;
  if (!drag) {
    if (mode === "detect") updateHover();
    else hoverCursor(p);
    if (mode === "selected") drawMag();
    return;
  }
  switch (drag.kind) {
    case "pending": {
      if (Math.hypot(p.x - drag.start.x, p.y - drag.start.y) >= 4 * dpr) {
        mode = "selecting";
        drag = { kind: "new", start: drag.start };
        sel = norm(drag.start, p);
      }
      break;
    }
    case "new":
      sel = norm(drag.start, { x: p.x + 1, y: p.y + 1 });
      sel = clip(sel);
      break;
    case "move": {
      sel.x = clamp(p.x - drag.off.x, 0, W - sel.w);
      sel.y = clamp(p.y - drag.off.y, 0, H - sel.h);
      break;
    }
    case "resize": {
      const s = drag.start;
      let x1 = s.x, y1 = s.y, x2 = s.x + s.w, y2 = s.y + s.h;
      if (drag.edge.includes("w")) x1 = p.x;
      if (drag.edge.includes("e")) x2 = p.x + 1;
      if (drag.edge.includes("n")) y1 = p.y;
      if (drag.edge.includes("s")) y2 = p.y + 1;
      sel = clip(norm({ x: x1, y: y1 }, { x: x2, y: y2 }));
      break;
    }
    case "draw": {
      const s = drag.shape;
      if (s.pts) {
        const last = s.pts[s.pts.length - 1];
        if (Math.hypot(p.x - last.x, p.y - last.y) >= 1.5) s.pts.push(p);
      } else {
        let b = p;
        if (e.shiftKey) b = constrain(s, p);
        s.b = b;
      }
      render(s);
      return;
    }
    case "shape-move": {
      const dx = p.x - drag.last.x, dy = p.y - drag.last.y;
      drag.last = p;
      drag.moved = true;
      const s = shapes[active];
      if (s.a) {
        s.a = { x: s.a.x + dx, y: s.a.y + dy };
        s.b = { x: s.b.x + dx, y: s.b.y + dy };
      } else {
        s.x += dx;
        s.y += dy;
      }
      render();
      return;
    }
    case "shape-handle": {
      const s = shapes[active];
      const other = drag.end === "a" ? s.b : s.a;
      if (!e.shiftKey) s[drag.end] = p;
      else if (s.type === "line" || s.type === "arrow") s[drag.end] = constrainFrom(other, p);
      else s[drag.end] = constrain({ ...s, a: other }, p);
      render();
      return;
    }
  }
  drawChrome();
});

/// Shift while drawing: squares, circles, and lines at 45° steps.
function constrain(s, p) {
  if (s.type === "rect" || s.type === "ellipse" || (s.type === "mosaic" && s.shape === "rect")) {
    const dx = p.x - s.a.x, dy = p.y - s.a.y;
    const d = Math.max(Math.abs(dx), Math.abs(dy));
    return { x: s.a.x + Math.sign(dx || 1) * d, y: s.a.y + Math.sign(dy || 1) * d };
  }
  return constrainFrom(s.a, p);
}
function constrainFrom(a, p) {
  const dx = p.x - a.x, dy = p.y - a.y;
  const ang = Math.round(Math.atan2(dy, dx) / (Math.PI / 4)) * (Math.PI / 4);
  const L = Math.hypot(dx, dy);
  return { x: Math.round(a.x + Math.cos(ang) * L), y: Math.round(a.y + Math.sin(ang) * L) };
}

document.addEventListener("mouseup", (e) => {
  if (!S || e.button !== 0 || !drag) return;
  const p = point(e);
  const d = drag;
  drag = null;
  switch (d.kind) {
    case "pending":
      // A click: take the highlighted window / control.
      if (hl) {
        sel = { ...hl };
        mode = "selected";
      }
      break;
    case "new":
      if (sel.w < 2 || sel.h < 2) {
        mode = "detect";
        sel = null;
        updateHover();
        return;
      }
      mode = "selected";
      break;
    case "draw": {
      const s = d.shape;
      const tiny = s.pts ? false : Math.hypot(s.b.x - s.a.x, s.b.y - s.a.y) < 3;
      if (!tiny) {
        snapshot();
        shapes.push(s);
      }
      render();
      break;
    }
    case "shape-move":
    case "shape-handle":
      if (d.kind === "shape-move" && !d.moved) undoStack.pop();
      render();
      break;
  }
  hoverCursor(p);
  drawChrome();
});

document.addEventListener("dblclick", (e) => {
  if (!S || mode !== "selected" || e.button !== 0) return;
  const p = point(e);
  const hit = hitShape(p);
  if (hit >= 0 && shapes[hit].type === "text") {
    tool = "text";
    active = -1;
    startText(0, 0, hit);
    drawChrome();
    return;
  }
  if (inside(sel, p.x, p.y) && !editing) finish(S.preset ? "pin" : "copy");
});

document.addEventListener(
  "wheel",
  (e) => {
    if (!S || mode !== "detect") return;
    e.preventDefault();
    // Wheel up: the enclosing element; down: back towards the smallest.
    level = clamp(level + (e.deltaY < 0 ? 1 : -1), 0, Math.max(0, lastCands.length - 1));
    hl = lastCands[Math.max(0, lastCands.length - 1 - level)] || hl;
    drawChrome();
  },
  { passive: false }
);

// --- keyboard ---------------------------------------------------------------------------

function nudgeSel(dx, dy, resize) {
  if (!sel || locked()) return;
  if (resize) {
    sel.w = clamp(sel.w + dx, 1, W - sel.x);
    sel.h = clamp(sel.h + dy, 1, H - sel.y);
  } else {
    sel.x = clamp(sel.x + dx, 0, W - sel.w);
    sel.y = clamp(sel.y + dy, 0, H - sel.h);
  }
  drawChrome();
}

function useHistory(step) {
  const h = S?.history || [];
  if (!h.length || locked()) return;
  histIdx = histIdx < 0 ? (step < 0 ? h.length - 1 : 0) : clamp(histIdx + step, 0, h.length - 1);
  const r = clip(rel([h[histIdx].x, h[histIdx].y, h[histIdx].w, h[histIdx].h]));
  if (r.w < 1 || r.h < 1) return;
  sel = r;
  mode = "selected";
  flash(`截图记录 ${histIdx + 1} / ${h.length}`);
  drawChrome();
}

let flashTimer = 0;
function flash(text) {
  hintEl.textContent = text;
  hintEl.hidden = false;
  clearTimeout(flashTimer);
  flashTimer = setTimeout(() => (hintEl.hidden = true), 1200);
}

document.addEventListener("keydown", (e) => {
  if (!S || editing) return;
  const k = e.key;
  const ctrl = e.ctrlKey || e.metaKey;
  if (k === "Escape") return cancel();
  if (k === "Shift" && !e.repeat) {
    colorFmt = colorFmt === "hex" ? "rgb" : "hex";
    drawMag();
    return;
  }
  if (ctrl && (k === "z" || k === "Z") && !e.shiftKey) return (e.preventDefault(), undo());
  if (ctrl && (k === "y" || k === "Y" || ((k === "z" || k === "Z") && e.shiftKey))) return (e.preventDefault(), redo());
  if (ctrl && (k === "s" || k === "S")) {
    e.preventDefault();
    if (mode === "selected") finish(e.shiftKey ? "save-as" : "save");
    return;
  }
  if (ctrl && (k === "a" || k === "A")) {
    e.preventDefault();
    if (locked()) return;
    sel = clip(monitorAt(cursor.x, cursor.y));
    mode = "selected";
    drawChrome();
    return;
  }
  if (k === "F3" && mode === "selected") return (e.preventDefault(), finish("pin"));
  if (k === "Enter" && mode === "selected" && S.preset) return (e.preventDefault(), finish("pin"));
  if ((ctrl && (k === "c" || k === "C")) || k === "Enter") {
    if (mode === "selected") return (e.preventDefault(), finish("copy"));
    if (ctrl && mode === "detect" && pix) return copyColor();
  }
  if (!ctrl && (k === "c" || k === "C") && (mode === "detect" || mode === "selecting")) return copyColor();
  if ((k === "Delete" || k === "Backspace") && active >= 0) {
    snapshot();
    shapes.splice(active, 1);
    active = -1;
    render();
    drawChrome();
    return;
  }
  if (k === "Tab") {
    e.preventDefault();
    detectControls = !detectControls;
    flash(detectControls ? "识别窗口和控件" : "只识别窗口");
    if (mode === "detect") updateHover();
    return;
  }
  if (k === "," || k === "<") return useHistory(-1);
  if (k === "." || k === ">") return useHistory(1);
  const arrows = { ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, -1], ArrowDown: [0, 1] };
  if (arrows[k]) {
    e.preventDefault();
    const m = ctrl ? 10 : 1;
    const [dx, dy] = arrows[k].map((v) => v * m);
    if (mode === "selected") nudgeSel(dx, dy, e.shiftKey);
    else invoke("capture_nudge", { dx, dy }).catch(() => {});
    return;
  }
  if (mode === "selected" && !ctrl && !e.altKey && TOOL_KEYS[k.toLowerCase()]) {
    const t = TOOL_KEYS[k.toLowerCase()];
    setTool(tool === t ? null : t);
  }
});

function copyColor() {
  const c = pixelAt(cursor.x, cursor.y);
  const text = colorFmt === "hex" ? hex(c) : rgbText(c);
  invoke("capture_copy_text", { text }).then(() => flash(`已复制 ${text}`)).catch(() => {});
}

/// Right click: one step back. Drawing -> stop; selection -> none; none ->
/// leave.
function back() {
  if (editing) return commitText();
  if (drag) {
    drag = null;
    render();
    drawChrome();
    return;
  }
  if (S?.preset) return cancel();
  if (mode === "selected" || mode === "selecting") {
    shapes = [];
    undoStack = [];
    redoStack = [];
    active = -1;
    tool = null;
    sel = null;
    mode = "detect";
    render();
    updateHover();
    return;
  }
  cancel();
}

// --- start / finish ---------------------------------------------------------------------

function reset() {
  mode = "idle";
  sel = hl = null;
  drag = null;
  tool = null;
  shapes = [];
  undoStack = [];
  redoStack = [];
  active = -1;
  editing = null;
  editEl.hidden = true;
  histIdx = -1;
  level = 0;
  lastCands = [];
  pixC = blurC = null;
  uia.clear();
  for (const e of sboxEls) e.remove();
  sboxEls = [];
  bar.hidden = sub.hidden = magEl.hidden = hintEl.hidden = sizeEl.hidden = true;
  setCursor("crosshair");
}

function hideAll() {
  S = null;
  reset();
  drawChrome();
  actx.clearRect(0, 0, W, H);
}

async function cancel(reason) {
  hideAll();
  await invoke("capture_cancel", reason ? { reason: String(reason) } : {}).catch(() => {});
}

async function finish(op) {
  if (!sel || sel.w < 1 || sel.h < 1) return;
  commitText();
  const r = { ...sel };
  const c = document.createElement("canvas");
  c.width = r.w;
  c.height = r.h;
  const x = c.getContext("2d", { willReadFrequently: true });
  x.drawImage(base, r.x, r.y, r.w, r.h, 0, 0, r.w, r.h);
  x.drawImage(ann, r.x, r.y, r.w, r.h, 0, 0, r.w, r.h);
  const data = x.getImageData(0, 0, r.w, r.h).data;
  hideAll();
  await invoke("capture_finish", new Uint8Array(data.buffer), {
    headers: {
      "x-op": op,
      "x-width": String(r.w),
      "x-height": String(r.h),
      "x-rx": String(r.x + OX),
      "x-ry": String(r.y + OY),
      "x-rw": String(r.w),
      "x-rh": String(r.h),
    },
  }).catch((e) => console.error(e));
}

const frames = (n) => new Promise((res) => {
  const step = () => (n-- <= 0 ? res() : requestAnimationFrame(step));
  requestAnimationFrame(step);
});

// F3 is also the global "pin the clipboard" hotkey; while capturing, the
// app forwards it here instead.
listen("capture:pin", () => {
  if (S && mode === "selected") finish("pin");
});

// The frozen screen arrives through WebView2 shared memory (see
// capture/shared.rs), usually just before capture:start.
let shared = null; // { id, buf }
let sharedWait = null;
if (window.chrome?.webview?.addEventListener) {
  window.chrome.webview.addEventListener("sharedbufferreceived", (e) => {
    let info = {};
    try { info = typeof e.additionalData === "string" ? JSON.parse(e.additionalData) : (e.additionalData || {}); } catch {}
    shared = { id: info.id, buf: e.getBuffer() };
    if (sharedWait && sharedWait.id === info.id) sharedWait.done();
  });
}
function sharedFrame(id, ms) {
  if (shared && shared.id === id) return Promise.resolve(shared.buf);
  return new Promise((res) => {
    const t = setTimeout(() => { sharedWait = null; res(null); }, ms);
    sharedWait = { id, done: () => { clearTimeout(t); sharedWait = null; res(shared.buf); } };
  });
}

// Windows to snap to come separately (listing them can be slow).
let pendingWins = null; // may arrive before capture:start
function takeWins(p) {
  wins = (p.windows || []).map((w) => ({ hwnd: w.hwnd, r: rel(w.rect), children: (w.children || []).map(rel) }));
}
listen("capture:windows", (e) => {
  if (!S || e.payload.id !== S.id) {
    pendingWins = e.payload;
    return;
  }
  takeWins(e.payload);
  if (mode === "detect") updateHover();
});

listen("capture:start", (e) => {
  startCapture(e).catch((err) => {
    console.error(err);
    cancel(`start: ${err && err.message ? err.message : err}`);
  });
});

async function startCapture(e) {
  reset();
  S = e.payload;
  dpr = window.devicePixelRatio || 1;
  [OX, OY] = S.origin;
  [W, H] = S.size;
  monitors = (S.monitors || []).map((m) => ({ x: m.x - OX, y: m.y - OY, w: m.w, h: m.h }));
  wins = (S.windows || []).map((w) => ({ hwnd: w.hwnd, r: rel(w.rect), children: (w.children || []).map(rel) }));
  if (pendingWins && pendingWins.id === S.id) takeWins(pendingWins);
  pendingWins = null;
  for (const c of [base, ann]) {
    if (c.width !== W || c.height !== H) {
      c.width = W;
      c.height = H;
    }
    c.style.width = css(W);
    c.style.height = css(H);
  }
  const id = S.id;
  const t0 = performance.now();
  let buf = S.shared ? await sharedFrame(id, 800) : null;
  const via = buf ? "shared" : "ipc";
  if (!buf) {
    try {
      buf = await invoke("capture_frame", { id });
    } catch (err) {
      console.error(err);
      return cancel();
    }
  }
  if (!S || S.id !== id) return;
  const t1 = performance.now();
  try {
    pix = new Uint8ClampedArray(buf);
    try {
      bctx.putImageData(new ImageData(pix, W, H), 0, 0);
    } catch {
      // Some WebView2 versions refuse an ImageData over shared memory:
      // copy it into an ordinary array first.
      pix = new Uint8ClampedArray(pix);
      bctx.putImageData(new ImageData(pix, W, H), 0, 0);
    }
  } catch (err) {
    console.error(err);
    return cancel(`draw ${via} ${W}x${H}: ${err && err.message ? err.message : err}`);
  }
  const t2 = performance.now();
  actx.clearRect(0, 0, W, H);
  mode = "detect";
  if (S.preset) {
    // Annotating a pin: its pixels go where the pin was, already selected.
    const [px, py, pw, ph] = S.preset.rect;
    try {
      const pb = await invoke("capture_preset", { id });
      const tmp = document.createElement("canvas");
      tmp.width = pw;
      tmp.height = ph;
      tmp.getContext("2d").putImageData(new ImageData(new Uint8ClampedArray(pb), pw, ph), 0, 0);
      bctx.drawImage(tmp, px - OX, py - OY);
      pix = bctx.getImageData(0, 0, W, H).data;
      sel = clip({ x: px - OX, y: py - OY, w: pw, h: ph });
      mode = sel.w > 0 && sel.h > 0 ? "selected" : "detect";
    } catch (err) {
      console.error(err);
    }
  }
  await invoke("capture_ready", { timing: `${via} ${Math.round(t1 - t0)} ms, draw ${Math.round(t2 - t1)} ms` });
  // The window may have been rescaled to another monitor's DPI while it
  // was being placed; everything is in physical pixels, so just re-read.
  if (Math.abs(window.devicePixelRatio - dpr) > 0.01) {
    dpr = window.devicePixelRatio;
    for (const c of [base, ann]) {
      c.style.width = css(W);
      c.style.height = css(H);
    }
  }
  await frames(2);
  invoke("capture_shown");
  if (S && S.id === id) {
    // Where the pointer is right now, before the first mouse move.
    const pos = await invoke("capture_cursor").catch(() => null);
    if (pos) cursor = { x: clamp(pos[0] - OX, 0, W - 1), y: clamp(pos[1] - OY, 0, H - 1) };
    updateHover();
  }
}
