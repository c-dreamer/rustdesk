//! Single-machine RMM-style tooling: inventory, threshold monitoring/alerting, a
//! script library. Everything here is additive and off by default -- see
//! `is_enabled`. No server, no new protocol; reads/writes go through the
//! existing option store and plain JSON files next to the existing config dir.

pub mod inventory;
pub mod monitor;
pub mod scripts;

/// Runs in-process in the server/service process, so reading `Config` directly
/// (not `crate::ipc`/`crate::ui_interface`'s caches) always sees the latest value,
/// including one just applied by an IPC `--option` call in this same process.
pub fn is_enabled(name: &str) -> bool {
    hbb_common::config::Config::get_option(name) == "Y"
}
