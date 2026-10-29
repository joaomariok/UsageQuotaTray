//! UsageQuotaTray: one per-user tray process.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::rc::Rc;
use std::sync::Arc;

use uqt_app::cli::{self, Command};
use uqt_app::{
    ModuleOptions, StatuslineChange, app_data_dir, enabled_modules_with, statusline_installer,
};
use uqt_assistant_anthropic::ConfigFolder;
use uqt_core::engine::Engine;
use uqt_core::http::UreqTransport;
use uqt_core::logging::FileLogger;
use uqt_core::push::pipe_name;
use uqt_core::settings::SettingsStore;
use uqt_core::time::SystemClock;
use uqt_ui::lifecycle::{
    CurrentUserRegistry, OutsideChange, RUNNING_MUTEX, RunningMarker, SingleInstance, instance_name,
};
use uqt_ui::{DevHooks, ShellOptions};

fn main() {
    match cli::parse(std::env::args().skip(1)) {
        Ok(Command::Tray) => run_tray(),
        Ok(Command::RemoveFromPc { delete_data }) => {
            std::process::exit(remove_from_pc(delete_data))
        }
        Err(_) => std::process::exit(cli::EXIT_USAGE),
    }
}

/// `--remove-from-pc`: what the in-app button does after confirming, with no UI. The
/// uninstaller runs it after closing the tray, and stops when it fails.
fn remove_from_pc(delete_data: bool) -> i32 {
    let data = app_data_dir();
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|p| p.to_path_buf()));
    let changes: Vec<StatuslineChange> = match (&data, &exe_dir) {
        (Some(d), Some(exe_dir)) => {
            statusline_installer(d, exe_dir, ConfigFolder::discover_default())
        }
        _ => None,
    }
    .into_iter()
    .map(StatuslineChange::new)
    .collect();
    let changes: Vec<&dyn OutsideChange> = changes.iter().map(|c| c as _).collect();
    cli::remove_from_pc(&CurrentUserRegistry, &changes, data.as_deref(), delete_data)
}

fn run_tray() {
    let data = app_data_dir();
    // One instance per session (and app-data folder); a second launch just exits.
    let _instance = match &data {
        Some(dir) => match SingleInstance::acquire(&instance_name(dir)) {
            Some(guard) => Some(guard),
            None => return,
        },
        None => None,
    };
    // Tells the installer a tray is running, whatever its data folder.
    let _running = RunningMarker::hold(RUNNING_MUTEX);

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
    // Removal from the PC puts Claude Code's statusLine back first.
    let outside_changes: Vec<Rc<dyn OutsideChange>> = statusline
        .iter()
        .map(|i| Rc::new(StatuslineChange::new(i.clone())) as Rc<dyn OutsideChange>)
        .collect();
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
        push_pipe: data.as_deref().map(pipe_name),
        data_dir: data,
        settings,
        settings_store: store,
        exe,
        outside_changes,
        dev: dev_hooks(),
    };
    if let Err(e) = uqt_ui::run(engine, clock, options) {
        log::error!("UI failed: {e}");
    }
    log::info!("UsageQuotaTray exiting");
    log::logger().flush();
}

/// The measurement switches, read only in a `dev-hooks` build.
fn dev_hooks() -> DevHooks {
    #[cfg(feature = "dev-hooks")]
    return DevHooks::from_env();
    #[cfg(not(feature = "dev-hooks"))]
    DevHooks::default()
}
