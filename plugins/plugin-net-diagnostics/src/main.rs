use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, Read};
use std::net::ToSocketAddrs;
use std::time::Instant;

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
    let args = req.payload.get("args").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let host = if args.is_empty() { "8.8.8.8".to_string() } else { sanitize_host(&args) };
    match req.action.as_str() {
        "dns"     => do_dns(&host),
        "latency" => do_latency(&host),
        _         => do_ping(&host),
    }
}

fn sanitize_host(raw: &str) -> String {
    raw.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':'))
        .take(253).collect()
}

fn do_ping(host: &str) -> PluginResponse {
    match std::process::Command::new("ping").args(["-c", "4", "-W", "2", host]).output() {
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout).to_string();
            let reachable = o.status.success();
            let rtt = stdout.lines()
                .find(|l| l.contains("min/avg/max") || l.contains("rtt"))
                .and_then(|l| l.split('=').nth(1)).map(|s| s.trim().to_string());
            let packet_loss = stdout.lines()
                .find(|l| l.contains("packet loss"))
                .and_then(|l| l.split(',').find(|p| p.contains("packet loss")))
                .map(|s| s.trim().to_string());
            PluginResponse { success: true, result: serde_json::json!({
                "host": host, "reachable": reachable,
                "packet_loss": packet_loss, "rtt_stats": rtt,
                "raw": &stdout[..stdout.len().min(400)] }), error: None }
        }
        Err(e) => err(format!("ping failed: {}", e)),
    }
}

fn do_dns(host: &str) -> PluginResponse {
    let t0 = Instant::now();
    match format!("{}:80", host).to_socket_addrs() {
        Ok(addrs) => {
            let elapsed_ms = t0.elapsed().as_millis() as u64;
            let resolved: Vec<String> = addrs.map(|a| a.ip().to_string()).collect();
            if resolved.is_empty() { return err(format!("No DNS records for '{}'", host)); }
            PluginResponse { success: true, result: serde_json::json!({
                "host": host, "resolved": resolved,
                "resolve_ms": elapsed_ms, "record_count": resolved.len() }), error: None }
        }
        Err(e) => err(format!("DNS failed for '{}': {}", host, e)),
    }
}

fn do_latency(host: &str) -> PluginResponse {
    let probes: Vec<Value> = [(443u16, "HTTPS"), (80, "HTTP"), (22, "SSH")].iter().map(|&(port, proto)| {
        let addr_str = format!("{}:{}", host, port);
        let t0 = Instant::now();
        let ok = addr_str.parse().ok().map(|a: std::net::SocketAddr| {
            std::net::TcpStream::connect_timeout(&a, std::time::Duration::from_millis(2000)).is_ok()
        }).unwrap_or(false);
        let ms = t0.elapsed().as_millis() as u64;
        serde_json::json!({ "protocol": proto, "port": port, "open": ok,
            "latency_ms": if ok { serde_json::json!(ms) } else { serde_json::json!(null) } })
    }).collect();
    PluginResponse { success: true,
        result: serde_json::json!({ "host": host, "probes": probes }), error: None }
}

fn err(msg: String) -> PluginResponse {
    PluginResponse { success: false, result: Value::Null, error: Some(msg) }
}
