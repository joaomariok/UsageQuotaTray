//! The statusline helper end to end, without Claude Code: the input Claude Code pipes
//! to it, a temporary app-data folder (so the pipe name is unique to the test) and,
//! where a tray is needed, a real push channel listening on that folder's pipe.

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use uqt_app::{HELPER_EXE, run_statusline_helper, statusline_installer};
use uqt_assistant_anthropic::statusline::chain::Shell;
use uqt_assistant_anthropic::statusline::installer::Installer;
use uqt_assistant_anthropic::{ConfigFolder, ID, sources};
use uqt_core::push::{PushServer, pipe_name};
use uqt_core::time::Timestamp;

const INPUT: &str = r#"{"model": {"display_name": "Opus"},
  "rate_limits": {"five_hour": {"used_percentage": 23.5, "resets_at": 1793285400},
                  "seven_day": {"used_percentage": 41.2, "resets_at": 1793757600}}}"#;

struct Dirs {
    _temp: tempfile::TempDir,
    data: PathBuf,
    folder: ConfigFolder,
}

fn dirs() -> Dirs {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("UsageQuotaTray");
    let claude = temp.path().join(".claude");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::create_dir_all(&claude).unwrap();
    Dirs {
        folder: ConfigFolder::new(claude),
        data,
        _temp: temp,
    }
}

#[test]
fn without_a_tray_it_still_prints_a_statusline_at_once() {
    let d = dirs();
    let started = Instant::now();
    let out = run_statusline_helper(
        INPUT.as_bytes(),
        Some(&d.data),
        Some(&d.folder),
        &Shell::powershell(),
    );
    assert!(started.elapsed() < Duration::from_millis(500));
    assert_eq!(String::from_utf8(out).unwrap(), "5h 24% \u{b7} 7d 41%\n");
}

#[test]
fn with_a_tray_the_rate_limits_are_pushed_to_it() {
    let d = dirs();
    let (tx, rx) = mpsc::channel();
    let server = PushServer::spawn(&pipe_name(&d.data), move |m| {
        let _ = tx.send(m);
    })
    .unwrap();
    run_statusline_helper(
        INPUT.as_bytes(),
        Some(&d.data),
        Some(&d.folder),
        &Shell::powershell(),
    );
    let m = rx.recv_timeout(Duration::from_secs(5)).expect("pushed");
    assert_eq!(m.assistant, ID);
    assert_eq!(m.source, sources::STATUSLINE);
    assert_eq!(
        m.payload["rate_limits"]["five_hour"]["used_percentage"],
        23.5
    );
    server.stop();
}

#[test]
fn an_installed_helper_prints_the_chained_commands_output_instead() {
    let d = dirs();
    let settings = d.folder.dir.join("settings.json");
    std::fs::write(
        &settings,
        r#"{"statusLine": {"type": "command", "command": "[Console]::In.ReadToEnd().Length"}}"#,
    )
    .unwrap();
    let installer = Installer::new(
        &d.folder,
        &d.data,
        PathBuf::from(r"C:\Apps\uqt-statusline.exe"),
    );
    installer.install(Timestamp(0)).unwrap();

    let out = run_statusline_helper(
        INPUT.as_bytes(),
        Some(&d.data),
        Some(&d.folder),
        &Shell::powershell(),
    );
    let printed = String::from_utf8(out).unwrap();
    // The chained command saw the very same input.
    assert_eq!(printed.trim(), INPUT.len().to_string());
}

#[test]
fn input_without_rate_limits_prints_nothing_and_pushes_nothing() {
    let d = dirs();
    let (tx, rx) = mpsc::channel();
    let server = PushServer::spawn(&pipe_name(&d.data), move |m| {
        let _ = tx.send(m);
    })
    .unwrap();
    let out = run_statusline_helper(
        br#"{"model": {}}"#,
        Some(&d.data),
        Some(&d.folder),
        &Shell::powershell(),
    );
    assert!(out.is_empty());
    assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
    server.stop();
}

#[test]
fn the_install_button_is_offered_only_when_the_helper_ships_beside_the_app() {
    let d = dirs();
    let exe_dir = d.data.join("app");
    std::fs::create_dir_all(&exe_dir).unwrap();
    assert_eq!(
        statusline_installer(&d.data, &exe_dir, Some(d.folder.clone())),
        None
    );
    std::fs::write(exe_dir.join(HELPER_EXE), b"").unwrap();
    let installer = statusline_installer(&d.data, &exe_dir, Some(d.folder.clone())).unwrap();
    assert_eq!(installer.helper, exe_dir.join(HELPER_EXE));
    assert_eq!(installer.settings_file, d.folder.dir.join("settings.json"));
    assert!(installer.state_file.starts_with(&d.data));
    assert_eq!(statusline_installer(&d.data, &exe_dir, None), None);
}
