//! UsageQuotaTray: one per-user tray process.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::sync::Arc;

use uqt_app::{ModuleOptions, app_data_dir, enabled_modules_with, statusline_installer};
use uqt_assistant_anthropic::ConfigFolder;
use uqt_core::engine::Engine;
use uqt_core::http::UreqTransport;
use uqt_core::logging::FileLogger;
use uqt_core::push::pipe_name;
use uqt_core::settings::SettingsStore;
use uqt_core::time::SystemClock;
use uqt_ui::ShellOptions;
use uqt_ui::lifecycle::{SingleInstance, instance_name};

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

    let exe = std::env::current_exe().ok();
    // The statusline install button, when the helper ships beside this exe.
    let statusline = match (&data, exe.as_deref().and_then(|e| e.parent())) {
        (Some(d), Some(exe_dir)) => {
            statusline_installer(d, exe_dir, ConfigFolder::discover_default())
        }
        _ => None,
    };
    let modules = enabled_modules_with(&ModuleOptions::from_env(), statusline);
    // Every request goes through each module's host client into this one transport,
    // on the scheduler thread.
    let engine = Engine::with_settings(
        modules,
        clock.clone(),
        Arc::new(UreqTransport::new()),
        &settings,
    );
    let options = ShellOptions {
        badges_dir: data.as_ref().map(|d| d.join("badges")),
        ready_file: std::env::var_os("UQT_READY_FILE").map(PathBuf::from),
        icon_churn: std::env::var("UQT_ICON_CHURN").is_ok_and(|v| v == "1"),
        push_pipe: data.as_deref().map(pipe_name),
        data_dir: data,
        settings,
        settings_store: store,
        exe,
        // Measurement runs turn this off so they never touch the registry.
        notifications: !std::env::var("UQT_NOTIFICATIONS").is_ok_and(|v| v == "off"),
    };
    if let Err(e) = uqt_ui::run(engine, clock, options) {
        log::error!("UI failed: {e}");
    }
    log::info!("UsageQuotaTray exiting");
    log::logger().flush();
}
