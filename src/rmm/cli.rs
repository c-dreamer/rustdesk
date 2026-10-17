//! Bodies of the RMM command-line flags, shared with the MCP tools so both report the
//! same data. `core_main` keeps only the settings gate and a one-line call per flag.

use hbb_common::config;
use serde_json::{json, Value};

pub fn status() -> Value {
    json!({"id": crate::ipc::get_id(), "version": crate::VERSION})
}

pub fn peers() -> Value {
    config::PeerConfig::peers(None)
        .into_iter()
        .map(|(id, _, p)| {
            json!({
                "id": id,
                "username": p.info.username,
                "hostname": p.info.hostname,
                "platform": p.info.platform,
            })
        })
        .collect()
}

/// Monitoring config (every rmm-* option except the alert list itself) and recent alerts.
pub fn alerts_status() -> Value {
    let recent_key = base::config::keys::OPTION_RMM_RECENT_ALERTS;
    let config: serde_json::Map<String, Value> = crate::ipc::get_options()
        .into_iter()
        .filter(|(k, _)| k.starts_with("rmm-") && k != recent_key)
        .map(|(k, v)| (k, Value::from(v)))
        .collect();
    json!({"config": config, "recent_alerts": crate::rmm::monitor::recent_alerts()})
}

/// JSON either way, so `ConvertFrom-Json` callers see the reason, not a parse error.
pub fn list_scripts() -> String {
    match crate::rmm::scripts::list() {
        Ok(list) => list.to_string(),
        Err(err) => json!({ "error": err.to_string() }).to_string(),
    }
}

/// Failures carry an `Error: ` prefix (as the Tools tab's FFI does), since the process
/// exits 0 either way and a script's own output can be any text.
pub fn run_script(name: &str) -> String {
    match crate::rmm::scripts::run(name) {
        Ok(output) => output,
        Err(err) => format!("Error: {err}"),
    }
}

/// `--add-script <name> <file> [schedule_secs]`
pub fn add_script(name: &str, file: &str, schedule: Option<&str>) -> String {
    let schedule_secs = match crate::rmm::scripts::parse_schedule_secs(schedule) {
        Ok(secs) => secs,
        Err(err) => return err,
    };
    match std::fs::read_to_string(file) {
        Ok(body) => match crate::rmm::scripts::add(name, &body, schedule_secs) {
            Ok(()) => "Done!".to_owned(),
            Err(err) => err.to_string(),
        },
        Err(err) => format!("Failed to read {file}: {err}"),
    }
}
