import { subsolarPoint, sunState, moonState, gmst, toVector, equatorialToEarthFixed } from "./astro.js";
import { createSky, spinMatrix } from "./sky.js";
import { decodeCloudTop } from "./cloudtop.js";
import { program, uniforms, loadTexture, textureOf, textureSizeOf, cross, normalize } from "./glutil.js";

const { invoke } = window.__TAURI__.core;

const DEFAULTS = {
  camera_anchor: "location",
  // Fitted against the user's iPad home screen (2026-09-22): twelve
  // coastline landmarks from Hainan to Tokyo Bay plus fourteen points on the
  // limb, solved together (tools/fit_camera.py); landmark rms 6 px at
  // 2000x1499. Apple's framing turns out to be a camera above the equator at
  // the home longitude with a long lens aimed at the northern limb.
  camera_offset_deg: -0.106,
  camera_lat_deg: 0.843,
  camera_distance: 3.213,
  camera_tilt_deg: 0,
  camera_heading_deg: 0.186,
  focal_length: 5.166,
  axis_offset_x: -0.0006,
  axis_offset_y: 1.541,
  frame_monitor: "primary",
  // A globe on every monitor, each framed like the primary.
  each_monitor: true,
  // "width": keep the iPad (4:3) framing across the width, crop top/bottom.
  // "height": keep it top to bottom and show more at the sides (old).
  frame_fit: "width",
  frame_aspect: 4 / 3,
  // "calm": open ocean as an even deep blue, the way it looks from orbit.
  // "relief": the sea-floor shading baked into NASA's topo-bathy composite.
  ocean_style: "apple",
  // Himawari visible-light clouds over its half of the planet (daytime).
  geo_clouds: true,
  geo_clouds_minutes: 20,
  geo_clouds_scale: 1,
  // Procedural detail layered onto the cloud map (0 = off, 1 = full).
  cloud_detail: 0.45,
  // "physical": single-scattering Rayleigh + Mie + ozone, integrated per
  // pixel; "painted": the older hand-tuned veil.
  atmosphere: "physical",
  atmosphere_exposure: 2.0,
  // Aerosol amount. 1 is a clean standard atmosphere; more is a hazier,
  // softer planet.
  haze: 15,
  // Visual thickness of the air shell (1 = real 100 km).
  atmosphere_scale: 1.2,
  atmosphere_extinction: 1,
  // The eye-soft limb: pale haze band (radii) and warm outer bloom (radii).
  limb_soft: 0.011,
  limb_glow: 0.026,
  // Lens glow: strength, and how bright a pixel must be to glow (0..1).
  bloom: 0.35,
  bloom_threshold: 0.55,
  // Faint wide scatter over the whole frame.
  veil: 0.12,
  render_scale: 1,
  drift_deg_per_hour: 0,
  cloud_opacity: 0.9,
  live_clouds: true,
  live_clouds_url: "https://clouds.matteason.co.uk/images/8192x4096/clouds.jpg",
  city_lights: 1,
  stars: 0.4,
  // The Milky Way behind the planet (NASA Deep Star Maps). 0 hides it.
  milky_way: 0.06,
  // Faint procedural dust nebula behind the planet, as on the iPad. 0 hides it.
  nebula: 1,
  sun: 1,
  moon: 1,
  // Seconds between frames. The scene only changes with the clock (a quarter
  // of a degree of rotation a minute), so frames are drawn on demand, not in
  // a loop; see "when to draw" below.
  redraw_seconds: 20,
};

/// How often to pull a fresh cloud map. The source regenerates it every few
/// hours from geostationary satellite imagery; asking more often only
/// re-downloads the same five megabytes.
const LIVE_CLOUD_REFRESH_MS = 3 * 3600 * 1000;

/// NASA GIBS WMS, Himawari-9 AHI band 3 (0.64 um red visible, 1 km), served
/// every ten minutes about forty minutes after capture. CORS-open.
const GEO_DEFAULT_URL =
  "https://gibs.earthdata.nasa.gov/wms/epsg4326/best/wms.cgi?SERVICE=WMS&REQUEST=GetMap" +
  "&VERSION=1.3.0&LAYERS=Himawari_AHI_Band3_Red_Visible_1km&STYLES=&CRS=EPSG:4326" +
  "&BBOX={BBOX}&WIDTH={W}&HEIGHT={H}&FORMAT=image/png&TIME={TIME}";

/// The ecliptic pole, which the moon's spin axis is within a degree and a half
/// of. Right ascension 18h, declination +66.56.
const ECLIPTIC_POLE = { ra: 270.0, dec: 66.5607 };

const VERT = `#version 300 es
in vec2 aPos;
void main() { gl_Position = vec4(aPos, 0.0, 1.0); }
`;

const FRAG = `#version 300 es
precision highp float;

out vec4 fragColor;

uniform vec2  uRes;
uniform vec2  uOrigin;        // lower-left corner of this monitor's viewport
uniform vec2  uCenter;
uniform float uFocal;
uniform mat3  uBasis;
uniform vec3  uCamPos;
uniform vec3  uSun;
uniform float uCloudShift;
uniform float uCloudOpacity;
uniform vec2  uCloudLevels;   // black point / white point of the cloud map
uniform float uCityLights;
uniform float uHours;          // wall-clock hours, wrapped; drives cloud detail drift
uniform float uCloudDetail;    // 0 = map only, 1 = full procedural detail
uniform float uAtmo;           // 1 = physically based atmosphere, 0 = the old hand-tuned veil
uniform float uAtmoExposure;
uniform float uHaze;           // aerosol (Mie) density multiplier
uniform float uAtmoExtinction;
uniform float uLimbIn;         // width of the pale haze band at the edge, radii
uniform float uLimbOut;        // width of the warm outer bloom, radii

uniform sampler2D uDay;
uniform sampler2D uNight;
uniform sampler2D uClouds;
uniform sampler2D uWater;
uniform sampler2D uNormalMap;

const float PI  = 3.141592653589793;
const float TAU = 6.283185307179586;

float hash21(vec2 p) {
  p = fract(p * vec2(123.34, 456.21));
  p += dot(p, p + 45.32);
  return fract(p.x * p.y);
}

// --- procedural detail -------------------------------------------------------
// Cheap 3-D value noise on the unit sphere. Evaluated in 3-D rather than on
// the texture's lat/lon grid, so it has no seam at the date line and no pinch
// at the poles.
float hash31(vec3 p) {
  p = fract(p * 0.3183099 + vec3(0.71, 0.113, 0.419));
  p *= 17.0;
  return fract(p.x * p.y * p.z * (p.x + p.y + p.z));
}
float vnoise(vec3 x) {
  vec3 i = floor(x);
  vec3 f = fract(x);
  f = f * f * (3.0 - 2.0 * f);
  return mix(mix(mix(hash31(i),                   hash31(i + vec3(1, 0, 0)), f.x),
                 mix(hash31(i + vec3(0, 1, 0)),   hash31(i + vec3(1, 1, 0)), f.x), f.y),
             mix(mix(hash31(i + vec3(0, 0, 1)),   hash31(i + vec3(1, 0, 1)), f.x),
                 mix(hash31(i + vec3(0, 1, 1)),   hash31(i + vec3(1, 1, 1)), f.x), f.y), f.z);
}
float fbm(vec3 p) {
  float s = 0.0;
  float a = 0.5;
  for (int i = 0; i < 5; i++) {
    s += a * vnoise(p);
    p = p * 2.03 + vec3(1.7, 9.2, 3.1);
    a *= 0.5;
  }
  return s / 0.96875;
}

// --- the atmosphere --------------------------------------------------------------
// Single scattering through a real Earth atmosphere, integrated per pixel:
// Rayleigh (air molecules: blue, the veil over the whole disc), Mie (aerosol
// and haze: white, strongly forward, the soft glow and the "air" between the
// camera and the ground), and ozone absorption (which keeps the limb blue
// instead of turning it teal at sunset). Coefficients are the standard Earth
// values used by Bruneton 2017 and Hillaire 2020, converted to earth radii.
//
// This is what the hand-tuned veil only imitated. It costs a few hundred
// exponentials per pixel -- far too much for an animation, and nothing for a
// wallpaper that is drawn once every twenty seconds.
// uAtmoScale thickens the shell for the eye: heights are multiplied and the
// coefficients divided by it, so looking straight down the air is exactly as
// dense as the real thing, but the limb -- where the path runs along the
// shell -- gets the broad, soft glow of the reference instead of a
// physically correct but hair-thin line. Space-game renderers do the same.
uniform float uAtmoScale;
const float RP   = 6360.0e3;                      // metres per earth radius
#define RA  (1.0 + 100.0e3 * uAtmoScale / RP)     // top of the atmosphere
#define HR  (8.0e3 * uAtmoScale / RP)
#define HM  (1.2e3 * uAtmoScale / RP)
#define BR  (vec3(5.802, 13.558, 33.100) * 1.0e-6 * RP / uAtmoScale)
#define BMS (3.996e-6 * RP / uAtmoScale)          // Mie scattering
#define BME (4.440e-6 * RP / uAtmoScale)          // Mie extinction
#define BO  (vec3(0.650, 1.881, 0.085) * 1.0e-6 * RP / uAtmoScale)

vec3 atmoDensity(vec3 p) {
  float h = max(length(p) - 1.0, 0.0);
  float oz = max(0.0, 1.0 - abs(h * RP / uAtmoScale - 25.0e3) / 15.0e3);
  return vec3(exp(-h / HR), exp(-h / HM) * uHaze, oz);
}

vec3 extinction(vec3 od) {
  return exp(-(BR * od.x + BME * od.y + BO * od.z));
}

// Optical depth from p toward the sun, and whether the planet is in the way
// (softened over a few kilometres so twilight is a gradient, not a line).
vec3 sunDepth(vec3 p, out float lit) {
  float b = dot(p, uSun);
  float c = dot(p, p) - RA * RA;
  float t1 = -b + sqrt(max(b * b - c, 0.0));
  float dmin = length(p - uSun * min(b, 0.0));
  lit = smoothstep(1.0 - 0.0015 * uAtmoScale, 1.0 + 0.0015 * uAtmoScale, dmin);
  vec3 od = vec3(0.0);
  const int NL = 6;
  float ds = t1 / float(NL);
  for (int i = 0; i < NL; i++) {
    od += atmoDensity(p + uSun * ds * (float(i) + 0.5));
  }
  return od * ds;
}

// In-scattered light along dir from the camera up to distance tMax, and the
// transmittance of that segment.
vec3 atmosphere(vec3 o, vec3 d, float tMax, out vec3 trans) {
  trans = vec3(1.0);
  float b = dot(o, d);
  float c = dot(o, o) - RA * RA;
  float disc = b * b - c;
  if (disc <= 0.0) return vec3(0.0);
  float sq = sqrt(disc);
  float t0 = max(-b - sq, 0.0);
  float t1 = min(-b + sq, tMax);
  if (t1 <= t0) return vec3(0.0);
  const int NV = 16;
  // Rays that end on the ground put most of their samples near the ground,
  // where the haze is (its scale height is 1.2 km; spread evenly over a
  // path hundreds of km long, 16 samples would miss it entirely).
  bool hit = tMax < -b + sq;
  vec3 odV = vec3(0.0);
  vec3 sumR = vec3(0.0), sumM = vec3(0.0), sumMs = vec3(0.0);
  for (int i = 0; i < NV; i++) {
    float u0 = float(i) / float(NV), u1 = float(i + 1) / float(NV);
    float ta = hit ? t0 + (t1 - t0) * (1.0 - pow(1.0 - u0, 3.0)) : mix(t0, t1, u0);
    float tb = hit ? t0 + (t1 - t0) * (1.0 - pow(1.0 - u1, 3.0)) : mix(t0, t1, u1);
    float ds = tb - ta;
    vec3 p = o + d * (0.5 * (ta + tb));
    vec3 dens = atmoDensity(p);
    odV += dens * ds * 0.5;
    float lit;
    vec3 odL = sunDepth(p, lit);
    vec3 att = extinction(odV + odL) * lit;
    sumR += dens.x * att * ds;
    sumM += dens.y * att * ds;
    // Multiple scattering, approximated as an isotropic fill proportional to
    // how much sunlight the sample receives (after Hillaire's psi_ms): it is
    // what lifts the veil over the whole day side and softens the terminator.
    float up = clamp(dot(normalize(p), uSun) + 0.25, 0.0, 1.0);
    sumMs += (dens.x * BR + dens.y * BMS) * extinction(odV) * up * ds;
    odV += dens * ds * 0.5;
  }
  trans = extinction(odV);
  float mu = dot(d, uSun);
  float pR = 3.0 / (16.0 * PI) * (1.0 + mu * mu);
  const float g = 0.78;
  float pM = 3.0 / (8.0 * PI) * ((1.0 - g * g) * (1.0 + mu * mu))
           / ((2.0 + g * g) * pow(1.0 + g * g - 2.0 * g * mu, 1.5));
  return sumR * BR * pR + sumM * BMS * pM + sumMs * 0.035;
}

// Worley (cellular) noise: distance to the nearest of one random point per
// cell. 1 - F1 is the "billow" shape -- rounded lobes packed together, the
// cauliflower tops of cumulus. Nubis (Schneider, Guerrilla 2015-2017) builds
// cloud shapes from exactly this mix of Perlin and inverted Worley noise,
// driven by a coverage map; here the coverage map is the satellite.
vec3 hash33(vec3 p) {
  p = fract(p * vec3(0.1031, 0.1030, 0.0973));
  p += dot(p, p.yxz + 33.33);
  return fract((p.xxy + p.yxx) * p.zyx);
}
float worley(vec3 x) {
  vec3 i = floor(x), f = fract(x);
  float d = 1.0;
  for (int z = -1; z <= 1; z++)
  for (int y = -1; y <= 1; y++)
  for (int xx = -1; xx <= 1; xx++) {
    vec3 g = vec3(float(xx), float(y), float(z));
    vec3 r = g + hash33(i + g) - f;
    d = min(d, dot(r, r));
  }
  return sqrt(d);
}

vec2 sphereUv(vec3 n) {
  return vec2(atan(n.z, n.x) / TAU + 0.5, 0.5 - asin(clamp(n.y, -1.0, 1.0)) / PI);
}

void main() {
  vec2 uv = (gl_FragCoord.xy - uOrigin) / uRes;
  vec2 p = (uv - uCenter) * 2.0;
  p.x *= uRes.x / uRes.y;

  vec3 dir = normalize(uBasis[0] * p.x + uBasis[1] * p.y + uBasis[2] * uFocal);

  float b = dot(uCamPos, dir);
  float c = dot(uCamPos, uCamPos) - 1.0;
  float disc = b * b - c;

  vec3 additive = vec3(0.0);
  vec3 surface = vec3(0.0);
  float coverage = 0.0;

  // --- the air, seen edge-on past the limb ---------------------------------
  // Two falloffs rather than one: a tight band that hugs the horizon and a
  // wide bloom behind it. A single exponential either stops too abruptly --
  // which reads as a hard cut-out against the stars -- or washes the whole
  // sky blue.
  // This vector is the ray's nearest approach to the centre, so its length is
  // the impact parameter: above 1 the ray misses the planet, below 1 it hits.
  // Running the glow off that one number on *both* sides of 1 is what keeps
  // the limb continuous. Evaluating it only outside leaves a few pixels just
  // inside the silhouette with the surface already faded and no glow yet --
  // a thin black arc traced around the planet, which is precisely the hard
  // edge this is supposed to avoid.
  vec3 closest = uCamPos + dir * max(-b, 0.0);
  float rim = length(closest);
  float sunward = dot(normalize(closest), uSun);
  // Air keeps glowing past the terminator: that is twilight, and it is why
  // the night limb fades out rather than stopping dead.
  // Forward scattering: when the sun is behind the planet the whole ring of
  // air is lit from behind. This is why, in the reference at midnight, the
  // night-side limb still glows pale blue all the way round, while at dusk
  // the ring stops at the terminator.
  float fwd = pow(max(dot(dir, uSun), 0.0), 1.5);
  float twilight = max(smoothstep(-0.30, 0.12, sunward), fwd * 0.9);
  // The colour of sunlit air seen edge-on. Measured off the reference, it is
  // a pale, fairly neutral grey-blue -- nothing like the saturated electric
  // blue a naive Rayleigh tint gives -- and it fades into space over a few
  // percent of the planet's radius rather than stopping at a bright rim.
  vec3 haze = mix(vec3(0.30, 0.39, 0.50), vec3(0.47, 0.62, 0.82), clamp(sunward * 1.4 + 0.3, 0.0, 1.0));
  // Sunset tint only in the band right at the terminator, not across the
  // whole night side.
  float dusk = smoothstep(-0.25, -0.05, sunward) * (1.0 - smoothstep(-0.02, 0.18, sunward));
  haze = mix(haze, vec3(0.62, 0.44, 0.36), dusk * 0.7);
  {
    // The tight part of the glow runs off |h| so it continues a little way
    // inside the silhouette too; gating it to h > 0 leaves a dark arc just
    // inside the edge, where the ground has already faded into the haze but
    // the glow has not started yet. The wide part only exists outside.
    float h = rim - 1.0;
    // Inside, the glow steps down only as the ground fades *in*; with a fixed
    // lower value there is a pixel or two where the ground is already gone and
    // the glow has dropped, which draws a dark line round the planet.
    float feather = smoothstep(0.0, 0.010, disc) * step(0.0, -b);
    // Apple's limb is a thick, bright blue-white band: a tight core that
    // hugs the horizon and a broad bloom behind it.
    float tight = exp(-abs(h) * 28.0) * (h > 0.0 ? 0.80 : mix(0.80, 0.42, feather));
    float wide = exp(-max(h, 0.0) * 6.0) * 0.20 * step(0.0, h);
    // Past the edge the glow is thinner air lit from further away, so it
    // turns bluer as it fades out.
    vec3 outer = mix(haze, vec3(0.30, 0.47, 0.80), smoothstep(0.0, 0.05, h));
    if (uAtmo < 0.5) additive += outer * (tight + wide) * twilight;
    else {
      // The reference's limb is brighter and wider than physics gives (its
      // rim is stylised). Keep a share of the painted rim and bloom on top
      // of the physical scattering.
      additive += outer * (tight * 0.35 + wide * 1.1) * twilight;
    }
  }
  vec3 atmoT = vec3(1.0);
  vec3 atmoIn = vec3(0.0);
  if (uAtmo > 0.5) {
    float tSurf = (disc > 0.0 && -b - sqrt(max(disc, 0.0)) > 0.0) ? -b - sqrt(disc) : 1.0e9;
    atmoIn = atmosphere(uCamPos, dir, tSurf, atmoT);
    // Into the display's space with a soft exposure curve, so the thick limb
    // saturates to a luminous pale blue instead of clipping.
    // Looking straight down the reference keeps the sea a deep, dark navy
    // and saves the veil for long paths toward the limb; scale the light by
    // air mass so short paths contribute less than the physics says.
    float airMass = 1.0 / max(dot(normalize(uCamPos + dir * min(tSurf, 10.0)), -dir), 0.05);
    atmoIn *= mix(0.40, 1.0, smoothstep(1.2, 4.0, tSurf < 1.0e8 ? airMass : 10.0));
    atmoIn = 1.0 - exp(-atmoIn * uAtmoExposure);
    atmoIn = pow(atmoIn, vec3(1.0 / 1.6));
    // Light reaching the night-side limb has crossed the whole terminator and
    // comes out sunset-red; the reference keeps that ring a pale blue-white.
    float nightRing = 1.0 - smoothstep(0.0, 0.45, sunward);
    float lumA = dot(atmoIn, vec3(0.2126, 0.7152, 0.0722));
    atmoIn = mix(atmoIn, lumA * vec3(0.78, 0.92, 1.18), nightRing);
    // Added outside the coverage feather for hits and misses alike, so the
    // glow runs continuously across the silhouette.
    additive += atmoIn;
  }

  // Surface point and texture derivatives are worked out for *every* pixel,
  // before the hit test. Screen-space derivatives taken inside the branch are
  // undefined for the 2x2 pixel quads that straddle the silhouette -- half the
  // quad misses the planet -- and the driver answers with the coarsest mip:
  // the average colour of the whole Earth, a dark line traced round the limb.
  // Rays that miss are pinned to the nearest point on the limb instead, which
  // keeps their neighbours' derivatives sane.
  float tHit = -b - sqrt(max(disc, 0.0));
  vec3 n = (disc > 0.0 && tHit > 0.0) ? normalize(uCamPos + dir * tHit) : normalize(closest);
  vec2 suv = sphereUv(n);
  // Unwrap the derivative across the 180-degree seam, or textureGrad picks
  // the coarsest mip there and draws a bright scar down the Pacific.
  vec2 ddx = dFdx(suv);
  vec2 ddy = dFdy(suv);
  ddx.x = fract(ddx.x + 0.5) - 0.5;
  ddy.x = fract(ddy.x + 0.5) - 0.5;

  if (disc > 0.0) {
    float t = tHit;
    if (t > 0.0) {

      vec3 dayCol   = textureGrad(uDay,    suv, ddx, ddy).rgb;
      vec3 nightCol = textureGrad(uNight,  suv, ddx, ddy).rgb;
      float water   = textureGrad(uWater,  suv, ddx, ddy).r;
      vec3 nmap     = textureGrad(uNormalMap, suv, ddx, ddy).rgb * 2.0 - 1.0;

      // Tangent frame: x east, y north. The relief map is generated from the
      // GEBCO elevation grid in exactly that convention (see tools/), so the
      // two have to agree or every mountain range is lit from the wrong side.
      vec3 tangent = normalize(vec3(-n.z, 0.0, n.x));
      vec3 bitan = cross(tangent, n);
      vec3 bumped = normalize(tangent * nmap.x + bitan * nmap.y + n * max(nmap.z, 0.2));
      // Strong relief: most of the "rich" look of the reference is mountain
      // ranges modelled by the light.
      vec3 N = normalize(mix(n, bumped, 0.9));

      float ndl = dot(n, uSun);
      // A wide terminator: the real one is softened by hundreds of kilometres
      // of atmosphere, and a narrow one looks like a stencil.
      float day = smoothstep(-0.17, 0.23, ndl);
      // Slightly wrapped diffuse: skylight fills in the far side of the lit
      // hemisphere, so the reference stays bright well away from the
      // sub-solar point instead of falling off like a bare Lambert sphere.
      float diff = clamp((dot(N, uSun) + 0.12) / 1.12, 0.0, 1.0);

      vec3 sunTint = mix(vec3(1.05, 0.79, 0.60), vec3(1.0), smoothstep(0.0, 0.45, ndl));
      // Blue Marble's open ocean is close to navy; seen through the air it
      // reads much lighter. Lift the water on its own, before the land.
      // The graded texture already carries the ocean colour; only a touch of
      // skylight on top.
      // (The baked ocean already has its final colour.)
      vec3 lit = dayCol * sunTint * (0.035 + 1.18 * pow(diff, 0.9));

      vec3 V = -dir;
      vec3 H = normalize(uSun + V);
      // --- the sea ----------------------------------------------------------
      // Water takes the sphere's own normal: the relief map carries sea-floor
      // bathymetry, and bumping the sea surface with it lights the trenches
      // through a kilometre of water.
      float mu = clamp(dot(n, V), 0.0, 1.0);
      // Schlick Fresnel. Face-on the sea reflects 2% of the sky; toward the
      // limb it turns into a mirror of it, which is the silver sheen the ocean
      // takes on near the horizon in orbital photographs.
      float fres = 0.02 + 0.98 * pow(1.0 - mu, 5.0);
      lit += haze * fres * water * day * 0.34;
      // Sun glint in two lobes: a broad, dim shimmer (the spread of wave
      // slopes) around a tight bright core. One lobe either looks like a
      // plastic highlight or a pinprick; the pair reads as sunlight on water.
      float ndh = max(dot(n, H), 0.0);
      float glint = pow(ndh, 900.0) * 0.12;
      lit += vec3(1.0, 0.95, 0.84) * glint * water * day;
      // Land keeps a faint sheen off wet ground and leaves, on the relief.
      lit += vec3(1.0, 0.96, 0.87) * pow(max(dot(N, H), 0.0), 96.0) * (1.0 - water) * day * 0.06;

      // --- clouds -----------------------------------------------------------
      // The live map is derived from infrared imagery, so cold clear ground
      // reads as a faint grey haze. Levels push that back to black and let the
      // real cloud decks stay white.
      vec2 cuv = vec2(fract(suv.x + uCloudShift), suv.y);
      vec3 east = tangent;
      vec3 north = bitan;
      vec2 toSun = vec2(dot(uSun, east), dot(uSun, north));
      float coslat = max(0.2, sqrt(1.0 - n.y * n.y));
      // Geostationary satellites cannot see the poles; the map's polar rows
      // are a smeared fill that turns into a pinwheel on a globe. Sample it
      // much softer up there.
      float polar = smoothstep(0.90, 0.985, abs(n.y));
      float blurK = mix(1.3, 16.0, polar);
      // Cloud tops stand up to 14 km above the ground (G channel of the map,
      // from Himawari's infrared). Parallax: a tall cloud is seen displaced
      // toward the horizon, so near the limb towers lean over the ground
      // they stand on and stacked layers slide apart -- depth, not a decal.
      const float HMAX = 14.0 / 6360.0;           // in earth radii
      vec3 Vw = -dir;
      float muV = max(dot(n, Vw), 0.12);
      vec3 vT = Vw - n * dot(Vw, n);
      float h0 = textureGrad(uClouds, cuv, ddx * 6.0, ddy * 6.0).g;
      vec3 offW = vT / muV * h0 * HMAX;
      cuv += vec2(dot(offW, east) / (coslat * TAU), -dot(offW, north) / PI);
      vec3 cloudRGB = textureGrad(uClouds, cuv, ddx * blurK, ddy * blurK).rgb;
      vec2 cloudRG = cloudRGB.rg;
      float thinC = cloudRGB.b;
      float cloudRaw = cloudRG.r;
      float cTop = cloudRG.g;                      // 0..1 of HMAX
      float base = smoothstep(uCloudLevels.x, uCloudLevels.y, cloudRaw);

      // The map is ~5 km a pixel and soft; real cloud has structure far below
      // that. Procedural detail, evaluated on the sphere and drifting slowly,
      // erodes the thin parts of each deck into wisps and streets while the
      // thick cores stay solid -- where the data says cloud, there is cloud,
      // the noise only decides its texture.
      vec3 q = n * 48.0 + vec3(uHours * 0.021, 0.0, uHours * 0.013);
      float warp = vnoise(q * 0.37 + 11.0);
      float det = fbm(q + warp * 2.2);
      float fine = fbm(q * 3.3 + 5.0);
      // --- shaping the satellite map into cloud ---------------------------
      // The satellite gives where and how much. The shape of the edges comes
      // from the kind of cloud, which the infrared top height tells us:
      // tall convective cloud (thunderstorms, the typhoon's bands) gets
      // billowy, lobed edges from inverted Worley noise; low decks and thin
      // cloud get wispy, streaky edges from Perlin-type noise. Edges are
      // eroded by that shape (Nubis's remap), cores stay whole.
      float conv = smoothstep(0.25, 0.70, cTop);
      vec3 qc = n * 190.0 + vec3(uHours * 0.05, 0.0, uHours * 0.03);
      float billow = (1.0 - worley(qc)) * 0.62 + (1.0 - worley(qc * 2.9 + 7.0)) * 0.38;
      float wispy = det * 0.55 + fine * 0.45;
      float shapeN = mix(wispy, billow, conv);
      float erode = (1.0 - shapeN) * mix(0.55, 0.80, conv) * uCloudDetail;
      // At night the reference's clouds are soft grey masses; no lobes.
      erode *= mix(0.35, 1.0, day);
      float body = clamp((base - erode) / max(1.0 - erode, 0.05), 0.0, 1.0);
      // keep a faint trace of what was eroded, so edges fade, not cut
      body = max(body, base * 0.22 * shapeN);
      // Opacity climbs slowly: most of a deck is translucent, only the cores
      // hide the ground completely.
      // Opacity follows density smoothly from zero -- no threshold, so edges
      // fade out instead of ending in a cut-out rim.
      // Stylised the way the reference is: opacity follows how tall a cloud
      // is, not only how much light it reflects. Deep convective cores and
      // anvils (cold, high tops) are solid white; low decks and stratus --
      // which reflect nearly as much from above -- become translucent veils
      // with the sea showing through. That hierarchy of thin to thick is
      // most of what separates layered cloud from a flat white stencil.
      float thick = smoothstep(0.10, 0.62, cTop);
      body = pow(body, mix(1.7, 1.0, thick));
      float bodyA = pow(body, 1.05) * mix(0.60, 0.97, thick);
      // A thin veil wherever the map has any cloud at all: cirrus and haze
      // lying over the ground, which is most of what makes the reference's
      // cloud look layered instead of stencilled.
      // The veil layer, drawn streaky: the same wispy noise, stretched along
      // east-west (the prevailing flow in both hemispheres' mid-latitudes and
      // the trades) so thin cloud reads as sheets and strands, not fog.
      vec3 qs = vec3(n.x * 60.0, n.y * 240.0, n.z * 60.0) + vec3(uHours * 0.03, 0.0, 0.0);
      float streak = fbm(qs) * 0.6 + fine * 0.4;
      float veil = clamp(thinC * (0.25 + 1.1 * streak), 0.0, 1.0) * (1.0 - bodyA);
      // Over land the thin-cloud estimate is unreliable (bright ground, warm
      // or cold surfaces fool both channels); keep it light there.
      veil *= mix(0.3, 1.0, water);
      veil *= mix(0.25, 1.0, day);          // thin cloud barely shows at night
      float cloud = clamp(bodyA + veil * 0.42, 0.0, 1.0) * uCloudOpacity;

      // Cloud-top relief: the same detail field sampled a step toward the sun.
      // Where it falls away the top faces the light; where it rises it is in
      // its own shade. Cheap, but it gives every deck volume.
      vec3 sunT = normalize(uSun - n * dot(uSun, n) + 1e-5);
      float detSun = fbm(q + sunT * 0.55 + warp * 2.2);
      float relief = clamp((det - detSun) * 3.2, -1.0, 1.0) * uCloudDetail;

      // Cloud shadow on the ground, cast along the real sun direction: a
      // cloud at height h throws its shadow h / tan(sun elevation) away, so
      // anvils drop long shadows at low sun and cumulus sit on their own.
      float sinE = max(dot(n, uSun), 0.06);
      float tanE = sinE / sqrt(max(1.0 - sinE * sinE, 1e-4));
      vec2 sunUv = vec2(dot(sunT, east) / (coslat * TAU), -dot(sunT, north) / PI);
      float hCast = textureGrad(uClouds, cuv + sunUv * (0.35 * HMAX / tanE), ddx * 3.0, ddy * 3.0).g;
      vec2 shadowUv = cuv + sunUv * (max(hCast, 0.08) * HMAX / tanE);
      float shade = smoothstep(uCloudLevels.x, uCloudLevels.y,
                               textureGrad(uClouds, shadowUv, ddx * 3.0, ddy * 3.0).r) * uCloudOpacity;
      lit *= 1.0 - 0.50 * shade * (1.0 - cloud);

      // --- the night side ----------------------------------------------------
      // The map is NASA's Black Marble 2016 with its moonlit ground stripped
      // at bake time (only warm, artificial light survives), so it is black
      // wherever nobody lives. A power above 1 keeps suburbs dim and lets city
      // cores burn.
      vec3 cityRaw = max(nightCol - vec3(0.004), 0.0);
      // Many small warm-yellow points rather than a few orange blobs: a lower
      // power lets towns show, and the colour is sodium-yellow as on the iPad.
      // A power below 1 lifts the countless small towns so the lit regions read
      // as a fine yellow dust (as on the iPad), then a soft shoulder keeps the
      // megacities from blooming into white blobs.
      vec3 core = pow(cityRaw, vec3(0.80)) * 0.85;
      core = core / (1.0 + dot(core, vec3(0.3, 0.5, 0.2)) * 1.1);
      // Sodium amber, as in the reference, rather than the map's pale yellow;
      // only the densest cores run toward white.
      float coreL = dot(core, vec3(0.3, 0.5, 0.2));
      // Golden, saturated; only the brightest cores lean toward warm white.
      core = mix(coreL * vec3(1.32, 0.92, 0.34), coreL * vec3(1.25, 1.02, 0.55), smoothstep(0.4, 1.0, coreL));
      // Light spill: the same map a few mip levels softer, so bright metros sit
      // in a warm haze of their own instead of ending at their last street lamp.
      vec3 spill = textureGrad(uNight, suv, ddx * 7.0, ddy * 7.0).rgb;
      // A tight halo round each light, a couple of pixels: the reference's
      // lights glow like a photograph, not like printed dots.
      vec3 halo = textureGrad(uNight, suv, ddx * 2.5, ddy * 2.5).rgb;
      spill = pow(spill, vec3(1.5)) * vec3(1.20, 0.90, 0.40) * 0.14;
      // Moonlit / starlit ground: barely there, but it keeps the continents
      // from vanishing entirely at night.
      // Apple's night side is not black: the ground stays visible, dim and
      // warm, the sea a deep navy, like a moonlit photograph.
      // Measured at 22:39 on the iPad: north China (51,41,25), south China
      // (26,30,21), Sea of Japan (15,26,34) -- a clearly visible, moonlit
      // earth, not a black disc.
      vec3 moonlit = mix(mix(dayCol, vec3(dot(dayCol, vec3(0.3, 0.5, 0.2))), 0.35) * vec3(0.24, 0.24, 0.20) + vec3(0.020, 0.017, 0.011), dayCol * vec3(0.16, 0.19, 0.20) + vec3(0.016, 0.020, 0.026), water);
      // Street lights come on in the dusk, not across the wide soft terminator
      // the ground uses -- otherwise cities glow white in the afternoon.
      float dark = 1.0 - smoothstep(-0.10, 0.03, ndl);
      spill *= dark;
      halo = pow(halo, vec3(0.9)) * vec3(1.25, 0.90, 0.38) * 0.45;
      vec3 lights = (core * (1.0 - 0.85 * cloud) + (spill + halo) * (1.0 - 0.4 * cloud)) * uCityLights * dark + moonlit;
      surface = mix(lights, lit, day);

      // Clouds float above the relief, so they are lit by the sphere's own
      // normal. Using the terrain normal here paints every mountain ridge's
      // shadow onto the cloud deck above it.
      // Cloud tops as a height field: the density map's own slope tilts the
      // normal, so each cumulus tower and each spiral band is lit on its
      // sunward flank and shaded on the far one -- the puffy, sculpted look of
      // the reference. Sampled ~2 px apart at the current mip so the
      // shading scale follows the zoom.
      // Shape of the cloud top: a height field made of the infrared top
      // height (towers, anvils, the eye wall) and the density (the rounded
      // body of each deck). Its slope gives the normal.
      vec2 e1 = vec2(max(length(ddx), length(ddy)) * 1.6, 0.0);
      vec2 dE = vec2(e1.x / coslat, 0.0), dN = vec2(0.0, e1.x);
      // Two scales: fine (a few pixels, the texture of each deck) and coarse
      // (the rounded mass of a whole system). The infrared height is 2 km a
      // pixel, so it is read a few mips soft or its pixels show as dimples.
      float bk = blurK * 2.5;
      vec2 rE = textureGrad(uClouds, cuv + dE, ddx * bk, ddy * bk).rg;
      vec2 rW = textureGrad(uClouds, cuv - dE, ddx * bk, ddy * bk).rg;
      vec2 rN = textureGrad(uClouds, cuv - dN, ddx * bk, ddy * bk).rg;
      vec2 rS = textureGrad(uClouds, cuv + dN, ddx * bk, ddy * bk).rg;
      float fE = rE.r * 0.7 + rE.g * 0.5, fW = rW.r * 0.7 + rW.g * 0.5;
      float fN = rN.r * 0.7 + rN.g * 0.5, fS = rS.r * 0.7 + rS.g * 0.5;
      float bc = blurK * 10.0;
      vec2 qE = textureGrad(uClouds, cuv + dE * 6.0, ddx * bc, ddy * bc).rg;
      vec2 qW = textureGrad(uClouds, cuv - dE * 6.0, ddx * bc, ddy * bc).rg;
      vec2 qN = textureGrad(uClouds, cuv - dN * 6.0, ddx * bc, ddy * bc).rg;
      vec2 qS = textureGrad(uClouds, cuv + dN * 6.0, ddx * bc, ddy * bc).rg;
      float gE = qE.r * 0.6 + qE.g, gW = qW.r * 0.6 + qW.g;
      float gN = qN.r * 0.6 + qN.g, gS = qS.r * 0.6 + qS.g;
      float bumpK = mix(1.1, 2.2, smoothstep(0.25, 0.7, cTop));
      vec3 cN3 = normalize(n - bumpK * ((fE - fW) * east + (fN - fS) * north)
                             - 0.7 * ((gE - gW) * east + (gN - gS) * north));
      // Cavities: where this spot is thinner than its surroundings it sits
      // in a trough between lobes and gets less sky -- the grey folds that
      // give a storm its sculpted body.
      float around = 0.25 * (qE.r + qW.r + qN.r + qS.r);
      float cavity = clamp((around - cloudRaw) * 2.2, 0.0, 1.0);

      // Self-shadowing: march a few steps toward the sun through the height
      // field; anything standing higher than the sun ray puts this spot in
      // shade. The flanks of towers and the lee of every band go dark.
      float here = cTop * HMAX;
      float occl = 0.0;
      for (int i = 1; i <= 4; i++) {
        float dist = float(i * i) * 0.0012 * (0.4 + cTop);     // radii
        float hs = textureGrad(uClouds, cuv + sunUv * dist, ddx * 6.0, ddy * 6.0).g * HMAX;
        float rayH = here + dist * tanE;
        occl = max(occl, clamp((hs - rayH) / (0.25 * HMAX), 0.0, 1.0));
      }
      float sunVis = 1.0 - 0.75 * occl * day;

      // Light: warm direct sun on the lit side, cool blue skylight in the
      // shade. That colour split -- not brightness alone -- is what makes
      // the reference's clouds read as volumes.
      float lamb = clamp((dot(cN3, uSun) + 0.15) / 1.15, 0.0, 1.0);
      float lambFlat = clamp((dot(n, uSun) + 0.22) / 1.22, 0.0, 1.0);
      float direct = mix(lambFlat, lamb, day) * sunVis;
      vec3 sunCol = vec3(1.06, 1.02, 0.95);
      vec3 skyCol = vec3(0.50, 0.60, 0.78);
      vec3 cloudLightC = sunCol * direct + skyCol * (0.18 + 0.20 * dot(cN3, n)) * mix(0.3, 1.0, day);
      // Night: clouds stay visible as a soft cool grey (typhoon ~ (30,31,34)),
      // still shaped by their own relief.
      cloudLightC += vec3(0.11, 0.115, 0.13) * (0.7 + 0.3 * clamp(dot(cN3, n), 0.0, 1.0)) * (1.0 - day);
      cloudLightC *= 1.0 + 0.10 * relief * day;
      cloudLightC *= 1.0 - 0.45 * cavity;
      float cloudLight = dot(cloudLightC, vec3(0.2126, 0.7152, 0.0722));

      // Colour by kind: high, cold tops (cirrus, anvils) are the brightest
      // pure white; low decks a touch grey; thin veil translucent blue-grey.
      vec3 cloudCol = mix(vec3(0.70, 0.76, 0.86), vec3(1.10, 1.09, 1.08), sqrt(body));
      cloudCol *= mix(0.86, 1.12, thick);
      cloudCol = mix(cloudCol * vec3(0.80, 0.84, 0.92), cloudCol, day);
      cloudCol *= cloudLightC / max(cloudLight, 1e-3);   // tint by the light's colour
      // At night clouds are only a faint grey hint, as in the reference.
      surface = mix(surface, cloudCol * cloudLight, cloud * mix(0.40, 1.0, day));
      // Cities light the underside of the cloud deck above them.
      surface += spill * cloud * (1.0 - day) * uCityLights * 0.20;

      // Film response: a long, soft shoulder. Sunlit cloud tops land around
      // 0.75 instead of clipping to white, which is most of what makes the
      // reference read as "photographed" rather than "rendered".
      // The shoulder is applied mostly to luminance, so bright colours keep
      // their hue: a per-channel curve flattens a sunlit desert toward beige
      // and every ocean toward grey, which is a large part of why the planet
      // looked thin next to the reference.
      surface *= 1.42;
      vec3 perChannel = 1.40 * surface / (surface + 0.89);
      float lum = dot(surface, vec3(0.2126, 0.7152, 0.0722));
      vec3 byLum = surface * (1.40 / (lum + 0.89));
      surface = min(mix(perChannel, byLum, 0.6), vec3(1.2));

      float grey = dot(surface, vec3(0.30, 0.59, 0.11));
      // Under the physical veil the ground needs a little more colour of its
      // own, or haze on top of it reads as grey rather than as air.
      // Land gets a little extra colour under the veil; the sea does not --
      // the reference's ocean is a restrained, slightly grey navy.
      float satK = uAtmo > 0.5 ? mix(1.34, 1.28, water * (1.0 - cloud)) : 1.0;
      surface = mix(vec3(grey), surface, satK);
      if (uAtmo > 0.5) surface *= mix(1.0, 0.88, water * (1.0 - cloud));

      // Aerial perspective. Even looking straight down there is a column of
      // lit air between the camera and the ground, so nothing on the day side
      // is ever truly dark; toward the limb that column gets long and the
      // ground dissolves into the haze. This is the veil over the whole disc
      // in the reference, and it is also what makes the edge soft.
      // Thin over the middle of the disc so the ground keeps its colour, then
      // climbing fast toward the limb, where the land dissolves into blue.
      float airPath = 0.07 + 0.93 * pow(1.0 - mu, 3.0);
      float airDay = max(smoothstep(-0.40, 0.26, ndl), fwd * 0.55);
      // Toward the limb the air column is long and brightly lit: the disc's
      // edge turns a pale luminous blue rather than darkening.
      vec3 airlight = mix(vec3(0.02, 0.03, 0.05), haze * mix(1.0, 1.35, pow(1.0 - mu, 3.0)), airDay);
      if (uAtmo < 0.5) {
        surface = mix(surface, airlight, clamp(airPath, 0.0, 1.0) * mix(0.25, 0.58, airDay));
      } else {
        // Ground seen through the air: dimmed and shifted by the path's
        // transmittance, with the light scattered into the path laid over it.
        // Only part of the extinction is applied: the reference's haze adds
        // light over the ground far more than it dims it, which keeps the
        // ocean a deep saturated blue under the veil.
        // Extinction dims the ground but, physically, also turns it orange on
        // long paths; the reference dims without the colour shift.
        vec3 tG = pow(atmoT, vec3(1.0 / 2.2));
        tG = mix(tG, vec3(dot(tG, vec3(0.2126, 0.7152, 0.0722))), 0.75);
        surface = surface * mix(vec3(1.0), tG, uAtmoExtinction);
      }

      // Feather the silhouette into the haze instead of ending on a hard
      // circle. The glow above is still being added outside it, so the two
      // meet in the middle.
      coverage = smoothstep(0.0, 0.010, disc);
    }
  }

  // --- the limb, as the eye sees it ---------------------------------------------
  // In the reference the edge of the planet is not a line. Toward the limb
  // the ground and clouds dissolve into a pale, luminous blue-white haze;
  // past the edge that same haze keeps going and turns warm and grey as it
  // thins into space, spreading several percent of the radius. The planet
  // and its air read as one object. Built here in screen space so it is
  // always a few dozen pixels wide, and wide enough that no step shows.
  if (uAtmo > 0.5) {
    float hL = rim - 1.0;                         // <0 inside, >0 outside
    float px = max(fwidth(hL), 1e-6);             // radii per pixel
    // How lit the air at this point of the limb is. Measured on the reference
    // (iPad, 2026-09-23 08:10): the ring is full strength well onto the day
    // side, fades through the twilight arc and is gone about 30 degrees past
    // the terminator -- it never switches off in a step, which is what made
    // our old hard floor read as a drawn-on white circle.
    float day = smoothstep(-0.17, -0.05, sunward);
    // How near this piece of limb is to the terminator. Through the twilight
    // arc the reference's halo loses its blue and goes a neutral pale grey
    // long before it fades out -- the long grazing path through the air.
    float blue = smoothstep(-0.10, 0.05, sunward);
    float lit = max(day, fwd * 0.8);
    // A trace survives all the way round: on a fully night-side view the
    // limb itself is at dawn and dusk, and the reference keeps it glowing.
    lit = max(lit, 0.06);
    // Measured off the reference: on the day side the edge haze is a muted
    // grey-blue; through the twilight arc it loses its blue and goes a
    // neutral pale grey (long grazing paths redden the light).
    vec3 pale = mix(vec3(0.78, 0.76, 0.72), vec3(0.60, 0.70, 0.82), blue);
    // Outer glow: a cool steel blue fading to black on the day side, turning
    // the same neutral grey where the sun is near the horizon.
    float wIn = uLimbIn;                          // radii
    float wOut = uLimbOut;
    // Inside: the ground keeps its colour much closer to the edge than it
    // used to -- in the reference you can still read coastlines a pixel or
    // two from the silhouette, and only the very last sliver goes to haze.
    float inner = exp(-max(-hL, 0.0) / (wIn * 0.75));
    surface = mix(surface, pale * (0.7 + 0.3 * lit), min(inner * 1.25, 1.0) * 0.62 * mix(0.35, 1.0, lit));
    // Anti-aliased silhouette: a couple of pixels, never less.
    float aa = 1.0 - smoothstep(-1.5 * px, 1.5 * px, hL);
    coverage = max(min(coverage, aa), 0.0);
    // Outside: the air does not fade out in one colour. Going up from the
    // horizon the reference runs white-blue, then cyan, then a deep blue
    // before it reaches black -- the band of colour that makes the edge read
    // as air rather than as a drawn outline. Two falloffs carry it: a tight
    // one hugging the silhouette and a broad halo, each with its own colour.
    float o = max(hL, 0.0);
    float aNear = exp(-o / (wIn * 1.9)) * 0.26;
    float aFar = exp(-o / wOut) * 0.52;
    vec3 cNear = mix(vec3(0.82, 0.86, 0.94), pale, 0.30);
    vec3 cMid = mix(vec3(0.44, 0.44, 0.44), vec3(0.30, 0.58, 0.86), blue);
    vec3 cFar = mix(vec3(0.20, 0.19, 0.18), vec3(0.05, 0.17, 0.40), blue);
    vec3 glow = cNear * aNear + mix(cMid, cFar, smoothstep(wIn * 1.5, wOut * 1.7, o)) * aFar;
    // The physical scattering peaks in a razor-thin line exactly at the
    // horizon; that is the hard edge the eye does not see. Let it through
    // only well inside the disc.
    additive *= mix(0.15, 1.0, smoothstep(0.5 * wIn, 3.0 * wIn, -hL));
    // Long grazing paths redden the scattered light; the reference keeps the
    // edge a cool blue-white, so pull it toward the haze colour there.
    float aL = dot(additive, vec3(0.2126, 0.7152, 0.0722));
    additive = mix(additive, aL * pale / dot(pale, vec3(0.2126, 0.7152, 0.0722)), exp(-max(-hL, 0.0) / (2.5 * wIn)));
    additive += glow * mix(0.10, 1.0, lit) * (1.0 - coverage);
  }
  vec3 color = surface * coverage + additive;
  color += (hash21(gl_FragCoord.xy) - 0.5) / 255.0;
  fragColor = vec4(color, coverage);
}
`;

/// Builds the cloud-density map the globe samples (unit 2), from two sources:
///
/// * the global live map (matteason, a geostationary infrared composite,
///   every three hours) -- everywhere, and the only source at night. It
///   carries pixel speckle, which is filled here, and its levels are applied
///   here so the output is plain 0..1 cloud density;
/// * Himawari-9's red visible channel (NASA GIBS, 1 km, every ten minutes)
///   over the half of the planet Himawari sees, which includes everything
///   the default view from Osaka shows. It is sunlight reflected off real
///   cloud tops: spiral bands, cloud streets, thin cirrus, the texture that
///   makes Apple's clouds look like clouds. An infrared map cannot show that.
///
/// Reflectance becomes density by subtracting what the clear ground would
/// reflect (the Blue Marble day map is itself a cloud-free red-band composite,
/// so it is the right reference), after compensating for the sun's height.
/// Himawari is used only where it is daytime, where the satellite sees the
/// spot at a reasonable angle, and inside the served area; outside, or at
/// night, the infrared map takes over, feathered.
const COMPOSE_FRAG = `#version 300 es
precision highp float;
uniform sampler2D uSrc;        // live IR map (raw)
uniform sampler2D uDayMap;     // Blue Marble day map, for clear-sky reflectance
uniform sampler2D uGeoA;       // Himawari visible, lon 70..180, lat -80..80
uniform sampler2D uGeoB;       // Himawari visible, lon -180..-150
uniform float uGeoOn;
uniform vec3  uGeoSun;         // sun direction at the Himawari capture time
uniform vec2  uLevels;         // IR map black / white point
uniform float uSrcOn;          // 0 when there is no live IR map yet
uniform float uStaticShift;    // for the bundled map when it stands in
uniform sampler2D uBtA;        // Himawari band 13 brightness temperature, same areas as uGeoA/B
uniform sampler2D uBtB;
uniform float uBtOn;
out vec4 fragColor;
const float PI  = 3.141592653589793;
const float TAU = 6.283185307179586;
// The satellite's disc is served as two images that meet at the antimeridian
// (70..180 and -180..-150). Sampling stops at each image's own edge -- the
// bilinear clamp, and the alpha mip cut off there -- which drew a hairline
// straight down the middle of the Pacific. Within a degree and a half of the
// seam both images are read (the same column of the same scan is the last
// one in A and the first one in B) and cross-faded, so value and coverage
// come out continuous across it.
vec2 geoUV(float lon, float lat) { return vec2((lon - 70.0) / 110.0, (80.0 - lat) / 160.0); }
float seamMix(float lon) { return 0.5 * (1.0 - smoothstep(0.0, 1.5, 180.0 - abs(lon))); }
float geoSample(vec2 lonlat, out float cover) {
  float lon = lonlat.x, lat = lonlat.y;
  float v = (80.0 - lat) / 160.0;
  bool sideA = lon >= 70.0;
  vec2 uvA = vec2(sideA ? clamp((lon - 70.0) / 110.0, 0.0, 1.0) : 1.0, v);
  vec2 uvB = vec2(sideA ? 0.0 : clamp((lon + 180.0) / 30.0, 0.0, 1.0), v);
  float m = seamMix(lon);
  float va = texture(uGeoA, uvA).r;
  float vb = texture(uGeoB, uvB).r;
  // Alpha from a soft mip, so the satellite's disc edge fades rather than
  // cuts. Both images are 11.4 px per degree, so the same mip level on each
  // means the same amount of smoothing -- different levels were themselves
  // a step at the seam.
  float aa = textureLod(uGeoA, uvA, 5.0).a;
  float ab = textureLod(uGeoB, uvB, 5.0).a;
  cover = smoothstep(0.85, 1.0, sideA ? mix(aa, ab, m) : mix(ab, aa, m));
  return sideA ? mix(va, vb, m) : mix(vb, va, m);
}
// Band 13 brightness temperature, same two images, same seam treatment.
float btSample(float lon, float lat) {
  float v = (80.0 - lat) / 160.0;
  bool sideA = lon >= 70.0;
  vec2 uvA = vec2(sideA ? clamp((lon - 70.0) / 110.0, 0.0, 1.0) : 1.0, v);
  vec2 uvB = vec2(sideA ? 0.0 : clamp((lon + 180.0) / 30.0, 0.0, 1.0), v);
  float m = seamMix(lon);
  // One mip soft: palette-decoding leaves isolated wrong pixels.
  float ta = textureLod(uBtA, uvA, 1.5).r;
  float tb = textureLod(uBtB, uvB, 1.5).r;
  // A missing reading is zero; blending one in would fake a warm top.
  if (min(ta, tb) < 0.004) return sideA ? ta : tb;
  return sideA ? mix(ta, tb, m) : mix(tb, ta, m);
}
void main() {
  ivec2 size = textureSize(uSrc, 0);
  ivec2 p = ivec2(gl_FragCoord.xy);
  vec2 uv = (vec2(p) + 0.5) / vec2(size);
  float lon = (uv.x - 0.5) * 360.0;
  float lat = (0.5 - uv.y) * 180.0;

  // --- infrared map, despeckled, levels applied -------------------------------
  float ir = 0.0;
  if (uSrcOn > 0.5) {
    float c = texelFetch(uSrc, p, 0).r;
    float sum = 0.0, wsum = 0.0;
    for (int dy = -2; dy <= 2; dy++) {
      for (int dx = -2; dx <= 2; dx++) {
        if (dx == 0 && dy == 0) continue;
        ivec2 q = ivec2((p.x + dx + size.x) % size.x, clamp(p.y + dy, 0, size.y - 1));
        float w = (abs(dx) <= 1 && abs(dy) <= 1) ? 1.0 : 0.5;
        sum += texelFetch(uSrc, q, 0).r * w;
        wsum += w;
      }
    }
    ir = smoothstep(uLevels.x, uLevels.y, max(c, sum / wsum * 0.96));
    // No geostationary satellite sees past ~70 degrees; the map's polar rows
    // are a smeared fill that renders as a bright pinwheel cap. Thin it out.
    ir *= 1.0 - 0.75 * smoothstep(60.0, 78.0, abs(lat));
  } else {
    ir = texture(uSrc, vec2(fract(uv.x + uStaticShift), uv.y)).r;
  }

  float cloud = ir;
  // Thin cloud and haze: what the main map throws away as "clear". The
  // reference is full of it -- translucent veils over most of the ocean.
  float thin = smoothstep(0.05, 0.45, ir) * 0.5;
  // Fallback cloud-top height where there is no infrared: thicker = higher.
  float height = ir * 0.35;
  bool inHima = abs(lat) < 79.0 && (lon >= 70.0 || lon <= -150.0);
  float la0 = radians(lat), lo0 = radians(lon);
  vec3 n0 = vec3(cos(la0) * cos(lo0), sin(la0), cos(la0) * sin(lo0));
  float satView0 = dot(n0, vec3(cos(radians(140.7)), 0.0, sin(radians(140.7))));
  float edge0 = min(min(lon >= 70.0 ? lon - 70.0 : 999.0, lon <= -150.0 ? -150.0 - lon : 999.0), 79.0 - abs(lat));
  if (uBtOn > 0.5 && inHima) {
    // Cloud-top height from how much colder the top is than the air near the
    // surface: about 6.5 C per km. The surface temperature is a smooth
    // latitude climatology -- plenty for telling a thunderstorm anvil
    // (-70 C, 14 km) from a stratocumulus deck (+10 C, 1-2 km).
    float tb = btSample(lon, lat);
    if (tb > 0.004) {
      float T = tb * 255.0 / 2.0 - 95.0;
      float tSurf = 29.0 - 0.30 * abs(lat) - 0.006 * lat * lat;
      float dT = tSurf - T;
      float hIr = clamp(dT / 6.5 / 14.0, 0.0, 1.0);
      float irCloud = smoothstep(10.0, 38.0, dT);
      float wIr = smoothstep(0.12, 0.35, satView0) * smoothstep(0.0, 4.0, edge0);
      // Night side: the infrared itself is the cloud map, at 2 km instead of
      // the global map's 5, and ten minutes old instead of hours.
      float czNow = dot(n0, uGeoSun);
      float nightW = 1.0 - smoothstep(0.02, 0.20, czNow);
      cloud = mix(cloud, irCloud, wIr * nightW);
      // Cirrus: cold (high) but not thick -- the infrared sees it even where
      // visible light barely does.
      thin = max(thin, smoothstep(10.0, 40.0, dT) * 0.8 * wIr);
      height = mix(height, hIr, wIr);
    }
  }
  if (uGeoOn > 0.5 && inHima) {
    float la = radians(lat), lo = radians(lon);
    vec3 n = vec3(cos(la) * cos(lo), sin(la), cos(la) * sin(lo));
    float cz = dot(n, uGeoSun);
    // Himawari-9 hangs over 140.7 E.
    float slo = radians(140.7);
    float satView = dot(n, vec3(cos(slo), 0.0, sin(slo)));
    float cover;
    float v = geoSample(vec2(lon, lat), cover);
    // GIBS serves reflectance close to linear; the day map is sRGB.
    vec3 day = textureLod(uDayMap, uv, 0.0).rgb;
    float clear = 0.03 + 0.9 * pow(day.r, 2.2);
    float refl = v / pow(max(cz, 0.12), 0.6);
    // Thin cloud matters: most of what makes the reference look layered is
    // semi-transparent veil a few percent brighter than the ground.
    float geo = pow(clamp((refl - clear - 0.018) / (0.50 - clear), 0.0, 1.0), 0.8);
    float edge = min(min(lon >= 70.0 ? lon - 70.0 : 999.0, lon <= -150.0 ? -150.0 - lon : 999.0), 79.0 - abs(lat));
    float w = cover
      * smoothstep(0.05, 0.22, cz)          // daylight only
      * smoothstep(0.18, 0.40, satView)     // not too near the satellite's limb
      * smoothstep(0.0, 4.0, edge);
    cloud = mix(cloud, geo, w);
    // Faint reflectance above the clear-ground level: thin stratus, haze,
    // cloud streets too small to resolve.
    float geoThin = smoothstep(0.004, 0.07, refl - clear);
    thin = mix(thin, max(thin * 0.6, geoThin), w);
  }
  // Height only means something where there is cloud.
  height *= smoothstep(0.02, 0.25, cloud);
  thin *= 1.0 - smoothstep(0.35, 0.9, cloud);   // only where there is no deck
  fragColor = vec4(cloud, height, thin, 1.0);
}
`;

// --- the lens -------------------------------------------------------------------
// Apple's picture is soft the way a photograph is: bright cloud tops and the
// limb bleed a little light into their surroundings, and the whole frame has a
// faint veil of scattered light. That is bloom -- a blur pyramid of the frame
// added back on top. Rendered once per wallpaper frame, it costs nothing.
const POST_VERT = `#version 300 es
in vec2 aPos;
out vec2 vUv;
void main() { vUv = aPos * 0.5 + 0.5; gl_Position = vec4(aPos, 0.0, 1.0); }
`;
// Downsample with a 13-tap filter (Jimenez 2014); the first pass also applies
// a soft threshold so only bright things bloom.
const POST_DOWN = `#version 300 es
precision highp float;
in vec2 vUv;
uniform sampler2D uSrc;
uniform vec2 uTexel;
uniform float uThreshold;   // < 0: no threshold
out vec4 o;
vec3 s(vec2 d) { return texture(uSrc, vUv + d * uTexel).rgb; }
void main() {
  vec3 a = s(vec2(-2, 2)), b = s(vec2(0, 2)), c = s(vec2(2, 2));
  vec3 d = s(vec2(-2, 0)), e = s(vec2(0, 0)), f = s(vec2(2, 0));
  vec3 g = s(vec2(-2, -2)), h = s(vec2(0, -2)), i = s(vec2(2, -2));
  vec3 j = s(vec2(-1, 1)), k = s(vec2(1, 1)), l = s(vec2(-1, -1)), m = s(vec2(1, -1));
  vec3 col = e * 0.125 + (a + c + g + i) * 0.03125 + (b + d + f + h) * 0.0625 + (j + k + l + m) * 0.125;
  if (uThreshold >= 0.0) {
    float br = max(col.r, max(col.g, col.b));
    float knee = 0.25;
    float soft = clamp(br - uThreshold + knee, 0.0, 2.0 * knee);
    soft = soft * soft / (4.0 * knee + 1e-4);
    col *= max(soft, br - uThreshold) / max(br, 1e-4);
  }
  o = vec4(col, 1.0);
}
`;
// Upsample with a 3x3 tent and add onto the next finer level.
const POST_UP = `#version 300 es
precision highp float;
in vec2 vUv;
uniform sampler2D uSrc;
uniform vec2 uTexel;
out vec4 o;
void main() {
  vec3 c = vec3(0.0);
  c += texture(uSrc, vUv + vec2(-1, 1) * uTexel).rgb * 1.0;
  c += texture(uSrc, vUv + vec2(0, 1) * uTexel).rgb * 2.0;
  c += texture(uSrc, vUv + vec2(1, 1) * uTexel).rgb * 1.0;
  c += texture(uSrc, vUv + vec2(-1, 0) * uTexel).rgb * 2.0;
  c += texture(uSrc, vUv).rgb * 4.0;
  c += texture(uSrc, vUv + vec2(1, 0) * uTexel).rgb * 2.0;
  c += texture(uSrc, vUv + vec2(-1, -1) * uTexel).rgb * 1.0;
  c += texture(uSrc, vUv + vec2(0, -1) * uTexel).rgb * 2.0;
  c += texture(uSrc, vUv + vec2(1, -1) * uTexel).rgb * 1.0;
  o = vec4(c / 16.0, 1.0);
}
`;
const POST_COMPOSITE = `#version 300 es
precision highp float;
in vec2 vUv;
uniform sampler2D uScene;
uniform sampler2D uBloom;   // thresholded glow pyramid, summed
uniform sampler2D uVeil;    // very wide blur of the whole frame
uniform float uBloomK;
uniform float uVeilK;
out vec4 o;
void main() {
  vec3 c = texture(uScene, vUv).rgb;
  vec3 bl = texture(uBloom, vUv).rgb;
  vec3 v = texture(uVeil, vUv).rgb;
  // Screen-blend the glow so highlights brighten without clipping hard.
  c = 1.0 - (1.0 - c) * (1.0 - bl * uBloomK);
  c = mix(c, max(c, v), uVeilK);
  o = vec4(c, 1.0);
}
`;

/// Camera basis in the earth-fixed frame.
///
/// The camera sits `distance` earth radii from the centre, above (lat, lon),
/// starts out looking straight down with north up, then turns by `heading`
/// about the vertical and pitches up by `tilt` toward the horizon.
///
/// Right has to be eye x north, not north x eye. The other order points
/// "right" at the west, and the whole planet renders as its mirror image --
/// Korea and China on the far side of Japan -- which is easy to miss when the
/// view is mostly ocean and cloud.
function cameraBasis(latDeg, lonDeg, distance, tiltDeg = 0, headingDeg = 0) {
  const eye = toVector(latDeg, lonDeg);
  const pos = eye.map((v) => v * distance);
  let forward = eye.map((v) => -v);

  let upHint = [0, 1, 0];
  if (Math.abs(eye[1]) > 0.999) upHint = [0, 0, 1];

  let right = normalize(cross(eye, upHint));
  let up = cross(right, eye);

  const h = (headingDeg * Math.PI) / 180;
  [right, up] = [
    right.map((v, i) => v * Math.cos(h) + up[i] * Math.sin(h)),
    right.map((v, i) => -v * Math.sin(h) + up[i] * Math.cos(h)),
  ];
  const t = (tiltDeg * Math.PI) / 180;
  [forward, up] = [
    forward.map((v, i) => v * Math.cos(t) + up[i] * Math.sin(t)),
    forward.map((v, i) => -v * Math.sin(t) + up[i] * Math.cos(t)),
  ];
  return { pos, basis: [right, up, forward] };
}

async function main() {
  const canvas = document.getElementById("gl");
  const gl = canvas.getContext("webgl2", {
    alpha: false,
    antialias: false,
    depth: false,
    powerPreference: "low-power",
    preserveDrawingBuffer: false,
  });

  if (!gl) {
    document.body.classList.add("failed");
    return;
  }

  let config = { ...DEFAULTS };
  let home = { lat: 34.6736, lon: 135.5433 };
  let screens = null;
  try {
    const [fromDisk, location, layout] = await Promise.all([
      invoke("get_wallpaper_config"),
      invoke("get_location"),
      invoke("get_screen_layout"),
    ]);
    if (fromDisk) config = { ...DEFAULTS, ...fromDisk };
    if (location) home = location;
    if (layout) screens = layout;
  } catch (e) {
    console.warn("falling back to built-in wallpaper settings", e);
  }

  /// The canvas spans every monitor. Framing has to be measured against one
  /// of them, or on a two-monitor desktop the globe ends up centred on the
  /// seam between them -- half on each screen, and the composition that was
  /// designed for one screen is nowhere to be seen.
  /// Where the camera's optical axis meets the canvas, and how the framing
  /// monitor's height compares with the canvas's. Offsets and focal length are
  /// in half-heights of the framing monitor, which keeps the composition the
  /// same on a 4:3 tablet and a 16:9 desktop: the vertical framing matches and
  /// a wider screen simply shows more planet to either side.
  function framing() {
    const virt = screens?.virtual;
    const frame =
      config.frame_monitor === "virtual" ? virt : screens?.primary ?? virt;
    const ox = config.axis_offset_x || 0;
    const oy = config.axis_offset_y || 0;
    if (!virt || !frame || !virt.w || !virt.h) {
      const aspect = canvas.clientWidth / Math.max(1, canvas.clientHeight);
      return { center: [0.5 + (ox * 0.5) / aspect, 0.5 - oy * 0.5], heightRatio: 1 };
    }
    const half = frame.h / 2;
    return {
      center: [
        (frame.x - virt.x + frame.w / 2 + ox * half) / virt.w,
        1 - (frame.y - virt.y + frame.h / 2 + oy * half) / virt.h,
      ],
      heightRatio: frame.h / virt.h,
    };
  }

  /// The rectangles to draw a globe into, each with its own framing.
  ///
  /// With `each_monitor` on (the default) every monitor gets a globe of its
  /// own, composed exactly as the primary's: the offsets and focal length are
  /// in half-heights of *that* monitor, so a secondary screen of a different
  /// size or shape shows the same picture scaled to its height. Off, it is
  /// the old behaviour -- one scene spanning the whole canvas, framed on one
  /// monitor, with the other screens showing whatever lies beside it.
  function views() {
    const W = canvas.width;
    const H = canvas.height;
    const ox = config.axis_offset_x || 0;
    const oy = config.axis_offset_y || 0;
    const virt = screens?.virtual;
    const mons = screens?.monitors;
    if (config.each_monitor !== false && config.frame_monitor !== "virtual"
        && virt && virt.w && virt.h && Array.isArray(mons) && mons.length) {
      const sx = W / virt.w;
      const sy = H / virt.h;
      const out = [];
      for (const m of mons) {
        const x = Math.round((m.x - virt.x) * sx);
        const top = Math.round((m.y - virt.y) * sy);
        const w = Math.round((m.x + m.w - virt.x) * sx) - x;
        const h = Math.round((m.y + m.h - virt.y) * sy) - top;
        if (w < 1 || h < 1) continue;
        // The composition was measured on a 4:3 iPad. On a wider screen,
        // keep the same picture across the width and crop top and bottom
        // (like the iPad image cut to 16:9) instead of keeping the height and
        // showing more planet at the sides, which makes Japan small and the
        // globe look like a different lens.
        const ref = config.frame_aspect || 4 / 3;
        const k = config.frame_fit === "height" ? 1 : Math.max(1, (w / h) / ref);
        out.push({
          x, y: H - top - h, w, h,
          center: [0.5 + (ox * k * 0.5 * h) / w, 0.5 - oy * k * 0.5],
          heightRatio: k,
        });
      }
      if (out.length) return out;
    }
    const f = framing();
    if (config.frame_monitor !== "virtual" && !(virt && virt.w)) {
      // No monitor information (e.g. a browser preview): same width-fit rule.
      const ref = config.frame_aspect || 4 / 3;
      const k = config.frame_fit === "height" ? 1 : Math.max(1, (W / H) / ref);
      return [{ x: 0, y: 0, w: W, h: H,
        center: [0.5 + (ox * k * 0.5 * H) / W, 0.5 - oy * k * 0.5], heightRatio: k }];
    }
    return [{ x: 0, y: 0, w: W, h: H, center: f.center, heightRatio: f.heightRatio }];
  }

  let globe;
  let sky;
  try {
    globe = program(gl, VERT, FRAG, "globe");
    sky = createSky(gl);
  } catch (e) {
    console.error(e);
    document.body.classList.add("failed");
    return;
  }

  const globeU = uniforms(gl, globe, [
    "uRes", "uOrigin", "uCenter", "uFocal", "uBasis", "uCamPos", "uSun",
    "uCloudShift", "uCloudOpacity", "uCloudLevels", "uCityLights", "uHours", "uCloudDetail", "uAtmo", "uAtmoExposure", "uHaze", "uAtmoScale", "uAtmoExtinction", "uLimbIn", "uLimbOut",
    "uDay", "uNight", "uClouds", "uWater", "uNormalMap",
  ]);

  const fullscreen = gl.createBuffer();
  gl.bindBuffer(gl.ARRAY_BUFFER, fullscreen);
  gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), gl.STATIC_DRAW);

  gl.useProgram(globe);
  const textures = [
    [{ relief: "textures/earth_day.jpg", calm: "textures/earth_day_calm.jpg" }[config.ocean_style]
      ?? "textures/earth_day_apple.jpg", "uDay", 0],
    ["textures/earth_night.jpg", "uNight", 1],
    ["textures/earth_clouds.jpg", "uClouds", 2],
    ["textures/earth_water.jpg", "uWater", 3],
    ["textures/earth_normal.jpg", "uNormalMap", 4],
  ];
  const pending = textures.map(([url, name, unit]) => {
    gl.uniform1i(globeU[name], unit);
    return loadTexture(gl, url, unit);
  });

  // Today's real clouds, when the network allows. Until the first map arrives,
  // and whenever a refresh fails, the bundled NASA composite stays in place --
  // it is a fine-looking sky, just not today's.
  let cloudsAreLive = false;
  let onCloudsChanged = () => {};
  let cloudBucket = null;
  let irLoaded = false;
  // Upload only when there is a new map. Re-uploading the same 8K texture
  // (and rebuilding its mip chain) on every check stalls the GPU for a
  // visible frame even though the picture has not changed.
  async function refreshLiveClouds() {
    if (!config.live_clouds || !config.live_clouds_url) return;
    // Hidden (wallpaper switched off, or covered): no downloads, no GPU
    // work. The next visibility change catches up.
    if (document.hidden) return;
    const bucket = Math.floor(Date.now() / LIVE_CLOUD_REFRESH_MS);
    if (bucket === cloudBucket) return;
    const sep = config.live_clouds_url.includes("?") ? "&" : "?";
    const ok = await loadTexture(gl, `${config.live_clouds_url}${sep}t=${bucket}`, 2, { crossOrigin: true });
    if (ok) {
      cloudBucket = bucket;
      irLoaded = true;
      recompose();
    }
  }

  // --- Himawari visible ----------------------------------------------------------
  const GEO_A = 7;
  const GEO_B = 8;
  let geoTime = null;       // capture time of what is loaded
  let geoOn = false;
  function geoUrl(t, bbox, w, h) {
    return (config.geo_clouds_url || GEO_DEFAULT_URL)
      .replace("{TIME}", t.toISOString().replace(".000Z", "Z"))
      .replace("{BBOX}", bbox).replace("{W}", w).replace("{H}", h);
  }
  // GIBS answers a time it does not have yet with a tiny transparent PNG, so
  // probe with a thumbnail first and only then pull the full image.
  async function geoAvailable(t) {
    try {
      const r = await fetch(geoUrl(t, "-80,70,80,180", 64, 93), { mode: "cors" });
      if (!r.ok) return false;
      const img = await createImageBitmap(await r.blob());
      const c = new OffscreenCanvas(64, 93).getContext("2d");
      c.drawImage(img, 0, 0);
      const d = c.getImageData(0, 0, 64, 93).data;
      let opaque = 0;
      for (let i = 3; i < d.length; i += 4) if (d[i] > 200) opaque++;
      return opaque > 64 * 93 * 0.3;
    } catch { return false; }
  }
  // --- Himawari infrared: cloud-top temperature, day and night ----------------------
  const BT_A = 12;
  const BT_B = 13;
  let btOn = false;
  function uploadR8(unit, img) {
    let tex = btTextures[unit];
    if (!tex) {
      tex = gl.createTexture();
      btTextures[unit] = tex;
    }
    gl.activeTexture(gl.TEXTURE0 + unit);
    gl.bindTexture(gl.TEXTURE_2D, tex);
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.R8, img.width, img.height, 0, gl.RED, gl.UNSIGNED_BYTE, img.data);
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 4);
    gl.generateMipmap(gl.TEXTURE_2D);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR_MIPMAP_LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
  }
  const btTextures = {};
  async function fetchBitmap(url) {
    const r = await fetch(url, { mode: "cors" });
    if (!r.ok) throw new Error(`${r.status}`);
    const blob = await r.blob();
    if (blob.size < 2000) throw new Error("empty");
    return createImageBitmap(blob);
  }

  async function refreshGeoClouds() {
    if (!config.live_clouds || config.geo_clouds === false) return;
    if (document.hidden) return;
    const step = 10 * 60000;
    const newest = Math.floor((Date.now() - 45 * 60000) / step) * step;
    for (let k = 0; k < 5; k++) {
      const t = new Date(newest - k * step);
      if (geoTime && t.getTime() <= geoTime.getTime()) return;   // nothing newer
      if (!(await geoAvailable(t))) continue;
      const scale = Math.max(0.25, Math.min(1, config.geo_clouds_scale || 1));
      const hA = Math.round(3641 * scale);
      // Visible light only while part of Himawari's view is in daylight.
      const ss = subsolarPoint(t);
      const d = ((ss.lon - 140.7 + 540) % 360) - 180;
      const wantVis = Math.abs(d) < 115;
      const irUrl = (bbox, w, h) => geoUrl(t, bbox, w, h)
        .replace("Himawari_AHI_Band3_Red_Visible_1km", "Himawari_AHI_Band13_Clean_Infrared");
      // Infrared is 2 km, so half the visible resolution loses nothing.
      const irH = Math.round(hA / 2);
      const jobs = [
        fetchBitmap(irUrl("-80,70,80,180", Math.round(1252 * scale), irH)),
        fetchBitmap(irUrl("-80,-180,80,-150", Math.round(342 * scale), irH)),
      ];
      if (wantVis) {
        jobs.push(loadTexture(gl, geoUrl(t, "-80,70,80,180", Math.round(2503 * scale), hA), GEO_A, { crossOrigin: true, wrapX: false }));
        jobs.push(loadTexture(gl, geoUrl(t, "-80,-180,80,-150", Math.round(683 * scale), hA), GEO_B, { crossOrigin: true, wrapX: false }));
      }
      const res = await Promise.allSettled(jobs);
      if (res[0].status === "fulfilled" && res[1].status === "fulfilled") {
        try {
          uploadR8(BT_A, decodeCloudTop(res[0].value));
          uploadR8(BT_B, decodeCloudTop(res[1].value));
          btOn = true;
        } catch (e) { console.warn("cloud-top decode failed", e); }
      }
      geoOn = wantVis && res[2]?.value === true && res[3]?.value === true;
      console.warn(`clouds: himawari ${t.toISOString()} visible=${geoOn} infrared=${btOn}`);
      geoTime = t;
      recompose();
      return;
    }
  }

  // Cloud-density map, single channel with a full mip chain, bound to unit 2.
  let compose = null;
  function recompose() {
    try { composeClouds(); } catch (e) { console.warn("cloud compose skipped", e); return; }
    cloudsAreLive = true;
    onCloudsChanged();
  }
  function composeClouds() {
    const raw = textureOf(2);
    if (!raw) return;
    if (!compose) {
      const prog = program(gl, VERT, COMPOSE_FRAG, "cloud compose");
      compose = {
        prog,
        u: uniforms(gl, prog, ["uSrc", "uDayMap", "uGeoA", "uGeoB", "uGeoOn", "uGeoSun",
          "uLevels", "uSrcOn", "uStaticShift", "uBtA", "uBtB", "uBtOn"]),
        fbo: gl.createFramebuffer(),
        tex: null,
        w: 0,
        h: 0,
      };
    }
    const size = irLoaded ? textureSizeOf(2) : [8192, 4096];
    if (!size) return;
    if (!compose.tex || compose.w !== size[0] || compose.h !== size[1]) {
      if (compose.tex) gl.deleteTexture(compose.tex);
      compose.tex = gl.createTexture();
      compose.w = size[0];
      compose.h = size[1];
      // Bind on a scratch unit. Binding on whatever unit happens to be
      // active replaces that unit's texture -- which, depending on load
      // order, was the day map on unit 0, and the whole planet went black.
      gl.activeTexture(gl.TEXTURE15);
      gl.bindTexture(gl.TEXTURE_2D, compose.tex);
      const levels = Math.floor(Math.log2(Math.max(size[0], size[1]))) + 1;
      // R: cloud density; G: cloud-top height (0..1 = 0..14 km).
      gl.texStorage2D(gl.TEXTURE_2D, levels, gl.RGBA8, size[0], size[1]);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.REPEAT);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR_MIPMAP_LINEAR);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    }
    gl.bindFramebuffer(gl.FRAMEBUFFER, compose.fbo);
    gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, compose.tex, 0);
    if (gl.checkFramebufferStatus(gl.FRAMEBUFFER) !== gl.FRAMEBUFFER_COMPLETE) {
      gl.bindFramebuffer(gl.FRAMEBUFFER, null);
      return;
    }
    gl.disable(gl.BLEND);
    gl.disable(gl.SCISSOR_TEST);
    gl.viewport(0, 0, size[0], size[1]);
    gl.useProgram(compose.prog);
    gl.activeTexture(gl.TEXTURE2);
    gl.bindTexture(gl.TEXTURE_2D, raw);
    const U = compose.u;
    gl.uniform1i(U.uSrc, 2);
    gl.uniform1i(U.uDayMap, 0);
    gl.uniform1i(U.uGeoA, GEO_A);
    gl.uniform1i(U.uGeoB, GEO_B);
    gl.uniform1f(U.uGeoOn, geoOn ? 1 : 0);
    const gs = geoTime ? sunState(geoTime).direction : [1, 0, 0];
    gl.uniform3f(U.uGeoSun, gs[0], gs[1], gs[2]);
    gl.uniform2f(U.uLevels, 0.36, 0.96);
    gl.uniform1f(U.uSrcOn, irLoaded ? 1 : 0);
    gl.uniform1f(U.uStaticShift, ((Date.now() / 3600000) * 7) / 360 % 1);
    gl.uniform1i(U.uBtA, BT_A);
    gl.uniform1i(U.uBtB, BT_B);
    gl.uniform1f(U.uBtOn, btOn ? 1 : 0);
    gl.bindBuffer(gl.ARRAY_BUFFER, fullscreen);
    const aPos = gl.getAttribLocation(compose.prog, "aPos");
    gl.enableVertexAttribArray(aPos);
    gl.vertexAttribPointer(aPos, 2, gl.FLOAT, false, 0, 0);
    gl.drawArrays(gl.TRIANGLES, 0, 3);
    gl.disableVertexAttribArray(aPos);
    gl.bindFramebuffer(gl.FRAMEBUFFER, null);
    gl.activeTexture(gl.TEXTURE2);
    gl.bindTexture(gl.TEXTURE_2D, compose.tex);
    gl.generateMipmap(gl.TEXTURE_2D);
    // The globe samples unit 2; from now on that is the composed map. The next
    // IR refresh rebinds the raw texture there before uploading into it.
    gl.useProgram(globe);
  }
  function resize() {
    const scale = Math.min(1, Math.max(0.25, config.render_scale || 1));
    const dpr = window.devicePixelRatio || 1;
    const w = Math.max(1, Math.round(canvas.clientWidth * dpr * scale));
    const h = Math.max(1, Math.round(canvas.clientHeight * dpr * scale));
    if (canvas.width !== w || canvas.height !== h) {
      canvas.width = w;
      canvas.height = h;
      gl.viewport(0, 0, w, h);
    }
  }

  // Half-angle the globe subtends, turned into the focal length that makes
  // `zoom === 1` put the limb exactly at the top and bottom of the screen.
  const distance = Math.max(1.02, config.camera_distance);
  const focalBase = Math.max(0.05, config.focal_length);

  function cameraLongitude(sun, now) {
    const drift = (config.drift_deg_per_hour || 0) * (now.getTime() / 3600000);
    switch (config.camera_anchor) {
      case "sun":
        return sun.lon + config.camera_offset_deg + drift;
      case "fixed":
        return config.camera_offset_deg + drift;
      default:
        return home.lon + config.camera_offset_deg + drift;
    }
  }

  // --- bloom pyramid -------------------------------------------------------------
  const post = {
    down: program(gl, POST_VERT, POST_DOWN, "bloom down"),
    up: program(gl, POST_VERT, POST_UP, "bloom up"),
    comp: program(gl, POST_VERT, POST_COMPOSITE, "bloom composite"),
    w: 0, h: 0, scene: null, levels: [], veilLevels: [],
  };
  function makeTarget(w, h) {
    const tex = gl.createTexture();
    gl.activeTexture(gl.TEXTURE15);            // scratch unit, see composeClouds
    gl.bindTexture(gl.TEXTURE_2D, tex);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA8, w, h, 0, gl.RGBA, gl.UNSIGNED_BYTE, null);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    const fbo = gl.createFramebuffer();
    gl.bindFramebuffer(gl.FRAMEBUFFER, fbo);
    gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, tex, 0);
    return { tex, fbo, w, h };
  }
  function freeTarget(t) { if (t) { gl.deleteTexture(t.tex); gl.deleteFramebuffer(t.fbo); } }
  function ensurePost() {
    const W = canvas.width, H = canvas.height;
    if (post.w === W && post.h === H && post.scene) return;
    freeTarget(post.scene);
    post.levels.forEach(freeTarget);
    post.scene = makeTarget(W, H);
    post.levels = [];
    let w = W, h = H;
    for (let i = 0; i < 7; i++) {
      w = Math.max(1, w >> 1); h = Math.max(1, h >> 1);
      post.levels.push(makeTarget(w, h));
    }
    post.w = W; post.h = H;
    gl.bindFramebuffer(gl.FRAMEBUFFER, null);
  }
  function postPass(prog, src, dst, extra) {
    gl.bindFramebuffer(gl.FRAMEBUFFER, dst ? dst.fbo : null);
    gl.viewport(0, 0, dst ? dst.w : canvas.width, dst ? dst.h : canvas.height);
    gl.useProgram(prog);
    gl.activeTexture(gl.TEXTURE9);
    gl.bindTexture(gl.TEXTURE_2D, src.tex);
    gl.uniform1i(gl.getUniformLocation(prog, "uSrc"), 9);
    gl.uniform2f(gl.getUniformLocation(prog, "uTexel"), 1 / src.w, 1 / src.h);
    if (extra) extra(prog);
    gl.bindBuffer(gl.ARRAY_BUFFER, fullscreen);
    const aPos = gl.getAttribLocation(prog, "aPos");
    gl.enableVertexAttribArray(aPos);
    gl.vertexAttribPointer(aPos, 2, gl.FLOAT, false, 0, 0);
    gl.drawArrays(gl.TRIANGLES, 0, 3);
    gl.disableVertexAttribArray(aPos);
  }
  function applyBloom() {
    const L = post.levels;
    gl.disable(gl.BLEND);
    gl.disable(gl.SCISSOR_TEST);
    // One pyramid serves both: its summed levels are the glow, and a coarse
    // level of it doubles as the wide veil.
    let src = post.scene;
    for (let i = 0; i < L.length; i++) {
      const th = i === 0 ? config.bloom_threshold : -1;
      postPass(post.down, src, L[i], (p) => gl.uniform1f(gl.getUniformLocation(p, "uThreshold"), th));
      src = L[i];
    }
    // Upsample additively back to level 0.
    gl.enable(gl.BLEND);
    gl.blendFunc(gl.ONE, gl.ONE);
    for (let i = L.length - 1; i > 0; i--) {
      postPass(post.up, L[i], L[i - 1]);
    }
    gl.disable(gl.BLEND);
    // Composite onto the canvas.
    gl.bindFramebuffer(gl.FRAMEBUFFER, null);
    gl.viewport(0, 0, canvas.width, canvas.height);
    gl.useProgram(post.comp);
    gl.activeTexture(gl.TEXTURE9);
    gl.bindTexture(gl.TEXTURE_2D, post.scene.tex);
    gl.activeTexture(gl.TEXTURE10);
    gl.bindTexture(gl.TEXTURE_2D, L[0].tex);
    gl.activeTexture(gl.TEXTURE11);
    gl.bindTexture(gl.TEXTURE_2D, L[3].tex);
    gl.uniform1i(gl.getUniformLocation(post.comp, "uScene"), 9);
    gl.uniform1i(gl.getUniformLocation(post.comp, "uBloom"), 10);
    gl.uniform1i(gl.getUniformLocation(post.comp, "uVeil"), 11);
    gl.uniform1f(gl.getUniformLocation(post.comp, "uBloomK"), config.bloom);
    gl.uniform1f(gl.getUniformLocation(post.comp, "uVeilK"), config.veil);
    gl.bindBuffer(gl.ARRAY_BUFFER, fullscreen);
    const aPos = gl.getAttribLocation(post.comp, "aPos");
    gl.enableVertexAttribArray(aPos);
    gl.vertexAttribPointer(aPos, 2, gl.FLOAT, false, 0, 0);
    gl.drawArrays(gl.TRIANGLES, 0, 3);
    gl.disableVertexAttribArray(aPos);
  }

  function draw() {
    resize();
    const usePost = config.bloom > 0 || config.veil > 0;
    if (usePost) {
      ensurePost();
      gl.bindFramebuffer(gl.FRAMEBUFFER, post.scene.fbo);
    } else {
      gl.bindFramebuffer(gl.FRAMEBUFFER, null);
    }
    const now = new Date();
    const subsolar = subsolarPoint(now);
    const sun = sunState(now);
    const moon = moonState(now);
    const siderealDeg = gmst(now);

    const { pos, basis } = cameraBasis(
      config.camera_lat_deg,
      cameraLongitude(subsolar, now),
      distance,
      config.camera_tilt_deg,
      config.camera_heading_deg
    );

    const basisArray = new Float32Array([
      basis[0][0], basis[0][1], basis[0][2],
      basis[1][0], basis[1][1], basis[1][2],
      basis[2][0], basis[2][1], basis[2][2],
    ]);

    // The reference's sky is a warm, very dark grey, not blue-black.
    gl.disable(gl.SCISSOR_TEST);
    gl.viewport(0, 0, canvas.width, canvas.height);
    // Apple's sky is black; the warmth the old grey stood in for is the Milky
    // Way, now drawn for real.
    gl.clearColor(0.004, 0.004, 0.004, 1.0);
    gl.clear(gl.COLOR_BUFFER_BIT);

    // Premultiplied over: a fragment with alpha 0 is added, which is how
    // starlight, glare and the atmosphere all want to behave.
    gl.enable(gl.BLEND);
    gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);

    const spin = spinMatrix(siderealDeg);
    const moonPole = equatorialToEarthFixed(ECLIPTIC_POLE.ra, ECLIPTIC_POLE.dec, siderealDeg);
    const list = views();
    if (list.length > 1) gl.enable(gl.SCISSOR_TEST);

    for (const v of list) {
      gl.viewport(v.x, v.y, v.w, v.h);
      if (list.length > 1) gl.scissor(v.x, v.y, v.w, v.h);
      const focal = focalBase * v.heightRatio;
      const cam = {
        width: v.w,
        height: v.h,
        aspect: v.w / v.h,
        origin: [v.x, v.y],
        center: v.center,
        focal,
        basis,
        basisArray,
        pos,
      };

      const pointScale = Math.max(1, (v.h * v.heightRatio / 1080) * 0.85);
      sky.drawMilkyWay(cam, siderealDeg, config.milky_way, config.nebula);
      sky.drawStars(cam, spin, pointScale, config.stars);
      sky.drawSun(cam, sun, config.sun);
      sky.drawMoon(cam, moon, sun, moonPole, config.moon);

      gl.useProgram(globe);
      gl.bindBuffer(gl.ARRAY_BUFFER, fullscreen);
      const aPos = gl.getAttribLocation(globe, "aPos");
      gl.enableVertexAttribArray(aPos);
      gl.vertexAttribPointer(aPos, 2, gl.FLOAT, false, 0, 0);

      gl.uniform2f(globeU.uRes, v.w, v.h);
      gl.uniform2f(globeU.uOrigin, v.x, v.y);
      gl.uniform2f(globeU.uCenter, cam.center[0], cam.center[1]);
      gl.uniform1f(globeU.uFocal, focal);
      gl.uniformMatrix3fv(globeU.uBasis, false, basisArray);
      gl.uniform3f(globeU.uCamPos, pos[0], pos[1], pos[2]);
      gl.uniform3f(globeU.uSun, sun.direction[0], sun.direction[1], sun.direction[2]);
      // A live map already is today's weather, so it stays put. The static one
      // drifts slowly so the same cloud is not parked over the same city forever.
      gl.uniform1f(globeU.uCloudShift,
        cloudsAreLive ? 0 : ((now.getTime() / 3600000) * 7) / 360 % 1);
      gl.uniform1f(globeU.uCloudOpacity, config.cloud_opacity);
      // The composed map is already density; levels live in COMPOSE_FRAG.
      gl.uniform2f(globeU.uCloudLevels, 0.0, 1.0);
      gl.uniform1f(globeU.uCityLights, config.city_lights);
      gl.uniform1f(globeU.uHours, (now.getTime() / 3600000) % 10000);
      gl.uniform1f(globeU.uCloudDetail, config.cloud_detail);
      gl.uniform1f(globeU.uAtmo, config.atmosphere === "painted" ? 0 : 1);
      gl.uniform1f(globeU.uAtmoExposure, config.atmosphere_exposure);
      gl.uniform1f(globeU.uHaze, config.haze);
      gl.uniform1f(globeU.uAtmoScale, config.atmosphere_scale);
      gl.uniform1f(globeU.uAtmoExtinction, config.atmosphere_extinction);
      gl.uniform1f(globeU.uLimbIn, config.limb_soft);
      gl.uniform1f(globeU.uLimbOut, config.limb_glow);

      gl.drawArrays(gl.TRIANGLES, 0, 3);
      gl.disableVertexAttribArray(aPos);
    }
    gl.disable(gl.SCISSOR_TEST);
    if (usePost) applyBloom();
  }

  await Promise.all([...pending, sky.ready]);
  refreshLiveClouds().then(refreshGeoClouds);
  setInterval(refreshLiveClouds, LIVE_CLOUD_REFRESH_MS / 6);
  setInterval(refreshGeoClouds, Math.max(10, config.geo_clouds_minutes || 20) * 60000);

  // --- when to draw -----------------------------------------------------------
  // Nothing in this scene moves on its own: the planet turns a quarter of a
  // degree a minute, the terminator and the stars follow the clock, and the
  // clouds change every few hours. So, like Apple's wallpaper, it is not an
  // animation: draw a frame, leave it on screen, and draw the next one only
  // when the clock has moved enough to matter. Between frames the GPU does
  // nothing and the compositor just keeps showing the last image, which costs
  // what a static wallpaper costs.
  //
  // A redraw is also forced by anything that changes the picture at once: a
  // new cloud map, a resize or monitor change, the page becoming visible again.
  const redrawMs = Math.max(1, Number(config.redraw_seconds) || 20) * 1000;
  let paused = false;
  let timer = 0;
  let queued = false;

  function frame() {
    queued = false;
    if (paused) return;
    draw();
    clearTimeout(timer);
    timer = setTimeout(requestRedraw, redrawMs);
  }
  function requestRedraw() {
    if (queued || paused) return;
    queued = true;
    requestAnimationFrame(frame);
  }

  document.addEventListener("visibilitychange", () => {
    paused = document.hidden;
    if (paused) clearTimeout(timer);
    else { refreshLiveClouds().then(refreshGeoClouds); requestRedraw(); }
  });
  // A monitor plugged in, unplugged or rearranged changes the virtual desktop,
  // and the host resizes this window to match; fetch the new layout so every
  // screen gets its globe again.
  window.addEventListener("resize", async () => {
    requestRedraw();
    try {
      const layout = await invoke("get_screen_layout");
      if (layout) screens = layout;
    } catch (_) { /* keep the old layout */ }
    requestRedraw();
  });
  onCloudsChanged = requestRedraw;

  // A new city picked on the settings page: turn the globe to it.
  window.__TAURI__.event?.listen("location:changed", (event) => {
    if (event.payload) home = event.payload;
    requestRedraw();
  });

  requestRedraw();
}

main();
