//! Hardware + installed-software snapshot, for `--inventory` / the Tools tab.

use serde_json::{json, Value};

/// `crate::common::get_sysinfo()` (hostname/os/cpu/memory) plus, on Windows,
/// the installed-software list read from the same `Uninstall` registry subkeys
/// RustDesk already writes to on install (`src/platform/windows.rs::get_subkey`).
pub fn snapshot() -> Value {
    let mut out = crate::common::get_sysinfo();
    out["installed_software"] = json!(installed_software());
    out
}

#[cfg(windows)]
fn installed_software() -> Vec<Value> {
    use winreg::{
        enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE},
        RegKey,
    };

    const UNINSTALL_PATH: &str = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall";
    const UNINSTALL_PATH_WOW64: &str =
        "SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall";

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    // Per-user installs (e.g. VS Code user setup); the calling user's hive, so SYSTEM sees none.
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let mut apps = Vec::new();
    for (hive, path) in [
        (&hklm, UNINSTALL_PATH),
        (&hklm, UNINSTALL_PATH_WOW64),
        (&hkcu, UNINSTALL_PATH),
    ] {
        let Ok(uninstall) = hive.open_subkey(path) else {
            continue;
        };
        for name in uninstall.enum_keys().flatten() {
            let Ok(entry) = uninstall.open_subkey(&name) else {
                continue;
            };
            // Hidden updates/redistributables set this; skip them, same as Windows'
            // own "Programs and Features" list does.
            if entry.get_value::<u32, _>("SystemComponent").unwrap_or(0) == 1 {
                continue;
            }
            let display_name: String = entry.get_value("DisplayName").unwrap_or_default();
            if display_name.is_empty() {
                continue;
            }
            let display_version: String = entry.get_value("DisplayVersion").unwrap_or_default();
            apps.push(json!({
                "name": display_name,
                "version": display_version,
            }));
        }
    }
    apps.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    apps.dedup();
    apps
}

#[cfg(not(windows))]
fn installed_software() -> Vec<Value> {
    Vec::new()
}
