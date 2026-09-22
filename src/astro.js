// Where the sun, the moon and the celestial sphere actually are, right now.
//
// Everything here is the low-precision series from the Astronomical Almanac:
// a fraction of a degree, which is far better than a wallpaper needs and costs
// a page of arithmetic instead of an ephemeris file.
//
// Frames used below:
//   equatorial  right ascension / declination, J2000, as star catalogues give
//   earth-fixed +X at (0°N, 0°E), +Y at the north pole, +Z at (0°N, 90°E)
// Converting between them is one rotation about the pole by Greenwich mean
// sidereal time, which is why GMST shows up everywhere.

const DEG = Math.PI / 180;

const sin = (deg) => Math.sin(deg * DEG);
const cos = (deg) => Math.cos(deg * DEG);

/// Days since J2000.0 (2000-01-01 12:00 UTC).
export function daysSinceJ2000(date) {
  return date.getTime() / 86400000 - 10957.5;
}

/// Which meridian faces the vernal equinox right now, in degrees.
export function gmst(date) {
  const n = daysSinceJ2000(date);
  return norm360(280.46061837 + 360.98564736629 * n);
}

function norm360(deg) {
  const d = deg % 360;
  return d < 0 ? d + 360 : d;
}

function wrap180(deg) {
  const d = norm360(deg);
  return d > 180 ? d - 360 : d;
}

/// Unit vector in the earth-fixed frame.
export function toVector(latDeg, lonDeg) {
  const c = cos(latDeg);
  return [c * cos(lonDeg), sin(latDeg), c * sin(lonDeg)];
}

/// A right ascension / declination pair, placed in the earth-fixed frame for
/// the given sidereal time.
export function equatorialToEarthFixed(raDeg, decDeg, gmstDeg) {
  return toVector(decDeg, raDeg - gmstDeg);
}

/// The point on the surface with the sun straight overhead.
export function subsolarPoint(date = new Date()) {
  const { ra, dec } = solarEquatorial(date);
  return { lat: dec, lon: wrap180(ra - gmst(date)) };
}

function solarEquatorial(date) {
  const n = daysSinceJ2000(date);

  const meanLon = norm360(280.46 + 0.9856474 * n);
  const meanAnom = norm360(357.528 + 0.9856003 * n);
  const eclipticLon =
    meanLon + 1.915 * sin(meanAnom) + 0.02 * sin(2 * meanAnom);
  const obliquity = 23.439 - 0.0000004 * n;

  const dec = Math.asin(sin(obliquity) * sin(eclipticLon)) / DEG;
  const ra =
    Math.atan2(cos(obliquity) * sin(eclipticLon), cos(eclipticLon)) / DEG;

  // Earth-sun distance in earth radii, for the sun's apparent size.
  const distanceAu =
    1.00014 - 0.01671 * cos(meanAnom) - 0.00014 * cos(2 * meanAnom);
  return { ra: norm360(ra), dec, distance: distanceAu * 23454.8 };
}

export function sunState(date = new Date()) {
  const { ra, dec, distance } = solarEquatorial(date);
  const g = gmst(date);
  return {
    direction: equatorialToEarthFixed(ra, dec, g),
    distance,
    // The sun's disc is 0.2666° in radius at one astronomical unit.
    angularRadius: Math.asin(696340 / 6371 / distance),
  };
}

/// Moon position, good to roughly a third of a degree -- the abridged series
/// from the Almanac. That is a few percent of the moon's own diameter, which
/// is the only thing it is used to place.
export function moonState(date = new Date()) {
  const T = daysSinceJ2000(date) / 36525;

  const lon =
    218.32 +
    481267.881 * T +
    6.29 * sin(135.0 + 477198.87 * T) -
    1.27 * sin(259.3 - 413335.36 * T) +
    0.66 * sin(235.7 + 890534.22 * T) +
    0.21 * sin(269.9 + 954397.74 * T) -
    0.19 * sin(357.5 + 35999.05 * T) -
    0.11 * sin(186.6 + 966404.03 * T);

  const lat =
    5.13 * sin(93.3 + 483202.02 * T) +
    0.28 * sin(228.2 + 960400.89 * T) -
    0.28 * sin(318.3 + 6003.15 * T) -
    0.17 * sin(217.6 - 407332.21 * T);

  // Horizontal parallax gives the distance: the earth's radius subtends this
  // angle as seen from the moon.
  const parallax =
    0.9508 +
    0.0518 * cos(135.0 + 477198.87 * T) +
    0.0095 * cos(259.3 - 413335.36 * T) +
    0.0078 * cos(235.7 + 890534.22 * T) +
    0.0028 * cos(269.9 + 954397.74 * T);

  const obliquity = 23.439281 - 0.0130042 * T;
  const dec =
    Math.asin(sin(lat) * cos(obliquity) + cos(lat) * sin(obliquity) * sin(lon)) /
    DEG;
  const ra =
    Math.atan2(
      sin(lon) * cos(obliquity) - Math.tan(lat * DEG) * sin(obliquity),
      cos(lon)
    ) / DEG;

  const distance = 1 / sin(parallax); // earth radii
  return {
    direction: equatorialToEarthFixed(norm360(ra), dec, gmst(date)),
    distance,
    // The moon's radius is 0.2725 earth radii.
    angularRadius: Math.asin(0.2725 / distance),
  };
}
