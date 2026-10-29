//! The headless command line the installer runs: `usagequotatray.exe --quit` before an
//! upgrade or uninstall, and `--remove-from-pc [--delete-data]` when uninstalling.

use std::cell::Cell;
use std::path::Path;
use std::time::Duration;

use uqt_app::cli::{self, Command};
use uqt_ui::lifecycle::{
    APP_ID, AUMID_KEY, MemoryRegistry, OutsideChange, QuitListener, RUN_KEY, Registry,
    RunningMarker, register_notifications,
};

fn parse(args: &[&str]) -> Result<Command, String> {
    cli::parse(args.iter().map(|a| a.to_string()))
}

#[test]
fn no_arguments_starts_the_tray() {
    assert_eq!(parse(&[]), Ok(Command::Tray));
}

#[test]
fn remove_from_pc_keeps_the_data_unless_asked() {
    assert_eq!(
        parse(&["--remove-from-pc"]),
        Ok(Command::RemoveFromPc { delete_data: false })
    );
    assert_eq!(
        parse(&["--remove-from-pc", "--delete-data"]),
        Ok(Command::RemoveFromPc { delete_data: true })
    );
    assert_eq!(
        parse(&["--delete-data", "--remove-from-pc"]),
        Ok(Command::RemoveFromPc { delete_data: true })
    );
}

#[test]
fn unknown_or_incomplete_arguments_are_refused() {
    assert!(parse(&["--remove-from-p"]).is_err());
    assert!(parse(&["--delete-data"]).is_err());
    assert!(parse(&["--remove-from-pc", "extra"]).is_err());
}

#[derive(Debug)]
struct FakeChange {
    pending: bool,
    fails: bool,
    undone: Cell<bool>,
}

impl FakeChange {
    fn new(pending: bool, fails: bool) -> Self {
        Self {
            pending,
            fails,
            undone: Cell::new(false),
        }
    }
}

impl OutsideChange for FakeChange {
    fn pending(&self) -> Option<String> {
        self.pending.then(|| "put back the statusLine".to_string())
    }

    fn undo(&self) -> Result<(), String> {
        if self.fails {
            return Err("settings.json is locked".to_string());
        }
        self.undone.set(true);
        Ok(())
    }
}

fn registered(reg: &MemoryRegistry) {
    reg.write_string(RUN_KEY, APP_ID, "\"C:\\x\\usagequotatray.exe\"")
        .unwrap();
    register_notifications(reg, Path::new(r"C:\data\notification.png")).unwrap();
}

#[test]
fn removal_undoes_the_statusline_deletes_the_entries_and_exits_zero() {
    let reg = MemoryRegistry::new();
    registered(&reg);
    let data = tempfile::tempdir().unwrap();
    std::fs::write(data.path().join("settings.json"), "{}").unwrap();
    let change = FakeChange::new(true, false);

    let code = cli::remove_from_pc(&reg, &[&change], Some(data.path()), false);

    assert_eq!(code, cli::EXIT_REMOVED);
    assert_eq!(code, 0);
    assert!(change.undone.get());
    assert_eq!(reg.read_string(RUN_KEY, APP_ID), None);
    assert_eq!(reg.read_string(AUMID_KEY, "DisplayName"), None);
    assert!(data.path().join("settings.json").exists());
}

#[test]
fn removal_with_delete_data_also_deletes_the_data_folder() {
    let reg = MemoryRegistry::new();
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("UsageQuotaTray");
    std::fs::create_dir_all(data.join("logs")).unwrap();

    let code = cli::remove_from_pc(&reg, &[], Some(&data), true);

    assert_eq!(code, cli::EXIT_REMOVED);
    assert!(!data.exists());
}

#[test]
fn when_the_statusline_cannot_be_put_back_nothing_is_removed_and_the_exit_code_says_so() {
    let reg = MemoryRegistry::new();
    registered(&reg);
    let data = tempfile::tempdir().unwrap();
    std::fs::write(data.path().join("settings.json"), "{}").unwrap();
    let change = FakeChange::new(true, true);

    let code = cli::remove_from_pc(&reg, &[&change], Some(data.path()), true);

    assert_eq!(code, cli::EXIT_NOT_UNDONE);
    assert_ne!(code, 0);
    assert!(reg.read_string(RUN_KEY, APP_ID).is_some());
    assert!(reg.read_string(AUMID_KEY, "DisplayName").is_some());
    assert!(data.path().join("settings.json").exists());
}

#[test]
fn nothing_pending_is_not_undone() {
    let reg = MemoryRegistry::new();
    let change = FakeChange::new(false, true);
    assert_eq!(
        cli::remove_from_pc(&reg, &[&change], None, false),
        cli::EXIT_REMOVED
    );
    assert!(!change.undone.get());
}

#[test]
fn quit_asks_the_running_tray_to_exit() {
    assert_eq!(parse(&["--quit"]), Ok(Command::Quit));
    assert!(parse(&["--quit", "--remove-from-pc"]).is_err());
    assert!(parse(&["--remove-from-pc", "--quit"]).is_err());
    assert!(parse(&["--quit", "--delete-data"]).is_err());
}

#[test]
fn quit_exits_zero_with_no_tray_and_non_zero_when_one_stays() {
    let id = std::process::id();
    let event = format!("UsageQuotaTray-test-cli-quit-{id}");
    let running = format!("UsageQuotaTray-test-cli-running-{id}");
    assert_eq!(
        cli::quit(&event, &running, Duration::from_secs(5)),
        cli::EXIT_REMOVED
    );

    let _tray = RunningMarker::hold(&running).expect("marker");
    let code = cli::quit(&event, &running, Duration::from_millis(200));
    assert_eq!(code, cli::EXIT_STILL_RUNNING);
    assert_ne!(code, 0);
}

#[test]
fn quit_exits_zero_once_the_tray_has_closed() {
    let id = std::process::id();
    let event = format!("UsageQuotaTray-test-cli-quit-closed-{id}");
    let running = format!("UsageQuotaTray-test-cli-running-closed-{id}");
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let tray = {
        let (event, running) = (event.clone(), running.clone());
        std::thread::spawn(move || {
            let marker = RunningMarker::hold(&running).expect("marker");
            let (tx, rx) = std::sync::mpsc::channel();
            let listener = QuitListener::spawn(&event, move || {
                let _ = tx.send(());
            })
            .expect("listener");
            ready_tx.send(()).unwrap();
            let _ = rx.recv_timeout(Duration::from_secs(10));
            drop(listener);
            drop(marker);
        })
    };
    ready_rx.recv().unwrap();
    assert_eq!(cli::quit(&event, &running, Duration::from_secs(10)), 0);
    tray.join().unwrap();
}
