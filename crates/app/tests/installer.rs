//! The installer script agrees with the app: the Start-with-Windows value it writes is
//! the one the app reads back as on, it waits on the mutex the tray holds, its shortcut
//! carries the app's notification identity, and its uninstaller runs the app's
//! headless removal. Reads `tools/installer/UsageQuotaTray.iss`; installs nothing.

use std::path::Path;

use uqt_app::cli;
use uqt_ui::lifecycle::{APP_ID, Autostart, MemoryRegistry, RUN_KEY, RUNNING_MUTEX, Registry};

fn script() -> String {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/installer/UsageQuotaTray.iss");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// An Inno Setup parameter value: quoted, embedded quotes doubled.
fn inno_quoted(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

/// The script's lines that are not comments.
fn lines(script: &str) -> Vec<&str> {
    script
        .lines()
        .map(str::trim)
        .filter(|l| !l.starts_with(';') && !l.starts_with("//"))
        .collect()
}

#[test]
fn the_run_value_is_written_exactly_as_the_app_writes_it() {
    let reg = MemoryRegistry::new();
    let exe = Path::new(r"{app}\usagequotatray.exe");
    Autostart::new(&reg, exe).set(true).unwrap();
    let app_value = reg.read_string(RUN_KEY, APP_ID).unwrap();
    // A known-good literal, independent of how the app builds it.
    assert_eq!(app_value, r#""{app}\usagequotatray.exe""#);

    let script = script();
    let entry = lines(&script)
        .into_iter()
        .find(|l| {
            l.starts_with("Root: HKCU;") && l.contains(&format!("Subkey: {}", inno_quoted(RUN_KEY)))
        })
        .expect("a [Registry] entry under the Run key");
    assert!(
        entry.contains(&format!("ValueName: {}", inno_quoted(APP_ID))),
        "{entry}"
    );
    assert!(entry.contains("ValueType: string"), "{entry}");
    assert!(
        entry.contains(&format!("ValueData: {}", inno_quoted(&app_value))),
        "{entry}"
    );
    assert!(entry.contains("uninsdeletevalue"), "{entry}");
}

#[test]
fn the_installer_waits_on_the_mutex_every_tray_holds() {
    let script = script();
    assert!(
        lines(&script).contains(&format!("AppMutex={RUNNING_MUTEX}").as_str()),
        "AppMutex={RUNNING_MUTEX}"
    );
    assert!(script.contains(&format!("CheckForMutexes('{RUNNING_MUTEX}')")));
}

#[test]
fn the_start_menu_shortcut_carries_the_notification_identity() {
    let script = script();
    let shortcut = lines(&script)
        .into_iter()
        .find(|l| l.starts_with("Name: \"{autoprograms}\\"))
        .expect("a Start menu shortcut");
    assert!(
        shortcut.contains(r#"Filename: "{app}\usagequotatray.exe""#),
        "{shortcut}"
    );
    assert!(
        shortcut.contains(&format!("AppUserModelID: {}", inno_quoted(APP_ID))),
        "{shortcut}"
    );
}

#[test]
fn the_uninstaller_runs_the_headless_removal_and_stops_when_it_says_so() {
    let script = script();
    assert!(script.contains(&format!("'{}'", cli::REMOVE_FROM_PC)));
    assert!(script.contains(&format!("'{}'", cli::DELETE_DATA)));
    assert!(script.contains(&format!("ExitNotUndone = {};", cli::EXIT_NOT_UNDONE)));
}
