//! Named script library: run a PowerShell (Windows) / shell (elsewhere) body
//! on demand via `--run-script`, or on its own schedule when one is set.

use base::config::keys;
use hbb_common::{anyhow::anyhow, config::Config, log, tokio, ResultType};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;

const SCRIPTS_FILE: &str = "rmm_scripts.json";
const LAST_RUN_FILE: &str = "rmm_scripts_last_run.json";
const SCHEDULER_TICK: std::time::Duration = std::time::Duration::from_secs(60);
const SCRIPT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);
const OUTPUT_CAP: usize = 64 * 1024;
static RUN_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

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
        Ok(content) => serde_json::from_str(&content)
            .map_err(|err| anyhow!("Script library {:?} is not valid JSON: {err}", store_path())),
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

/// Adds or replaces a script's body, keeping its schedule: the Tools tab has no schedule
/// field, so re-saving a scheduled script there must not turn it into on-demand only.
pub fn update_body(name: &str, body: &str) -> ResultType<()> {
    let mut scripts = load()?;
    let schedule_secs = scripts.get(name).and_then(|s| s.schedule_secs);
    scripts.insert(
        name.to_owned(),
        Script {
            body: body.to_owned(),
            schedule_secs,
        },
    );
    save(&scripts)
}

/// The optional schedule argument of `--add-script`: absent means on demand only.
pub fn parse_schedule_secs(arg: Option<&str>) -> Result<Option<u64>, String> {
    match arg.map(str::parse::<u64>) {
        None => Ok(None),
        Some(Ok(secs)) if secs > 0 => Ok(Some(secs)),
        Some(_) => Err("Schedule must be a positive number of seconds".to_owned()),
    }
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

/// The body runs from a file next to the library, not the command line: that has a
/// 32K limit and lands in EDR process-creation logs. Same directory, same ACL.
fn run_body(body: &str) -> ResultType<String> {
    let seq = RUN_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = store_path().with_file_name(format!(
        "rmm_script_run_{}_{seq}.{SCRIPT_EXT}",
        std::process::id()
    ));
    std::fs::write(&path, script_file_contents(body)).map_err(access_error)?;
    let result = run_limited(script_command(&path), SCRIPT_TIMEOUT);
    if let Err(err) = std::fs::remove_file(&path) {
        log::warn!("rmm: failed to remove {:?}: {err}", path);
    }
    result
}

#[cfg(windows)]
const SCRIPT_EXT: &str = "ps1";
#[cfg(not(windows))]
const SCRIPT_EXT: &str = "sh";

// Windows PowerShell 5.1 reads a BOM-less .ps1 as the ANSI code page. A body read from
// a file that already had a BOM (VS Code's default for .ps1) must not get a second one:
// PowerShell would take it as part of the first command's name and skip that line.
#[cfg(windows)]
fn script_file_contents(body: &str) -> String {
    format!("\u{FEFF}{}", body.trim_start_matches('\u{FEFF}'))
}

#[cfg(not(windows))]
fn script_file_contents(body: &str) -> String {
    body.trim_start_matches('\u{FEFF}').to_owned()
}

#[cfg(windows)]
fn script_command(path: &std::path::Path) -> std::process::Command {
    use std::os::windows::process::CommandExt;
    let mut cmd = std::process::Command::new("powershell.exe");
    cmd.args([
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
    ])
    .arg(path)
    .creation_flags(winapi::um::winbase::CREATE_NO_WINDOW);
    cmd
}

#[cfg(not(windows))]
fn script_command(path: &std::path::Path) -> std::process::Command {
    let mut cmd = std::process::Command::new("sh");
    cmd.arg(path);
    cmd
}

/// Kills the script after `timeout`, keeps at most `OUTPUT_CAP` bytes of each stream,
/// and reports a non-zero exit as an error carrying stderr.
fn run_limited(mut cmd: std::process::Command, timeout: std::time::Duration) -> ResultType<String> {
    use std::process::Stdio;
    // Own process group, so a timeout can kill everything the script started.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = capture(child.stdout.take());
    let stderr = capture(child.stderr.take());
    let started = std::time::Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if started.elapsed() >= timeout {
            kill_tree(child.id());
            if let Err(err) = child.kill() {
                log::warn!("rmm: failed to kill timed-out script: {err}");
            }
            child.wait().ok();
            break None;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    // A background grandchild can hold the pipes open; don't wait on it for long.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let stdout = captured(stdout, deadline);
    let stderr = captured(stderr, deadline);
    match status {
        None => Err(anyhow!("Script timed out after {}s", timeout.as_secs())),
        Some(status) if !status.success() => {
            Err(anyhow!("Script exited with {status}: {}", stderr.trim()))
        }
        Some(_) => Ok(stdout.trim().to_owned()),
    }
}

#[cfg(windows)]
fn kill_tree(pid: u32) {
    use std::os::windows::process::CommandExt;
    let result = std::process::Command::new("taskkill")
        .args(["/T", "/F", "/PID", &pid.to_string()])
        .creation_flags(winapi::um::winbase::CREATE_NO_WINDOW)
        .output();
    if let Err(err) = result {
        log::warn!("rmm: taskkill failed for script {pid}: {err}");
    }
}

#[cfg(unix)]
fn kill_tree(pid: u32) {
    let result = std::process::Command::new("kill")
        .args(["-KILL", "--", &format!("-{pid}")])
        .output();
    if let Err(err) = result {
        log::warn!("rmm: kill failed for script group {pid}: {err}");
    }
}

type Capture = (
    std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    Option<std::thread::JoinHandle<()>>,
);

fn capture<R: std::io::Read + Send + 'static>(source: Option<R>) -> Capture {
    let buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let thread = source.map(|mut source| {
        let buf = buf.clone();
        std::thread::spawn(move || {
            let mut chunk = [0u8; 8192];
            while let Ok(n) = source.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                let mut buf = buf.lock().unwrap();
                let room = (OUTPUT_CAP + 1).saturating_sub(buf.len());
                buf.extend_from_slice(&chunk[..n.min(room)]);
            }
        })
    });
    (buf, thread)
}

fn captured((buf, thread): Capture, deadline: std::time::Instant) -> String {
    if let Some(thread) = thread {
        while !thread.is_finished() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
    let buf = buf.lock().unwrap();
    let mut text = String::from_utf8_lossy(&buf[..buf.len().min(OUTPUT_CAP)]).into_owned();
    if buf.len() > OUTPUT_CAP {
        text.push_str("\n[output truncated]");
    }
    text
}

fn last_run_path() -> std::path::PathBuf {
    store_path().with_file_name(LAST_RUN_FILE)
}

/// Persisted because the service restarts `--server` on every session change;
/// in-memory state would rerun every scheduled script at each logon.
fn load_last_run() -> HashMap<String, i64> {
    std::fs::read_to_string(last_run_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn is_due(last_run: Option<i64>, now: i64, schedule_secs: u64) -> bool {
    last_run.map_or(true, |t| {
        now.saturating_sub(t) as u64 >= schedule_secs || t > now
    })
}

/// Polls once a minute; a script with `schedule_secs` set runs once that much
/// time has elapsed since its last run.
pub async fn run_scheduler() {
    let mut last_run = load_last_run();
    let mut timer = crate::rustdesk_interval(tokio::time::interval(SCHEDULER_TICK));
    loop {
        timer.tick().await;
        if !crate::rmm::is_enabled(keys::OPTION_RMM_SCRIPTS_ENABLED) {
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
            let now = hbb_common::get_time() / 1000;
            if !is_due(last_run.get(&name).copied(), now, schedule_secs) {
                continue;
            }
            last_run.insert(name.clone(), now);
            match serde_json::to_string(&last_run) {
                Ok(json) => {
                    if let Err(err) = std::fs::write(last_run_path(), json) {
                        log::warn!("rmm: failed to save script last-run state: {err}");
                    }
                }
                Err(err) => log::warn!("rmm: failed to encode script last-run state: {err}"),
            }
            match tokio::task::spawn_blocking(move || run_body(&script.body)).await {
                Ok(Err(err)) => log::warn!("rmm: scheduled script '{name}' failed: {err}"),
                Err(err) => log::warn!("rmm: scheduled script '{name}' panicked: {err}"),
                Ok(Ok(_)) => {}
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
    fn due_after_interval_or_clock_rollback() {
        assert!(is_due(None, 100, 60));
        assert!(!is_due(Some(100), 159, 60));
        assert!(is_due(Some(100), 160, 60));
        assert!(is_due(Some(500), 100, 60));
    }

    #[cfg(not(windows))]
    fn sh(script: &str) -> std::process::Command {
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", script]);
        cmd
    }

    #[cfg(not(windows))]
    #[test]
    fn failed_script_is_an_error_with_stderr() {
        let err = run_limited(sh("echo out; echo boom >&2; exit 3"), SCRIPT_TIMEOUT)
            .unwrap_err()
            .to_string();
        assert!(err.contains("boom"), "{err}");
        assert_eq!(run_limited(sh("echo ok"), SCRIPT_TIMEOUT).unwrap(), "ok");
    }

    #[cfg(not(windows))]
    #[test]
    fn hung_script_is_killed_at_timeout() {
        let started = std::time::Instant::now();
        let err = run_limited(sh("sleep 30"), std::time::Duration::from_millis(300))
            .unwrap_err()
            .to_string();
        assert!(err.contains("timed out"), "{err}");
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    #[cfg(not(windows))]
    #[test]
    fn timeout_kills_processes_the_script_started() {
        let pid_file = std::env::temp_dir().join(format!("rmm-bg-{}", std::process::id()));
        let script = format!("sleep 30 & echo $! > {}; wait", pid_file.display());
        assert!(run_limited(sh(&script), std::time::Duration::from_millis(500)).is_err());
        let pid = std::fs::read_to_string(&pid_file).unwrap();
        std::fs::remove_file(&pid_file).ok();
        // Gone, or a zombie nobody has reaped yet (no init reaper in some containers).
        let dead = || {
            let out = std::process::Command::new("ps")
                .args(["-o", "stat=", "-p", pid.trim()])
                .output()
                .unwrap();
            let stat = String::from_utf8_lossy(&out.stdout);
            stat.trim().is_empty() || stat.trim_start().starts_with('Z')
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !dead() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(
            dead(),
            "background child {} survived the timeout",
            pid.trim()
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn output_is_capped() {
        let out = run_limited(sh("head -c 200000 /dev/zero | tr '\\0' a"), SCRIPT_TIMEOUT).unwrap();
        assert!(out.ends_with("[output truncated]"));
        assert!(out.len() < OUTPUT_CAP + 64);
    }

    #[test]
    fn schedule_argument_must_be_positive_seconds() {
        assert_eq!(parse_schedule_secs(None), Ok(None));
        assert_eq!(parse_schedule_secs(Some("3600")), Ok(Some(3600)));
        for bad in ["0", "-5", "1h", "", " 60"] {
            assert!(parse_schedule_secs(Some(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_body_saved_with_a_bom_runs_its_first_line() {
        let body = "\u{FEFF}echo first";
        assert_eq!(
            script_file_contents(body).matches('\u{FEFF}').count(),
            if cfg!(windows) { 1 } else { 0 }
        );
        #[cfg(not(windows))]
        {
            let dir = std::env::temp_dir().join(format!("rmm-bom-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("bom.sh");
            std::fs::write(&path, script_file_contents(body)).unwrap();
            let out = run_limited(script_command(&path), SCRIPT_TIMEOUT);
            std::fs::remove_dir_all(&dir).ok();
            assert_eq!(out.unwrap(), "first");
        }
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
