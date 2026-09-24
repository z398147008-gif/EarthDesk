// 贴图窗口。照 Snipaste：拖动移动，滚轮缩放，Ctrl+滚轮调透明度，双击缩略，
// 右键菜单；1/2 旋转，3/4 翻转，空格标注，Ctrl+C 复制，Ctrl+S 保存，Esc 关闭。
//
// `src` holds the picture at its natural size (physical pixels). `view` is
// `src` with rotation/flip applied, still at natural size; zoom is only the
// CSS size of `view`. Copy / save / annotate use `view`, i.e. what is shown,
// at full resolution.

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const win = window.__TAURI__.window.getCurrentWindow();
const MenuApi = window.__TAURI__.menu;

const view = document.getElementById("view");
const vctx = view.getContext("2d");
const frame = document.getElementById("frame");
const badge = document.getElementById("badge");

const PAD = 10; // CSS px of glow around the picture
let src = null; // canvas at natural size
let info = null;
let zoom = 1;
let opacity = 1;
let rot = 0; // quarter turns clockwise
let flipH = false, flipV = false;
let thumb = false;
let through = false;
let anchor = [0, 0]; // screen position of the picture's top-left (physical)
const dpr = () => window.devicePixelRatio || 1;

// --- loading ------------------------------------------------------------------

async function load() {
  info = await invoke("pin_info");
  anchor = info.anchor;
  const bytes = await invoke("pin_bytes");
  src = document.createElement("canvas");
  const c = src.getContext("2d");
  if (info.kind === "rgba") {
    src.width = info.w;
    src.height = info.h;
    c.putImageData(new ImageData(new Uint8ClampedArray(bytes), info.w, info.h), 0, 0);
  } else if (info.kind === "encoded") {
    const bmp = await createImageBitmap(new Blob([bytes], { type: info.mime }));
    src.width = bmp.width;
    src.height = bmp.height;
    c.drawImage(bmp, 0, 0);
  } else {
    renderText(src, new TextDecoder().decode(bytes));
  }
  zoom = 1;
  rot = 0;
  flipH = flipV = false;
  thumb = false;
  redraw();
  layout();
  await invoke("pin_show");
}

/// Copied text becomes a card of text (or a colour swatch for a colour
/// value), drawn at the screen's real pixel density.
function renderText(canvas, text) {
  const s = dpr();
  const c = canvas.getContext("2d");
  const t = text.replace(/\r\n/g, "\n").replace(/\t/g, "    ");
  const color = t.trim().match(/^#?([0-9a-f]{6})$/i) || t.trim().match(/^rgb\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*\)$/i);
  if (color) {
    const css = color.length === 2 ? `#${color[1]}` : `rgb(${color[1]},${color[2]},${color[3]})`;
    canvas.width = Math.round(140 * s);
    canvas.height = Math.round(96 * s);
    c.fillStyle = css;
    c.fillRect(0, 0, canvas.width, canvas.height);
    c.fillStyle = "rgba(0,0,0,0.55)";
    c.fillRect(0, canvas.height - 24 * s, canvas.width, 24 * s);
    c.fillStyle = "#fff";
    c.font = `${12 * s}px "Segoe UI", sans-serif`;
    c.textBaseline = "middle";
    c.fillText(t.trim().toUpperCase(), 8 * s, canvas.height - 12 * s);
    return;
  }
  const size = 14 * s, line = Math.round(size * 1.45), pad = Math.round(12 * s), maxW = 560 * s;
  const font = `${size}px "Microsoft YaHei UI", "Segoe UI", sans-serif`;
  c.font = font;
  const lines = [];
  for (const para of t.split("\n")) {
    let cur = "";
    for (const ch of para) {
      if (c.measureText(cur + ch).width > maxW && cur) {
        lines.push(cur);
        cur = ch;
      } else cur += ch;
    }
    lines.push(cur);
    if (lines.length > 400) break;
  }
  const w = Math.min(maxW, Math.max(...lines.map((l) => c.measureText(l).width), 20 * s));
  canvas.width = Math.ceil(w + pad * 2);
  canvas.height = Math.ceil(lines.length * line + pad * 2 - (line - size));
  c.fillStyle = "#fffef8";
  c.fillRect(0, 0, canvas.width, canvas.height);
  c.fillStyle = "#1c1c1e";
  c.font = font;
  c.textBaseline = "top";
  lines.forEach((l, i) => c.fillText(l, pad, pad + i * line));
}

function redraw() {
  const turned = rot % 2 === 1;
  view.width = turned ? src.height : src.width;
  view.height = turned ? src.width : src.height;
  vctx.save();
  vctx.translate(view.width / 2, view.height / 2);
  vctx.rotate((rot * Math.PI) / 2);
  vctx.scale(flipH ? -1 : 1, flipV ? -1 : 1);
  vctx.drawImage(src, -src.width / 2, -src.height / 2);
  vctx.restore();
}

// --- geometry -------------------------------------------------------------------

function shown() {
  if (thumb) {
    const t = Math.round(72 * dpr());
    return [t, t];
  }
  return [Math.max(1, Math.round(view.width * zoom)), Math.max(1, Math.round(view.height * zoom))];
}

let expected = null;
function layout() {
  const s = dpr();
  const pad = Math.round(PAD * s);
  const [w, h] = shown();
  document.documentElement.style.setProperty("--pad", `${pad / s}px`);
  frame.style.width = `${w / s}px`;
  frame.style.height = `${h / s}px`;
  view.style.objectFit = thumb ? "cover" : "";
  document.body.classList.toggle("pixelated", zoom >= 2 && !thumb);
  frame.style.opacity = String(opacity);
  expected = { x: anchor[0] - pad, y: anchor[1] - pad, w: w + pad * 2, h: h + pad * 2 };
  invoke("pin_geometry", expected);
}

// Moving to a monitor with another scale makes Windows resize the window
// (and changes devicePixelRatio). Put the physical size back.
let fixing = 0;
window.addEventListener("resize", () => {
  if (!expected || dragging) return;
  const s = dpr();
  const w = Math.round(innerWidth * s), h = Math.round(innerHeight * s);
  if (Math.abs(w - expected.w) > 2 || Math.abs(h - expected.h) > 2) {
    const now = performance.now();
    if (now - fixing < 150) return;
    fixing = now;
    layout();
  }
});

win.onMoved(({ payload }) => {
  const pad = Math.round(PAD * dpr());
  anchor = [payload.x + pad, payload.y + pad];
  if (expected) {
    expected.x = payload.x;
    expected.y = payload.y;
  }
  invoke("pin_moved", { x: anchor[0], y: anchor[1] });
});

let badgeTimer = 0;
function flash(text) {
  badge.textContent = text;
  badge.hidden = false;
  clearTimeout(badgeTimer);
  badgeTimer = setTimeout(() => (badge.hidden = true), 900);
}

function setZoom(z, about) {
  const nz = Math.min(10, Math.max(0.1, z));
  if (thumb || Math.abs(nz - zoom) < 1e-6) return;
  // Keep the point under the pointer where it is.
  const [px, py] = about ?? [anchor[0] + (view.width * zoom) / 2, anchor[1] + (view.height * zoom) / 2];
  const k = nz / zoom;
  anchor = [Math.round(px - (px - anchor[0]) * k), Math.round(py - (py - anchor[1]) * k)];
  zoom = nz;
  layout();
  flash(`${Math.round(zoom * 100)}%`);
}

function screenPoint(e) {
  const s = dpr();
  const pad = Math.round(PAD * s);
  return [anchor[0] - pad + Math.round(e.clientX * s), anchor[1] - pad + Math.round(e.clientY * s)];
}

function rotate(dir) {
  const [w0, h0] = shown();
  rot = (rot + dir + 4) % 4;
  redraw();
  // Turn about the centre.
  const [w1, h1] = shown();
  anchor = [anchor[0] + Math.round((w0 - w1) / 2), anchor[1] + Math.round((h0 - h1) / 2)];
  layout();
}

function flip(horizontal) {
  if (horizontal) flipH = !flipH;
  else flipV = !flipV;
  redraw();
}

/// Collapse to a small square (and back), anchored at the top-left.
function toggleThumb() {
  thumb = !thumb;
  layout();
}

async function setThrough(on) {
  through = on;
  document.body.classList.toggle("through", on);
  await win.setIgnoreCursorEvents(on);
}
listen("pin:solid", () => setThrough(false));
listen("pin:reload", () => load());

// --- export -----------------------------------------------------------------------

function pixels() {
  return vctx.getImageData(0, 0, view.width, view.height).data;
}

async function exportAs(op) {
  const d = pixels();
  await invoke("pin_export", new Uint8Array(d.buffer), {
    headers: { "x-op": op, "x-width": String(view.width), "x-height": String(view.height) },
  }).catch((e) => flash(String(e)));
  if (op === "copy") flash("已复制");
}

async function annotate() {
  if (thumb) toggleThumb();
  const d = pixels();
  await invoke("pin_edit", new Uint8Array(d.buffer), {
    headers: { "x-width": String(view.width), "x-height": String(view.height), "x-x": String(anchor[0]), "x-y": String(anchor[1]) },
  }).catch((e) => flash(String(e)));
}

const close = () => invoke("pin_close");

// --- pointer ------------------------------------------------------------------------

let press = null;
let dragging = false;

document.addEventListener("mousedown", (e) => {
  if (e.button === 0) press = { x: e.screenX, y: e.screenY };
});
document.addEventListener("mousemove", async (e) => {
  if (!press || dragging) return;
  if (Math.hypot(e.screenX - press.x, e.screenY - press.y) >= 3) {
    dragging = true;
    press = null;
    try {
      await win.startDragging();
    } finally {
      dragging = false;
    }
  }
});
document.addEventListener("mouseup", () => (press = null));
document.addEventListener("dblclick", (e) => {
  if (e.button === 0) toggleThumb();
});
document.addEventListener(
  "wheel",
  (e) => {
    e.preventDefault();
    const up = e.deltaY < 0;
    if (e.ctrlKey) {
      opacity = Math.min(1, Math.max(0.1, Math.round((opacity + (up ? 0.1 : -0.1)) * 10) / 10));
      frame.style.opacity = String(opacity);
      flash(`不透明度 ${Math.round(opacity * 100)}%`);
      return;
    }
    setZoom(zoom * (up ? 1.1 : 1 / 1.1), screenPoint(e));
  },
  { passive: false }
);

window.addEventListener("focus", () => document.body.classList.add("focus"));
window.addEventListener("blur", () => document.body.classList.remove("focus"));

// --- keyboard -----------------------------------------------------------------------

document.addEventListener("keydown", (e) => {
  const k = e.key;
  const ctrl = e.ctrlKey || e.metaKey;
  if (k === "Escape" || (ctrl && (k === "w" || k === "W"))) return close();
  if (ctrl && (k === "c" || k === "C")) return exportAs("copy");
  if (ctrl && (k === "s" || k === "S")) {
    e.preventDefault();
    return exportAs(e.shiftKey ? "save-as" : "save");
  }
  if (ctrl && (k === "0" || k === "=")) return setZoom(1);
  if (k === "=" || k === "+") return setZoom(zoom * 1.1);
  if (k === "-" || k === "_") return setZoom(zoom / 1.1);
  if (k === "1") return rotate(-1);
  if (k === "2") return rotate(1);
  if (k === "3") return flip(true);
  if (k === "4") return flip(false);
  if (k === " ") {
    e.preventDefault();
    return annotate();
  }
});

// --- menu ----------------------------------------------------------------------------

document.addEventListener("contextmenu", async (e) => {
  e.preventDefault();
  if (!MenuApi) return;
  const { Menu, MenuItem, CheckMenuItem, PredefinedMenuItem, Submenu } = MenuApi;
  // Shortcuts go after a tab: Windows shows that part right-aligned, like
  // an accelerator, without registering anything.
  const item = (text, action, keys) => MenuItem.new({ text: keys ? `${text}\t${keys}` : text, action });
  const sep = () => PredefinedMenuItem.new({ item: "Separator" });
  const menu = await Menu.new({
    items: [
      await item("复制", () => exportAs("copy"), "Ctrl+C"),
      await item("另存为…", () => exportAs("save-as"), "Ctrl+Shift+S"),
      await item("保存到截图文件夹", () => exportAs("save"), "Ctrl+S"),
      await sep(),
      await item("标注", annotate, "空格"),
      await Submenu.new({
        text: "旋转 / 翻转",
        items: [
          await item("向左旋转", () => rotate(-1), "1"),
          await item("向右旋转", () => rotate(1), "2"),
          await item("水平翻转", () => flip(true), "3"),
          await item("垂直翻转", () => flip(false), "4"),
        ],
      }),
      await Submenu.new({
        text: `缩放（${Math.round(zoom * 100)}%）`,
        items: [
          await item("100%", () => setZoom(1), "Ctrl+0"),
          await item("50%", () => setZoom(0.5)),
          await item("200%", () => setZoom(2)),
        ],
      }),
      await Submenu.new({
        text: `不透明度（${Math.round(opacity * 100)}%）`,
        items: await Promise.all(
          [100, 80, 60, 40, 20].map((v) =>
            CheckMenuItem.new({
              text: `${v}%`,
              checked: Math.round(opacity * 100) === v,
              action: () => {
                opacity = v / 100;
                frame.style.opacity = String(opacity);
              },
            })
          )
        ),
      }),
      await CheckMenuItem.new({ text: "缩略图\t双击", checked: thumb, action: toggleThumb }),
      await CheckMenuItem.new({
        text: "鼠标穿透（在托盘菜单「贴图」里取消）",
        checked: through,
        action: () => setThrough(!through),
      }),
      await sep(),
      await item("隐藏所有贴图", () => invoke("pins_toggle_hidden")),
      await item("关闭所有贴图", () => invoke("pins_close_all")),
      await item("关闭", close, "Esc"),
    ],
  });
  await menu.popup();
});

load().catch((e) => {
  console.error(e);
  close();
});
