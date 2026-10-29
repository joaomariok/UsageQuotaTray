//! UsageQuotaTray: one per-user tray process.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::sync::Arc;

use uqt_app::{ModuleOptions, enabled_modules};
use uqt_core::engine::Engine;
use uqt_core::http::UreqTransport;
use uqt_core::logging::FileLogger;
use uqt_core::time::SystemClock;
use uqt_ui::ShellOptions;

/// `%LOCALAPPDATA%\UsageQuotaTray`.
fn app_data_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("UsageQuotaTray"))
}

fn main() {
    let clock = Arc::new(SystemClock);
    let data = app_data_dir();
    if let Some(dir) = &data {
        let _ = FileLogger::new(&dir.join("logs"), clock.clone()).install(log::LevelFilter::Info);
    }
    log::info!("UsageQuotaTray {} starting", env!("CARGO_PKG_VERSION"));

    let modules = enabled_modules(&ModuleOptions::from_env());
    // Every request goes through each module's host client into this one transport,
    // on the scheduler thread.
    let engine = Engine::new(modules, clock.clone(), Arc::new(UreqTransport::new()));
    let options = ShellOptions {
        badges_dir: data.map(|d| d.join("badges")),
        ready_file: std::env::var_os("UQT_READY_FILE").map(PathBuf::from),
        icon_churn: std::env::var("UQT_ICON_CHURN").is_ok_and(|v| v == "1"),
    };
    if let Err(e) = uqt_ui::run(engine, clock, options) {
        log::error!("UI failed: {e}");
    }
    log::info!("UsageQuotaTray exiting");
    log::logger().flush();
}
