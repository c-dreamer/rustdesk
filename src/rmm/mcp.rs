//! `rustdesk --mcp`: a Model Context Protocol server over stdio (newline-delimited
//! JSON-RPC 2.0), so local agents (Claude Code, Codex, opencode) can drive the RMM
//! tooling headlessly. No listening port. Read tools are always available; tools that
//! change state require the `rmm-agent-write` option, which the agent cannot set itself.

use hbb_common::config;
use serde_json::{json, Value};
use std::io::{BufRead, Write};

const AGENT_WRITE_OPTION: &str = "rmm-agent-write";
const PROTOCOL_VERSION: &str = "2025-06-18";

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
    !cli_settings_disabled
        && crate::ipc::get_options()
            .get(AGENT_WRITE_OPTION)
            .map_or(false, |v| v == "Y")
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
            "Set an rmm-* option, e.g. rmm-monitor-enabled=Y or rmm-cpu-threshold=90. Requires the rmm-agent-write option.",
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
            "Write tools are disabled. A human must run: rustdesk --option {AGENT_WRITE_OPTION} Y"
        ))
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
                .filter(|(k, _)| k.starts_with("rmm-"))
                .map(|(k, v)| (k.clone(), Value::from(v.as_str())))
                .collect();
            let recent_alerts: Vec<String> =
                std::fs::read_to_string(crate::rmm::monitor::alerts_log_path())
                    .unwrap_or_default()
                    .lines()
                    .rev()
                    .take(20)
                    .map(str::to_owned)
                    .collect();
            Ok(json!({"config": config, "recent_alerts": recent_alerts}).to_string())
        }
        "list_scripts" => Ok(crate::rmm::scripts::list().to_string()),
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
            if !key.starts_with("rmm-") || key == AGENT_WRITE_OPTION {
                return Err(format!(
                    "Only rmm-* options other than {AGENT_WRITE_OPTION} can be set"
                ));
            }
            crate::ipc::set_option(key, str_arg(args, "value")?);
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
        for key in [AGENT_WRITE_OPTION, "custom-rendezvous-server"] {
            let res = call("set_option", json!({"name": key, "value": "Y"}), true);
            assert_eq!(res["isError"], true, "{key}");
        }
    }

    #[test]
    fn unknown_method_is_an_error() {
        let req = json!({"jsonrpc": "2.0", "id": 7, "method": "nope"});
        assert_eq!(handle(&req, &|| false).unwrap()["error"]["code"], -32601);
    }
}
