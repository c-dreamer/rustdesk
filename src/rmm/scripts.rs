//! Named script library: run a PowerShell (Windows) / shell (elsewhere) body
//! on demand via `--run-script`, or on its own schedule when one is set.

use hbb_common::{anyhow::anyhow, config::Config, log, tokio, ResultType};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;

const SCRIPTS_FILE: &str = "rmm_scripts.json";
const SCRIPTS_ENABLED_OPTION: &str = "rmm-scripts-enabled";
const SCHEDULER_TICK: std::time::Duration = std::time::Duration::from_secs(60);

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Script {
    pub body: String,
    pub schedule_secs: Option<u64>,
}

/// When installed, the scheduler runs in the SYSTEM server process, whose config dir is
/// the LocalService profile. Every caller must use that same file, and its ACL (admins
/// and SYSTEM only) is what stops a standard user queueing a script that runs as SYSTEM.
fn store_path() -> std::path::PathBuf {
    #[cfg(windows)]
    if crate::platform::is_installed() {
        if let Some(path) = service_store_path() {
            return path;
        }
    }
    Config::path(SCRIPTS_FILE)
}

#[cfg(windows)]
fn service_store_path() -> Option<std::path::PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows::Win32::System::SystemInformation::GetSystemDirectoryW;
    let mut buffer = vec![0u16; 260];
    let len = unsafe { GetSystemDirectoryW(Some(&mut buffer)) } as usize;
    if len == 0 || len >= buffer.len() {
        return None;
    }
    buffer.truncate(len);
    let system32 = std::path::PathBuf::from(std::ffi::OsString::from_wide(&buffer));
    let mut path = system32.parent()?.to_path_buf();
    path.push(r"ServiceProfiles\LocalService\AppData\Roaming");
    path.push(hbb_common::config::APP_NAME.read().unwrap().as_str());
    path.push("config");
    path.push(SCRIPTS_FILE);
    Some(path)
}

fn load() -> ResultType<HashMap<String, Script>> {
    match std::fs::read_to_string(store_path()) {
        Ok(content) => Ok(serde_json::from_str(&content).unwrap_or_default()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(HashMap::new()),
        Err(err) => Err(access_error(err)),
    }
}

fn access_error(err: std::io::Error) -> hbb_common::anyhow::Error {
    if err.kind() == std::io::ErrorKind::PermissionDenied {
        anyhow!("Administrator privileges required to use the script library")
    } else {
        err.into()
    }
}

fn save(scripts: &HashMap<String, Script>) -> ResultType<()> {
    let path = store_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(access_error)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(scripts)?).map_err(access_error)?;
    Ok(())
}

pub fn add(name: &str, body: &str, schedule_secs: Option<u64>) -> ResultType<()> {
    let mut scripts = load()?;
    scripts.insert(
        name.to_owned(),
        Script {
            body: body.to_owned(),
            schedule_secs,
        },
    );
    save(&scripts)
}

pub fn list() -> ResultType<Value> {
    let names: Vec<_> = load()?
        .into_iter()
        .map(|(name, s)| json!({"name": name, "schedule_secs": s.schedule_secs}))
        .collect();
    Ok(json!(names))
}

pub fn run(name: &str) -> ResultType<String> {
    let scripts = load()?;
    let script = scripts
        .get(name)
        .ok_or_else(|| anyhow!("No such script: {name}"))?;
    run_body(&script.body)
}

#[cfg(windows)]
fn run_body(body: &str) -> ResultType<String> {
    use std::os::windows::process::CommandExt;
    let output = std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            body,
        ])
        .creation_flags(winapi::um::winbase::CREATE_NO_WINDOW)
        .output()?;
    if !output.status.success() {
        log::warn!(
            "rmm script exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

#[cfg(not(windows))]
fn run_body(body: &str) -> ResultType<String> {
    hbb_common::sh::run_cmds_trim_newline(body)
}

/// Polls once a minute; a script with `schedule_secs` set runs once that much
/// time has elapsed since the last run. Elapsed time is tracked in memory only
/// (resets on restart) -- acceptable for a schedule granularity of minutes.
pub async fn run_scheduler() {
    let mut last_run: HashMap<String, std::time::Instant> = HashMap::new();
    let mut timer = crate::rustdesk_interval(tokio::time::interval(SCHEDULER_TICK));
    loop {
        timer.tick().await;
        if !crate::rmm::is_enabled(SCRIPTS_ENABLED_OPTION) {
            continue;
        }
        let scripts = match load() {
            Ok(scripts) => scripts,
            Err(err) => {
                log::warn!("rmm: failed to load scripts: {err}");
                continue;
            }
        };
        for (name, script) in scripts {
            let Some(schedule_secs) = script.schedule_secs else {
                continue;
            };
            let due = last_run
                .get(&name)
                .map(|t| t.elapsed().as_secs() >= schedule_secs)
                .unwrap_or(true);
            if !due {
                continue;
            }
            last_run.insert(name.clone(), std::time::Instant::now());
            if let Err(err) = run_body(&script.body) {
                log::warn!("rmm: scheduled script '{name}' failed: {err}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn service_store_is_the_system_process_config_dir() {
        let path = service_store_path().unwrap();
        let s = path.to_string_lossy().to_lowercase();
        assert!(
            s.ends_with(
                r"\serviceprofiles\localservice\appdata\roaming\rustdesk\config\rmm_scripts.json"
            ),
            "{s}"
        );
    }

    #[test]
    fn round_trips_through_json() {
        let mut scripts = HashMap::new();
        scripts.insert(
            "disk-cleanup".to_owned(),
            Script {
                body: "cleanmgr /sagerun:1".to_owned(),
                schedule_secs: Some(3600),
            },
        );
        let json = serde_json::to_string(&scripts).unwrap();
        let back: HashMap<String, Script> = serde_json::from_str(&json).unwrap();
        assert_eq!(back["disk-cleanup"].body, "cleanmgr /sagerun:1");
        assert_eq!(back["disk-cleanup"].schedule_secs, Some(3600));
    }
}
