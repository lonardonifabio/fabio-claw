use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, Read};

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
    match req.action.as_str() {
        "plan" | "update-plan" => build_update_plan(&args),
        _                      => check_updates(&args),
    }
}

fn check_updates(target: &str) -> PluginResponse {
    let do_system  = target.is_empty() || target.contains("system") || target.contains("apt");
    let do_runtime = target.is_empty() || target.contains("fabio");
    let mut report = serde_json::json!({});
    if do_system  { report["system"]     = check_apt(); }
    if do_runtime { report["fabio_claw"] = check_runtime(); }
    report["checked_at"] = serde_json::json!(unix_now());
    report["safe_mode"]  = serde_json::json!(true);
    report["note"] = serde_json::json!("READ-ONLY check — no changes made. Use /update-plan for steps.");
    PluginResponse { success: true, result: report, error: None }
}

fn check_apt() -> Value {
    match std::process::Command::new("apt-get").args(["-s", "upgrade", "-q"]).output() {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            let upgradable: Vec<String> = stdout.lines()
                .filter(|l| l.starts_with("Inst "))
                .map(|l| l[5..].split_whitespace().next().unwrap_or("").to_string())
                .filter(|s| !s.is_empty()).take(30).collect();
            serde_json::json!({
                "available_updates": upgradable.len(),
                "packages": &upgradable[..upgradable.len().min(20)],
                "status": if upgradable.is_empty() { "up_to_date" } else { "updates_available" }
            })
        }
        Err(e) => serde_json::json!({ "status": "check_failed", "error": format!("{}", e) }),
    }
}

fn check_runtime() -> Value {
    let git_log = std::process::Command::new("git")
        .args(["-C", "/home/pi/fabio-claw", "log", "--oneline", "-3"]).output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "git unavailable".to_string());
    serde_json::json!({
        "binary_path": "/usr/local/bin/fabio-claw",
        "git_recent_commits": git_log
    })
}

fn build_update_plan(target: &str) -> PluginResponse {
    let do_system  = target.is_empty() || target.contains("system") || target.contains("apt");
    let do_runtime = target.is_empty() || target.contains("fabio");
    let mut steps: Vec<Value> = vec![];
    if do_system {
        steps.push(serde_json::json!({
            "step": 1, "action": "Update system packages",
            "commands": ["sudo apt-get update", "sudo apt-get upgrade -y", "sudo apt-get autoremove -y"],
            "risk": "low", "requires_reboot": false
        }));
    }
    if do_runtime {
        let n = steps.len() + 1;
        steps.push(serde_json::json!({
            "step": n, "action": "Update fabio-claw runtime",
            "commands": ["cd ~/fabio-claw", "git pull origin main", "cargo build --release",
                "sudo cp target/release/fabio-claw /usr/local/bin/",
                "sudo systemctl restart fabio-claw"],
            "risk": "medium", "downtime_estimate": "30-60 minutes on Raspberry Pi"
        }));
        let n2 = steps.len() + 1;
        steps.push(serde_json::json!({
            "step": n2, "action": "Rebuild and reinstall plugins",
            "commands": ["cd ~/fabio-claw/plugins", "cargo build --release",
                "sudo cp target/release/plugin-* /opt/fabio-claw/plugins/",
                "sudo systemctl restart fabio-claw"],
            "risk": "low"
        }));
    }
    let nv = steps.len() + 1;
    steps.push(serde_json::json!({
        "step": nv, "action": "Verify health after update",
        "commands": ["curl http://localhost:8080/health", "curl http://localhost:8080/health/ready",
            "journalctl -u fabio-claw | tail -20"],
        "risk": "none"
    }));
    PluginResponse { success: true, result: serde_json::json!({
        "plan_generated_at": unix_now(), "total_steps": steps.len(), "steps": steps,
        "warning": "Review each step before executing. No commands have been run.",
        "safe_mode": true
    }), error: None }
}

fn unix_now() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| format!("unix:{}", d.as_secs()))
        .unwrap_or_else(|_| "unknown".to_string())
}
