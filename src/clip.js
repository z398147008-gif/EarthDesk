// 剪贴板历史：Alt+V 弹出面板（mode=panel）和管理窗口（mode=manager）。
//
// Panel: type to search, ↑↓ to choose, Enter pastes into the window that
// was in front (Shift+Enter as plain text), Ctrl+1…9 pastes that entry,
// Esc or clicking elsewhere closes.
// Manager: browse, search, see the whole entry, copy, star, pin to the
// screen, delete, clear.

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (s) => document.querySelector(s);

const MODE = new URLSearchParams(location.search).get("mode") === "manager" ? "manager" : "panel";
document.body.classList.add(MODE);

const listEl = $("#list");
const q = $("#q");
const detail = $("#detail");
const PAGE = 60;

let items = [];
let total = 0;
let sel = 0;
let kind = "";
let loading = false;
let done = false;
const thumbs = new Map(); // id -> object URL

const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
const ICON = {
  text: '<svg viewBox="0 0 24 24"><path d="M5 6h14M5 11h14M5 16h9"/></svg>',
  files: '<svg viewBox="0 0 24 24"><path d="M4 7a2 2 0 0 1 2-2h4l2 2h6a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2z"/></svg>',
  star: '<svg viewBox="0 0 24 24"><path d="m12 4 2.4 5 5.3.6-4 3.6 1.1 5.3L12 15.9 7.2 18.5l1.1-5.3-4-3.6L9.6 9z"/></svg>',
  pin: '<svg viewBox="0 0 24 24"><path d="M9 4h6l-1 5 3 3H7l3-3z"/><path d="M12 12v8"/></svg>',
  del: '<svg viewBox="0 0 24 24"><path d="M5 7h14M10 7V5h4v2M7 7l1 12h8l1-12"/></svg>',
};

function ago(ms) {
  const d = Date.now() - ms;
  const t = new Date(ms);
  const hm = `${String(t.getHours()).padStart(2, "0")}:${String(t.getMinutes()).padStart(2, "0")}`;
  if (d < 60_000) return "刚刚";
  if (d < 3_600_000) return `${Math.floor(d / 60_000)} 分钟前`;
  const today = new Date();
  today.setHours(0, 0, 0, 0);
  if (ms >= today.getTime()) return `今天 ${hm}`;
  if (ms >= today.getTime() - 86_400_000) return `昨天 ${hm}`;
  return `${t.getMonth() + 1}-${String(t.getDate()).padStart(2, "0")} ${hm}`;
}

function appName(exe) {
  return exe ? exe.replace(/\.exe$/i, "") : "";
}

function toast(text) {
  const t = $("#toast");
  t.textContent = text;
  t.classList.add("show");
  clearTimeout(t._h);
  t._h = setTimeout(() => t.classList.remove("show"), 1500);
}

// --- loading --------------------------------------------------------------------

let loadSeq = 0;
async function reload(keepId) {
  const seq = ++loadSeq;
  loading = true;
  const r = await invoke("clip_list", { query: q.value, kind, offset: 0, limit: PAGE }).catch((e) => (toast(String(e)), null));
  if (seq !== loadSeq) return;
  loading = false;
  if (!r) return;
  items = r.items;
  total = r.total;
  done = items.length < PAGE;
  sel = 0;
  if (keepId != null) {
    const i = items.findIndex((x) => x.id === keepId);
    if (i >= 0) sel = i;
  }
  render();
}

async function more() {
  if (loading || done) return;
  loading = true;
  const seq = loadSeq;
  const r = await invoke("clip_list", { query: q.value, kind, offset: items.length, limit: PAGE }).catch(() => null);
  loading = false;
  if (!r || seq !== loadSeq) return;
  items = items.concat(r.items);
  done = r.items.length < PAGE;
  render(false);
}

listEl.addEventListener("scroll", () => {
  if (listEl.scrollTop + listEl.clientHeight > listEl.scrollHeight - 200) more();
});

// Thumbnails load as rows scroll into view.
const seen = new IntersectionObserver((entries) => {
  for (const e of entries) {
    if (!e.isIntersecting) continue;
    seen.unobserve(e.target);
    const id = Number(e.target.dataset.id);
    loadThumb(id).then((url) => url && (e.target.src = url));
  }
});

async function loadThumb(id, full = false) {
  const key = full ? `f${id}` : id;
  if (thumbs.has(key)) return thumbs.get(key);
  const bytes = await invoke("clip_thumb", { id, full }).catch(() => null);
  if (!bytes) return null;
  const url = URL.createObjectURL(new Blob([bytes], { type: "image/png" }));
  thumbs.set(key, url);
  return url;
}

// --- rendering ----------------------------------------------------------------------

function row(it, i) {
  const li = document.createElement("li");
  li.className = `k-${it.kind} ${i === sel ? "sel" : ""}`;
  li.dataset.index = i;
  let body;
  if (it.kind === "image") {
    body = `<div class="text">图片 ${it.width} × ${it.height}</div>`;
  } else if (it.kind === "files") {
    const names = it.preview.split("\n").map((p) => p.split("\\").pop());
    body = `<div class="text">${esc(names.slice(0, 3).join("、"))}${it.chars > 3 ? ` 等 ${it.chars} 个` : ""}</div>`;
  } else {
    body = `<div class="text">${esc(it.preview.replace(/^\s+/, "").slice(0, 300))}</div>`;
  }
  const icon = it.kind === "image" ? `<img data-id="${it.id}" alt="">` : ICON[it.kind] || ICON.text;
  const n = MODE === "panel" && i < 9 ? `<span class="n">${i + 1}</span>` : "";
  const size = it.kind === "text" && it.chars > 300 ? `<span>${it.chars} 字</span>` : "";
  li.innerHTML = `
    ${it.pinned ? '<i class="pinmark"></i>' : ""}
    <div class="icon">${icon}</div>
    <div class="body">${body}<div class="meta">${n}<span>${esc(appName(it.app))}</span><span>${ago(it.used)}</span>${size}</div></div>
    <div class="actions">
      <button data-act="star" class="${it.pinned ? "star" : ""}" title="${it.pinned ? "取消收藏" : "收藏（不会被自动清理）"}">${ICON.star}</button>
      <button data-act="pin" title="贴到屏幕">${ICON.pin}</button>
      <button data-act="del" title="删除">${ICON.del}</button>
    </div>`;
  const img = li.querySelector("img");
  if (img) {
    const cached = thumbs.get(it.id);
    if (cached) img.src = cached;
    else seen.observe(img);
  }
  return li;
}

function render(scroll = true) {
  listEl.replaceChildren();
  if (!items.length) {
    const li = document.createElement("li");
    li.className = "empty";
    li.textContent = q.value ? "没有找到" : kind === "pinned" ? "还没有收藏" : "复制点什么，这里就会有记录";
    listEl.append(li);
  }
  items.forEach((it, i) => listEl.append(row(it, i)));
  if (scroll) listEl.children[sel]?.scrollIntoView({ block: "nearest" });
  if (MODE === "manager") showDetail();
}

function select(i) {
  if (!items.length) return;
  sel = Math.max(0, Math.min(items.length - 1, i));
  for (const li of listEl.children) li.classList.toggle("sel", Number(li.dataset.index) === sel);
  listEl.children[sel]?.scrollIntoView({ block: "nearest" });
  if (sel > items.length - 10) more();
  if (MODE === "manager") showDetail();
}

let detailSeq = 0;
async function showDetail() {
  const it = items[sel];
  const seq = ++detailSeq;
  if (!it) {
    detail.innerHTML = `<div class="none">选择左边的一条记录</div>`;
    return;
  }
  const full = await invoke("clip_get", { id: it.id }).catch(() => null);
  if (seq !== detailSeq || !full) return;
  const info = it.kind === "image" ? `图片 ${full.width} × ${full.height}` : it.kind === "files" ? `${full.files.length} 个文件` : `${it.chars} 字`;
  detail.innerHTML = `
    <div class="bar">
      <span class="info">${esc(info)} · ${esc(appName(it.app))} · ${ago(it.used)}</span>
      <button class="act" data-act="copy">复制</button>
      ${full.has_html ? '<button class="act" data-act="plain">复制为纯文本</button>' : ""}
      <button class="act" data-act="pin">贴到屏幕</button>
      <button class="act" data-act="star">${it.pinned ? "取消收藏" : "收藏"}</button>
      <button class="act danger" data-act="del">删除</button>
    </div>
    <div class="content"></div>`;
  const c = detail.querySelector(".content");
  if (it.kind === "image") {
    const img = document.createElement("img");
    c.append(img);
    loadThumb(it.id, true).then((u) => u && (img.src = u));
  } else if (it.kind === "files") {
    c.innerHTML = `<ul>${full.files.map((f) => `<li>${esc(f)}</li>`).join("")}</ul>`;
  } else {
    const pre = document.createElement("pre");
    pre.textContent = full.text;
    c.append(pre);
  }
}

detail?.addEventListener("click", (e) => {
  const b = e.target.closest("button[data-act]");
  if (b) act(b.dataset.act, items[sel]);
});

// --- actions -------------------------------------------------------------------------

async function act(what, it) {
  if (!it) return;
  switch (what) {
    case "paste":
    case "paste-plain":
      if (MODE === "panel") return invoke("clip_paste", { id: it.id, plain: what === "paste-plain" }).catch((e) => toast(String(e)));
      return act(what === "paste" ? "copy" : "plain", it);
    case "copy":
    case "plain":
      await invoke("clip_copy", { id: it.id, plain: what === "plain" }).catch((e) => toast(String(e)));
      return toast("已复制");
    case "star":
      await invoke("clip_set_pinned", { id: it.id, on: !it.pinned });
      it.pinned = !it.pinned;
      return render(false);
    case "pin":
      return invoke("clip_pin", { id: it.id }).catch((e) => toast(String(e)));
    case "del": {
      await invoke("clip_delete", { id: it.id });
      items.splice(items.indexOf(it), 1);
      sel = Math.min(sel, items.length - 1);
      return render(false);
    }
  }
}

listEl.addEventListener("click", (e) => {
  const li = e.target.closest("li[data-index]");
  if (!li) return;
  const i = Number(li.dataset.index);
  const b = e.target.closest("button[data-act]");
  if (b) {
    e.stopPropagation();
    return act(b.dataset.act, items[i]);
  }
  if (MODE === "panel") return act("paste", items[i]);
  select(i);
});
listEl.addEventListener("dblclick", (e) => {
  const li = e.target.closest("li[data-index]");
  if (li && MODE === "manager" && !e.target.closest("button")) act("copy", items[Number(li.dataset.index)]);
});

let typing = 0;
q.addEventListener("input", () => {
  clearTimeout(typing);
  typing = setTimeout(() => reload(), 120);
});

$("#kinds").addEventListener("click", (e) => {
  const b = e.target.closest("button[data-kind]");
  if (!b) return;
  kind = b.dataset.kind;
  for (const x of document.querySelectorAll("#kinds button")) x.classList.toggle("on", x === b);
  reload();
  q.focus();
});

document.addEventListener("keydown", (e) => {
  const k = e.key;
  const ctrl = e.ctrlKey || e.metaKey;
  if (k === "Escape") {
    if (q.value) {
      q.value = "";
      reload();
      return;
    }
    if (MODE === "panel") invoke("clip_hide_panel");
    return;
  }
  if (k === "ArrowDown") return (e.preventDefault(), select(sel + 1));
  if (k === "ArrowUp") return (e.preventDefault(), select(sel - 1));
  if (k === "PageDown") return (e.preventDefault(), select(sel + 8));
  if (k === "PageUp") return (e.preventDefault(), select(sel - 8));
  if (k === "Enter") return (e.preventDefault(), act(e.shiftKey ? "paste-plain" : "paste", items[sel]));
  if (ctrl && /^[1-9]$/.test(k) && MODE === "panel") return (e.preventDefault(), act("paste", items[Number(k) - 1]));
  if (ctrl && (k === "p" || k === "P")) return (e.preventDefault(), act("star", items[sel]));
  if (ctrl && (k === "t" || k === "T")) return (e.preventDefault(), act("pin", items[sel]));
  if (ctrl && (k === "c" || k === "C") && document.activeElement !== q && !getSelection().toString()) return act("copy", items[sel]);
  if (k === "Delete" && (!q.value || ctrl)) return (e.preventDefault(), act("del", items[sel]));
  // Any printable key goes to the search box.
  if (!ctrl && k.length === 1 && document.activeElement !== q) q.focus();
});

$("#manage")?.addEventListener("click", () => invoke("clip_open_manager"));
$("#openFolder")?.addEventListener("click", () => invoke("clip_open_folder"));
$("#clearAll")?.addEventListener("click", async () => {
  if (!confirm("清空所有未收藏的剪贴板记录？")) return;
  const n = await invoke("clip_clear").catch(() => 0);
  toast(`已清空 ${n} 条`);
  reload();
});

// --- life cycle -----------------------------------------------------------------------

function opened() {
  q.value = "";
  kind = "";
  for (const x of document.querySelectorAll("#kinds button")) x.classList.toggle("on", x.dataset.kind === "");
  reload();
  requestAnimationFrame(() => q.focus());
}

listen("clip:open", opened);
listen("clip:changed", () => {
  if (document.visibilityState === "visible") reload(items[sel]?.id);
});

if (MODE === "panel") {
  // Clicking anywhere else closes the panel, like a menu.
  window.addEventListener("blur", () => {
    setTimeout(() => {
      if (!document.hasFocus()) invoke("clip_hide_panel");
    }, 60);
  });
}

reload();
q.focus();
