// 设置页的「手势」「快捷键」两栏（以及之后的截图、剪贴板）。
//
// The whole of toolkit.json is held here as `tk`; every change edits it and
// sends it back with toolkit_set (debounced). Rust validates, normalises and
// re-reads it into the hooks at once.

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

let tk = null; // toolkit.json
let meta = { elevated: false, task: false, dev: false, builtins: [] };

const $ = (sel, root = document) => root.querySelector(sel);
const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);

// --- tabs --------------------------------------------------------------------

const TAB_KEY = "settings.tab";
function showTab(name) {
  for (const b of document.querySelectorAll("#tabs button")) b.classList.toggle("on", b.dataset.tab === name);
  for (const t of document.querySelectorAll(".tab")) t.hidden = t.dataset.tab !== name;
  try { localStorage.setItem(TAB_KEY, name); } catch {}
  window.scrollTo(0, 0);
}
document.getElementById("tabs").addEventListener("click", (e) => {
  const b = e.target.closest("button[data-tab]");
  if (b) showTab(b.dataset.tab);
});
try {
  const t = localStorage.getItem(TAB_KEY);
  if (t && document.querySelector(`.tab[data-tab="${t}"]`)) showTab(t);
} catch {}

// --- saving ------------------------------------------------------------------

let saveTimer = null;
let ownSaves = 0; // toolkit:changed events that are echoes of our own saves
function save(now = false) {
  clearTimeout(saveTimer);
  const go = async () => {
    try {
      ownSaves++;
      await invoke("toolkit_set", { config: tk });
    } catch (e) {
      ownSaves = Math.max(0, ownSaves - 1);
      toast(String(e));
    }
  };
  if (now) return go();
  saveTimer = setTimeout(go, 250);
}

function toast(text) {
  let el = $("#toast");
  if (!el) {
    el = document.createElement("div");
    el.id = "toast";
    document.body.append(el);
  }
  el.textContent = text;
  el.classList.add("show");
  clearTimeout(el._t);
  el._t = setTimeout(() => el.classList.remove("show"), 3200);
}

// --- shared pieces -------------------------------------------------------------

function switchEl(checked, onChange) {
  const label = document.createElement("label");
  label.className = "switch";
  label.innerHTML = `<input type="checkbox" ${checked ? "checked" : ""}/><span class="knob"></span>`;
  label.firstChild.addEventListener("change", (e) => onChange(e.target.checked));
  return label;
}

function row(title, desc, control, cls = "") {
  const r = document.createElement("div");
  r.className = `row ${cls}`;
  r.innerHTML = `<div class="text"><div class="title">${title}</div>${desc ? `<div class="desc">${desc}</div>` : ""}</div>`;
  if (control) r.append(control);
  return r;
}

function slider(value, min, max, step, fmt, onInput) {
  const wrap = document.createElement("div");
  wrap.className = "slider";
  wrap.innerHTML = `<input type="range" min="${min}" max="${max}" step="${step}" value="${value}"/><output>${fmt(value)}</output>`;
  const [input, out] = wrap.children;
  input.addEventListener("input", () => {
    out.textContent = fmt(Number(input.value));
    onInput(Number(input.value));
  });
  return wrap;
}

function card(...rows) {
  const c = document.createElement("div");
  c.className = "card";
  c.append(...rows);
  return c;
}

function group(title, ...children) {
  const g = document.createElement("section");
  g.className = "group";
  if (title) {
    const h = document.createElement("h2");
    h.innerHTML = title;
    g.append(h);
  }
  g.append(...children);
  return g;
}

function button(text, onClick, cls = "button") {
  const b = document.createElement("button");
  b.className = cls;
  b.type = "button";
  b.textContent = text;
  b.addEventListener("click", onClick);
  return b;
}

/// An editable list of program names as chips.
function appChips(list, onChange, emptyText = "还没有") {
  const wrap = document.createElement("div");
  wrap.className = "chips apps";
  const draw = () => {
    wrap.replaceChildren();
    if (!list.length) {
      const e = document.createElement("span");
      e.className = "muted";
      e.textContent = emptyText;
      wrap.append(e);
    }
    list.forEach((exe, i) => {
      const c = document.createElement("span");
      c.className = "chip removable";
      c.innerHTML = `${esc(exe)}<button type="button" title="移除">×</button>`;
      c.lastChild.addEventListener("click", () => {
        list.splice(i, 1);
        draw();
        onChange();
      });
      wrap.append(c);
    });
    wrap.append(
      button("＋ 添加程序", async () => {
        const picked = await pickApps(list);
        for (const p of picked) if (!list.includes(p)) list.push(p);
        draw();
        onChange();
      }, "chip add")
    );
  };
  draw();
  return wrap;
}

// --- directions ----------------------------------------------------------------

const ARROWS = [["→", 0], ["↘", 45], ["↓", 90], ["↙", 135], ["←", 180], ["↖", 225], ["↑", 270], ["↗", 315]];

/// Same algorithm as src-tauri/src/toolkit/gesture.rs, so what the pad shows
/// is what the real gesture will be recognised as.
class Recognizer {
  constructor(start, scale, diagonals) {
    this.step = 6 * scale;
    this.minSeg = 26 * scale;
    this.diagonals = diagonals;
    this.anchor = start;
    this.pending = null;
    this.pendingLen = 0;
    this.dirs = [];
  }
  quantise(dx, dy) {
    let a = (Math.atan2(dy, dx) * 180) / Math.PI;
    if (a < 0) a += 360;
    if (!this.diagonals) return ["→", "↓", "←", "↑"][Math.min(3, Math.floor(((a + 45) % 360) / 90))];
    for (const [arrow, centre] of ARROWS) {
      const half = centre % 90 === 0 ? 30 : 15;
      let d = Math.abs(a - centre);
      if (d > 180) d = 360 - d;
      if (d <= half) return arrow;
    }
    return "→";
  }
  push(p) {
    const dx = p[0] - this.anchor[0];
    const dy = p[1] - this.anchor[1];
    const len = Math.hypot(dx, dy);
    if (len < this.step) return false;
    this.anchor = p;
    const dir = this.quantise(dx, dy);
    if (this.dirs[this.dirs.length - 1] === dir) {
      this.pending = null;
      this.pendingLen = 0;
      return false;
    }
    if (this.pending === dir) this.pendingLen += len;
    else {
      this.pending = dir;
      this.pendingLen = len;
    }
    if (this.pendingLen >= this.minSeg) {
      this.dirs.push(dir);
      this.pending = null;
      this.pendingLen = 0;
      return true;
    }
    return false;
  }
  get gesture() {
    return this.dirs.join("");
  }
}

// --- actions -------------------------------------------------------------------

const ACTION_TYPES = [
  ["keys", "按键"],
  ["run", "启动程序"],
  ["open", "打开网址 / 文件"],
  ["text", "输入文字"],
  ["builtin", "内置功能"],
  ["none", "不做任何事"],
];

function builtinName(key) {
  return meta.builtins.find((b) => b.key === key)?.name ?? key;
}

function describeAction(a) {
  switch (a?.type) {
    case "keys": return `按键 ${a.keys}`;
    case "run": return `启动 ${fileName(a.path)}${a.admin ? "（管理员）" : ""}`;
    case "open": return `打开 ${a.target}`;
    case "text": return `输入「${a.text.length > 18 ? a.text.slice(0, 18) + "…" : a.text}」`;
    case "builtin": return builtinName(a.name);
    case "none": return "屏蔽（不做任何事）";
    default: return "";
  }
}

function fileName(p) {
  return String(p || "").split(/[\\/]/).pop();
}

function describeScope(s) {
  if (!s || s.mode === "global" || !s.apps?.length) return "所有程序";
  const names = s.apps.slice(0, 2).join("、") + (s.apps.length > 2 ? ` 等 ${s.apps.length} 个程序` : "");
  return s.mode === "only" ? `仅 ${names}` : `除 ${names} 外`;
}

// --- key recording -------------------------------------------------------------

let recording = null; // { resolve }
listen("toolkit:recorded", (e) => {
  if (!recording) return;
  const r = recording;
  recording = null;
  r.resolve(e.payload);
});

/// Ask the hook for the next key combination the user presses. Resolves to
/// "Ctrl+Alt+T", or "" when they pressed Esc.
function recordKeys() {
  if (recording) recording.resolve(null);
  return new Promise((resolve) => {
    recording = { resolve };
    invoke("toolkit_record", { on: true }).catch(() => resolve(null));
  });
}

function stopRecording() {
  if (recording) {
    recording.resolve(null);
    recording = null;
  }
  invoke("toolkit_record", { on: false }).catch(() => {});
}

function keyCaps(text) {
  if (!text) return `<span class="muted">未设置</span>`;
  return text
    .split(" ")
    .map((combo) => combo.split("+").filter(Boolean).map((k) => `<kbd>${esc(k)}</kbd>`).join("<i>+</i>"))
    .join(`<i class="then">，</i>`);
}

/// A field that records combinations when clicked.
function keyField(value, onChange, { multi = false } = {}) {
  const wrap = document.createElement("div");
  wrap.className = "keyfield";
  let keys = value || "";
  const show = document.createElement("button");
  show.type = "button";
  show.className = "keys";
  const draw = (state) => {
    show.innerHTML = state === "rec" ? `<span class="rec">请按下组合键…（Esc 取消）</span>` : keyCaps(keys);
    show.classList.toggle("recording", state === "rec");
  };
  const record = async (append) => {
    draw("rec");
    const got = await recordKeys();
    if (got) {
      keys = append && keys ? `${keys} ${got}` : got;
      onChange(keys);
    }
    draw();
  };
  show.addEventListener("click", () => record(false));
  wrap.append(show);
  if (multi) {
    wrap.append(button("再加一组", () => record(true), "button small"));
    wrap.append(
      button("清空", () => {
        keys = "";
        onChange(keys);
        draw();
      }, "button small plain")
    );
  }
  draw();
  return wrap;
}

// --- app picker ------------------------------------------------------------------

async function pickApps(already) {
  const dlg = $("#appDialog");
  const list = $("#appList", dlg);
  const manual = $("#appManual", dlg);
  list.innerHTML = `<div class="muted pad">正在读取…</div>`;
  manual.value = "";
  const picked = new Set();
  dlg.showModal();
  const apps = await invoke("toolkit_running_apps").catch(() => []);
  list.replaceChildren();
  if (!apps.length) list.innerHTML = `<div class="muted pad">没有找到打开着的程序，可以在下面手动输入。</div>`;
  for (const a of apps) {
    const item = document.createElement("label");
    item.className = "app-item";
    const dis = already.includes(a.exe);
    item.innerHTML = `<input type="checkbox" ${dis ? "checked disabled" : ""}/><div><div class="title">${esc(a.exe)}</div><div class="desc">${esc(a.title)}</div></div>`;
    item.firstChild.addEventListener("change", (e) => (e.target.checked ? picked.add(a.exe) : picked.delete(a.exe)));
    list.append(item);
  }
  return new Promise((resolve) => {
    dlg.onclose = () => {
      if (dlg.returnValue !== "ok") return resolve([]);
      for (const m of manual.value.split(/[\s,，;；]+/)) {
        let v = m.trim().toLowerCase();
        if (!v) continue;
        if (!v.includes(".")) v += ".exe";
        picked.add(v);
      }
      resolve([...picked]);
    };
  });
}

// --- the rule editor ---------------------------------------------------------------

function uid() {
  return `user.${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
}

function blankAction(type) {
  switch (type) {
    case "keys": return { type, keys: "" };
    case "run": return { type, path: "", args: "", cwd: "", admin: false };
    case "open": return { type, target: "" };
    case "text": return { type, text: "" };
    case "builtin": return { type, name: meta.builtins[0]?.key ?? "capture" };
    default: return { type: "none" };
  }
}

function actionEditor(action, onChange) {
  const host = document.createElement("div");
  host.className = "action-editor";
  const seg = document.createElement("div");
  seg.className = "segmented wrap";
  const body = document.createElement("div");
  body.className = "action-body";
  host.append(seg, body);

  const field = (label, input) => {
    const f = document.createElement("label");
    f.className = "field";
    f.innerHTML = `<span>${label}</span>`;
    f.append(input);
    return f;
  };
  const text = (value, placeholder, onInput) => {
    const i = document.createElement("input");
    i.type = "text";
    i.value = value ?? "";
    i.placeholder = placeholder ?? "";
    i.spellcheck = false;
    i.addEventListener("input", () => onInput(i.value));
    return i;
  };

  const drawBody = () => {
    body.replaceChildren();
    const a = action;
    if (a.type === "keys") {
      body.append(field("按下", keyField(a.keys, (k) => { a.keys = k; onChange(); }, { multi: true })));
      body.append(hint("点上面的框，再按组合键。可以录多组，会依次按下（例如 Ctrl+K 然后 Ctrl+C）。"));
    } else if (a.type === "run") {
      const path = text(a.path, "C:\\Program Files\\…\\app.exe 或 wt.exe", (v) => { a.path = v; onChange(); });
      const browse = button("浏览…", async () => {
        const p = await invoke("toolkit_pick_file").catch(() => null);
        if (p) {
          path.value = p;
          a.path = p;
          onChange();
        }
      }, "button small");
      const line = document.createElement("div");
      line.className = "inline";
      line.append(path, browse);
      body.append(field("程序", line));
      body.append(field("参数", text(a.args, "可不填", (v) => { a.args = v; onChange(); })));
      body.append(field("起始位置", text(a.cwd, "可不填，默认是程序所在文件夹", (v) => { a.cwd = v; onChange(); })));
      const admin = document.createElement("label");
      admin.className = "check";
      admin.innerHTML = `<input type="checkbox" ${a.admin ? "checked" : ""}/> 以管理员身份运行`;
      admin.firstChild.addEventListener("change", (e) => { a.admin = e.target.checked; onChange(); });
      body.append(admin);
      body.append(hint("不勾选时，程序按普通权限启动（即使地球桌面本身是管理员身份）。路径里可以用 %USERPROFILE% 这类环境变量。"));
    } else if (a.type === "open") {
      const t = text(a.target, "https://… 或 D:\\资料 或某个文件", (v) => { a.target = v; onChange(); });
      const browse = button("浏览…", async () => {
        const p = await invoke("toolkit_pick_file").catch(() => null);
        if (p) {
          t.value = p;
          a.target = p;
          onChange();
        }
      }, "button small");
      const line = document.createElement("div");
      line.className = "inline";
      line.append(t, browse);
      body.append(field("打开", line));
      body.append(hint("网址用默认浏览器打开，文件夹用资源管理器，文件用它的默认程序。"));
    } else if (a.type === "text") {
      const ta = document.createElement("textarea");
      ta.rows = 4;
      ta.value = a.text ?? "";
      ta.addEventListener("input", () => { a.text = ta.value; onChange(); });
      body.append(field("文字", ta));
      body.append(hint("在当前光标处输入这段文字，支持多行和中文。"));
    } else if (a.type === "builtin") {
      const sel = document.createElement("select");
      const groups = {};
      for (const b of meta.builtins) (groups[b.group] ||= []).push(b);
      for (const [g, items] of Object.entries(groups)) {
        const og = document.createElement("optgroup");
        og.label = g;
        for (const b of items) {
          const o = document.createElement("option");
          o.value = b.key;
          o.textContent = b.name;
          og.append(o);
        }
        sel.append(og);
      }
      sel.value = a.name;
      sel.addEventListener("change", () => { a.name = sel.value; onChange(); });
      body.append(field("功能", sel));
      body.append(hint("窗口类功能作用于手势起点下的窗口；快捷键作用于当前窗口。"));
    } else {
      body.append(hint("用来在某些程序里屏蔽一个全局手势 / 快捷键：给这条规则设「仅在这些程序」，它会优先于全局规则。"));
    }
  };

  const drawSeg = () => {
    seg.replaceChildren(
      ...ACTION_TYPES.map(([type, label]) => {
        const b = button(label, () => {
          if (action.type === type) return;
          const fresh = blankAction(type);
          for (const k of Object.keys(action)) delete action[k];
          Object.assign(action, fresh);
          drawSeg();
          drawBody();
          onChange();
        }, "");
        b.classList.toggle("on", action.type === type);
        return b;
      })
    );
  };
  drawSeg();
  drawBody();
  return host;
}

function hint(text) {
  const d = document.createElement("div");
  d.className = "hint";
  d.textContent = text;
  return d;
}

function scopeEditor(scope, onChange) {
  const host = document.createElement("div");
  host.className = "scope-editor";
  const seg = document.createElement("div");
  seg.className = "segmented";
  const apps = document.createElement("div");
  const draw = () => {
    seg.replaceChildren(
      ...[["global", "所有程序"], ["only", "仅在这些程序"], ["except", "除了这些程序"]].map(([m, label]) => {
        const b = button(label, () => {
          scope.mode = m;
          draw();
          onChange();
        }, "");
        b.classList.toggle("on", scope.mode === m);
        return b;
      })
    );
    apps.replaceChildren();
    apps.hidden = scope.mode === "global";
    if (!apps.hidden) apps.append(appChips(scope.apps, onChange, "还没有选择程序"));
  };
  host.append(seg, apps);
  draw();
  return host;
}

function gesturePad(value, diagonals, onChange) {
  const host = document.createElement("div");
  host.className = "gesture-pad";
  host.innerHTML = `
    <canvas></canvas>
    <div class="pad-text"><span class="big"></span><span class="tip">按住鼠标右键（或左键）在这里画</span></div>`;
  const canvas = host.querySelector("canvas");
  const big = host.querySelector(".big");
  const tip = host.querySelector(".tip");
  const pads = document.createElement("div");
  pads.className = "arrow-buttons";
  let gesture = value || "";
  const draw = () => {
    big.textContent = gesture || "";
    tip.hidden = !!gesture && !drawing;
  };
  for (const [a] of [["↑"], ["↓"], ["←"], ["→"], ["↖"], ["↗"], ["↙"], ["↘"]]) {
    pads.append(
      button(a, () => {
        if (gesture.slice(-1) === a) return;
        gesture += a;
        onChange(gesture);
        clearCanvas();
        draw();
      }, "arrow")
    );
  }
  pads.append(
    button("⌫", () => {
      gesture = [...gesture].slice(0, -1).join("");
      onChange(gesture);
      draw();
    }, "arrow")
  );

  let drawing = false;
  let rec = null;
  let ctx = null;
  const dpr = () => window.devicePixelRatio || 1;
  const clearCanvas = () => {
    const r = canvas.getBoundingClientRect();
    canvas.width = Math.round(r.width * dpr());
    canvas.height = Math.round(r.height * dpr());
    ctx = canvas.getContext("2d");
    ctx.clearRect(0, 0, canvas.width, canvas.height);
  };
  const pos = (e) => {
    const r = canvas.getBoundingClientRect();
    return [(e.clientX - r.left) * dpr(), (e.clientY - r.top) * dpr()];
  };
  canvas.addEventListener("contextmenu", (e) => e.preventDefault());
  canvas.addEventListener("pointerdown", (e) => {
    if (e.button !== 0 && e.button !== 2) return;
    e.preventDefault();
    canvas.setPointerCapture(e.pointerId);
    clearCanvas();
    drawing = true;
    const p = pos(e);
    // Screen pixels: the real recogniser works in physical pixels and
    // scales its thresholds by the monitor's DPI, i.e. by dpr here.
    rec = new Recognizer(p, dpr(), diagonals);
    ctx.lineCap = ctx.lineJoin = "round";
    ctx.lineWidth = 4 * dpr();
    ctx.strokeStyle = getComputedStyle(host).getPropertyValue("--accent") || "#0a84ff";
    ctx.beginPath();
    ctx.moveTo(...p);
    gesture = "";
    draw();
  });
  canvas.addEventListener("pointermove", (e) => {
    if (!drawing) return;
    const p = pos(e);
    ctx.lineTo(...p);
    ctx.stroke();
    if (rec.push(p)) {
      gesture = rec.gesture;
      draw();
    }
  });
  const finish = () => {
    if (!drawing) return;
    drawing = false;
    gesture = rec.gesture;
    onChange(gesture);
    draw();
  };
  canvas.addEventListener("pointerup", finish);
  canvas.addEventListener("pointercancel", finish);
  requestAnimationFrame(clearCanvas);
  host.append(pads);
  draw();
  return host;
}

/// Open the editor for a gesture or hotkey rule. `rule` null = new.
function editRule(kind, rule) {
  const isNew = !rule;
  const list = kind === "gesture" ? tk.gestures.rules : tk.hotkeys.rules;
  const draft = structuredClone(
    rule ?? (kind === "gesture"
      ? { id: uid(), gesture: "", name: "", action: blankAction("keys"), scope: { mode: "global", apps: [] }, enabled: true }
      : { id: uid(), keys: "", name: "", action: blankAction("run"), scope: { mode: "global", apps: [] }, enabled: true })
  );
  const dlg = $("#ruleDialog");
  const body = $("#ruleBody", dlg);
  const warn = $("#ruleWarn", dlg);
  $("#ruleTitle", dlg).textContent = `${isNew ? "新建" : "编辑"}${kind === "gesture" ? "手势" : "快捷键"}`;
  body.replaceChildren();

  const section = (label, el) => {
    const s = document.createElement("div");
    s.className = "editor-section";
    s.innerHTML = `<div class="label">${label}</div>`;
    s.append(el);
    body.append(s);
  };

  const validate = () => {
    const trigger = kind === "gesture" ? draft.gesture : draft.keys;
    let msg = "";
    if (!trigger) msg = kind === "gesture" ? "请先画一个手势" : "请先录一个组合键";
    else {
      const same = list.filter(
        (r) => r.id !== draft.id && r.enabled && (kind === "gesture" ? r.gesture === draft.gesture : r.keys === draft.keys)
      );
      // A clash at the same specificity matters more than one where the
      // program-specific rule simply wins.
      const only = (s) => s.mode === "only" && s.apps.length > 0;
      const clash =
        same.find((r) => overlaps(r.scope, draft.scope) && only(r.scope) === only(draft.scope)) ??
        same.find((r) => overlaps(r.scope, draft.scope));
      if (clash) {
        const specific = draft.scope.mode === "only" && clash.scope.mode !== "only";
        msg = specific
          ? `与「${clash.name}」相同；在所选程序里以这一条为准。`
          : `与「${clash.name}」（${describeScope(clash.scope)}）重复${clash.scope.mode === "only" ? "，在那些程序里以那一条为准" : "，只会执行排在前面的一条"}。`;
      }
    }
    warn.textContent = msg;
    warn.className = trigger ? "note" : "note bad";
    $("#ruleSave", dlg).disabled = !trigger;
    const a = draft.action;
    // Keys and text would land in this settings window; only offer a
    // trial run for things that make sense from here.
    $("#ruleTry", dlg).hidden = !(
      (a.type === "run" && a.path) ||
      (a.type === "open" && a.target) ||
      (a.type === "builtin" && !a.name.startsWith("window_"))
    );
  };

  if (kind === "gesture") {
    section("手势", gesturePad(draft.gesture, tk.gestures.diagonals, (g) => { draft.gesture = g; validate(); }));
  } else {
    section("快捷键", keyField(draft.keys, (k) => { draft.keys = k.split(" ")[0]; validate(); }));
  }
  const name = document.createElement("input");
  name.type = "text";
  name.value = draft.name;
  name.placeholder = "给它起个名字，例如「打开微信」";
  name.addEventListener("input", () => (draft.name = name.value));
  section("名称", name);
  section("动作", actionEditor(draft.action, validate));
  section("生效范围", scopeEditor(draft.scope, validate));

  $("#ruleDelete", dlg).hidden = isNew;
  const tryBtn = $("#ruleTry", dlg);
  tryBtn.onclick = () => invoke("toolkit_try", { action: draft.action }).catch((e) => toast(String(e)));
  validate();
  dlg.showModal();
  dlg.onclose = () => {
    stopRecording();
    if (dlg.returnValue === "delete") {
      const i = list.findIndex((r) => r.id === draft.id);
      if (i >= 0) list.splice(i, 1);
    } else if (dlg.returnValue === "ok") {
      if (!draft.name.trim()) draft.name = describeAction(draft.action) || "未命名";
      if (draft.scope.mode !== "global" && !draft.scope.apps.length) draft.scope.mode = "global";
      const i = list.findIndex((r) => r.id === draft.id);
      if (i >= 0) list[i] = draft;
      else list.push(draft);
    } else return;
    save(true);
    renderTools();
  };
}

function overlaps(a, b) {
  const g = (s) => s.mode === "global" || !s.apps.length;
  if (g(a) || g(b)) return true;
  if (a.mode === "except" || b.mode === "except") return true;
  return a.apps.some((x) => b.apps.includes(x));
}

// --- rule lists ------------------------------------------------------------------------

function ruleList(kind) {
  const rules = kind === "gesture" ? tk.gestures.rules : tk.hotkeys.rules;
  const c = document.createElement("div");
  c.className = "card rules";
  if (!rules.length) c.append(row(`<span class="muted">还没有规则</span>`, ""));
  // Global rules first, then program-specific ones, keeping the file order
  // inside each group (that order breaks ties).
  const order = [...rules.keys()].sort((a, b) => {
    const ga = rules[a].scope.mode === "only" ? 1 : 0;
    const gb = rules[b].scope.mode === "only" ? 1 : 0;
    return ga - gb || a - b;
  });
  for (const i of order) {
    const r = rules[i];
    const el = document.createElement("div");
    el.className = `row rule ${r.enabled ? "" : "off"}`;
    const trigger = kind === "gesture" ? `<div class="glyph">${esc(r.gesture) || "?"}</div>` : `<div class="combo">${keyCaps(r.keys)}</div>`;
    el.innerHTML = `
      ${trigger}
      <div class="text grow">
        <div class="title">${esc(r.name)}</div>
        <div class="desc">${esc(describeAction(r.action))} · ${esc(describeScope(r.scope))}</div>
      </div>`;
    const sw = switchEl(r.enabled, (on) => {
      r.enabled = on;
      el.classList.toggle("off", !on);
      save();
    });
    el.append(button("编辑", () => editRule(kind, r), "button small"), sw);
    el.querySelector(".text").addEventListener("dblclick", () => editRule(kind, r));
    c.append(el);
  }
  return c;
}

function elevationBanner() {
  if (meta.elevated) return null;
  const d = document.createElement("div");
  d.className = "banner";
  d.innerHTML = meta.dev
    ? `试运行模式没有管理员身份：在任务管理器等以管理员身份运行的窗口上，手势和快捷键不起作用。安装后的版本会自动以管理员身份启动。`
    : tk.elevate
      ? `地球桌面还没有以管理员身份运行，在任务管理器等管理员窗口上手势和快捷键不起作用。退出后重新打开即可。`
      : `「以管理员身份运行」已关闭：在任务管理器等管理员窗口上，手势和快捷键不起作用。可以在「桌面 → 通用」里打开。`;
  return d;
}

function renderGestures() {
  const host = $('.tab[data-tab="gestures"]');
  const g = tk.gestures;
  const parts = [];
  const b = elevationBanner();
  if (b) parts.push(b);

  const color = document.createElement("input");
  color.type = "color";
  color.value = g.trail_color;
  color.addEventListener("input", () => { g.trail_color = color.value; save(); });

  parts.push(
    group(
      "",
      card(
        row("鼠标手势", "按住右键拖动画出方向。只点一下右键时，右键菜单照常弹出", switchEl(g.enabled, (on) => { g.enabled = on; save(); renderTools(); }), "parent"),
        row("显示轨迹", "", switchEl(g.trail, (on) => { g.trail = on; save(); }), "child"),
        row("轨迹颜色", "", color, "child"),
        row("轨迹粗细", "", slider(g.trail_width, 1, 12, 0.5, (v) => `${v} px`, (v) => { g.trail_width = v; save(); }), "child"),
        row("显示提示", "在指针旁显示识别出的手势和要执行的动作", switchEl(g.hint, (on) => { g.hint = on; save(); }), "child"),
        row("识别斜向", "除了上下左右，也识别 ↖ ↗ ↙ ↘", switchEl(g.diagonals, (on) => { g.diagonals = on; save(); }), "child"),
        row("全屏时停用", "全屏游戏、视频、演示时不接管右键", switchEl(g.skip_fullscreen, (on) => { g.skip_fullscreen = on; save(); }), "child"),
        row("起笔距离", "按住右键移动超过这个距离才算手势", slider(g.min_distance, 4, 40, 1, (v) => `${v} px`, (v) => { g.min_distance = v; save(); }), "child"),
        row("按住不动", "按住右键不动超过这个时间，就当普通右键拖动交还给程序", slider(g.hold_ms, 200, 1500, 50, (v) => `${(v / 1000).toFixed(2)} 秒`, (v) => { g.hold_ms = v; save(); }), "child")
      )
    )
  );
  if (!g.enabled) for (const r of parts[parts.length - 1].querySelectorAll(".row.child")) r.classList.add("disabled");

  parts.push(group("不使用手势的程序", card(row("", "在这些程序里右键完全不受影响（远程桌面、虚拟机默认在列）", null), (() => { const r = document.createElement("div"); r.className = "row"; r.append(appChips(g.exclude_apps, () => save())); return r; })())));

  const head = document.createElement("div");
  head.className = "list-head";
  head.innerHTML = `<h2>手势列表</h2>`;
  head.append(button("＋ 新建手势", () => editRule("gesture", null), "button"));
  const list = document.createElement("section");
  list.className = "group";
  list.append(head, ruleList("gesture"));
  list.append(hint("同一个手势可以有多条：写了「仅在这些程序」的那条在对应程序里优先，其余地方用全局的那条。"));
  parts.push(list);
  host.replaceChildren(...parts);
}

function renderHotkeys() {
  const host = $('.tab[data-tab="hotkeys"]');
  const h = tk.hotkeys;
  const parts = [];
  const b = elevationBanner();
  if (b) parts.push(b);
  parts.push(group("", card(row("全局快捷键", "在任何程序里按下组合键执行动作；也可以设成只在某些程序里生效", switchEl(h.enabled, (on) => { h.enabled = on; save(); }), "parent"))));
  parts.push(group("不使用快捷键的程序", card((() => { const r = document.createElement("div"); r.className = "row"; r.append(appChips(h.exclude_apps, () => save())); return r; })())));
  const head = document.createElement("div");
  head.className = "list-head";
  head.innerHTML = `<h2>快捷键列表</h2>`;
  head.append(button("＋ 新建快捷键", () => editRule("hotkey", null), "button"));
  const list = document.createElement("section");
  list.className = "group";
  list.append(head, ruleList("hotkey"));
  list.append(hint("快捷键会被地球桌面接管，不再传给程序本身；想让某个程序保留原来的按键，把它加到上面的列表，或给这条规则设「除了这些程序」。"));
  parts.push(list);
  host.replaceChildren(...parts);
}

// --- 以管理员身份运行 (on the desk tab's 通用 card) ------------------------------------

function renderElevation() {
  const holder = document.getElementById("elevateRow");
  if (!holder) return;
  const desc = meta.elevated
    ? "已以管理员身份运行。手势、快捷键和截图在所有窗口上都能用；启动的程序仍按普通权限运行"
    : meta.dev
      ? "试运行模式下不生效（安装后的版本会自动以管理员身份启动）"
      : "开启后，手势、快捷键和截图在任务管理器等管理员窗口上也能用。开机启动不会弹确认框";
  holder.replaceChildren(
    ...row("以管理员身份运行", desc, switchEl(tk.elevate, async (on) => {
      try {
        const st = await invoke("toolkit_set_elevate", { on });
        meta = { ...meta, ...st };
        tk = st.config;
        if (on && !meta.elevated) toast("已设置，退出地球桌面再打开后生效");
      } catch (e) {
        toast(String(e));
      }
      renderTools();
    })).childNodes
  );
}

function textInput(value, placeholder, onInput, width = "260px") {
  const i = document.createElement("input");
  i.type = "text";
  i.className = "text-input";
  i.style.width = width;
  i.value = value ?? "";
  i.placeholder = placeholder ?? "";
  i.spellcheck = false;
  i.addEventListener("input", () => onInput(i.value));
  return i;
}

/// The hotkey rule that runs a built-in (the first one, if several do).
function builtinHotkey(name, fallbackKeys, label) {
  let r = tk.hotkeys.rules.find((x) => x.action?.type === "builtin" && x.action.name === name);
  if (!r) {
    r = { id: uid(), keys: fallbackKeys, name: label, action: { type: "builtin", name }, scope: { mode: "global", apps: [] }, enabled: true };
    tk.hotkeys.rules.push(r);
  }
  return r;
}

let defaultDir = "";
function renderCapture() {
  const host = $('.tab[data-tab="capture"]');
  const c = tk.capture;
  const parts = [];
  const hk = builtinHotkey("capture", "F1", "截图");
  const keys = keyField(hk.keys, (k) => {
    hk.keys = k.split(" ")[0];
    hk.enabled = true;
    save();
  });
  const pinHk = builtinHotkey("pin_clipboard", "F3", "把剪贴板贴到屏幕");
  const pinKeys = keyField(pinHk.keys, (k) => {
    pinHk.keys = k.split(" ")[0];
    pinHk.enabled = true;
    save();
  });
  parts.push(group("", card(
    row("截图快捷键", "按下后冻结屏幕，拖动或单击选择区域", keys, "parent"),
    row("贴图快捷键", "把剪贴板里的图片或文字贴在屏幕最前面；截图时按它则把选区贴出来", pinKeys, "parent"),
    row("识别窗口和控件", "指针下的窗口、按钮、输入框等自动高亮，单击即可选中；滚轮切换上一层 / 下一层", switchEl(c.detect_elements, (on) => { c.detect_elements = on; save(); }), "child"),
    row("放大镜", "显示指针附近的像素和颜色值，按 C 复制颜色", switchEl(c.magnifier, (on) => { c.magnifier = on; save(); }), "child"),
    row("截取鼠标指针", "把指针也画进截图里", switchEl(c.include_cursor, (on) => { c.include_cursor = on; save(); }), "child"),
    row("截图记录", "保留最近的选区，截图时按 , 和 . 翻回去", slider(c.history, 5, 200, 5, (v) => `${v} 个`, (v) => { c.history = v; save(); }), "child")
  )));

  const dir = textInput(c.save_dir, defaultDir || "图片\\EarthDesk", (v) => { c.save_dir = v.trim(); save(); }, "300px");
  const dirLine = document.createElement("div");
  dirLine.className = "inline";
  dirLine.append(
    dir,
    button("更改…", async () => {
      const p = await invoke("capture_pick_folder", { start: c.save_dir }).catch(() => null);
      if (p) {
        c.save_dir = p;
        dir.value = p;
        save();
      }
    }, "button small"),
    button("打开", () => invoke("toolkit_try", { action: { type: "open", target: c.save_dir || defaultDir } }).catch((e) => toast(String(e))), "button small plain")
  );
  const fmt = document.createElement("div");
  fmt.className = "segmented";
  for (const [v, label] of [["png", "PNG"], ["jpg", "JPG"]]) {
    fmt.append(button(label, () => { c.format = v; save(); renderCapture(); }, c.format === v ? "on" : ""));
  }
  parts.push(group("保存", card(
    row("保存位置", "Ctrl+S 直接存到这里；Ctrl+Shift+S 另存为", dirLine),
    row("文件名", "{yyyy} 年 {MM} 月 {dd} 日 {HH} 时 {mm} 分 {ss} 秒 {ms} 毫秒", textInput(c.file_name, "截图_{yyyy}{MM}{dd}_{HH}{mm}{ss}", (v) => { c.file_name = v; save(); }, "260px")),
    row("格式", "", fmt),
    ...(c.format === "jpg" ? [row("JPG 质量", "", slider(c.jpeg_quality, 50, 100, 1, (v) => `${v}`, (v) => { c.jpeg_quality = v; save(); }), "child")] : []),
    row("保存时也复制", "保存文件的同时放进剪贴板", switchEl(c.copy_on_save, (on) => { c.copy_on_save = on; save(); }))
  )));

  const help = document.createElement("details");
  help.className = "row guide";
  help.innerHTML = `<summary>截图时的按键</summary><ol class="keys-help">
    <li><b>选区</b>：拖动框选；单击选中高亮的窗口 / 控件；滚轮切换外层 / 内层；Tab 切换「只识别窗口」；Ctrl+A 整个屏幕；, . 翻截图记录</li>
    <li><b>微调</b>：方向键移动 1 像素（选区出现前移动指针），Shift+方向键调整大小，加 Ctrl 为 10 像素；拖边和角调整</li>
    <li><b>标注</b>：R 矩形 · E 椭圆 · L 直线 · A 箭头 · P 画笔 · M 马克笔 · X 马赛克 / 模糊 · T 文字 · N 序号 · Q 橡皮擦；按住 Shift 画正方形 / 正圆 / 45° 线；已画的标注可以拖动、拖端点改形状，Delete 删除，双击文字重新编辑</li>
    <li><b>完成</b>：Enter / Ctrl+C / 双击 复制；Ctrl+S 保存；Ctrl+Shift+S 另存为；Ctrl+Z 撤销 · Ctrl+Y 重做</li>
    <li><b>退出</b>：Esc；右键返回上一步（没有选区时退出）</li>
    <li><b>取色</b>：放大镜里按 C 复制颜色值，Shift 切换 HEX / RGB</li>
    <li><b>贴图</b>：拖动移动 · 滚轮缩放 · Ctrl+滚轮 透明度 · 双击缩略 · 1 / 2 旋转 · 3 / 4 翻转 · 空格 标注 · Ctrl+C 复制 · Ctrl+S 保存 · Esc 关闭 · 右键菜单里有鼠标穿透（托盘「贴图」取消）</li></ol>`;
  parts.push(group("", card(help)));
  host.replaceChildren(...parts);
}

function renderClipboard() {
  const host = $('.tab[data-tab="clipboard"]');
  const c = tk.clipboard;
  const hk = builtinHotkey("clipboard_panel", "Alt+V", "剪贴板历史");
  const keys = keyField(hk.keys, (k) => {
    hk.keys = k.split(" ")[0];
    hk.enabled = true;
    save();
  });
  const parts = [];
  const main = card(
    row("记录剪贴板", "复制过的文字、图片和文件都记下来，随时再粘贴", switchEl(c.enabled, (on) => { c.enabled = on; save(); renderClipboard(); }), "parent"),
    row("弹出面板快捷键", "在光标旁弹出历史列表：输入即搜索，↑↓ 选择，Enter 粘贴，Shift+Enter 粘贴纯文本", keys, "child"),
    row("选中后直接粘贴", "关闭时只放进剪贴板，由你自己按 Ctrl+V", switchEl(c.paste_on_pick, (on) => { c.paste_on_pick = on; save(); }), "child"),
    row("记录图片", "截图、复制的图片也记下来（存成 PNG）", switchEl(c.images, (on) => { c.images = on; save(); }), "child"),
    row("最多保留", "超出后最旧的先删除；收藏的不算在内，也不会被删除", slider(c.max_items, 100, 5000, 100, (v) => `${v} 条`, (v) => { c.max_items = v; save(); }), "child"),
    row("保留时间", "", slider(c.max_days, 0, 365, 1, (v) => (v === 0 ? "不限" : `${v} 天`), (v) => { c.max_days = v; save(); }), "child"),
    row("单张图片上限", "超过这个大小的图片不记录", slider(c.max_image_mb, 5, 200, 5, (v) => `${v} MB`, (v) => { c.max_image_mb = v; save(); }), "child")
  );
  if (!c.enabled) for (const r of main.querySelectorAll(".row.child")) r.classList.add("disabled");
  parts.push(group("", main));
  const ex = document.createElement("div");
  ex.className = "row";
  ex.append(appChips(c.exclude_apps, () => save()));
  parts.push(group("不记录这些程序里的复制", card(row("", "密码管理器默认在列。另外，程序自己标记为「不要记录」的内容（多数密码管理器会这样做）一律跳过", null), ex)));
  parts.push(group("", card(
    row("管理窗口", "浏览、搜索全部历史，收藏、删除、贴到屏幕", button("打开", () => invoke("clip_open_manager"), "button")),
    row("数据位置", "%APPDATA%\\EarthDesk\\clipboard（只在这台电脑上，不会上传）", button("打开文件夹", () => invoke("clip_open_folder"), "button plain")),
    row("清空历史", "删除所有未收藏的记录", button("清空", async () => {
      if (!confirm("清空所有未收藏的剪贴板记录？")) return;
      const n = await invoke("clip_clear").catch(() => 0);
      toast(`已清空 ${n} 条`);
    }, "button danger"))
  )));
  host.replaceChildren(...parts);
}

// --- 输入法 ----------------------------------------------------------------------
//
// 地球桌面输入法是独立的程序（引擎 EarthDeskIME.exe + 系统输入法模块）；这一栏
// 只看它的状态、改 settings.json 并让引擎重新部署。

let ime = null; // ime_status()
let imeTimer = null;

function imeSave() {
  clearTimeout(imeTimer);
  imeTimer = setTimeout(async () => {
    try {
      const deployed = await invoke("ime_save", { settings: ime.settings });
      toast(deployed ? "已保存，输入法正在应用（几秒钟）" : "已保存，下次输入法启动时生效");
      setTimeout(loadIme, 1500);
    } catch (e) {
      toast(String(e));
    }
  }, 600);
}

function imeStatusText() {
  if (!ime.dll) return ["还没有安装", "用安装包安装地球桌面时会一起装好（开发时双击仓库里的 5-试用输入法.bat）"];
  const e = ime.engine;
  const no32 = ime.dll32 ? "" : " · 32 位的老程序里暂不可用";
  if (!e) return ["已安装，后台引擎没在运行", "第一次在某个程序里切换到地球桌面输入法时会自动启动，也可以点右边立即启动" + no32];
  if (e.maintaining) return ["正在部署词库…", "第一次约需一分钟；这期间按键照常输入英文"];
  const ja = e.mozc ? ` · 日语引擎 Mozc ${esc(e.mozc)}` : " · 日语引擎没有安装（只能打中文）";
  return ["正常运行", `librime ${esc(e.librime)}${ja} · ${e.clients} 个输入窗口在用${no32}`];
}

function renderIme() {
  const host = $('.tab[data-tab="ime"]');
  if (!host || !ime) return;
  const st = ime.settings;
  const [title, desc] = imeStatusText();
  const actions = document.createElement("div");
  actions.className = "buttons";
  if (ime.dll && !ime.engine) {
    actions.append(button("启动", async () => {
      try { await invoke("ime_start"); } catch (e) { toast(String(e)); }
      setTimeout(loadIme, 1500);
    }, "button"));
  }
  actions.append(button("刷新", loadIme, "button plain"));
  const parts = [];
  parts.push(group("地球桌面输入法", card(
    row(title, desc, actions),
    row("切换到它", "<b>Win + 空格</b> 在输入法之间切换。列表里没有「地球桌面输入法」的话，在 Windows 设置 → 时间和语言 → 语言和区域 → 中文 → 键盘 里添加", button("打开 Windows 设置", () => invoke("ime_open", { what: "keyboards" }).catch((e) => toast(String(e))), "button plain"))
  )));
  const typing = card(
    row("每页候选个数", "", slider(st.page_size, 3, 9, 1, (v) => `${v} 个`, (v) => { st.page_size = v; imeSave(); })),
    row("Shift 切换中 / 英", "单按 Shift：把已经打的字母原样上屏并切到英文，再按一次回到中文", switchEl(st.shift_toggle, (on) => { st.shift_toggle = on; imeSave(); })),
    row("Emoji 候选", "打「xiao」时在候选里给出 😄 之类", switchEl(st.emoji, (on) => { st.emoji = on; imeSave(); })),
    row("中日英混合输入", "不用切换：同一串字母同时按双拼、罗马字和英文单词解析，按词库和上下文把候选排在一起（日语标「日」、英文标「英」，句首英文自动大写）。关掉后默认只打中文，仍可用 /ja 或 Ctrl+Shift+J 切到日语", switchEl(st.mixed !== false, (on) => { st.mixed = on; imeSave(); }))
  );
  parts.push(group("打字", typing));
  const skin = document.createElement("div");
  skin.className = "segmented";
  for (const [v, label] of [["", "跟随系统"], ["weather", "天气"], ["time", "时段"]]) {
    skin.append(button(label, () => { st.skin = v; imeSave(); renderIme(); }, (st.skin || "") === v ? "on" : ""));
  }
  const skinDesc = {
    "": "白色；Windows 用深色模式时跟着变深",
    weather: "候选条画出接下来几个小时的天气：最左边是现在，往右是之后每一小时（晴、云、雨、雪、雾、雷），候选条越长看得越远；角上写着气温和天气。天气跟着「位置」走",
    time: "颜色随一天的时段慢慢变：清晨桃粉、上午清蓝、正午暖白、下午琥珀、傍晚珊瑚到淡紫、夜里浅蓝紫；始终是浅色"
  }[st.skin || ""];
  parts.push(group("外观", card(row("候选窗皮肤", skinDesc, skin))));
  const apps = document.createElement("div");
  apps.className = "row";
  apps.append(appChips(st.ascii_apps, () => imeSave()));
  parts.push(group("这些程序里默认英文", card(row("", "游戏、终端、代码编辑器之类。仍可以按 Shift 临时切回中文", null), apps)));
  parts.push(group("词库", card(
    row("快捷短语", "每行 <code>文字&lt;Tab&gt;编码&lt;Tab&gt;权重</code>，打出编码时它排第一（邮箱、地址、常用句子）。改完点下面的「重新部署」", button("编辑", () => invoke("ime_open", { what: "phrases" }).catch((e) => toast(String(e))), "button plain")),
    row("重新部署", "手动改过词库或快捷短语后，让输入法重新读一遍", button("重新部署", async () => {
      toast((await invoke("ime_deploy").catch(() => false)) ? "正在重新部署…" : "输入法引擎没在运行");
      setTimeout(loadIme, 1500);
    }, "button")),
    row("用户词库", `你打过的词和使用频率都记在这里，只在这台电脑上：${esc(ime.user_dir)}`, button("打开文件夹", () => invoke("ime_open", { what: "userdir" }).catch((e) => toast(String(e))), "button plain")),
    row("日志", "打不出字时把它发给开发者", button("打开", () => invoke("ime_open", { what: "log" }).catch((e) => toast(String(e))), "button plain"))
  )));
  const help = document.createElement("details");
  help.className = "row guide";
  help.open = true;
  help.innerHTML = `<summary>怎么用</summary><ol>
    <li><b>微软双拼</b>；候选按你的使用频率和最近使用自动排序，打过的词组会记住</li>
    <li><b>选词</b>：<b>← →</b> 移动高亮 · <b>↑ ↓</b> 翻页 · 空格上屏高亮的词 · 数字键直接选 · 鼠标点 · <b>- =</b> 或滚轮也能翻页 · <b>[ ]</b> 以词定字（取词的第一个 / 最后一个字）</li>
    <li><b>上屏英文</b>：打字时文字里带下划线的就是你按下的字母（候选窗第一行是它的双拼读音，如 rjhz · ran hou），Enter 原样上屏；词库里没有的英文词也会作为「英」排在候选最后，一样能选；Shift 上屏并切到英文</li>
    <li><b>中日混合</b>：直接按罗马字打日语（watashi → 私），中文、日语、英文候选排在一起；高亮落在日语词上时按 <b>↑ ↓</b> 竖着展开它的其他写法（汉字 / ひらがな / カタカナ …），展开后 <b>↑ ↓</b> 选、<b>← →</b> 翻页、空格上屏、Esc 收回；日语刚上屏后按 <b>\`</b> 在 原样 / ひらがな / カタカナ 之间切换；标点跟着正在打的这句话走：日语句子里 , . 是 、。，中文句子里（夹着英文单词也一样）是 ，。，纯英文句子是 , .</li>
    <li><b>切换</b>：Ctrl+Shift+J 轮换 中日混合 → 日本語 → 中文；或打 <b>/mix</b> <b>/ja</b> <b>/zh</b>（也可以 /hh /ry /zw）空格。每个程序记住自己的选择；F6–F10 日语假名 / 半角 / 英数转换</li>
    <li><b>大写</b>：Caps Lock 打开后字母直接以大写输入；日式键盘上 Shift+逗号键 出 、</li>
    <li><b>更多</b>：F4 或 Ctrl+\` 打开菜单（简繁、全角、Emoji）；按住 Shift 打大写字母开头：V 特殊符号、U 按 Unicode 编码、R 数字转大写金额、N 公历转农历；cC 开头是计算器；打 date / time / week 出当前日期、时间、星期</li></ol>`;
  parts.push(group("", card(help)));
  host.replaceChildren(...parts);
}

async function loadIme() {
  ime = await invoke("ime_status").catch(() => null);
  renderIme();
}

function renderTools() {
  if (!tk) return;
  renderClipboard();
  renderGestures();
  renderHotkeys();
  renderCapture();
  renderElevation();
}

async function load() {
  if (!defaultDir) defaultDir = await invoke("capture_default_dir").catch(() => "");
  const st = await invoke("toolkit_get").catch(() => null);
  if (!st) return;
  tk = st.config;
  meta = st;
  renderTools();
}

listen("toolkit:changed", () => {
  // Our own saves come back here too: skip those (re-rendering would yank a
  // slider out from under the pointer). Otherwise reload, unless a dialog is
  // open -- an edit in progress is never replaced under the user.
  if (ownSaves > 0) {
    ownSaves--;
    return;
  }
  if (!document.querySelector("dialog[open]")) load();
});

load();
loadIme();
