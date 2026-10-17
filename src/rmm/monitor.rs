//! Background CPU/memory/disk threshold + service-down monitoring, gated by the
//! `rmm-monitor-enabled` option (off by default). Runs in-process in the
//! server/service process alongside the rest of RustDesk's background loops
//! (see `crate::rustdesk_interval` callers elsewhere in this crate).

#[cfg(windows)]
use hbb_common::config;
use hbb_common::{config::Config, log, sysinfo::System, tokio};
#[cfg(windows)]
use tauri_winrt_notification::{Duration as ToastDuration, Sound, Toast};

const MONITOR_ENABLED_OPTION: &str = "rmm-monitor-enabled";
const CPU_THRESHOLD_OPTION: &str = "rmm-cpu-threshold";
const MEM_THRESHOLD_OPTION: &str = "rmm-mem-threshold";
const DISK_THRESHOLD_OPTION: &str = "rmm-disk-threshold";
#[cfg(windows)]
const WATCH_SERVICES_OPTION: &str = "rmm-watch-services";
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);
const ALERTS_LOG_FILE: &str = "rmm_alerts.log";
const RECENT_ALERTS_OPTION: &str = "rmm-recent-alerts";
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
    watch_list(&Config::get_option(WATCH_SERVICES_OPTION))
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
        .get(RECENT_ALERTS_OPTION)
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
    let recent = merge_recent(&Config::get_option(RECENT_ALERTS_OPTION), now, alerts);
    if let Ok(json) = serde_json::to_string(&recent) {
        Config::set_option(RECENT_ALERTS_OPTION.to_owned(), json);
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

/// Entries of `recent` (newest first) raised at or after `since` and newer than `seen`.
#[cfg(any(windows, test))]
fn new_alerts(recent: &str, since: i64, seen: Option<&str>) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(recent)
        .unwrap_or_default()
        .into_iter()
        .take_while(|a| Some(a.as_str()) != seen)
        .filter(|a| {
            a.split_once(' ')
                .and_then(|(t, _)| t.parse::<i64>().ok())
                .map_or(false, |t| t >= since)
        })
        .collect()
}

/// Installed endpoints run `run` in the SYSTEM service, whose toasts never reach the
/// user's desktop, so the per-session tray process re-raises new alerts instead.
#[cfg(windows)]
#[tokio::main(flavor = "current_thread")]
pub async fn tray_alert_watch() {
    // Filtered by timestamp, not seeded from the current list, because at logon the
    // service may not answer IPC yet and the fallback options carry no alerts.
    let started = hbb_common::get_time();
    let mut seen: Option<String> = None;
    let mut timer = tokio::time::interval(POLL_INTERVAL);
    loop {
        timer.tick().await;
        let options = crate::ipc::get_options_async().await;
        if options.get(MONITOR_ENABLED_OPTION).map(String::as_str) != Some("Y") {
            continue;
        }
        let alerts = new_alerts(
            options.get(RECENT_ALERTS_OPTION).map_or("", String::as_str),
            started,
            seen.as_deref(),
        );
        if let Some(newest) = alerts.first() {
            notify(&alerts);
            seen = Some(newest.clone());
        }
    }
}

pub async fn run() {
    let mut system = System::new();
    let mut active = std::collections::HashSet::new();
    let mut timer = crate::rustdesk_interval(tokio::time::interval(POLL_INTERVAL));
    loop {
        timer.tick().await;
        if !crate::rmm::is_enabled(MONITOR_ENABLED_OPTION) {
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
            option_threshold(CPU_THRESHOLD_OPTION),
            option_threshold(MEM_THRESHOLD_OPTION),
            option_threshold(DISK_THRESHOLD_OPTION),
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
    fn new_alerts_skips_old_and_seen() {
        let recent = r#"["300 c","200 b","100 a"]"#;
        assert_eq!(new_alerts(recent, 150, None), vec!["300 c", "200 b"]);
        assert_eq!(new_alerts(recent, 150, Some("200 b")), vec!["300 c"]);
        assert!(new_alerts(recent, 150, Some("300 c")).is_empty());
        assert!(new_alerts("", 0, None).is_empty());
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
