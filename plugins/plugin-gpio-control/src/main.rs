use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, Read};
use std::fs;
use std::path::Path;

#[derive(Debug, Deserialize)]
struct PluginRequest { action: String, payload: Value }
#[derive(Debug, Serialize)]
struct PluginResponse { success: bool, result: Value, error: Option<String> }

const SAFE_PINS: &[u8] = &[
    2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27
];

fn main() {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input).unwrap_or(0);
    let response = match serde_json::from_str::<PluginRequest>(&input) {
        Ok(req) => handle(req),
        Err(e)  => PluginResponse { success: false, result: Value::Null,
            error: Some(format!("Invalid request JSON: {}", e)) },
    };
    println!("{}", serde_json::to_string(&response).unwrap());
}

fn handle(req: PluginRequest) -> PluginResponse {
    let args = req.payload.get("args").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if args.is_empty() || args == "status" || args == "all" { return gpio_status_all(); }
    let parts: Vec<&str> = args.splitn(2, char::is_whitespace).collect();
    match parts.as_slice() {
        [cmd, pin_str] => match pin_str.trim().parse::<u8>() {
            Ok(pin) => {
                if !SAFE_PINS.contains(&pin) {
                    return err(format!("Pin {} not in safe list {:?}", pin, SAFE_PINS));
                }
                match *cmd {
                    "on"  | "high" | "set"   => gpio_write(pin, true),
                    "off" | "low"  | "clear" => gpio_write(pin, false),
                    "read" | "get"           => gpio_read(pin),
                    other => err(format!("Unknown command '{}'. Use: on, off, read, status", other)),
                }
            }
            Err(_) => err(format!("Invalid pin number '{}'", pin_str.trim())),
        },
        [cmd] => match *cmd {
            "status" | "all" => gpio_status_all(),
            other => err(format!("Usage: /gpio <on|off|read> <pin>  Got: '{}'", other)),
        },
        _ => err("Usage: /gpio <on|off|read> <pin>".into()),
    }
}

fn gpio_export(pin: u8) -> Result<(), String> {
    let p = format!("/sys/class/gpio/gpio{}", pin);
    if !Path::new(&p).exists() {
        fs::write("/sys/class/gpio/export", pin.to_string()).map_err(|e|
            format!("Export pin {}: {} (add user to gpio group: sudo adduser $USER gpio)", pin, e))?;
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Ok(())
}

fn gpio_write(pin: u8, high: bool) -> PluginResponse {
    if let Err(e) = gpio_export(pin) { return err(e); }
    let base = format!("/sys/class/gpio/gpio{}", pin);
    if let Err(e) = fs::write(format!("{}/direction", base), "out") {
        return err(format!("Set direction pin {}: {}", pin, e));
    }
    match fs::write(format!("{}/value", base), if high { "1" } else { "0" }) {
        Ok(_) => PluginResponse { success: true, result: serde_json::json!({
            "pin":       pin,
            "state":     if high { "HIGH" } else { "LOW" },
            "value":     if high { 1 } else { 0 },
            "direction": "out"
        }), error: None },
        Err(e) => err(format!("Write pin {}: {}", pin, e)),
    }
}

fn gpio_read(pin: u8) -> PluginResponse {
    if let Err(e) = gpio_export(pin) { return err(e); }
    let base      = format!("/sys/class/gpio/gpio{}", pin);
    let direction = fs::read_to_string(format!("{}/direction", base))
        .unwrap_or_else(|_| "?".to_string()).trim().to_string();
    match fs::read_to_string(format!("{}/value", base)) {
        Ok(raw) => {
            let v: u8 = raw.trim().parse().unwrap_or(0);
            PluginResponse { success: true, result: serde_json::json!({
                "pin": pin, "value": v,
                "state": if v == 1 { "HIGH" } else { "LOW" },
                "direction": direction
            }), error: None }
        }
        Err(e) => err(format!("Read pin {}: {}", pin, e)),
    }
}

fn gpio_status_all() -> PluginResponse {
    let pins: Vec<Value> = SAFE_PINS.iter().filter_map(|&pin| {
        let p = format!("/sys/class/gpio/gpio{}", pin);
        if !Path::new(&p).exists() { return None; }
        let dir = fs::read_to_string(format!("{}/direction", p))
            .unwrap_or_default().trim().to_string();
        let v: u8 = fs::read_to_string(format!("{}/value", p))
            .unwrap_or_default().trim().parse().unwrap_or(0);
        Some(serde_json::json!({
            "pin": pin, "exported": true, "direction": dir,
            "value": v, "state": if v == 1 { "HIGH" } else { "LOW" }
        }))
    }).collect();
    PluginResponse { success: true, result: serde_json::json!({
        "exported_pins": pins.len(), "safe_pins": SAFE_PINS, "pins": pins,
        "note": "Only exported pins shown. Use /gpio on|off|read <pin> to interact."
    }), error: None }
}

fn err(msg: String) -> PluginResponse {
    PluginResponse { success: false, result: Value::Null, error: Some(msg) }
}
