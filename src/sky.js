// The sky behind the planet: the real stars, in the real place, plus the sun
// and the moon where they actually are.
//
// Nothing here is a backdrop image. Every star is a catalogue entry
// (d3-celestial's magnitude-6 set, derived from HYG) carried as a J2000 unit
// vector; each frame it is turned into the earth-fixed frame by one rotation
// about the pole through Greenwich mean sidereal time, then projected with
// exactly the same camera the globe uses. Turn the clock forward and the sky
// wheels the way it does outside.
//
// Everything draws premultiplied, so a fragment with alpha 0 and colour above
// zero is simply added to what is already there -- which is what starlight
// and glare do.

import { program, uniforms, loadTexture, cross, dot, normalize, scale, sub } from "./glutil.js";

const CAMERA_UNIFORMS = `
uniform vec2  uRes;
uniform vec2  uOrigin;      // lower-left corner of this monitor's viewport
uniform vec2  uCenter;
uniform float uFocal;
uniform mat3  uBasis;
uniform vec3  uCamPos;
`;

/// Shared with the globe pass, and it has to stay shared: if the two disagree
/// by a pixel the stars slide against the limb.
const RAY_HELPERS = `
vec3 rayThrough(vec2 fragCoord) {
  vec2 uv = (fragCoord - uOrigin) / uRes;
  vec2 p = (uv - uCenter) * 2.0;
  p.x *= uRes.x / uRes.y;
  return normalize(uBasis[0] * p.x + uBasis[1] * p.y + uBasis[2] * uFocal);
}

bool behindEarth(vec3 dir) {
  float b = dot(uCamPos, dir);
  float c = dot(uCamPos, uCamPos) - 1.0;
  float disc = b * b - c;
  return disc > 0.0 && (-b - sqrt(disc)) > 0.0;
}
`;

const STAR_VERT = `#version 300 es
in vec3 aDir;
in float aMag;
in float aBv;
${CAMERA_UNIFORMS}
uniform mat3  uSpin;        // J2000 equatorial -> earth-fixed
uniform float uPointScale;
uniform float uBrightness;
out vec3 vColor;
out float vIntensity;

/// B-V colour index to something eye-like: hot stars blue-white, the sun's
/// class near white, giants orange.
vec3 colorFromBv(float bv) {
  float t = clamp((bv + 0.4) / 2.4, 0.0, 1.0);
  vec3 hot  = vec3(0.62, 0.72, 1.00);
  vec3 warm = vec3(1.00, 0.97, 0.93);
  vec3 cool = vec3(1.00, 0.76, 0.55);
  return t < 0.42 ? mix(hot, warm, t / 0.42) : mix(warm, cool, (t - 0.42) / 0.58);
}

void main() {
  vec3 d = uSpin * aDir;

  // Occlusion by the planet: is the star inside the earth's angular disc?
  float camDist = length(uCamPos);
  vec3 toEarth = -uCamPos / camDist;
  float cosEarth = sqrt(max(0.0, 1.0 - 1.0 / (camDist * camDist)));

  float ahead = dot(d, uBasis[2]);
  if (ahead <= 0.001 || dot(d, toEarth) > cosEarth) {
    gl_Position = vec4(2.0, 2.0, 2.0, 1.0);
    gl_PointSize = 0.0;
    return;
  }

  float px = uFocal * dot(d, uBasis[0]) / ahead;
  float py = uFocal * dot(d, uBasis[1]) / ahead;
  vec2 uv = uCenter + vec2(px / (uRes.x / uRes.y), py) * 0.5;

  gl_Position = vec4(uv * 2.0 - 1.0, 0.0, 1.0);
  gl_PointSize = clamp(1.7 + (6.5 - aMag) * 0.62, 1.5, 8.5) * uPointScale;
  vColor = colorFromBv(aBv);
  // Raw flux spans a factor of 2900 between Sirius and a magnitude-6 star,
  // which on an 8-bit display means either a blown-out Sirius or nothing at
  // all below magnitude 4. Compressing the exponent is what an eye (and every
  // astrophotograph) does anyway.
  vIntensity = clamp(pow(10.0, -0.4 * (aMag - 3.0) * 0.55), 0.10, 2.4) * uBrightness;
}
`;

const STAR_FRAG = `#version 300 es
precision highp float;
in vec3 vColor;
in float vIntensity;
out vec4 fragColor;
void main() {
  float r = length(gl_PointCoord - 0.5) * 2.0;
  float a = exp(-r * r * 2.7);
  if (a < 0.004) discard;
  fragColor = vec4(vColor * vIntensity * a, 0.0);
}
`;

/// The Milky Way: NASA's Deep Star Maps 2020 background (bright stars removed,
/// which the catalogue above supplies), sampled per pixel through the same
/// camera and the same sidereal rotation as the stars, so the dust lanes sit
/// where they really are. Apple's wallpaper shows the same warm band behind
/// the planet; it is most of what keeps a large expanse of sky from reading
/// as flat black.
const FULL_VERT = `#version 300 es
in vec2 aPos;
void main() { gl_Position = vec4(aPos, 0.0, 1.0); }
`;

const MILKYWAY_FRAG = `#version 300 es
precision highp float;
${CAMERA_UNIFORMS}
uniform vec2  uSpinCS;       // cos, sin of Greenwich sidereal time
uniform float uBrightness;
uniform sampler2D uMap;
uniform float uNebula;
out vec4 fragColor;
${RAY_HELPERS}
const float PI  = 3.141592653589793;
const float TAU = 6.283185307179586;
float nh(vec3 p) { p = fract(p * 0.3183099 + 0.1); p *= 17.0; return fract(p.x * p.y * p.z * (p.x + p.y + p.z)); }
float nnoise(vec3 x) {
  vec3 i = floor(x), f = fract(x); f = f * f * (3.0 - 2.0 * f);
  return mix(mix(mix(nh(i), nh(i + vec3(1,0,0)), f.x), mix(nh(i + vec3(0,1,0)), nh(i + vec3(1,1,0)), f.x), f.y),
             mix(mix(nh(i + vec3(0,0,1)), nh(i + vec3(1,0,1)), f.x), mix(nh(i + vec3(0,1,1)), nh(i + vec3(1,1,1)), f.x), f.y), f.z);
}
float nfbm(vec3 p) { float s = 0.0, a = 0.5; for (int i = 0; i < 5; i++) { s += a * nnoise(p); p = p * 2.02 + 1.7; a *= 0.5; } return s / 0.97; }
void main() {
  vec3 d = rayThrough(gl_FragCoord.xy);
  if (behindEarth(d)) discard;
  // Earth-fixed (y = pole) back to J2000 equatorial (z = pole).
  float c = uSpinCS.x;
  float s = uSpinCS.y;
  vec3 e = vec3(c * d.x - s * d.z, s * d.x + c * d.z, d.y);
  float ra = atan(e.y, e.x);
  float dec = asin(clamp(e.z, -1.0, 1.0));
  // The map is drawn as seen from inside the sphere: RA 0 in the middle,
  // increasing to the left.
  vec2 uv = vec2(fract(0.5 - ra / TAU), 0.5 - dec / PI);
  // A couple of mip levels soft: the map's faint unresolved stars turn into
  // grain at wallpaper scale, and only the glow and the dust lanes matter.
  // Explicit level of detail: implicit derivatives jump across the RA seam
  // (uv.x wraps 1 -> 0) and pick the coarsest mip, drawing a thin vertical
  // line through the sky.
  float pxAng = length(fwidth(d));
  float lod = max(log2(pxAng * float(textureSize(uMap, 0).x) / TAU), 0.0) + 1.5;
  vec3 m = textureLod(uMap, uv, lod).rgb;
  // Warm and a little desaturated, as in the reference; the black point
  // keeps empty sky black.
  float l = dot(m, vec3(0.3, 0.5, 0.2));
  vec3 col = mix(vec3(l), m, 0.55) * vec3(1.10, 0.92, 0.74);
  col = max(col - 0.13, 0.0) * 1.0;
  col = col / (col + 0.35) * 0.9 * uBrightness; // soft shoulder: glow, not blowout

  // Nebula: the reference's sky IS essentially black, so this is only the
  // faintest suggestion of dust -- visible as structure in a dark room, never
  // as a wash over the sky. Procedural, fixed to the stars (J2000 frame), so
  // it turns with them. Domain-warped fbm, thresholded hard so most of the
  // sky gets exactly nothing.
  vec3 pn = e * 2.2;
  vec3 w = vec3(nfbm(pn + 3.1), nfbm(pn + 17.7), nfbm(pn + 41.3));
  float dust = nfbm(pn * 1.7 + w * 1.6);
  float fine = nfbm(pn * 6.0 + w * 0.8);
  float neb = smoothstep(0.62, 0.95, dust) * (0.55 + 0.45 * fine);
  float cool = smoothstep(0.55, 0.85, nfbm(pn * 1.1 - w + 9.0));
  vec3 nebCol = mix(vec3(0.026, 0.018, 0.011), vec3(0.014, 0.019, 0.030), cool * 0.6);
  // Dark lanes cut through it, as in real dust.
  float lane = smoothstep(0.55, 0.75, nfbm(pn * 3.0 + w * 2.0 + 5.0));
  col += nebCol * neb * (1.0 - 0.6 * lane) * uNebula;
  fragColor = vec4(col, 0.0);
}
`;

const QUAD_VERT = `#version 300 es
in vec2 aCorner;
uniform vec2 uCenterClip;
uniform vec2 uRadiusClip;
out vec2 vLocal;
void main() {
  vLocal = aCorner;
  gl_Position = vec4(uCenterClip + aCorner * uRadiusClip, 0.0, 1.0);
}
`;

const SUN_FRAG = `#version 300 es
precision highp float;
in vec2 vLocal;
${CAMERA_UNIFORMS}
uniform float uSpread;     // quad half-width, in solar radii
uniform float uBrightness;
out vec4 fragColor;
${RAY_HELPERS}
void main() {
  if (behindEarth(rayThrough(gl_FragCoord.xy))) discard;
  float r = length(vLocal) * uSpread;
  float disc = smoothstep(1.04, 0.92, r);
  float glare = exp(-r * 1.15) * 0.42 + exp(-r * 0.32) * 0.12;
  float v = disc * 2.6 + glare;
  if (v < 0.002) discard;
  fragColor = vec4(vec3(1.0, 0.97, 0.90) * v * uBrightness, 0.0);
}
`;

const MOON_FRAG = `#version 300 es
precision highp float;
in vec2 vLocal;
${CAMERA_UNIFORMS}
uniform vec3 uAxisX;       // quad frame, in earth-fixed coordinates
uniform vec3 uAxisY;
uniform vec3 uAxisZ;       // toward the camera
uniform vec3 uLightLocal;  // sunward direction, in the quad frame
uniform vec3 uMoonX;       // selenographic frame
uniform vec3 uMoonY;
uniform vec3 uMoonZ;       // toward the earth, i.e. the sub-earth point
uniform float uBrightness;
uniform sampler2D uMoonMap;
out vec4 fragColor;
${RAY_HELPERS}

const float PI  = 3.141592653589793;
const float TAU = 6.283185307179586;

void main() {
  float r2 = dot(vLocal, vLocal);
  if (r2 > 1.0) discard;
  if (behindEarth(rayThrough(gl_FragCoord.xy))) discard;

  vec3 n = vec3(vLocal, sqrt(max(0.0, 1.0 - r2)));
  vec3 world = uAxisX * n.x + uAxisY * n.y + uAxisZ * n.z;

  // The moon keeps one face to us, so the sub-earth point is a fixed spot on
  // the map and the rest follows from the selenographic frame.
  vec2 uv = vec2(
    atan(dot(world, uMoonX), dot(world, uMoonZ)) / TAU + 0.5,
    0.5 - asin(clamp(dot(world, uMoonY), -1.0, 1.0)) / PI
  );
  float albedo = texture(uMoonMap, uv).r;

  float ndl = dot(n, uLightLocal);
  // A body with no atmosphere has a hard terminator; a pixel or two of
  // softening is all that keeps it from aliasing.
  float lit = smoothstep(-0.04, 0.06, ndl) * clamp(ndl, 0.0, 1.0);
  // Faint earthshine so the dark limb does not vanish into a bitten-off disc.
  vec3 color = vec3(1.0, 0.98, 0.94) * albedo * (lit * 1.35 + 0.018);

  float alpha = smoothstep(1.0, 0.985, r2) * uBrightness;
  if (alpha < 0.003) discard;
  fragColor = vec4(color * alpha, alpha);
}
`;

/// Project an earth-fixed direction into 0..1 screen space with the same
/// camera the globe uses. Returns null when it is behind the camera.
function projectDirection(dir, cam) {
  const ahead = dot(dir, cam.basis[2]);
  if (ahead <= 1e-4) return null;
  const px = (cam.focal * dot(dir, cam.basis[0])) / ahead;
  const py = (cam.focal * dot(dir, cam.basis[1])) / ahead;
  return [cam.center[0] + (px / cam.aspect) * 0.5, cam.center[1] + py * 0.5];
}

/// An orthonormal frame around `dir`, oriented to the camera so the quad we
/// draw is screen-aligned.
function frameAround(dir, cam) {
  let x = cross(cam.basis[1], dir);
  if (Math.hypot(x[0], x[1], x[2]) < 1e-6) x = cross(cam.basis[0], dir);
  x = normalize(x);
  const y = normalize(cross(dir, x));
  return { x, y };
}

export function createSky(gl) {
  const starProgram = program(gl, STAR_VERT, STAR_FRAG, "stars");
  const sunProgram = program(gl, QUAD_VERT, SUN_FRAG, "sun");
  const moonProgram = program(gl, QUAD_VERT, MOON_FRAG, "moon");
  const mwProgram = program(gl, FULL_VERT, MILKYWAY_FRAG, "milkyway");

  const CAM = ["uRes", "uOrigin", "uCenter", "uFocal", "uBasis", "uCamPos"];
  const starU = uniforms(gl, starProgram, [...CAM, "uSpin", "uPointScale", "uBrightness"]);
  const sunU = uniforms(gl, sunProgram, [...CAM, "uCenterClip", "uRadiusClip", "uSpread", "uBrightness"]);
  const moonU = uniforms(gl, moonProgram, [
    ...CAM, "uCenterClip", "uRadiusClip", "uAxisX", "uAxisY", "uAxisZ",
    "uLightLocal", "uMoonX", "uMoonY", "uMoonZ", "uBrightness", "uMoonMap",
  ]);

  const mwU = uniforms(gl, mwProgram, [...CAM, "uSpinCS", "uBrightness", "uMap", "uNebula"]);
  const fullTri = gl.createBuffer();
  gl.bindBuffer(gl.ARRAY_BUFFER, fullTri);
  gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), gl.STATIC_DRAW);
  const mwUnit = 6;
  const mwReady = loadTexture(gl, "textures/milkyway.jpg", mwUnit, { wrapX: true });

  const quad = gl.createBuffer();
  gl.bindBuffer(gl.ARRAY_BUFFER, quad);
  gl.bufferData(gl.ARRAY_BUFFER,
    new Float32Array([-1, -1, 1, -1, -1, 1, -1, 1, 1, -1, 1, 1]), gl.STATIC_DRAW);

  const starBuffer = gl.createBuffer();
  let starCount = 0;

  const moonUnit = 5;
  const moonReady = loadTexture(gl, "textures/moon.jpg", moonUnit, { wrapX: true });

  const catalogue = fetch("textures/stars.bin")
    .then((r) => r.arrayBuffer())
    .then((buffer) => {
      const header = new DataView(buffer);
      const magic = String.fromCharCode(...new Uint8Array(buffer, 0, 4));
      if (magic !== "STR1") throw new Error("star catalogue header mismatch");
      starCount = header.getUint32(4, true);
      const data = new Float32Array(buffer, 8, starCount * 5);
      gl.bindBuffer(gl.ARRAY_BUFFER, starBuffer);
      gl.bufferData(gl.ARRAY_BUFFER, data, gl.STATIC_DRAW);
      return starCount;
    })
    .catch((e) => {
      console.error("stars unavailable", e);
      starCount = 0;
      return 0;
    });

  function setCamera(u, cam) {
    gl.uniform2f(u.uRes, cam.width, cam.height);
    gl.uniform2f(u.uOrigin, cam.origin ? cam.origin[0] : 0, cam.origin ? cam.origin[1] : 0);
    gl.uniform2f(u.uCenter, cam.center[0], cam.center[1]);
    gl.uniform1f(u.uFocal, cam.focal);
    gl.uniformMatrix3fv(u.uBasis, false, cam.basisArray);
    gl.uniform3f(u.uCamPos, cam.pos[0], cam.pos[1], cam.pos[2]);
  }

  /// Centre and half-extent of a disc of angular radius `theta` around `dir`,
  /// in clip space. Measured by projecting two points on its rim rather than
  /// by a small-angle formula, so it stays honest near the edge of frame.
  function discExtent(dir, theta, cam, frame) {
    const center = projectDirection(dir, cam);
    if (!center) return null;
    const t = Math.tan(theta);
    const edgeX = projectDirection(normalize([
      dir[0] + frame.x[0] * t, dir[1] + frame.x[1] * t, dir[2] + frame.x[2] * t,
    ]), cam);
    const edgeY = projectDirection(normalize([
      dir[0] + frame.y[0] * t, dir[1] + frame.y[1] * t, dir[2] + frame.y[2] * t,
    ]), cam);
    if (!edgeX || !edgeY) return null;
    return {
      centerClip: [center[0] * 2 - 1, center[1] * 2 - 1],
      radiusClip: [
        Math.abs(edgeX[0] - center[0]) * 2,
        Math.abs(edgeY[1] - center[1]) * 2,
      ],
    };
  }

  function drawStars(cam, spin, pointScale, brightness) {
    if (!starCount || brightness <= 0) return;
    gl.useProgram(starProgram);
    setCamera(starU, cam);
    gl.uniformMatrix3fv(starU.uSpin, false, spin);
    gl.uniform1f(starU.uPointScale, pointScale);
    gl.uniform1f(starU.uBrightness, brightness);

    gl.bindBuffer(gl.ARRAY_BUFFER, starBuffer);
    const stride = 5 * 4;
    const aDir = gl.getAttribLocation(starProgram, "aDir");
    const aMag = gl.getAttribLocation(starProgram, "aMag");
    const aBv = gl.getAttribLocation(starProgram, "aBv");
    gl.enableVertexAttribArray(aDir);
    gl.vertexAttribPointer(aDir, 3, gl.FLOAT, false, stride, 0);
    gl.enableVertexAttribArray(aMag);
    gl.vertexAttribPointer(aMag, 1, gl.FLOAT, false, stride, 12);
    gl.enableVertexAttribArray(aBv);
    gl.vertexAttribPointer(aBv, 1, gl.FLOAT, false, stride, 16);

    gl.drawArrays(gl.POINTS, 0, starCount);

    gl.disableVertexAttribArray(aDir);
    gl.disableVertexAttribArray(aMag);
    gl.disableVertexAttribArray(aBv);
  }

  function bindQuad(prog) {
    gl.bindBuffer(gl.ARRAY_BUFFER, quad);
    const aCorner = gl.getAttribLocation(prog, "aCorner");
    gl.enableVertexAttribArray(aCorner);
    gl.vertexAttribPointer(aCorner, 2, gl.FLOAT, false, 0, 0);
    return aCorner;
  }

  function drawSun(cam, sun, brightness) {
    if (brightness <= 0) return;
    const fromCam = normalize(sub(scale(sun.direction, sun.distance), cam.pos));
    const frame = frameAround(fromCam, cam);
    // The disc itself is a third of a degree; the quad has to be much wider
    // than that to have somewhere to put the glare.
    const spread = 16;
    const extent = discExtent(fromCam, sun.angularRadius * spread, cam, frame);
    if (!extent) return;

    gl.useProgram(sunProgram);
    setCamera(sunU, cam);
    gl.uniform2f(sunU.uCenterClip, extent.centerClip[0], extent.centerClip[1]);
    gl.uniform2f(sunU.uRadiusClip, extent.radiusClip[0], extent.radiusClip[1]);
    gl.uniform1f(sunU.uSpread, spread);
    gl.uniform1f(sunU.uBrightness, brightness);
    const aCorner = bindQuad(sunProgram);
    gl.drawArrays(gl.TRIANGLES, 0, 6);
    gl.disableVertexAttribArray(aCorner);
  }

  function drawMoon(cam, moon, sun, eclipticNorth, brightness) {
    if (brightness <= 0) return;
    const moonPos = scale(moon.direction, moon.distance);
    const fromCam = normalize(sub(moonPos, cam.pos));
    const frame = frameAround(fromCam, cam);
    const extent = discExtent(fromCam, moon.angularRadius, cam, frame);
    if (!extent) return;

    // Quad frame: x right, y up, z toward the camera.
    const axisZ = scale(fromCam, -1);
    const toSun = normalize(sub(scale(sun.direction, sun.distance), moonPos));
    const lightLocal = [dot(toSun, frame.x), dot(toSun, frame.y), dot(toSun, axisZ)];

    // Selenographic frame. Tidal locking puts the sub-earth point at the
    // origin of the map; the moon's spin axis is close enough to the ecliptic
    // pole that ignoring the 1.5 degree tilt and the librations costs less
    // than a pixel at this size.
    const moonZ = normalize(scale(moon.direction, -1));
    const northComponent = dot(eclipticNorth, moonZ);
    const moonY = normalize(sub(eclipticNorth, scale(moonZ, northComponent)));
    const moonX = normalize(cross(moonY, moonZ));

    gl.useProgram(moonProgram);
    setCamera(moonU, cam);
    gl.uniform2f(moonU.uCenterClip, extent.centerClip[0], extent.centerClip[1]);
    gl.uniform2f(moonU.uRadiusClip, extent.radiusClip[0], extent.radiusClip[1]);
    gl.uniform3f(moonU.uAxisX, frame.x[0], frame.x[1], frame.x[2]);
    gl.uniform3f(moonU.uAxisY, frame.y[0], frame.y[1], frame.y[2]);
    gl.uniform3f(moonU.uAxisZ, axisZ[0], axisZ[1], axisZ[2]);
    gl.uniform3f(moonU.uLightLocal, lightLocal[0], lightLocal[1], lightLocal[2]);
    gl.uniform3f(moonU.uMoonX, moonX[0], moonX[1], moonX[2]);
    gl.uniform3f(moonU.uMoonY, moonY[0], moonY[1], moonY[2]);
    gl.uniform3f(moonU.uMoonZ, moonZ[0], moonZ[1], moonZ[2]);
    gl.uniform1f(moonU.uBrightness, brightness);
    gl.uniform1i(moonU.uMoonMap, moonUnit);
    const aCorner = bindQuad(moonProgram);
    gl.drawArrays(gl.TRIANGLES, 0, 6);
    gl.disableVertexAttribArray(aCorner);
  }

  function drawMilkyWay(cam, gmstDeg, brightness, nebula = 1) {
    if (brightness <= 0 && nebula <= 0) return;
    gl.useProgram(mwProgram);
    setCamera(mwU, cam);
    const g = (gmstDeg * Math.PI) / 180;
    gl.uniform2f(mwU.uSpinCS, Math.cos(g), Math.sin(g));
    gl.uniform1f(mwU.uBrightness, brightness);
    gl.uniform1f(mwU.uNebula, nebula);
    gl.uniform1i(mwU.uMap, mwUnit);
    gl.bindBuffer(gl.ARRAY_BUFFER, fullTri);
    const aPos = gl.getAttribLocation(mwProgram, "aPos");
    gl.enableVertexAttribArray(aPos);
    gl.vertexAttribPointer(aPos, 2, gl.FLOAT, false, 0, 0);
    gl.drawArrays(gl.TRIANGLES, 0, 3);
    gl.disableVertexAttribArray(aPos);
  }

  return {
    ready: Promise.all([catalogue, moonReady, mwReady]),
    drawMilkyWay,
    get starCount() {
      return starCount;
    },
    drawStars,
    drawSun,
    drawMoon,
  };
}

/// Rotation taking a J2000 equatorial unit vector into the earth-fixed frame
/// used by the globe: one turn about the pole by sidereal time. Column-major,
/// ready for uniformMatrix3fv.
export function spinMatrix(gmstDeg) {
  const g = (gmstDeg * Math.PI) / 180;
  const c = Math.cos(g);
  const s = Math.sin(g);
  return new Float32Array([
    c, 0, -s,
    s, 0, c,
    0, 1, 0,
  ]);
}
