//! `rustdesk --mcp`: a Model Context Protocol server over stdio (newline-delimited
//! JSON-RPC 2.0), so local agents (Claude Code, Codex, opencode) can drive the RMM
//! tooling headlessly. No listening port. Read tools are always available; tools that
//! change state require the `rmm-agent-write` option, which the agent cannot set itself.

use base::config::keys;
use hbb_common::{config, tokio};
use serde_json::{json, Value};
use std::io::{BufRead, Write};

const PROTOCOL_VERSION: &str = "2025-06-18";
// 2025-03-26 is left out: it allows JSON-RPC batches, which this server does not handle.
const SUPPORTED_VERSIONS: &[&str] = &[PROTOCOL_VERSION, "2024-11-05"];

pub fn serve(cli_settings_disabled: bool) {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(&line) {
            Ok(req) => handle(&req, &|| write_allowed(cli_settings_disabled)),
            Err(err) => Some(error(Value::Null, -32700, &format!("Parse error: {err}"))),
        };
        if let Some(response) = response {
            if writeln!(stdout, "{response}")
                .and_then(|_| stdout.flush())
                .is_err()
            {
                break;
            }
        }
    }
}

fn write_allowed(cli_settings_disabled: bool) -> bool {
    !cli_settings_disabled && server_option(keys::OPTION_RMM_AGENT_WRITE).as_deref() == Some("Y")
}

/// Fails closed: `ipc::get_options` falls back to the caller's own (user-writable)
/// config when the server is unreachable, which must not open the write gate.
#[tokio::main(flavor = "current_thread")]
async fn server_option(key: &str) -> Option<String> {
    let mut conn = crate::ipc::connect(1000, "").await.ok()?;
    conn.send(&crate::ipc::Data::Options(None)).await.ok()?;
    match conn.next_timeout(1000).await {
        Ok(Some(crate::ipc::Data::Options(Some(options)))) => options.get(key).cloned(),
        _ => None,
    }
}

fn handle(req: &Value, write_allowed: &dyn Fn() -> bool) -> Option<Value> {
    // Requests without an id are notifications and get no response.
    let id = req.get("id")?.clone();
    let params = req.get("params").cloned().unwrap_or(Value::Null);
    Some(
        match req.get("method").and_then(Value::as_str).unwrap_or("") {
            "initialize" => {
                let version = params
                    .get("protocolVersion")
                    .and_then(Value::as_str)
                    .filter(|v| SUPPORTED_VERSIONS.contains(v))
                    .unwrap_or(PROTOCOL_VERSION);
                result(
                    id,
                    json!({
                        "protocolVersion": version,
                        "capabilities": {"tools": {}},
                        "serverInfo": {"name": "rustdesk", "version": crate::VERSION},
                    }),
                )
            }
            "ping" => result(id, json!({})),
            "tools/list" => result(id, json!({"tools": tool_list()})),
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or("");
                if !tool_list().iter().any(|t| t["name"] == name) {
                    return Some(error(id, -32602, &format!("Unknown tool: {name}")));
                }
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                let (text, is_error) = match call_tool(name, &args, write_allowed) {
                    Ok(text) => (text, false),
                    Err(text) => (text, true),
                };
                result(
                    id,
                    json!({"content": [{"type": "text", "text": text}], "isError": is_error}),
                )
            }
            method => error(id, -32601, &format!("Method not found: {method}")),
        },
    )
}

fn result(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn error(id: Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": {"type": "object", "properties": properties, "required": required},
    })
}

fn tool_list() -> Vec<Value> {
    let none = json!({});
    vec![
        tool("get_status", "RustDesk ID and version of this machine.", none.clone(), &[]),
        tool(
            "get_inventory",
            "Hardware, OS and installed software on this machine.",
            none.clone(),
            &[],
        ),
        tool(
            "get_alerts",
            "Monitoring thresholds and the 20 most recent alerts.",
            none.clone(),
            &[],
        ),
        tool("list_scripts", "Scripts in the local script library.", none.clone(), &[]),
        tool("list_peers", "Remote peers this machine has connected to.", none, &[]),
        tool(
            "run_script",
            "Run a script from the local library and return its output. Requires the rmm-agent-write option.",
            json!({"name": {"type": "string"}}),
            &["name"],
        ),
        tool(
            "set_option",
            "Set a monitoring option: rmm-monitor-enabled (Y/N), rmm-cpu-threshold, rmm-mem-threshold, rmm-disk-threshold (1-100, empty to clear) or rmm-watch-services (comma-separated). Requires the rmm-agent-write option.",
            json!({"name": {"type": "string"}, "value": {"type": "string"}}),
            &["name", "value"],
        ),
    ]
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("Missing string argument '{key}'"))
}

fn require_write(write_allowed: &dyn Fn() -> bool) -> Result<(), String> {
    if write_allowed() {
        Ok(())
    } else {
        Err(format!(
            "Write tools are disabled. A human must turn on Settings > Tools > AI agents, or run: rustdesk --option {} Y",
            keys::OPTION_RMM_AGENT_WRITE
        ))
    }
}

/// Changes one key on the server, or fails. `ipc::set_option` is unsuitable here: when
/// the read times out it pushes this process's whole local option map to the service,
/// keys unchecked, and it reports nothing when the write does not arrive.
#[tokio::main(flavor = "current_thread")]
async fn set_server_option(key: &str, value: &str) -> Result<(), String> {
    use crate::ipc::Data;
    let unreachable =
        |err: hbb_common::anyhow::Error| format!("RustDesk service unreachable: {err}");
    let mut conn = crate::ipc::connect(1000, "").await.map_err(unreachable)?;
    conn.send(&Data::Options(None)).await.map_err(unreachable)?;
    let mut options = match conn.next_timeout(1000).await {
        Ok(Some(Data::Options(Some(options)))) => options,
        _ => return Err("RustDesk service did not return its options".to_owned()),
    };
    if value.is_empty() {
        options.remove(key);
    } else {
        options.insert(key.to_owned(), value.to_owned());
    }
    conn.send(&Data::Options(Some(options)))
        .await
        .map_err(unreachable)?;
    match conn.next_timeout(1000).await {
        Ok(Some(Data::Options(None))) => Ok(()),
        _ => Err("RustDesk service did not confirm the change".to_owned()),
    }
}

/// Agents may tune monitoring only. Script execution (rmm-scripts-enabled runs the
/// library as SYSTEM) and the agent's own gate stay with a human.
fn check_settable(key: &str, value: &str) -> Result<(), String> {
    let ok = match key {
        keys::OPTION_RMM_MONITOR_ENABLED => value == "Y" || value == "N",
        keys::OPTION_RMM_CPU_THRESHOLD
        | keys::OPTION_RMM_MEM_THRESHOLD
        | keys::OPTION_RMM_DISK_THRESHOLD => {
            value.is_empty()
                || value
                    .parse::<f32>()
                    .map_or(false, |v| (1.0..=100.0).contains(&v))
        }
        keys::OPTION_RMM_WATCH_SERVICES => value.len() <= 1024 && !value.contains(['\r', '\n']),
        _ => return Err(format!("Option '{key}' cannot be set by an agent")),
    };
    if ok {
        Ok(())
    } else {
        Err(format!("Invalid value for {key}: '{value}'"))
    }
}

fn call_tool(name: &str, args: &Value, write_allowed: &dyn Fn() -> bool) -> Result<String, String> {
    match name {
        "get_status" => {
            Ok(json!({"id": crate::ipc::get_id(), "version": crate::VERSION}).to_string())
        }
        "get_inventory" => Ok(crate::rmm::inventory::snapshot().to_string()),
        "get_alerts" => {
            let options = crate::ipc::get_options();
            let config: serde_json::Map<String, Value> = options
                .iter()
                .filter(|(k, _)| {
                    k.starts_with("rmm-") && k.as_str() != keys::OPTION_RMM_RECENT_ALERTS
                })
                .map(|(k, v)| (k.clone(), Value::from(v.as_str())))
                .collect();
            let recent_alerts = crate::rmm::monitor::recent_alerts();
            Ok(json!({"config": config, "recent_alerts": recent_alerts}).to_string())
        }
        "list_scripts" => crate::rmm::scripts::list()
            .map(|v| v.to_string())
            .map_err(|err| err.to_string()),
        "list_peers" => {
            let peers: Vec<Value> = config::PeerConfig::peers(None)
                .into_iter()
                .map(|(id, _, p)| {
                    json!({
                        "id": id,
                        "username": p.info.username,
                        "hostname": p.info.hostname,
                        "platform": p.info.platform,
                    })
                })
                .collect();
            Ok(Value::from(peers).to_string())
        }
        "run_script" => {
            require_write(write_allowed)?;
            crate::rmm::scripts::run(str_arg(args, "name")?).map_err(|err| err.to_string())
        }
        "set_option" => {
            require_write(write_allowed)?;
            let key = str_arg(args, "name")?;
            let value = str_arg(args, "value")?;
            check_settable(key, value)?;
            set_server_option(key, value)?;
            Ok("Done!".to_owned())
        }
        _ => Err(format!("Unknown tool: {name}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(name: &str, args: Value, write: bool) -> Value {
        let req = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": name, "arguments": args}});
        handle(&req, &|| write).unwrap()["result"].clone()
    }

    #[test]
    fn notifications_get_no_response() {
        let req = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
        assert!(handle(&req, &|| false).is_none());
    }

    #[test]
    fn lists_all_tools() {
        let req = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"});
        let tools = handle(&req, &|| false).unwrap()["result"]["tools"].clone();
        assert_eq!(tools.as_array().unwrap().len(), 7);
    }

    #[test]
    fn write_tools_blocked_without_gate() {
        let res = call("run_script", json!({"name": "x"}), false);
        assert_eq!(res["isError"], true);
        assert!(res["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("disabled"));
    }

    #[test]
    fn agent_cannot_open_its_own_gate_or_touch_other_options() {
        for key in [keys::OPTION_RMM_AGENT_WRITE, "custom-rendezvous-server"] {
            let res = call("set_option", json!({"name": key, "value": "Y"}), true);
            assert_eq!(res["isError"], true, "{key}");
        }
    }

    #[test]
    fn negotiates_only_supported_versions() {
        let init = |v: &str| {
            let req = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"protocolVersion": v}});
            handle(&req, &|| false).unwrap()["result"]["protocolVersion"].clone()
        };
        assert_eq!(init("2024-11-05"), "2024-11-05");
        assert_eq!(init("2099-01-01"), PROTOCOL_VERSION);
    }

    #[test]
    fn set_option_allows_only_monitoring_keys_and_valid_values() {
        for (key, value) in [
            ("rmm-monitor-enabled", "Y"),
            ("rmm-monitor-enabled", "N"),
            ("rmm-cpu-threshold", "90"),
            ("rmm-disk-threshold", ""),
            ("rmm-watch-services", "Spooler, W32Time"),
        ] {
            assert!(check_settable(key, value).is_ok(), "{key}={value}");
        }
        for (key, value) in [
            ("rmm-scripts-enabled", "Y"),
            ("rmm-agent-write", "Y"),
            ("rmm-recent-alerts", "[]"),
            ("custom-rendezvous-server", "x"),
            ("rmm-monitor-enabled", "true"),
            ("rmm-cpu-threshold", "0"),
            ("rmm-cpu-threshold", "101"),
            ("rmm-mem-threshold", "abc"),
            ("rmm-watch-services", "a\nb"),
        ] {
            assert!(check_settable(key, value).is_err(), "{key}={value}");
        }
    }

    #[test]
    fn unknown_tool_is_a_protocol_error() {
        let req = json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": {"name": "nope", "arguments": {}}});
        let res = handle(&req, &|| true).unwrap();
        assert_eq!(res["error"]["code"], -32602);
        assert_eq!(res["id"], 3);
    }

    #[test]
    fn unknown_method_is_an_error() {
        let req = json!({"jsonrpc": "2.0", "id": 7, "method": "nope"});
        assert_eq!(handle(&req, &|| false).unwrap()["error"]["code"], -32601);
    }
}
