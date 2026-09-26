//! Weather. Everything is handed to the widget in Open-Meteo's shape (it
//! answers in the local timezone, which is what the timeline wants).
//!
//! In Japan the Japan Meteorological Agency is the most accurate source, so
//! there the answer is built from it:
//!   - hours and days: JMA's own models (MSM 5 km, then GSM), served by
//!     Open-Meteo as `models=jma_seamless`;
//!   - chance of rain: JMA's official 降水確率 (6-hour blocks for today and
//!     tomorrow, then the weekly forecast) for the forecast area around us;
//!   - the days' weather: JMA's official forecast (天気予報 / 週間天気予報);
//!   - now: the nearest AMeDAS station (measured rain and temperature,
//!     every 10 minutes).
//! Each JMA part is optional. Only when the JMA model itself cannot be had
//! does it fall back to Open-Meteo's default forecast, as everywhere else.

use serde_json::Value;
use std::sync::OnceLock;

fn encode(s: &str) -> String {
    s.replace('/', "%2F")
}

const CURRENT: &str = "temperature_2m,apparent_temperature,relative_humidity_2m,is_day,\
precipitation,weather_code,wind_speed_10m,wind_direction_10m,pressure_msl";
const HOURLY: &str = "temperature_2m,weather_code,precipitation_probability,is_day";
const DAILY: &str = "weather_code,temperature_2m_max,temperature_2m_min,sunrise,sunset,\
precipitation_probability_max,precipitation_sum,wind_speed_10m_max,uv_index_max";

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())
}

async fn get_json(client: &reqwest::Client, url: &str) -> Result<Value, String> {
    let resp = client.get(url).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("{url} returned {}", resp.status()));
    }
    resp.json::<Value>().await.map_err(|e| e.to_string())
}

async fn open_meteo(client: &reqwest::Client, lat: f64, lon: f64, tz: &str, model: Option<&str>) -> Result<Value, String> {
    let models = model.map(|m| format!("&models={m}")).unwrap_or_default();
    let url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={lat:.4}&longitude={lon:.4}\
&current={CURRENT}&hourly={HOURLY}&daily={DAILY}&timezone={tz}&forecast_days=7{models}",
        tz = encode(tz)
    );
    get_json(client, &url).await
}

pub async fn fetch(lat: f64, lon: f64, tz: &str) -> Result<Value, String> {
    let client = client()?;
    if in_japan(lat, lon) {
        match fetch_jma(&client, lat, lon, tz).await {
            Ok(v) => return Ok(v),
            Err(e) => eprintln!("JMA weather unavailable ({e}), using Open-Meteo's default forecast"),
        }
    }
    let mut v = open_meteo(&client, lat, lon, tz, None).await?;
    v["source"] = "open-meteo".into();
    Ok(v)
}

/// Roughly Japan (JMA's forecast areas; the MSM model covers this and more).
fn in_japan(lat: f64, lon: f64) -> bool {
    (24.0..=46.0).contains(&lat) && (122.5..=146.5).contains(&lon)
}

async fn fetch_jma(client: &reqwest::Client, lat: f64, lon: f64, tz: &str) -> Result<Value, String> {
    // Not near any JMA forecast area: not Japan after all.
    let place = jma_place(client, lat, lon).await;
    if let Err(Far) = place {
        return Err("no JMA forecast area nearby".into());
    }
    let mut v = open_meteo(client, lat, lon, tz, Some("jma_seamless")).await?;
    if v.pointer("/hourly/time").and_then(|t| t.as_array()).map(|t| t.is_empty()).unwrap_or(true) {
        return Err("JMA model: no hours".into());
    }
    // JMA's models have no chance of rain (nor UV): Open-Meteo's default
    // fills what is missing, and JMA's official figures override it below.
    if let Ok(generic) = open_meteo(client, lat, lon, tz, None).await {
        fill_missing(&mut v, &generic);
    }
    match place {
        Ok(place) => {
            match get_json(client, &format!("https://www.jma.go.jp/bosai/forecast/data/forecast/{}.json", place.office)).await {
                Ok(f) => apply_official(&mut v, &f, &place.class10),
                Err(e) => eprintln!("JMA forecast: {e}"),
            }
            match amedas_now(client).await {
                Ok(obs) => {
                    if let Some(o) = nearest_obs(&obs, &place.stations, lat, lon) {
                        apply_observation(&mut v, o);
                    }
                }
                Err(e) => eprintln!("AMeDAS: {e}"),
            }
        }
        Err(PlaceError::Net(e)) => eprintln!("JMA area tables: {e}"),
        Err(Far) => {}
    }
    v["source"] = "jma".into();
    Ok(v)
}

/// Every hourly / daily series (and current value) the JMA answer lacks or
/// has holes in, from Open-Meteo's default model (same hours and days).
fn fill_missing(v: &mut Value, generic: &Value) {
    for block in ["hourly", "daily"] {
        let Some(src) = generic.get(block).and_then(|b| b.as_object()).cloned() else { continue };
        let Some(dst) = v.get_mut(block).and_then(|b| b.as_object_mut()) else { continue };
        for (k, sv) in src {
            let Some(sarr) = sv.as_array() else { continue };
            match dst.get_mut(&k).and_then(|d| d.as_array_mut()) {
                Some(darr) => {
                    for (i, d) in darr.iter_mut().enumerate() {
                        if d.is_null() {
                            if let Some(s) = sarr.get(i) {
                                *d = s.clone();
                            }
                        }
                    }
                }
                None => {
                    dst.insert(k, sv.clone());
                }
            }
        }
    }
    if let (Some(src), Some(dst)) = (generic.get("current").and_then(|c| c.as_object()).cloned(), v.get_mut("current").and_then(|c| c.as_object_mut())) {
        for (k, sv) in src {
            if dst.get(&k).map(|d| d.is_null()).unwrap_or(true) {
                dst.insert(k, sv);
            }
        }
    }
}

// --- JMA: where we are -------------------------------------------------------------

struct Place {
    /// Forecast office (府県予報区), e.g. 270000 大阪府.
    office: String,
    /// Its subdivision (一次細分区域) we are in, e.g. 270000.
    class10: String,
    /// AMeDAS stations with their position (degrees).
    stations: Vec<(String, f64, f64)>,
}

static STATIONS: OnceLock<Vec<(String, f64, f64)>> = OnceLock::new();
static AREAS: OnceLock<Vec<(String, String, String)>> = OnceLock::new();

/// AMeDAS positions are [degrees, minutes].
fn degrees(v: &Value) -> Option<f64> {
    let a = v.as_array()?;
    Some(a.first()?.as_f64()? + a.get(1).and_then(|m| m.as_f64()).unwrap_or(0.0) / 60.0)
}

fn parse_stations(table: &Value) -> Vec<(String, f64, f64)> {
    table
        .as_object()
        .map(|m| m.iter().filter_map(|(id, s)| Some((id.clone(), degrees(s.get("lat")?)?, degrees(s.get("lon")?)?))).collect())
        .unwrap_or_default()
}

/// (office, class10, representative AMeDAS station) for every area.
fn parse_areas(areas: &Value) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for (office, list) in areas.as_object().into_iter().flatten() {
        for a in list.as_array().into_iter().flatten() {
            let class10 = a.get("class10").and_then(|c| c.as_str()).unwrap_or_default().to_string();
            for st in a.get("amedas").and_then(|s| s.as_array()).into_iter().flatten() {
                if let Some(st) = st.as_str() {
                    out.push((office.clone(), class10.clone(), st.to_string()));
                }
            }
        }
    }
    out
}

/// Squared distance, good enough to compare (km-ish scale).
fn dist2(lat: f64, lon: f64, la: f64, lo: f64) -> f64 {
    let dx = (lo - lon) * lat.to_radians().cos() * 111.0;
    let dy = (la - lat) * 111.0;
    dx * dx + dy * dy
}

/// The forecast area whose representative station is nearest, if one is
/// within 80 km (the rectangle `in_japan` also holds Seoul and Busan).
fn pick_area(stations: &[(String, f64, f64)], areas: &[(String, String, String)], lat: f64, lon: f64) -> Option<(String, String)> {
    areas
        .iter()
        .filter_map(|(office, class10, st)| {
            let (_, la, lo) = stations.iter().find(|s| &s.0 == st)?;
            Some((dist2(lat, lon, *la, *lo), office, class10))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .filter(|(d, _, _)| *d <= 80.0 * 80.0)
        .map(|(_, o, c)| (o.clone(), c.clone()))
}

#[derive(Debug)]
enum PlaceError {
    /// The tables could not be fetched (JMA's model is still used).
    Net(String),
    /// No forecast area near: not in Japan.
    Far,
}
use PlaceError::Far;

async fn jma_place(client: &reqwest::Client, lat: f64, lon: f64) -> Result<Place, PlaceError> {
    if STATIONS.get().is_none() {
        let t = get_json(client, "https://www.jma.go.jp/bosai/amedas/const/amedastable.json").await.map_err(PlaceError::Net)?;
        let _ = STATIONS.set(parse_stations(&t));
    }
    if AREAS.get().is_none() {
        let a = get_json(client, "https://www.jma.go.jp/bosai/forecast/const/forecast_area.json").await.map_err(PlaceError::Net)?;
        let _ = AREAS.set(parse_areas(&a));
    }
    let stations = STATIONS.get().cloned().unwrap_or_default();
    let (office, class10) = pick_area(&stations, AREAS.get().map(|a| a.as_slice()).unwrap_or(&[]), lat, lon).ok_or(Far)?;
    Ok(Place { office, class10, stations })
}

// --- JMA: the official forecast --------------------------------------------------------

/// JMA's weather telop code (100 晴れ, 200 くもり, 300 雨, 400 雪 and their
/// combinations: 101 晴時々曇, 202 曇一時雨, 313 雨後曇 ...) as the WMO code
/// the widget draws.
pub fn jma_code(c: u32) -> u8 {
    match c {
        100 | 123 | 124 | 130 | 131 => 0,
        101 | 110 | 111 | 132 => 2,
        104 | 105 | 115..=117 | 160 | 170 | 181 => 85,
        102..=108 | 112..=114 | 118..=122 | 125..=129 | 140 => 80,
        200 | 231 => 3,
        201 | 210 | 211 | 223 => 2,
        209 => 45,
        204 | 205 | 215..=217 | 228..=230 | 260 | 270 | 281 => 71,
        202..=208 | 212..=214 | 218..=222 | 224..=227 | 240 | 250 => 61,
        306 | 308 | 328 => 65,
        301 | 302 | 311 | 313 | 320 | 321 | 323..=325 => 61,
        300..=399 => 63,
        401 | 411 => 71,
        405..=407 | 425..=427 => 75,
        400..=499 => 73,
        c if c / 100 == 1 => 1,
        _ => 3,
    }
}

/// "2026-09-26T11:00:00+09:00" -> ("2026-09-26", 11).
fn date_hour(t: &str) -> Option<(&str, u32)> {
    Some((t.get(..10)?, t.get(11..13)?.parse().ok()?))
}

fn area_in<'a>(series: &'a Value, code: &str) -> Option<&'a Value> {
    let areas = series.get("areas")?.as_array()?;
    areas.iter().find(|a| a.pointer("/area/code").and_then(|c| c.as_str()) == Some(code)).or_else(|| areas.first())
}

fn strings(v: Option<&Value>) -> Vec<String> {
    v.and_then(|a| a.as_array()).map(|a| a.iter().map(|x| x.as_str().unwrap_or_default().to_string()).collect()).unwrap_or_default()
}

/// What the official forecast says: per date (weather code, chance of
/// rain), and the 6-hour chance-of-rain blocks (date, start hour, %).
#[derive(Default, Debug, PartialEq)]
struct Official {
    days: Vec<(String, Option<u8>, Option<u32>)>,
    blocks: Vec<(String, u32, u32)>,
}

fn parse_official(f: &Value, class10: &str) -> Official {
    let mut out = Official::default();
    let day = |out: &mut Official, date: &str, code: Option<u8>, pop: Option<u32>| {
        match out.days.iter_mut().find(|d| d.0 == date) {
            Some(d) => {
                d.1 = d.1.or(code);
                d.2 = match (d.2, pop) {
                    (Some(a), Some(b)) => Some(a.max(b)),
                    (a, b) => a.or(b),
                };
            }
            None => out.days.push((date.to_string(), code, pop)),
        }
    };
    let Some(parts) = f.as_array() else { return out };
    // [0]: today to the day after tomorrow, [1]: the week.
    for (pi, part) in parts.iter().enumerate() {
        let Some(series) = part.get("timeSeries").and_then(|s| s.as_array()) else { continue };
        for s in series {
            let times = strings(s.get("timeDefines"));
            let Some(area) = area_in(s, class10) else { continue };
            let codes = strings(area.get("weatherCodes"));
            let pops = strings(area.get("pops"));
            for (i, t) in times.iter().enumerate() {
                let Some((date, hour)) = date_hour(t) else { continue };
                let code = codes.get(i).and_then(|c| c.parse::<u32>().ok()).map(jma_code);
                let pop = pops.get(i).and_then(|p| p.parse::<u32>().ok());
                if pi == 0 && !pops.is_empty() {
                    if let Some(p) = pop {
                        out.blocks.push((date.to_string(), hour, p));
                        day(&mut out, date, None, Some(p));
                    }
                } else if code.is_some() || pop.is_some() {
                    // The short forecast comes first and wins for its days.
                    let known = out.days.iter().any(|d| d.0 == date && d.1.is_some());
                    day(&mut out, date, if known { None } else { code }, if pi == 0 { None } else { pop });
                }
            }
        }
    }
    out
}

/// Put the official forecast into the Open-Meteo shaped answer.
fn apply_official(v: &mut Value, f: &Value, class10: &str) {
    let o = parse_official(f, class10);
    if o.days.is_empty() && o.blocks.is_empty() {
        return;
    }
    let daily_pop = |date: &str| o.days.iter().find(|d| d.0 == date).and_then(|d| d.2);
    if let Some(daily) = v.get_mut("daily").and_then(|d| d.as_object_mut()) {
        let dates = strings(daily.get("time"));
        for (i, date) in dates.iter().enumerate() {
            let Some(d) = o.days.iter().find(|d| &d.0 == date) else { continue };
            if let (Some(code), Some(arr)) = (d.1, daily.get_mut("weather_code").and_then(|a| a.as_array_mut())) {
                if let Some(slot) = arr.get_mut(i) {
                    *slot = code.into();
                }
            }
            if let (Some(pop), Some(arr)) = (d.2, daily.get_mut("precipitation_probability_max").and_then(|a| a.as_array_mut())) {
                if let Some(slot) = arr.get_mut(i) {
                    *slot = pop.into();
                }
            }
        }
    }
    if let Some(hourly) = v.get_mut("hourly").and_then(|h| h.as_object_mut()) {
        let times = strings(hourly.get("time"));
        if let Some(arr) = hourly.get_mut("precipitation_probability").and_then(|a| a.as_array_mut()) {
            for (i, t) in times.iter().enumerate() {
                let Some((date, hour)) = date_hour(t) else { continue };
                // The 6-hour block holding this hour, else the day's figure.
                let block = o.blocks.iter().find(|b| b.0 == date && hour >= b.1 && hour < b.1 + 6).map(|b| b.2);
                let pop = block.or_else(|| if o.blocks.iter().any(|b| b.0 == date) { None } else { daily_pop(date) });
                if let (Some(p), Some(slot)) = (pop, arr.get_mut(i)) {
                    *slot = p.into();
                }
            }
        }
    }
}

// --- JMA: what it is doing now ---------------------------------------------------------

async fn amedas_now(client: &reqwest::Client) -> Result<Value, String> {
    let resp = client.get("https://www.jma.go.jp/bosai/amedas/data/latest_time.txt").send().await.map_err(|e| e.to_string())?;
    let t = resp.text().await.map_err(|e| e.to_string())?;
    let t = t.trim();
    // "2026-09-26T10:40:00+09:00" -> "20260926104000"
    let key: String = t.get(..19).ok_or("bad AMeDAS time")?.chars().filter(|c| c.is_ascii_digit()).collect();
    get_json(client, &format!("https://www.jma.go.jp/bosai/amedas/data/map/{key}.json")).await
}

/// A reading of one station: [value, quality flag]; 0 = normal.
fn reading(s: &Value, key: &str) -> Option<f64> {
    let a = s.get(key)?.as_array()?;
    if a.get(1).and_then(|f| f.as_i64()).unwrap_or(0) != 0 {
        return None;
    }
    a.first()?.as_f64()
}

/// The nearest station (within 30 km) that measures rain right now.
fn nearest_obs<'a>(obs: &'a Value, stations: &[(String, f64, f64)], lat: f64, lon: f64) -> Option<&'a Value> {
    let map = obs.as_object()?;
    stations
        .iter()
        .filter_map(|(id, la, lo)| {
            let s = map.get(id)?;
            reading(s, "precipitation10m")?;
            Some((dist2(lat, lon, *la, *lo), s))
        })
        .filter(|(d, _)| *d <= 30.0 * 30.0)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, s)| s)
}

fn is_wet(code: i64) -> bool {
    matches!(code, 51..=67 | 71..=77 | 80..=86 | 95..=99)
}

/// Measured beats modelled for "now": the temperature and humidity, and
/// rain when the gauge has some. A dry gauge changes nothing: it counts in
/// 0.5 mm steps, and drizzle stays below that.
fn apply_observation(v: &mut Value, s: &Value) {
    let Some(cur) = v.get_mut("current").and_then(|c| c.as_object_mut()) else { return };
    if let Some(t) = reading(s, "temp") {
        cur.insert("temperature_2m".into(), t.into());
    }
    if let Some(h) = reading(s, "humidity") {
        cur.insert("relative_humidity_2m".into(), h.into());
    }
    if let Some(w) = reading(s, "wind") {
        cur.insert("wind_speed_10m".into(), (w * 3.6).into());
    }
    let p10 = reading(s, "precipitation10m").unwrap_or(0.0);
    let p1h = reading(s, "precipitation1h").unwrap_or(0.0);
    if p10 > 0.0 {
        let rate = (p10 * 6.0).max(p1h);
        let cold = reading(s, "temp").map(|t| t <= 1.0).unwrap_or(false);
        let model = cur.get("weather_code").and_then(|c| c.as_i64()).unwrap_or(0);
        // Keep the model's word when it already says wet (thunder, snow...).
        let code = if is_wet(model) && model != 51 && model != 53 {
            model
        } else if cold {
            if rate < 1.0 { 71 } else { 73 }
        } else if rate < 3.0 {
            61
        } else if rate < 10.0 {
            63
        } else {
            65
        };
        cur.insert("weather_code".into(), code.into());
        cur.insert("precipitation".into(), p1h.max(p10).into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn official() -> Value {
        json!([
            {"timeSeries": [
                {"timeDefines": ["2026-09-26T11:00:00+09:00", "2026-09-27T00:00:00+09:00", "2026-09-28T00:00:00+09:00"],
                 "areas": [{"area": {"code": "270000"}, "weatherCodes": ["300", "200", "202"]}]},
                {"timeDefines": ["2026-09-26T12:00:00+09:00", "2026-09-26T18:00:00+09:00", "2026-09-27T00:00:00+09:00", "2026-09-27T06:00:00+09:00", "2026-09-27T12:00:00+09:00", "2026-09-27T18:00:00+09:00"],
                 "areas": [{"area": {"code": "270000"}, "pops": ["80", "70", "20", "10", "10", "30"]}]},
                {"timeDefines": ["2026-09-26T09:00:00+09:00"], "areas": [{"area": {"code": "62078"}, "temps": ["23"]}]}
            ]},
            {"timeSeries": [
                {"timeDefines": ["2026-09-27T00:00:00+09:00", "2026-09-28T00:00:00+09:00", "2026-09-29T00:00:00+09:00"],
                 "areas": [{"area": {"code": "270000"}, "weatherCodes": ["200", "202", "101"], "pops": ["", "60", "40"]}]}
            ]}
        ])
    }

    #[test]
    fn codes() {
        assert_eq!(jma_code(100), 0);
        assert_eq!(jma_code(101), 2);
        assert_eq!(jma_code(200), 3);
        assert_eq!(jma_code(202), 61);
        assert_eq!(jma_code(300), 63);
        assert_eq!(jma_code(313), 61);
        assert_eq!(jma_code(400), 73);
        assert_eq!(jma_code(209), 45);
    }

    #[test]
    fn official_forecast() {
        let o = parse_official(&official(), "270000");
        // Today: 雨, the higher of its blocks; tomorrow: くもり (short
        // forecast wins), max of its blocks; later days from the week.
        assert_eq!(o.days[0], ("2026-09-26".into(), Some(63), Some(80)));
        assert_eq!(o.days[1], ("2026-09-27".into(), Some(3), Some(30)));
        assert_eq!(o.days[2], ("2026-09-28".into(), Some(61), Some(60)));
        assert_eq!(o.days[3], ("2026-09-29".into(), Some(2), Some(40)));
        assert_eq!(o.blocks.len(), 6);

        let mut v = json!({
            "daily": {"time": ["2026-09-26", "2026-09-27", "2026-09-28", "2026-09-29", "2026-09-30"],
                      "weather_code": [51, 3, 3, 3, 0], "precipitation_probability_max": [null, 5, 5, 5, 7]},
            "hourly": {"time": ["2026-09-26T13:00", "2026-09-26T19:00", "2026-09-27T07:00", "2026-09-29T10:00", "2026-09-30T10:00"],
                       "precipitation_probability": [1, 2, 3, 4, 9]}
        });
        apply_official(&mut v, &official(), "270000");
        assert_eq!(v["daily"]["weather_code"], json!([63, 3, 61, 2, 0]));
        assert_eq!(v["daily"]["precipitation_probability_max"], json!([80, 30, 60, 40, 7]));
        assert_eq!(v["hourly"]["precipitation_probability"], json!([80, 70, 10, 40, 9]));
    }

    #[test]
    fn fill_from_default() {
        let mut v = json!({"hourly": {"time": ["a", "b"], "precipitation_probability": [null, null]}, "daily": {"time": ["d"]}, "current": {"weather_code": 51, "pressure_msl": null}});
        let g = json!({"hourly": {"time": ["a", "b"], "precipitation_probability": [10, 20]}, "daily": {"time": ["d"], "uv_index_max": [5.0]}, "current": {"weather_code": 3, "pressure_msl": 1012.0}});
        fill_missing(&mut v, &g);
        assert_eq!(v["hourly"]["precipitation_probability"], json!([10, 20]));
        assert_eq!(v["daily"]["uv_index_max"], json!([5.0]));
        assert_eq!(v["current"], json!({"weather_code": 51, "pressure_msl": 1012.0}));
    }

    #[test]
    fn observation() {
        let wet = json!({"temp": [21.4, 0], "humidity": [95, 0], "precipitation10m": [0.5, 0], "precipitation1h": [1.5, 0], "wind": [2.0, 0]});
        let mut v = json!({"current": {"weather_code": 3, "temperature_2m": 23.0}});
        apply_observation(&mut v, &wet);
        assert_eq!(v["current"]["weather_code"], 63);
        assert_eq!(v["current"]["temperature_2m"], 21.4);
        // A dry gauge leaves the model's drizzle alone (below 0.5 mm).
        let dry = json!({"temp": [22.7, 0], "precipitation10m": [0.0, 0], "precipitation1h": [0.0, 0]});
        let mut v = json!({"current": {"weather_code": 51}});
        apply_observation(&mut v, &dry);
        assert_eq!(v["current"]["weather_code"], 51);
        // Suspect readings (flag != 0) are ignored.
        let bad = json!({"temp": [99.0, 5], "precipitation10m": [0.0, 0]});
        let mut v = json!({"current": {"temperature_2m": 20.0}});
        apply_observation(&mut v, &bad);
        assert_eq!(v["current"]["temperature_2m"], 20.0);
    }

    #[test]
    fn places() {
        let st = parse_stations(&json!({"62078": {"lat": [34, 40.9], "lon": [135, 31.1]}, "44132": {"lat": [35, 41.5], "lon": [139, 45.0]}}));
        let areas = parse_areas(&json!({"270000": [{"class10": "270000", "amedas": ["62078"]}], "130000": [{"class10": "130010", "amedas": ["44132"]}]}));
        assert_eq!(pick_area(&st, &areas, 34.69, 135.50), Some(("270000".into(), "270000".into())));
        assert_eq!(pick_area(&st, &areas, 35.68, 139.76), Some(("130000".into(), "130010".into())));
        assert!(in_japan(34.69, 135.5) && !in_japan(31.23, 121.47));
        // Seoul is in the rectangle but far from any JMA area.
        assert_eq!(pick_area(&st, &areas, 37.57, 126.98), None);
    }
}
