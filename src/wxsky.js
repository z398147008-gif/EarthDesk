// Animated sky behind the weather card, in the spirit of the iOS Weather app:
// drifting clouds, rain, snow, stars, a sun or moon glow and the odd lightning
// flash, all driven by the current WMO code and whether it is day.
//
// It is drawn procedurally rather than played from a video: a shader has no
// loop point, so there is no seam to hide, it stays sharp at any widget size,
// and it costs one small quad per frame. Apple's own animations are Apple's
// artwork and cannot be shipped here; this recreates the look, not the files.

import { describe } from "./wmo.js";

const VERT = `#version 300 es
in vec2 aPos;
void main() { gl_Position = vec4(aPos, 0.0, 1.0); }`;

const FRAG = `#version 300 es
precision highp float;
uniform vec2 uRes;
uniform float uTime;
uniform vec3 uTop, uBot, uLit, uShade, uGlowColor;
uniform float uCover, uRain, uSnow, uStars, uGlow, uFog, uFlash, uWind;
out vec4 outColor;

float hash(vec2 p) {
  vec3 p3 = fract(vec3(p.xyx) * 0.1031);
  p3 += dot(p3, p3.yzx + 33.33);
  return fract((p3.x + p3.y) * p3.z);
}
float noise(vec2 p) {
  vec2 i = floor(p), f = fract(p);
  vec2 u = f * f * (3.0 - 2.0 * f);
  return mix(mix(hash(i), hash(i + vec2(1, 0)), u.x),
             mix(hash(i + vec2(0, 1)), hash(i + vec2(1, 1)), u.x), u.y);
}
const mat2 ROT = mat2(1.6, 1.2, -1.2, 1.6);
float fbm(vec2 p) {
  float a = 0.5, s = 0.0;
  for (int i = 0; i < 5; i++) { s += a * noise(p); p = ROT * p; a *= 0.47; }
  return s;
}

void main() {
  vec2 uv = gl_FragCoord.xy / uRes;          // 0..1, y up
  float aspect = uRes.x / uRes.y;
  vec2 p = vec2(uv.x * aspect, uv.y);
  float t = uTime;

  // Sky: vertical gradient, a touch lighter toward the horizon.
  vec3 col = mix(uBot, uTop, smoothstep(0.0, 1.0, uv.y));

  // Sun (or moon) glow from just off the top-right corner.
  vec2 sp = vec2(aspect * 0.86, 1.02);
  float d = length(p - sp);
  col += uGlow * uGlowColor * (0.42 * exp(-d * 2.6) + 0.55 * exp(-d * 11.0));

  // Stars, twinkling, only where the clouds let them through (applied below).
  vec3 stars = vec3(0.0);
  if (uStars > 0.0) {
    for (int k = 0; k < 2; k++) {
      float scale = k == 0 ? 42.0 : 75.0;
      vec2 g = p * scale;
      vec2 id = floor(g);
      float r = hash(id + float(k) * 17.0);
      if (r > 0.955) {
        vec2 c = fract(g) - 0.5 - (vec2(hash(id + 3.1), hash(id + 7.7)) - 0.5) * 0.6;
        float s = smoothstep(0.11, 0.0, length(c));
        float tw = 0.55 + 0.45 * sin(t * (0.8 + 2.5 * hash(id + 1.3)) + r * 60.0);
        stars += s * tw * vec3(0.92, 0.94, 1.0) * (r - 0.955) * 22.0 * (k == 0 ? 1.0 : 0.6);
      }
    }
  }
  float clear = 1.0;

  // Clouds: two domain-warped fbm layers drifting at different speeds, so the
  // near layer slides over the far one. Lit from above.
  for (int L = 0; L < 2; L++) {
    float fl = float(L);
    float sc = 1.7 + fl * 1.3;
    vec2 q = p * sc * vec2(1.0, 1.3) + vec2(t * uWind * (0.030 + fl * 0.022), fl * 3.7);
    vec2 w = vec2(fbm(q * 0.6 + vec2(0.0, t * 0.010)), fbm(q * 0.6 + vec2(5.2, 1.3) - t * 0.008));
    float n = fbm(q + 0.55 * w);
    float lo = 0.86 - uCover * 0.78;
    float dens = smoothstep(lo, lo + 0.26, n);
    // Lighting: brighter where density falls off upward (the lit tops),
    // darker in the thick middles.
    float nUp = fbm(q + 0.55 * w + vec2(0.0, 0.09));
    float lit = clamp(0.62 + (n - nUp) * 5.5 - (n - lo - 0.25) * 0.9 + (uv.y - 0.5) * 0.3, 0.0, 1.0);
    vec3 cc = mix(uShade, uLit, lit);
    float a = dens * (0.92 - fl * 0.18);
    col = mix(col, cc, a);
    clear *= 1.0 - a;
  }
  col += stars * uStars * clear;

  // Fog: slow horizontal bands, thicker toward the bottom.
  if (uFog > 0.0) {
    float f = fbm(vec2(p.x * 1.4 + t * 0.03, p.y * 3.0));
    float band = uFog * (0.45 + 0.4 * f) * smoothstep(1.15, 0.05, uv.y);
    col = mix(col, uLit * 0.92, clamp(band, 0.0, 0.85));
  }

  // Rain: three depths of slanted streaks, the near ones longer and faster.
  // Speeds in card heights per second, the same as the input method's
  // weather bar (0.9 / 1.4 / 1.9): a drop takes as long to cross either.
  if (uRain > 0.0) {
    for (int i = 0; i < 3; i++) {
      float fi = float(i);
      float cols = 34.0 - fi * 9.0;
      vec2 rp = p;
      rp.x += rp.y * 0.16;                       // wind slant
      float ys = 3.2 - fi * 0.8;
      vec2 g = vec2(rp.x * cols, rp.y * ys + t * (0.9 + fi * 0.5) * ys);
      float cid = floor(g.x);
      float off = hash(vec2(cid, fi * 11.0));
      float y = fract(g.y + off * 7.0);
      float cell = floor(g.y + off * 7.0);
      float on = step(1.0 - uRain * (0.55 + 0.25 * fi), hash(vec2(cid, cell + fi * 31.0)));
      float x = abs(fract(g.x) - 0.5);
      float streak = smoothstep(0.05 + fi * 0.02, 0.0, x) * smoothstep(0.0, 0.06, y) * smoothstep(0.22 + fi * 0.06, 0.05, y);
      col += on * streak * vec3(0.80, 0.86, 0.96) * (0.10 + fi * 0.08);
    }
  }

  // Snow: soft flakes falling and swaying, three depths.
  if (uSnow > 0.0) {
    for (int i = 0; i < 3; i++) {
      float fi = float(i);
      float sc = 14.0 - fi * 3.5;
      vec2 sp2 = p * sc;
      sp2.y += t * (0.35 + fi * 0.2) * sc * 0.12;
      sp2.x += sin(t * 0.6 + sp2.y * 0.7 + fi) * 0.35;
      vec2 id = floor(sp2);
      float r = hash(id + fi * 13.0);
      if (r < uSnow * (0.45 + fi * 0.12)) {
        vec2 c = fract(sp2) - 0.5 - (vec2(hash(id + 2.0), hash(id + 5.0)) - 0.5) * 0.5;
        float flake = smoothstep(0.10 + fi * 0.03, 0.0, length(c));
        col += flake * vec3(0.95) * (0.45 + fi * 0.2);
      }
    }
  }

  // Lightning lights the whole sky from inside the clouds.
  col += uFlash * vec3(0.75, 0.78, 0.95) * (0.35 + 0.45 * uv.y);

  outColor = vec4(col, 1.0);
}`;

// Scene presets. Colours are linear-ish sRGB, picked against screenshots of
// the iOS app at the same conditions.
const DAY = {
  sky:      { top: [0.16, 0.39, 0.74], bot: [0.40, 0.64, 0.90] },
  grey:     { top: [0.36, 0.43, 0.53], bot: [0.54, 0.60, 0.68] },
  wet:      { top: [0.24, 0.29, 0.36], bot: [0.39, 0.44, 0.51] },
  storm:    { top: [0.13, 0.15, 0.22], bot: [0.28, 0.30, 0.38] },
  snow:     { top: [0.50, 0.56, 0.65], bot: [0.66, 0.70, 0.77] },
  cloudLit: [0.97, 0.98, 1.0],
  cloudShade: [0.60, 0.65, 0.74],
  greyLit: [0.84, 0.87, 0.92],
  greyShade: [0.50, 0.55, 0.63],
  wetLit: [0.64, 0.68, 0.74],
  wetShade: [0.30, 0.33, 0.40],
  glow: [1.0, 0.92, 0.72],
};
const NIGHT = {
  sky:      { top: [0.025, 0.045, 0.13], bot: [0.10, 0.13, 0.29] },
  grey:     { top: [0.12, 0.12, 0.21], bot: [0.25, 0.24, 0.39] },
  wet:      { top: [0.09, 0.10, 0.16], bot: [0.19, 0.20, 0.29] },
  storm:    { top: [0.05, 0.05, 0.10], bot: [0.14, 0.14, 0.22] },
  snow:     { top: [0.18, 0.20, 0.28], bot: [0.30, 0.32, 0.42] },
  cloudLit: [0.52, 0.48, 0.68],
  cloudShade: [0.19, 0.18, 0.30],
  greyLit: [0.46, 0.43, 0.60],
  greyShade: [0.17, 0.16, 0.27],
  wetLit: [0.34, 0.33, 0.45],
  wetShade: [0.12, 0.12, 0.19],
  glow: [0.70, 0.78, 1.0],
};

function scene(code, isDay) {
  const P = isDay ? DAY : NIGHT;
  const { kind } = describe(code);
  const s = {
    sky: P.sky, lit: P.cloudLit, shade: P.cloudShade, glowColor: P.glow,
    cover: 0.1, rain: 0, snow: 0, stars: isDay ? 0 : 1, glow: isDay ? 1 : 0.55,
    fog: 0, thunder: 0, wind: 1,
  };
  const wet = () => { s.lit = P.wetLit; s.shade = P.wetShade; };
  switch (kind) {
    case "clear":        s.cover = 0.06; break;
    case "mostly-clear": s.cover = 0.32; s.glow *= 0.85; break;
    case "partly":       s.cover = 0.46; s.glow *= 0.55; s.sky = { top: mix3(P.sky.top, P.grey.top, 0.35), bot: mix3(P.sky.bot, P.grey.bot, 0.35) }; break;
    case "overcast":     s.cover = 0.98; s.glow *= 0.15; s.sky = P.grey; s.lit = P.greyLit; s.shade = P.greyShade; break;
    case "fog":          s.cover = 0.7;  s.glow *= 0.2; s.sky = P.grey; s.lit = P.greyLit; s.shade = P.greyShade; s.fog = 1; break;
    case "drizzle":      s.cover = 0.95; s.glow = 0; s.sky = P.wet; wet(); s.rain = 0.35; break;
    case "rain-light":   s.cover = 0.95; s.glow = 0; s.sky = P.wet; wet(); s.rain = 0.5; break;
    case "showers":      s.cover = 0.85; s.glow *= 0.2; s.sky = P.wet; wet(); s.rain = 0.65; break;
    case "rain":         s.cover = 1.0;  s.glow = 0; s.sky = P.wet; wet(); s.rain = 0.95; s.wind = 1.6; break;
    case "sleet":        s.cover = 1.0;  s.glow = 0; s.sky = P.wet; wet(); s.rain = 0.5; s.snow = 0.4; break;
    case "snow-light":
    case "snow-showers": s.cover = 0.85; s.glow *= 0.2; s.sky = P.snow; s.snow = 0.55; break;
    case "snow":         s.cover = 1.0;  s.glow = 0; s.sky = P.snow; s.snow = 0.95; break;
    case "thunder":
    case "thunder-hail": s.cover = 1.0;  s.glow = 0; s.sky = P.storm; wet(); s.rain = 1.0; s.thunder = 1; s.wind = 2; break;
    default: break;
  }
  s.stars *= Math.max(0, 1 - s.cover);
  return s;
}

function mix3(a, b, t) { return a.map((v, i) => v + (b[i] - v) * t); }

function flatten(s) {
  return {
    uTop: s.sky.top, uBot: s.sky.bot, uLit: s.lit, uShade: s.shade, uGlowColor: s.glowColor,
    uCover: s.cover, uRain: s.rain, uSnow: s.snow, uStars: s.stars, uGlow: s.glow,
    uFog: s.fog, uWind: s.wind, thunder: s.thunder,
  };
}

/// Starts the animation on `canvas`. Returns `{ set(code, isDay) }`, or null
/// when WebGL2 is unavailable (the card then keeps its plain tint).
export function createSky(canvas, { fps = 60 } = {}) {
  const gl = canvas.getContext("webgl2", { antialias: false, alpha: false, preserveDrawingBuffer: false });
  if (!gl) return null;

  const compile = (type, src) => {
    const sh = gl.createShader(type);
    gl.shaderSource(sh, src);
    gl.compileShader(sh);
    if (!gl.getShaderParameter(sh, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(sh));
    return sh;
  };
  const prog = gl.createProgram();
  try {
    gl.attachShader(prog, compile(gl.VERTEX_SHADER, VERT));
    gl.attachShader(prog, compile(gl.FRAGMENT_SHADER, FRAG));
    gl.linkProgram(prog);
    if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(prog));
  } catch (e) {
    console.error("weather sky shader:", e);
    return null;
  }
  gl.useProgram(prog);

  const buf = gl.createBuffer();
  gl.bindBuffer(gl.ARRAY_BUFFER, buf);
  gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), gl.STATIC_DRAW);
  const loc = gl.getAttribLocation(prog, "aPos");
  gl.enableVertexAttribArray(loc);
  gl.vertexAttribPointer(loc, 2, gl.FLOAT, false, 0, 0);

  const U = {};
  for (const name of ["uRes", "uTime", "uTop", "uBot", "uLit", "uShade", "uGlowColor",
    "uCover", "uRain", "uSnow", "uStars", "uGlow", "uFog", "uFlash", "uWind"]) {
    U[name] = gl.getUniformLocation(prog, name);
  }

  // Current values ease toward the target over a couple of seconds, so a
  // change in the weather drifts in instead of cutting.
  let target = flatten(scene(2, true));
  let current = JSON.parse(JSON.stringify(target));
  let flash = 0;
  let nextStrike = 4 + Math.random() * 6;

  function resize() {
    // The sky is soft clouds and gradients; the browser's own upscaling is
    // invisible at 60% resolution and it cuts the shader work by two thirds.
    const scale = Math.min(window.devicePixelRatio || 1, 1.25) * 0.6;
    const w = Math.max(1, Math.round(canvas.clientWidth * scale));
    const h = Math.max(1, Math.round(canvas.clientHeight * scale));
    if (canvas.width !== w || canvas.height !== h) {
      canvas.width = w;
      canvas.height = h;
    }
  }

  const start = performance.now();
  let last = 0;
  let lastFrame = 0;
  // Rain, snow, lightning and a weather change easing in get the display's
  // full rate (60); a calm sky with only drifting clouds half of it. Rates
  // that do not divide the refresh rate (24 on 60 Hz) judder: frames come
  // two and three refreshes apart by turns.
  const calmGap = 1000 / Math.max(1, Math.round(fps / 2));
  const busyGap = 1000 / fps;
  let hidden = document.hidden;
  document.addEventListener("visibilitychange", () => {
    hidden = document.hidden;
    if (!hidden) { last = 0; requestAnimationFrame(frame); }
  });

  function frame(now) {
    if (hidden) return;
    requestAnimationFrame(frame);
    const busy = current.uRain > 0.05 || current.uSnow > 0.05 || current.thunder > 0.3 || flash > 0 ||
      Object.keys(target).some((key) => {
        const tv = target[key], cv = current[key];
        return Array.isArray(tv) ? tv.some((v, i) => Math.abs(v - cv[i]) > 0.01) : Math.abs(tv - cv) > 0.01;
      });
    const minGap = busy ? busyGap : calmGap;
    // A few ms of slack: requestAnimationFrame at 60 Hz arrives 16.4–16.9 ms
    // apart, and a strict gate would drop every other frame.
    if (now - lastFrame < minGap - 3) return;
    const dt = Math.min(0.2, (now - (last || now)) / 1000);
    last = now;
    lastFrame = now;

    const k = 1 - Math.exp(-dt / 0.8);
    for (const key of Object.keys(target)) {
      const tv = target[key];
      if (Array.isArray(tv)) current[key] = current[key].map((v, i) => v + (tv[i] - v) * k);
      else current[key] += (tv - current[key]) * k;
    }

    const t = (now - start) / 1000;
    if (current.thunder > 0.5 && t > nextStrike) {
      flash = 1;
      nextStrike = t + 5 + Math.random() * 9;
    }
    // Two quick pulses then a decay, the way a real strike flickers.
    flash = Math.max(0, flash - dt * 3.2);
    const flick = flash > 0.6 ? flash : flash * (0.6 + 0.4 * Math.sin(t * 60));

    resize();
    gl.viewport(0, 0, canvas.width, canvas.height);
    gl.uniform2f(U.uRes, canvas.width, canvas.height);
    gl.uniform1f(U.uTime, t);
    for (const n of ["uTop", "uBot", "uLit", "uShade", "uGlowColor"]) gl.uniform3fv(U[n], current[n]);
    for (const n of ["uCover", "uRain", "uSnow", "uStars", "uGlow", "uFog", "uWind"]) gl.uniform1f(U[n], current[n]);
    gl.uniform1f(U.uFlash, flick * 0.55);
    gl.drawArrays(gl.TRIANGLES, 0, 3);
  }
  requestAnimationFrame(frame);

  return {
    set(code, isDay) {
      target = flatten(scene(code, isDay));
    },
    /// Jump straight to the current target (first paint, no fade-in).
    snap() {
      current = JSON.parse(JSON.stringify(target));
    },
  };
}
