// WMO 4677 weather codes, as Open-Meteo reports them, mapped to a label and
// to a Meteocons icon (MIT, Bas Milius -- see icons/LICENSE.txt).

const TABLE = {
  0:  ["晴",           "clear"],
  1:  ["晴间多云",     "mostly-clear"],
  2:  ["多云",         "partly"],
  3:  ["阴",           "overcast"],
  45: ["雾",           "fog"],
  48: ["雾凇",         "fog"],
  51: ["小毛毛雨",     "drizzle"],
  53: ["毛毛雨",       "drizzle"],
  55: ["浓毛毛雨",     "drizzle"],
  56: ["冻毛毛雨",     "sleet"],
  57: ["强冻毛毛雨",   "sleet"],
  61: ["小雨",         "rain-light"],
  63: ["中雨",         "rain"],
  65: ["大雨",         "rain"],
  66: ["冻雨",         "sleet"],
  67: ["强冻雨",       "sleet"],
  71: ["小雪",         "snow-light"],
  73: ["中雪",         "snow"],
  75: ["大雪",         "snow"],
  77: ["米雪",         "snow"],
  80: ["阵雨",         "showers"],
  81: ["强阵雨",       "showers"],
  82: ["暴雨",         "rain"],
  85: ["阵雪",         "snow-showers"],
  86: ["强阵雪",       "snow"],
  95: ["雷阵雨",       "thunder"],
  96: ["雷阵雨伴冰雹", "thunder-hail"],
  99: ["强雷暴伴冰雹", "thunder-hail"],
};

/// Which icon file to draw. Day and night variants exist for everything that
/// has a sun or a moon in it; pure cloud and rain look the same either way.
function iconName(kind, isDay) {
  const dn = isDay ? "day" : "night";
  switch (kind) {
    case "clear":        return `clear-${dn}`;
    case "mostly-clear": return `partly-cloudy-${dn}`;
    case "partly":       return `partly-cloudy-${dn}`;
    case "overcast":     return `overcast-${dn}`;
    case "fog":          return `fog-${dn}`;
    case "drizzle":      return `partly-cloudy-${dn}-drizzle`;
    case "rain-light":   return `partly-cloudy-${dn}-rain`;
    case "rain":         return "rain";
    case "showers":      return `partly-cloudy-${dn}-rain`;
    case "sleet":        return "sleet";
    case "snow-light":   return `partly-cloudy-${dn}-snow`;
    case "snow-showers": return `partly-cloudy-${dn}-snow`;
    case "snow":         return "snow";
    case "thunder":      return `thunderstorms-${dn}-rain`;
    case "thunder-hail": return "hail";
    default:             return "not-available";
  }
}

export function describe(code) {
  const hit = TABLE[code];
  return hit ? { label: hit[0], kind: hit[1] } : { label: "—", kind: "unknown" };
}

export function iconUrl(code, isDay = true) {
  return `icons/${iconName(describe(code).kind, isDay)}.svg`;
}

export const SUNRISE_ICON = "icons/sunrise.svg";
export const SUNSET_ICON = "icons/sunset.svg";

/// Card tint, the way the system weather widget changes colour with the sky.
export function cardTone(code, isDay) {
  const { kind } = describe(code);
  const wet = ["drizzle", "rain-light", "rain", "showers", "sleet", "thunder", "thunder-hail"].includes(kind);
  const grey = ["partly", "overcast", "fog", "snow", "snow-light", "snow-showers"].includes(kind);
  if (!isDay) return wet || grey ? "night-cloudy" : "night-clear";
  if (wet) return "rain";
  if (grey) return "cloudy";
  return "clear";
}
