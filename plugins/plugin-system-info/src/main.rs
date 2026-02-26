use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, Read};
use std::fs;

#[derive(Debug, Deserialize)]
struct PluginRequest { action: String, payload: Value }
#[derive(Debug, Serialize)]
struct PluginResponse { success: bool, result: Value, error: Option<String> }

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
    let args = req.payload.get("args").and_then(|v| v.as_str()).unwrap_or("").trim().to_lowercase();
    let sub = if !args.is_empty() { args.as_str() } else { req.action.as_str() };
    match sub { "uptime" => get_uptime(), "disk" => get_disk(), _ => get_full_sysinfo() }
}

fn get_uptime() -> PluginResponse {
    match fs::read_to_string("/proc/uptime") {
        Ok(content) => {
            let secs = content.split_whitespace().next()
                .and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
            let total = secs as u64;
            let days = total / 86400; let hours = (total % 86400) / 3600;
            let minutes = (total % 3600) / 60; let seconds = total % 60;
            let human = if days > 0 { format!("{}d {}h {}m {}s", days, hours, minutes, seconds) }
                        else if hours > 0 { format!("{}h {}m {}s", hours, minutes, seconds) }
                        else { format!("{}m {}s", minutes, seconds) };
            PluginResponse { success: true, result: serde_json::json!({
                "uptime_seconds": total, "uptime_human": human,
                "days": days, "hours": hours, "minutes": minutes }), error: None }
        }
        Err(e) => err(format!("Cannot read /proc/uptime: {}", e)),
    }
}

fn get_disk() -> PluginResponse {
    match std::process::Command::new("df").args(["-h", "/"]).output() {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            let mut lines = stdout.lines(); let _hdr = lines.next();
            if let Some(line) = lines.next() {
                let c: Vec<&str> = line.split_whitespace().collect();
                if c.len() >= 6 {
                    return PluginResponse { success: true, result: serde_json::json!({
                        "filesystem": c[0], "size": c[1], "used": c[2],
                        "available": c[3], "use_pct": c[4], "mount": c[5] }), error: None };
                }
            }
            err("Could not parse df output".into())
        }
        Err(e) => err(format!("df failed: {}", e)),
    }
}

fn get_full_sysinfo() -> PluginResponse {
    let uptime = fs::read_to_string("/proc/uptime").ok()
        .and_then(|s| s.split_whitespace().next().and_then(|v| v.parse::<f64>().ok()))
        .map(|f| f as u64).unwrap_or(0);
    let days = uptime / 86400; let hours = (uptime % 86400) / 3600;
    let minutes = (uptime % 3600) / 60;
    let uptime_human = if days > 0 { format!("{}d {}h {}m", days, hours, minutes) }
                       else if hours > 0 { format!("{}h {}m", hours, minutes) }
                       else { format!("{}m", minutes) };

    let cpu_temp = ["/sys/class/thermal/thermal_zone0/temp",
                    "/sys/devices/virtual/thermal/thermal_zone0/temp"]
        .iter().find_map(|p| fs::read_to_string(p).ok()
            .and_then(|s| s.trim().parse::<f64>().ok()).map(|v| v / 1000.0));

    let mem = {
        let content = fs::read_to_string("/proc/meminfo").unwrap_or_default();
        let mut total_kb = 0u64; let mut free_kb = 0u64; let mut avail_kb = 0u64;
        for line in content.lines() {
            let mut p = line.split_whitespace();
            match p.next() {
                Some("MemTotal:")     => { total_kb = p.next().and_then(|v| v.parse().ok()).unwrap_or(0); }
                Some("MemFree:")      => { free_kb  = p.next().and_then(|v| v.parse().ok()).unwrap_or(0); }
                Some("MemAvailable:") => { avail_kb = p.next().and_then(|v| v.parse().ok()).unwrap_or(0); }
                _ => {}
            }
        }
        let used = total_kb.saturating_sub(avail_kb);
        let pct = if total_kb > 0 { (used as f64 / total_kb as f64 * 100.0) as u32 } else { 0 };
        serde_json::json!({ "total_mb": total_kb/1024, "free_mb": free_kb/1024,
            "available_mb": avail_kb/1024, "used_mb": used/1024, "used_pct": pct })
    };

    let load_avg = {
        let c = fs::read_to_string("/proc/loadavg").unwrap_or_default();
        let p: Vec<&str> = c.split_whitespace().collect();
        serde_json::json!({ "1min":  p.first().copied().unwrap_or("?"),
            "5min":  p.get(1).copied().unwrap_or("?"),
            "15min": p.get(2).copied().unwrap_or("?") })
    };

    let disk = match std::process::Command::new("df").args(["-h", "/"]).output() {
        Ok(out) => {
            let s = String::from_utf8_lossy(&out.stdout).to_string();
            let mut lines = s.lines(); let _h = lines.next();
            lines.next().map(|line| {
                let c: Vec<&str> = line.split_whitespace().collect();
                if c.len() >= 5 {
                    serde_json::json!({"size":c[1],"used":c[2],"available":c[3],"use_pct":c[4]})
                } else { serde_json::json!(null) }
            }).unwrap_or(serde_json::json!(null))
        }
        Err(_) => serde_json::json!(null),
    };

    PluginResponse { success: true, result: serde_json::json!({
        "uptime": uptime_human, "uptime_seconds": uptime,
        "cpu_temp_celsius": cpu_temp, "memory": mem,
        "load_avg": load_avg, "disk_root": disk
    }), error: None }
}

fn err(msg: String) -> PluginResponse {
    PluginResponse { success: false, result: Value::Null, error: Some(msg) }
}
