//! UsageQuotaTray: one per-user tray process.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::sync::Arc;

use uqt_app::{ModuleOptions, enabled_modules};
use uqt_core::engine::Engine;
use uqt_core::http::NoNetwork;
use uqt_core::logging::FileLogger;
use uqt_core::settings::SettingsStore;
use uqt_core::time::SystemClock;
use uqt_ui::ShellOptions;
use uqt_ui::lifecycle::{SingleInstance, instance_name};

/// `%LOCALAPPDATA%\UsageQuotaTray`.
fn app_data_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("UsageQuotaTray"))
}

fn main() {
    let data = app_data_dir();
    // One instance per session (and app-data folder); a second launch just exits.
    let _instance = match &data {
        Some(dir) => match SingleInstance::acquire(&instance_name(dir)) {
            Some(guard) => Some(guard),
            None => return,
        },
        None => None,
    };

    let clock = Arc::new(SystemClock);
    if let Some(dir) = &data {
        let _ = FileLogger::new(&dir.join("logs"), clock.clone()).install(log::LevelFilter::Info);
    }
    log::info!("UsageQuotaTray {} starting", env!("CARGO_PKG_VERSION"));

    let store = data
        .as_ref()
        .map(|d| SettingsStore::new(d.join("settings.json")));
    let settings = store.as_ref().map(|s| s.load()).unwrap_or_default();

    let modules = enabled_modules(&ModuleOptions::from_env());
    // No module needs the network yet; the real transport arrives with the first one.
    let engine = Engine::with_settings(modules, clock.clone(), Arc::new(NoNetwork), &settings);
    let options = ShellOptions {
        badges_dir: data.as_ref().map(|d| d.join("badges")),
        ready_file: std::env::var_os("UQT_READY_FILE").map(PathBuf::from),
        icon_churn: std::env::var("UQT_ICON_CHURN").is_ok_and(|v| v == "1"),
        data_dir: data,
        settings,
        settings_store: store,
        exe: std::env::current_exe().ok(),
        // Measurement runs turn this off so they never touch the registry.
        notifications: !std::env::var("UQT_NOTIFICATIONS").is_ok_and(|v| v == "off"),
    };
    if let Err(e) = uqt_ui::run(engine, clock, options) {
        log::error!("UI failed: {e}");
    }
    log::info!("UsageQuotaTray exiting");
    log::logger().flush();
}
