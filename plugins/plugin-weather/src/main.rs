use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, Read};

#[derive(Debug, Deserialize)]
struct PluginRequest {
    action: String,
    payload: Value,
}

#[derive(Debug, Serialize)]
struct PluginResponse {
    success: bool,
    result: Value,
    error: Option<String>,
}

// Geocoding response from Open-Meteo
#[derive(Debug, Deserialize)]
struct GeoResponse {
    results: Option<Vec<GeoResult>>,
}

#[derive(Debug, Deserialize)]
struct GeoResult {
    name: String,
    country: String,
    latitude: f64,
    longitude: f64,
}

fn main() {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input).unwrap_or(0);

    let response = match serde_json::from_str::<PluginRequest>(&input) {
        Ok(req) => handle(req),
        Err(e) => PluginResponse {
            success: false,
            result: Value::Null,
            error: Some(format!("Invalid request JSON: {}", e)),
        },
    };

    println!("{}", serde_json::to_string(&response).unwrap());
}

fn handle(req: PluginRequest) -> PluginResponse {
    match req.action.as_str() {
        "weather" | "forecast" | "current" => {
            let city = match req.payload.get("city").and_then(|v| v.as_str()) {
                Some(c) => c.to_string(),
                None => {
                    return PluginResponse {
                        success: false,
                        result: Value::Null,
                        error: Some("Missing 'city' in payload".into()),
                    }
                }
            };

            // Step 1: geocode city → lat/lon using Open-Meteo Geocoding API
            let geo_url = format!(
                "https://geocoding-api.open-meteo.com/v1/search?name={}&count=1&language=en&format=json",
                urlencoded(&city)
            );

            let geo: GeoResponse = match ureq::get(&geo_url).call() {
                Ok(r) => match r.into_json() {
                    Ok(j) => j,
                    Err(e) => return err(format!("Geocoding parse error: {}", e)),
                },
                Err(e) => return err(format!("Geocoding request failed: {}", e)),
            };

            let location = match geo.results.and_then(|r| r.into_iter().next()) {
                Some(l) => l,
                None => return err(format!("City '{}' not found", city)),
            };

            // Step 2: fetch weather from Open-Meteo (free, no API key)
            let weather_url = format!(
                "https://api.open-meteo.com/v1/forecast?\
                 latitude={}&longitude={}&\
                 current=temperature_2m,apparent_temperature,relative_humidity_2m,\
                 wind_speed_10m,wind_direction_10m,weather_code,is_day&\
                 daily=temperature_2m_max,temperature_2m_min,precipitation_sum&\
                 timezone=auto&forecast_days=3",
                location.latitude, location.longitude
            );

            let weather: Value = match ureq::get(&weather_url).call() {
                Ok(r) => match r.into_json() {
                    Ok(j) => j,
                    Err(e) => return err(format!("Weather parse error: {}", e)),
                },
                Err(e) => return err(format!("Weather request failed: {}", e)),
            };

            let current = &weather["current"];
            let daily   = &weather["daily"];

            let temp      = current["temperature_2m"].as_f64().unwrap_or(0.0);
            let feels     = current["apparent_temperature"].as_f64().unwrap_or(0.0);
            let humidity  = current["relative_humidity_2m"].as_f64().unwrap_or(0.0);
            let wind      = current["wind_speed_10m"].as_f64().unwrap_or(0.0);
            let wcode     = current["weather_code"].as_u64().unwrap_or(0);
            let is_day    = current["is_day"].as_u64().unwrap_or(1) == 1;

            let condition = weather_code_to_string(wcode, is_day);

            // 3-day forecast
            let mut forecast = vec![];
            if let Some(dates) = daily["time"].as_array() {
                for i in 0..dates.len().min(3) {
                    forecast.push(serde_json::json!({
                        "date":      dates[i].as_str().unwrap_or(""),
                        "max_temp":  daily["temperature_2m_max"][i].as_f64().unwrap_or(0.0),
                        "min_temp":  daily["temperature_2m_min"][i].as_f64().unwrap_or(0.0),
                        "rain_mm":   daily["precipitation_sum"][i].as_f64().unwrap_or(0.0),
                    }));
                }
            }

            PluginResponse {
                success: true,
                result: serde_json::json!({
                    "location":    format!("{}, {}", location.name, location.country),
                    "condition":   condition,
                    "temperature": format!("{:.1}°C", temp),
                    "feels_like":  format!("{:.1}°C", feels),
                    "humidity":    format!("{}%", humidity),
                    "wind":        format!("{:.1} km/h", wind),
                    "forecast":    forecast,
                }),
                error: None,
            }
        }
        _ => PluginResponse {
            success: false,
            result: Value::Null,
            error: Some(format!(
                "Unknown action '{}'. Supported: weather, forecast, current",
                req.action
            )),
        },
    }
}

fn err(msg: String) -> PluginResponse {
    PluginResponse { success: false, result: Value::Null, error: Some(msg) }
}

fn urlencoded(s: &str) -> String {
    s.chars().map(|c| if c == ' ' { '+' } else { c }).collect()
}

fn weather_code_to_string(code: u64, is_day: bool) -> &'static str {
    match code {
        0  => if is_day { "Clear sky ☀️" } else { "Clear sky 🌙" },
        1  => "Mainly clear 🌤️",
        2  => "Partly cloudy ⛅",
        3  => "Overcast ☁️",
        45 | 48 => "Foggy 🌫️",
        51 | 53 | 55 => "Drizzle 🌦️",
        61 | 63 | 65 => "Rain 🌧️",
        71 | 73 | 75 => "Snow 🌨️",
        80 | 81 | 82 => "Rain showers 🌦️",
        85 | 86 => "Snow showers 🌨️",
        95 => "Thunderstorm ⛈️",
        96 | 99 => "Thunderstorm with hail ⛈️",
        _ => "Unknown",
    }
}
