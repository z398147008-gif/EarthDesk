//! Open-Meteo client. No API key, no attribution banner, and it answers in
//! the local timezone, which is what the timeline on the widget wants.

use serde_json::Value;

fn encode(s: &str) -> String {
    s.replace('/', "%2F")
}

pub async fn fetch(lat: f64, lon: f64, tz: &str) -> Result<Value, String> {
    let url = format!(
        "https://api.open-meteo.com/v1/forecast\
?latitude={lat:.4}&longitude={lon:.4}\
&current=temperature_2m,apparent_temperature,relative_humidity_2m,is_day,\
precipitation,weather_code,wind_speed_10m,wind_direction_10m,pressure_msl\
&hourly=temperature_2m,weather_code,precipitation_probability,is_day\
&daily=weather_code,temperature_2m_max,temperature_2m_min,sunrise,sunset,\
precipitation_probability_max,precipitation_sum,wind_speed_10m_max,uv_index_max\
&timezone={tz}&forecast_days=7",
        tz = encode(tz)
    );

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;

    let resp = client.get(&url).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("open-meteo returned {}", resp.status()));
    }
    resp.json::<Value>().await.map_err(|e| e.to_string())
}
