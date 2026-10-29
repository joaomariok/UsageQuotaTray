//! The headless command line the installer's uninstaller runs:
//! `usagequotatray.exe --remove-from-pc [--delete-data]`.

use std::cell::Cell;
use std::path::Path;

use uqt_app::cli::{self, Command};
use uqt_ui::lifecycle::{
    APP_ID, AUMID_KEY, MemoryRegistry, OutsideChange, RUN_KEY, Registry, register_notifications,
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
