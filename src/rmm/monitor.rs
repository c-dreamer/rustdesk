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
const WATCH_SERVICES_OPTION: &str = "rmm-watch-services";
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);
const ALERTS_LOG_FILE: &str = "rmm_alerts.log";

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

#[cfg(windows)]
fn down_services() -> Vec<String> {
    Config::get_option(WATCH_SERVICES_OPTION)
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter(|s| !crate::platform::is_service_running(s))
        .map(str::to_owned)
        .collect()
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

pub fn alerts_log_path() -> std::path::PathBuf {
    Config::log_path().join(ALERTS_LOG_FILE)
}

fn log_alerts(alerts: &[String]) {
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

pub async fn run() {
    let mut system = System::new();
    let mut timer = crate::rustdesk_interval(tokio::time::interval(POLL_INTERVAL));
    loop {
        timer.tick().await;
        if !crate::rmm::is_enabled(MONITOR_ENABLED_OPTION) {
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
        if !alerts.is_empty() {
            log_alerts(&alerts);
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
