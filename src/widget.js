const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { getCurrentWindow } = window.__TAURI__.window;

const win = getCurrentWindow();
const label = win.label;

let editing = false;
let drag = null;

/// Every dimension in the widgets is expressed in rem. Pinning the root font
/// size to the window height means dragging the resize handle scales the whole
/// layout instead of reflowing it.
///
/// The design heights are CSS pixels, and the measurement below is in CSS
/// pixels too. That is the whole trick: a window declared 460 logical pixels
/// tall is 690 physical pixels on a display at 150%, but it is still 460 CSS
/// pixels, so the layout comes out the same *logical* size and therefore the
/// same apparent size as everything else on the system.
///
/// (Converting to physical pixels and dividing by devicePixelRatio, as this
/// did at first, cancels the system scale out exactly -- which is why the text
/// came out 1/scale too small on a 4K display at 150%.)
const DESIGN = { weather: [440, 440], perf: [440, 440] }[label] || [440, 440];

let uiScale = 1;
/// Extra shrink a page can ask for when its content outgrows the card (the
/// performance card with a pile of drives plugged in). 1 = none.
let contentFit = 1;

function rescale() {
  // Whichever dimension is tighter wins, so a widget dragged short-and-wide
  // (or tall-and-narrow) shrinks to fit rather than spilling out of its box.
  const fit = Math.min(window.innerWidth / DESIGN[0], window.innerHeight / DESIGN[1]);
  const size = Math.max(7, fit * 16 * uiScale * contentFit);
  document.documentElement.style.fontSize = `${size}px`;
}

rescale();
window.addEventListener("resize", rescale);
window.__widgetFit = (value) => {
  contentFit = Math.min(1, Math.max(0.5, Number(value) || 1));
  rescale();
};
// Moving between monitors with different scaling changes the CSS size without
// firing `resize` on every platform; observing the element catches both.
new ResizeObserver(rescale).observe(document.documentElement);

listen("ui-scale", (event) => {
  const value = Number(event.payload);
  if (value > 0) {
    uiScale = value;
    rescale();
  }
});

invoke("get_ui_scale")
  .then((value) => {
    if (typeof value === "number" && value > 0) {
      uiScale = value;
      rescale();
    }
  })
  .catch(() => {});

// How see-through the card behind the text is (settings page slider).
// Both cards read it as --card-alpha.
function applyCardOpacity(value) {
  const v = Number(value);
  if (Number.isFinite(v)) {
    document.documentElement.style.setProperty("--card-alpha", String(Math.min(1, Math.max(0, v))));
  }
}
listen("card-opacity", (event) => applyCardOpacity(event.payload));
invoke("get_settings")
  .then((s) => s && s.card_opacity !== undefined && applyCardOpacity(s.card_opacity))
  .catch(() => {});

// screenX/screenY arrive in CSS pixels; the backend works in physical pixels.
function toPhysical(event) {
  const r = window.devicePixelRatio || 1;
  return { x: Math.round(event.screenX * r), y: Math.round(event.screenY * r) };
}

listen("edit-mode", (event) => {
  editing = event.payload === true;
  document.body.classList.toggle("editing", editing);
  if (!editing) {
    drag = null;
    document.body.classList.remove("dragging");
  }
});

async function beginMove(event) {
  const pointer = toPhysical(event);
  const pos = await win.outerPosition();
  drag = {
    kind: "move",
    grabX: pointer.x - pos.x,
    grabY: pointer.y - pos.y,
  };
  document.body.classList.add("dragging");
}

async function beginResize(event) {
  const pointer = toPhysical(event);
  const size = await win.outerSize();
  drag = {
    kind: "resize",
    fromX: pointer.x,
    fromY: pointer.y,
    startW: size.width,
    startH: size.height,
  };
}

document.addEventListener("pointerdown", (event) => {
  if (!editing || event.button !== 0) return;
  event.preventDefault();
  document.body.setPointerCapture?.(event.pointerId);
  if (event.target.closest(".resize-handle")) {
    beginResize(event);
  } else {
    beginMove(event);
  }
});

document.addEventListener("pointermove", (event) => {
  if (!drag) return;
  const pointer = toPhysical(event);
  if (drag.kind === "move") {
    invoke("drag_widget", {
      label,
      x: pointer.x - drag.grabX,
      y: pointer.y - drag.grabY,
    });
  } else {
    invoke("resize_widget", {
      label,
      w: Math.max(240, drag.startW + (pointer.x - drag.fromX)),
      h: Math.max(120, drag.startH + (pointer.y - drag.fromY)),
    });
    rescale();
  }
});

function endDrag() {
  if (!drag) return;
  drag = null;
  document.body.classList.remove("dragging");
  invoke("commit_layout");
}

document.addEventListener("pointerup", endDrag);
document.addEventListener("pointercancel", endDrag);

invoke("is_editing").then((on) => {
  editing = on === true;
  document.body.classList.toggle("editing", editing);
});

// Tell the backend when the browser engine hides this page. Purely a
// diagnostic for watchdog.log: if a widget goes missing while its window still
// checks out as visible, this line is what says whether the page itself was
// switched off.
document.addEventListener("visibilitychange", () => {
  invoke("report_visibility", { label, hidden: document.hidden }).catch(() => {});
});

// Show Desktop: the backend hides the widget while the desktop is on top of
// it and then brings it back. Instead of popping back in, the page is veiled
// (opacity 0, instantly) and fades up when the backend says it is time.
const root = document.documentElement;
let veilTimer = 0;

function unveil() {
  clearTimeout(veilTimer);
  // Make sure the veiled state is committed before it is allowed to change,
  // otherwise there is nothing to transition from.
  void root.offsetWidth;
  const drop = () => root.classList.remove("veil");
  requestAnimationFrame(drop);
  // A window that is still covered gets no animation frames; do not leave the
  // widget invisible waiting for one.
  setTimeout(drop, 150);
}

listen("widget:veil", () => {
  root.classList.add("veil");
  clearTimeout(veilTimer);
  // Whatever happens on the Rust side, never stay invisible.
  veilTimer = setTimeout(unveil, 2500);
});
listen("widget:unveil", unveil);
