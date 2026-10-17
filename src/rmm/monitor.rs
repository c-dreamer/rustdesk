//! Background CPU/memory/disk threshold + service-down monitoring, gated by the
//! `rmm-monitor-enabled` option (off by default). Runs on its own thread in the
//! server/service process.

use base::config::keys;
#[cfg(windows)]
use hbb_common::{config, tokio};
use hbb_common::{config::Config, log, sysinfo::System};
#[cfg(windows)]
use tauri_winrt_notification::{Duration as ToastDuration, Sound, Toast};

const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);
const ALERTS_LOG_FILE: &str = "rmm_alerts.log";
const RECENT_ALERTS_KEPT: usize = 20;

fn option_threshold(key: &str) -> Option<f32> {
    Config::get_option(key).trim().parse().ok()
}

/// Pure so the branching is unit-testable without a real `System`/registry.
/// `disk` is `(mount point label, used percent)` for the worst (fullest) fixed disk,
/// if any disks were found.
fn check_thresholds(
    cpu_pct: f32,
    mem_pct: f32,
    disk: Option<(&str, f32)>,
    cpu_threshold: Option<f32>,
    mem_threshold: Option<f32>,
    disk_threshold: Option<f32>,
    down_services: &[String],
) -> Vec<String> {
    let mut alerts = Vec::new();
    if let Some(t) = cpu_threshold {
        if cpu_pct > t {
            alerts.push(format!("CPU usage {cpu_pct:.0}% exceeds threshold {t:.0}%"));
        }
    }
    if let Some(t) = mem_threshold {
        if mem_pct > t {
            alerts.push(format!(
                "Memory usage {mem_pct:.0}% exceeds threshold {t:.0}%"
            ));
        }
    }
    if let (Some(t), Some((mount, used_pct))) = (disk_threshold, disk) {
        if used_pct > t {
            alerts.push(format!(
                "Disk usage on {mount} {used_pct:.0}% exceeds threshold {t:.0}%"
            ));
        }
    }
    for name in down_services {
        alerts.push(format!("Service '{name}' is not running"));
    }
    alerts
}

/// A breach that persists across polls is reported once, when it starts; it is
/// reported again only after it clears. Keyed on the alert text minus its
/// percentages, which change from poll to poll.
fn newly_breached(
    active: &mut std::collections::HashSet<String>,
    alerts: Vec<String>,
) -> Vec<String> {
    let key = |a: &str| {
        a.split_whitespace()
            .filter(|w| !w.ends_with('%'))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let current: std::collections::HashSet<String> = alerts.iter().map(|a| key(a)).collect();
    let fresh = alerts
        .into_iter()
        .filter(|a| !active.contains(&key(a)))
        .collect();
    *active = current;
    fresh
}

#[cfg(windows)]
fn down_services() -> Vec<String> {
    watch_list(&Config::get_option(keys::OPTION_RMM_WATCH_SERVICES))
        .filter(|s| !crate::platform::is_service_running(s))
        .map(str::to_owned)
        .collect()
}

#[cfg(any(windows, test))]
fn watch_list(raw: &str) -> impl Iterator<Item = &str> {
    raw.split(',').map(str::trim).filter(|s| !s.is_empty())
}

#[cfg(not(windows))]
fn down_services() -> Vec<String> {
    Vec::new()
}

fn worst_fixed_disk() -> Option<(String, f32)> {
    use hbb_common::sysinfo::Disks;
    Disks::new_with_refreshed_list()
        .list()
        .iter()
        .filter(|d| !d.is_removable() && d.total_space() > 0)
        .map(|d| {
            let used = d.total_space().saturating_sub(d.available_space());
            let pct = used as f32 / d.total_space() as f32 * 100.0;
            (d.mount_point().display().to_string(), pct)
        })
        .fold(None, |worst: Option<(String, f32)>, cur| match &worst {
            Some(w) if w.1 >= cur.1 => worst,
            _ => Some(cur),
        })
}

fn alerts_log_path() -> std::path::PathBuf {
    Config::log_path().join(ALERTS_LOG_FILE)
}

/// The log lives in the monitoring process's own (possibly SYSTEM-only) log dir, so
/// the latest alerts are also mirrored into an option, which every client reads over IPC.
pub fn recent_alerts() -> Vec<String> {
    crate::ipc::get_options()
        .get(keys::OPTION_RMM_RECENT_ALERTS)
        .and_then(|v| serde_json::from_str(v).ok())
        .unwrap_or_default()
}

/// Newest first, capped at `RECENT_ALERTS_KEPT`.
fn merge_recent(existing: &str, now: i64, alerts: &[String]) -> Vec<String> {
    let mut recent: Vec<String> = serde_json::from_str(existing).unwrap_or_default();
    recent.splice(0..0, alerts.iter().rev().map(|a| format!("{now} {a}")));
    recent.truncate(RECENT_ALERTS_KEPT);
    recent
}

fn remember_alerts(now: i64, alerts: &[String]) {
    let recent = merge_recent(
        &Config::get_option(keys::OPTION_RMM_RECENT_ALERTS),
        now,
        alerts,
    );
    if let Ok(json) = serde_json::to_string(&recent) {
        Config::set_option(keys::OPTION_RMM_RECENT_ALERTS.to_owned(), json);
    }
}

fn log_alerts(alerts: &[String]) {
    remember_alerts(hbb_common::get_time(), alerts);
    let path = alerts_log_path();
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        log::error!("rmm: failed to open {:?} for alert logging", path);
        return;
    };
    let now = hbb_common::get_time();
    for alert in alerts {
        allow_err_write(&mut file, format!("{now} {alert}\n"));
    }
}

fn allow_err_write(file: &mut std::fs::File, line: String) {
    use std::io::Write;
    if let Err(err) = file.write_all(line.as_bytes()) {
        log::error!("rmm: failed to write alert log line: {err}");
    }
}

// ponytail: alerts surface via a Windows toast + rmm_alerts.log only, not the tray
// tooltip too. The tray icon runs in a separate (GUI) process from this service-side
// loop; wiring the two would need a new cross-process channel. Toast already covers
// "the user sees it now" and the log covers the audit trail, so skipped for v1.
// Upgrade path: add an IPC `Data` variant carrying the alert text, same channel
// `src/tray.rs` already reads `Data::ControlledSessionCount` from.
#[cfg(windows)]
fn notify(alerts: &[String]) {
    let text = alerts.join("\n");
    Toast::new(Toast::POWERSHELL_APP_ID)
        .title(&config::APP_NAME.read().unwrap())
        .text1(&text)
        .sound(Some(Sound::Default))
        .duration(ToastDuration::Long)
        .show()
        .ok();
}

#[cfg(not(windows))]
fn notify(_alerts: &[String]) {}

/// Entries of `recent` raised at or after `since` (ms), with the newest timestamp among
/// them. Keyed on time, not on the last entry shown: a whole-map options write can drop
/// that entry, and then every alert since startup would look new again.
#[cfg(any(windows, test))]
fn new_alerts(recent: &str, since: i64) -> (Vec<String>, Option<i64>) {
    let stamp = |a: &str| a.split_once(' ').and_then(|(t, _)| t.parse::<i64>().ok());
    let alerts: Vec<String> = serde_json::from_str::<Vec<String>>(recent)
        .unwrap_or_default()
        .into_iter()
        .filter(|a| stamp(a).map_or(false, |t| t >= since))
        .collect();
    let newest = alerts.iter().filter_map(|a| stamp(a)).max();
    (alerts, newest)
}

/// With hide-tray set, the `--tray` process would exit without an icon, and nothing in
/// the user's session would raise alerts. Keep it alive (still iconless) only while
/// monitoring is on; enabling monitoring later takes effect at the next logon.
#[cfg(windows)]
pub fn tray_alert_watch_hidden() {
    // The visible tray takes this lock in make_tray; check_process("--tray") alone
    // misses duplicates (#15689), and a hidden duplicate would now live forever.
    if !crate::platform::is_installed()
        || !crate::platform::windows::try_lock_tray_single_instance()
    {
        return;
    }
    // At logon the service may not answer yet, and ipc::get_options would then fall
    // back to this user's config, which never holds the option; wait for a real answer.
    for _ in 0..24 {
        match server_monitoring_enabled() {
            Some(true) => return tray_alert_watch(),
            Some(false) => return,
            None => std::thread::sleep(std::time::Duration::from_secs(5)),
        }
    }
    log::warn!("rmm: service IPC did not answer; no monitoring alerts in this session");
}

#[cfg(windows)]
#[tokio::main(flavor = "current_thread")]
async fn server_monitoring_enabled() -> Option<bool> {
    let mut conn = crate::ipc::connect(1000, "").await.ok()?;
    conn.send(&crate::ipc::Data::Options(None)).await.ok()?;
    match conn.next_timeout(1000).await {
        Ok(Some(crate::ipc::Data::Options(Some(options)))) => Some(
            options
                .get(keys::OPTION_RMM_MONITOR_ENABLED)
                .map_or(false, |v| v == "Y"),
        ),
        _ => None,
    }
}

/// Installed endpoints run `run` in the SYSTEM service, whose toasts never reach the
/// user's desktop, so the per-session tray process re-raises new alerts instead.
#[cfg(windows)]
#[tokio::main(flavor = "current_thread")]
pub async fn tray_alert_watch() {
    // Filtered by timestamp, not seeded from the current list, because at logon the
    // service may not answer IPC yet and the fallback options carry no alerts.
    let mut since = hbb_common::get_time();
    let mut timer = tokio::time::interval(POLL_INTERVAL);
    loop {
        timer.tick().await;
        let options = crate::ipc::get_options_async().await;
        if options
            .get(keys::OPTION_RMM_MONITOR_ENABLED)
            .map(String::as_str)
            != Some("Y")
        {
            continue;
        }
        let (alerts, newest) = new_alerts(
            options
                .get(keys::OPTION_RMM_RECENT_ALERTS)
                .map_or("", String::as_str),
            since,
        );
        if let Some(newest) = newest {
            notify(&alerts);
            since = newest + 1;
        }
    }
}

/// Runs on its own thread: disk and service queries, config writes and toasts all
/// block, and must not stall the async runtime.
pub fn run() {
    let mut system = System::new();
    let mut active = std::collections::HashSet::new();
    loop {
        std::thread::sleep(POLL_INTERVAL);
        if !crate::rmm::is_enabled(keys::OPTION_RMM_MONITOR_ENABLED) {
            active.clear();
            continue;
        }
        system.refresh_cpu_usage();
        system.refresh_memory();
        let cpu_pct = system.global_cpu_info().cpu_usage();
        let mem_pct = if system.total_memory() > 0 {
            system.used_memory() as f32 / system.total_memory() as f32 * 100.0
        } else {
            0.0
        };
        let disk = worst_fixed_disk();
        let alerts = check_thresholds(
            cpu_pct,
            mem_pct,
            disk.as_ref().map(|(m, p)| (m.as_str(), *p)),
            option_threshold(keys::OPTION_RMM_CPU_THRESHOLD),
            option_threshold(keys::OPTION_RMM_MEM_THRESHOLD),
            option_threshold(keys::OPTION_RMM_DISK_THRESHOLD),
            &down_services(),
        );
        let alerts = newly_breached(&mut active, alerts);
        if !alerts.is_empty() {
            log_alerts(&alerts);
            #[cfg(windows)]
            if crate::platform::is_installed() {
                continue;
            }
            notify(&alerts);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_thresholds_set_means_no_alerts() {
        assert!(
            check_thresholds(99.0, 99.0, Some(("C:\\", 99.0)), None, None, None, &[]).is_empty()
        );
    }

    #[test]
    fn breaches_are_reported_independently() {
        let alerts = check_thresholds(
            95.0,
            50.0,
            Some(("C:\\", 30.0)),
            Some(90.0),
            Some(90.0),
            Some(90.0),
            &[],
        );
        assert_eq!(alerts.len(), 1);
        assert!(alerts[0].contains("CPU"));
    }

    #[test]
    fn at_threshold_is_not_a_breach() {
        assert!(check_thresholds(90.0, 0.0, None, Some(90.0), None, None, &[]).is_empty());
    }

    #[test]
    fn down_service_always_reported() {
        let alerts = check_thresholds(0.0, 0.0, None, None, None, None, &["Spooler".to_owned()]);
        assert_eq!(alerts, vec!["Service 'Spooler' is not running".to_owned()]);
    }

    #[test]
    fn recent_alerts_newest_first_and_capped() {
        let first = merge_recent("", 1, &["a".to_owned(), "b".to_owned()]);
        assert_eq!(first, vec!["1 b", "1 a"]);
        let json = serde_json::to_string(&first).unwrap();
        let many: Vec<String> = (0..30).map(|i| i.to_string()).collect();
        let merged = merge_recent(&json, 2, &many);
        assert_eq!(merged.len(), RECENT_ALERTS_KEPT);
        assert_eq!(merged[0], "2 29");
        assert!(merge_recent("not json", 3, &[]).is_empty());
    }

    #[test]
    fn sustained_breach_reported_once_until_cleared() {
        let mut active = std::collections::HashSet::new();
        let cpu = |p: u32| format!("CPU usage {p}% exceeds threshold 80%");
        let svc = |n: &str| format!("Service '{n}' is not running");
        assert_eq!(newly_breached(&mut active, vec![cpu(90)]), vec![cpu(90)]);
        assert!(newly_breached(&mut active, vec![cpu(95)]).is_empty());
        assert_eq!(
            newly_breached(&mut active, vec![cpu(96), svc("svc1")]),
            vec![svc("svc1")]
        );
        assert_eq!(
            newly_breached(&mut active, vec![cpu(97), svc("svc1"), svc("svc2")]),
            vec![svc("svc2")]
        );
        assert!(newly_breached(&mut active, vec![]).is_empty());
        assert_eq!(newly_breached(&mut active, vec![cpu(85)]), vec![cpu(85)]);
    }

    #[test]
    fn watch_list_trims_and_skips_empty_entries() {
        let names: Vec<_> = watch_list(" Spooler, ,W32Time ,,").collect();
        assert_eq!(names, ["Spooler", "W32Time"]);
        assert_eq!(watch_list("").count(), 0);
    }

    #[test]
    fn new_alerts_skips_old_and_already_shown() {
        let recent = r#"["300 c","300 d","200 b","100 a"]"#;
        let (alerts, newest) = new_alerts(recent, 150);
        assert_eq!(alerts, vec!["300 c", "300 d", "200 b"]);
        assert_eq!(newest, Some(300));
        // Next poll starts after the newest shown; a dropped entry changes nothing.
        let (alerts, newest) = new_alerts(r#"["300 d","200 b"]"#, 301);
        assert!(alerts.is_empty() && newest.is_none());
        assert_eq!(new_alerts("", 0), (vec![], None));
    }

    #[test]
    fn all_breaches_reported_together() {
        let alerts = check_thresholds(
            95.0,
            96.0,
            Some(("D:\\", 97.0)),
            Some(90.0),
            Some(90.0),
            Some(90.0),
            &["Spooler".to_owned()],
        );
        assert_eq!(alerts.len(), 4);
    }
}
