//! The app's Windows lifecycle over a registry seam: Start with Windows, the
//! notification identity, removal from the PC, and one instance per session. Tests use
//! the in-memory registry and temporary folders; nothing touches the real HKCU.

use std::cell::RefCell;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use uqt_ui::lifecycle::{
    APP_ID, AUMID_KEY, Autostart, MemoryRegistry, OutsideChange, QUIT_EVENT, QuitListener,
    QuitOutcome, RUN_KEY, RUNNING_MUTEX, Registry, RemovalError, RunningMarker, SingleInstance,
    UNINSTALL_KEY, installed_uninstaller, instance_name, register_notifications, removal_question,
    remove_from_pc, remove_from_pc_undoing, request_quit,
};

const EXE: &str = r"C:\Tools\UsageQuotaTray\usagequotatray.exe";

#[test]
fn start_with_windows_writes_the_quoted_exe_under_the_run_key() {
    let reg = MemoryRegistry::new();
    let auto = Autostart::new(&reg, Path::new(EXE));
    assert!(!auto.is_enabled());

    auto.set(true).unwrap();
    assert!(auto.is_enabled());
    assert_eq!(
        reg.read_string(RUN_KEY, APP_ID).as_deref(),
        Some("\"C:\\Tools\\UsageQuotaTray\\usagequotatray.exe\"")
    );

    auto.set(false).unwrap();
    assert!(!auto.is_enabled());
    assert_eq!(reg.read_string(RUN_KEY, APP_ID), None);
    auto.set(false).unwrap();
}

#[test]
fn an_entry_for_a_moved_exe_reads_as_off_and_enabling_fixes_it() {
    let reg = MemoryRegistry::new();
    reg.write_string(RUN_KEY, APP_ID, "\"D:\\old\\usagequotatray.exe\"")
        .unwrap();
    let auto = Autostart::new(&reg, Path::new(EXE));
    assert!(!auto.is_enabled());
    auto.set(true).unwrap();
    assert!(auto.is_enabled());
}

#[test]
fn the_notification_identity_has_a_display_name_and_icon() {
    let reg = MemoryRegistry::new();
    register_notifications(&reg, Path::new(r"C:\data\notification.png")).unwrap();
    assert_eq!(AUMID_KEY, r"Software\Classes\AppUserModelId\UsageQuotaTray");
    assert_eq!(
        reg.read_string(AUMID_KEY, "DisplayName").as_deref(),
        Some("UsageQuotaTray")
    );
    assert_eq!(
        reg.read_string(AUMID_KEY, "IconUri").as_deref(),
        Some(r"C:\data\notification.png")
    );
}

#[test]
fn removal_deletes_the_run_value_and_notification_registration_only() {
    let reg = MemoryRegistry::new();
    Autostart::new(&reg, Path::new(EXE)).set(true).unwrap();
    reg.write_string(RUN_KEY, "SomethingElse", "x.exe").unwrap();
    register_notifications(&reg, Path::new(r"C:\n.png")).unwrap();
    reg.write_string(
        r"Software\Classes\AppUserModelId\Another.App",
        "DisplayName",
        "Another",
    )
    .unwrap();
    let data = tempfile::tempdir().unwrap();

    remove_from_pc(&reg, Some(data.path()), false).unwrap();

    assert_eq!(reg.read_string(RUN_KEY, APP_ID), None);
    assert_eq!(reg.read_string(AUMID_KEY, "DisplayName"), None);
    assert_eq!(reg.read_string(AUMID_KEY, "IconUri"), None);
    assert_eq!(
        reg.read_string(RUN_KEY, "SomethingElse").as_deref(),
        Some("x.exe")
    );
    assert!(
        reg.read_string(
            r"Software\Classes\AppUserModelId\Another.App",
            "DisplayName"
        )
        .is_some()
    );
    assert!(data.path().exists());
}

#[test]
fn removal_can_delete_the_data_folder_even_with_the_log_file_open() {
    let reg = MemoryRegistry::new();
    let parent = tempfile::tempdir().unwrap();
    let data = parent.path().join("UsageQuotaTray");
    std::fs::create_dir_all(data.join("logs")).unwrap();
    std::fs::write(data.join("settings.json"), "{}").unwrap();
    let mut log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(data.join("logs").join("usagequotatray-2026-10-29.log"))
        .unwrap();
    log.write_all(b"line\n").unwrap();

    remove_from_pc(&reg, Some(&data), true).unwrap();
    assert!(!data.exists());
    drop(log);
}

/// A change outside the app's own entries, recording when it was undone.
#[derive(Debug)]
struct FakeChange {
    pending: Option<&'static str>,
    fails: bool,
    undone_with_run_value: RefCell<Option<bool>>,
    registry: Rc<MemoryRegistry>,
    keeps: Option<(PathBuf, String)>,
}

impl FakeChange {
    fn new(registry: &Rc<MemoryRegistry>, pending: Option<&'static str>, fails: bool) -> Self {
        Self {
            pending,
            fails,
            undone_with_run_value: RefCell::new(None),
            registry: registry.clone(),
            keeps: None,
        }
    }

    fn keeping(mut self, file: PathBuf, why: &str) -> Self {
        self.keeps = Some((file, why.to_string()));
        self
    }
}

impl OutsideChange for FakeChange {
    fn pending(&self) -> Option<String> {
        self.pending.map(str::to_string)
    }

    fn undo(&self) -> Result<(), String> {
        let run_value = self.registry.read_string(RUN_KEY, APP_ID).is_some();
        *self.undone_with_run_value.borrow_mut() = Some(run_value);
        if self.fails {
            Err("settings.json is locked".into())
        } else {
            Ok(())
        }
    }

    fn kept_in_data(&self) -> Option<(PathBuf, String)> {
        self.keeps.clone()
    }
}

#[test]
fn deleting_the_data_keeps_a_record_another_copy_still_needs_and_says_why() {
    let reg = Rc::new(MemoryRegistry::new());
    Autostart::new(reg.as_ref(), Path::new(EXE))
        .set(true)
        .unwrap();
    let parent = tempfile::tempdir().unwrap();
    let data = parent.path().join("UsageQuotaTray");
    std::fs::create_dir_all(data.join("logs")).unwrap();
    std::fs::create_dir_all(data.join("badges")).unwrap();
    std::fs::write(data.join("logs").join("a.log"), "line").unwrap();
    std::fs::write(data.join("settings.json"), "{}").unwrap();
    std::fs::write(data.join("statusline.json"), "{\"command\": \"x\"}").unwrap();
    let change = FakeChange::new(&reg, None, false).keeping(
        data.join("statusline.json"),
        "another copy's statusline helper still uses it",
    );

    let error = remove_from_pc_undoing(reg.as_ref(), &[&change], Some(&data), true)
        .expect_err("a partial removal");

    assert!(matches!(error, RemovalError::Partly(_)), "{error:?}");
    let text = error.to_string();
    assert!(text.contains("statusline.json"), "{text}");
    assert!(
        text.contains("another copy's statusline helper still uses it"),
        "{text}"
    );
    let left: Vec<_> = std::fs::read_dir(&data)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(left, ["statusline.json"]);
    assert_eq!(
        std::fs::read_to_string(data.join("statusline.json")).unwrap(),
        "{\"command\": \"x\"}"
    );
    assert_eq!(reg.read_string(RUN_KEY, APP_ID), None);
}

#[test]
fn a_record_outside_the_data_folder_does_not_stop_deleting_it() {
    let reg = Rc::new(MemoryRegistry::new());
    let parent = tempfile::tempdir().unwrap();
    let data = parent.path().join("UsageQuotaTray");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(data.join("settings.json"), "{}").unwrap();
    let change = FakeChange::new(&reg, None, false).keeping(parent.path().join("x.json"), "why");

    remove_from_pc_undoing(reg.as_ref(), &[&change], Some(&data), true).unwrap();
    assert!(!data.exists());
}

#[test]
fn removal_first_undoes_outside_changes_then_deletes_the_entries() {
    let reg = Rc::new(MemoryRegistry::new());
    Autostart::new(reg.as_ref(), Path::new(EXE))
        .set(true)
        .unwrap();
    let change = FakeChange::new(&reg, Some("puts back the statusLine"), false);

    remove_from_pc_undoing(reg.as_ref(), &[&change], None, false).unwrap();

    assert_eq!(
        *change.undone_with_run_value.borrow(),
        Some(true),
        "undone while the Run value was still there"
    );
    assert_eq!(reg.read_string(RUN_KEY, APP_ID), None);
}

#[test]
fn removal_stops_and_keeps_everything_when_undoing_fails() {
    let reg = Rc::new(MemoryRegistry::new());
    Autostart::new(reg.as_ref(), Path::new(EXE))
        .set(true)
        .unwrap();
    let data = tempfile::tempdir().unwrap();
    let change = FakeChange::new(&reg, Some("puts back the statusLine"), true);

    let error = remove_from_pc_undoing(reg.as_ref(), &[&change], Some(data.path()), true)
        .unwrap_err()
        .to_string();

    assert!(error.contains("settings.json is locked"), "{error}");
    assert!(reg.read_string(RUN_KEY, APP_ID).is_some());
    assert!(data.path().exists());
}

#[test]
fn the_removal_question_names_what_is_undone_first() {
    let reg = Rc::new(MemoryRegistry::new());
    let statusline = FakeChange::new(
        &reg,
        Some("put back Claude Code's original statusLine setting"),
        false,
    );
    let nothing = FakeChange::new(&reg, None, false);

    let text = removal_question(&[&statusline, &nothing], Some(r"C:\data\UsageQuotaTray"));
    assert_eq!(
        text,
        "This will first put back Claude Code's original statusLine setting, then delete \
         the Start with Windows entry, the notification registration and the folder \
         C:\\data\\UsageQuotaTray, and quit UsageQuotaTray.\n\nRemove it now?"
    );
    assert_eq!(
        removal_question(&[&nothing], None),
        "This will delete the Start with Windows entry and the notification registration, \
         and quit UsageQuotaTray.\n\nRemove it now?"
    );
}

#[test]
fn removal_works_with_nothing_registered() {
    let reg = MemoryRegistry::new();
    remove_from_pc(&reg, None, true).unwrap();
}

#[test]
fn only_one_instance_holds_a_name_at_a_time() {
    let name = format!("UsageQuotaTray-test-{}", std::process::id());
    let first = SingleInstance::acquire(&name).expect("first instance");
    assert!(SingleInstance::acquire(&name).is_none());
    let other = SingleInstance::acquire(&format!("{name}-other"));
    assert!(other.is_some());
    drop(first);
    assert!(SingleInstance::acquire(&name).is_some());
}

#[test]
fn the_instance_name_is_per_data_folder_and_ignores_case() {
    let a = instance_name(Path::new(r"C:\Users\me\AppData\Local\UsageQuotaTray"));
    let b = instance_name(Path::new(r"c:\users\ME\appdata\local\usagequotatray"));
    let c = instance_name(Path::new(r"C:\Temp\sandbox\UsageQuotaTray"));
    assert_eq!(a, b);
    assert_ne!(a, c);
    assert!(a.starts_with("Local\\UsageQuotaTray-"));
}

#[test]
fn the_running_marker_has_the_name_the_installer_waits_on() {
    assert_eq!(RUNNING_MUTEX, "UsageQuotaTray.Running");
}

#[test]
fn every_running_instance_holds_the_marker_until_it_exits() {
    let name = format!("UsageQuotaTray-test-running-{}", std::process::id());
    let first = RunningMarker::hold(&name).expect("first instance");
    let second = RunningMarker::hold(&name).expect("an instance for another data folder");
    // The installer's check: does a mutex with that name exist?
    assert!(SingleInstance::acquire(&name).is_none());
    drop(first);
    assert!(SingleInstance::acquire(&name).is_none());
    drop(second);
    assert!(SingleInstance::acquire(&name).is_some());
}

/// The Apps entry Inno Setup writes for our AppId, as an installed copy has it.
fn apps_entry(reg: &MemoryRegistry, install_dir: &Path, uninstaller: &Path) {
    reg.write_string(
        UNINSTALL_KEY,
        "InstallLocation",
        &format!("{}\\", install_dir.display()),
    )
    .unwrap();
    reg.write_string(
        UNINSTALL_KEY,
        "UninstallString",
        &format!("\"{}\"", uninstaller.display()),
    )
    .unwrap();
}

#[test]
fn the_apps_entry_key_is_the_one_inno_setup_writes_for_our_app_id() {
    assert_eq!(
        UNINSTALL_KEY,
        r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{9C02A488-5E56-4016-835D-BE4F735473FD}_is1"
    );
}

#[test]
fn an_installed_copy_is_recognised_by_the_apps_entry_for_its_folder() {
    let reg = MemoryRegistry::new();
    let dir = tempfile::tempdir().unwrap();
    let uninstaller = dir.path().join("unins001.exe");
    std::fs::write(&uninstaller, b"").unwrap();
    // A guess by file name alone is not enough.
    std::fs::write(dir.path().join("unins000.exe"), b"").unwrap();
    assert_eq!(installed_uninstaller(&reg, dir.path()), None);

    apps_entry(&reg, dir.path(), &uninstaller);
    assert_eq!(installed_uninstaller(&reg, dir.path()), Some(uninstaller));
}

#[test]
fn the_apps_entry_is_matched_ignoring_case_and_a_trailing_backslash() {
    let reg = MemoryRegistry::new();
    let dir = tempfile::tempdir().unwrap();
    let uninstaller = dir.path().join("unins000.exe");
    std::fs::write(&uninstaller, b"").unwrap();
    let upper = Path::new(&dir.path().display().to_string().to_uppercase()).to_path_buf();
    apps_entry(&reg, &upper, &uninstaller);
    reg.write_string(
        UNINSTALL_KEY,
        "UninstallString",
        &format!("\"{}\" /LOG", uninstaller.display()),
    )
    .unwrap();
    assert_eq!(installed_uninstaller(&reg, dir.path()), Some(uninstaller));
}

#[test]
fn a_copy_in_another_folder_than_the_installed_one_has_no_uninstaller() {
    let reg = MemoryRegistry::new();
    let installed = tempfile::tempdir().unwrap();
    let zip_copy = tempfile::tempdir().unwrap();
    let uninstaller = installed.path().join("unins000.exe");
    std::fs::write(&uninstaller, b"").unwrap();
    std::fs::write(zip_copy.path().join("unins000.exe"), b"").unwrap();
    apps_entry(&reg, installed.path(), &uninstaller);
    assert_eq!(installed_uninstaller(&reg, zip_copy.path()), None);
}

#[test]
fn an_apps_entry_whose_uninstaller_is_gone_has_no_uninstaller() {
    let reg = MemoryRegistry::new();
    let dir = tempfile::tempdir().unwrap();
    apps_entry(&reg, dir.path(), &dir.path().join("unins000.exe"));
    assert_eq!(installed_uninstaller(&reg, dir.path()), None);
    assert_eq!(installed_uninstaller(&reg, &dir.path().join("gone")), None);
}

/// Names no real tray uses, unique to one test.
fn quit_names(test: &str) -> (String, String) {
    let id = format!("{}-{test}", std::process::id());
    (
        format!("UsageQuotaTray-test-quit-{id}"),
        format!("UsageQuotaTray-test-running-{id}"),
    )
}

#[test]
fn the_quit_request_has_the_name_the_installer_uses() {
    assert_eq!(QUIT_EVENT, "UsageQuotaTray.Quit");
}

#[test]
fn a_quit_request_reaches_a_running_loop_which_then_exits() {
    let (event, running) = quit_names("reaches");
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let tray = {
        let (event, running) = (event.clone(), running.clone());
        std::thread::spawn(move || {
            // A stand-in for the tray: holds the marker until asked to quit.
            let marker = RunningMarker::hold(&running).expect("marker");
            let (quit_tx, quit_rx) = std::sync::mpsc::channel();
            let listener = QuitListener::spawn(&event, move || {
                let _ = quit_tx.send(());
            })
            .expect("listener");
            ready_tx.send(()).unwrap();
            let asked = quit_rx.recv_timeout(Duration::from_secs(10)).is_ok();
            drop(listener);
            drop(marker);
            asked
        })
    };
    ready_rx.recv().unwrap();

    let outcome = request_quit(&event, &running, Duration::from_secs(10));

    assert_eq!(outcome, QuitOutcome::Closed);
    assert!(tray.join().unwrap(), "the loop saw the quit request");
}

#[test]
fn a_quit_request_with_no_tray_running_is_done_at_once() {
    let (event, running) = quit_names("none");
    let started = Instant::now();
    assert_eq!(
        request_quit(&event, &running, Duration::from_secs(10)),
        QuitOutcome::NoneRunning
    );
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn a_tray_that_does_not_close_in_time_is_reported_still_running() {
    let (event, running) = quit_names("timeout");
    // Holds the marker but never listens.
    let _marker = RunningMarker::hold(&running).expect("marker");
    let started = Instant::now();

    let outcome = request_quit(&event, &running, Duration::from_millis(300));

    assert_eq!(outcome, QuitOutcome::StillRunning);
    assert!(started.elapsed() >= Duration::from_millis(300));
}

#[test]
fn a_quit_request_is_not_left_behind_for_the_next_tray() {
    let (event, running) = quit_names("reset");
    {
        let _marker = RunningMarker::hold(&running).expect("marker");
        request_quit(&event, &running, Duration::from_millis(100));
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let _listener = QuitListener::spawn(&event, move || {
        let _ = tx.send(());
    })
    .expect("listener");
    assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
}

#[test]
fn stopping_the_listener_does_not_report_a_quit() {
    let (event, _) = quit_names("stop");
    let (tx, rx) = std::sync::mpsc::channel();
    let listener = QuitListener::spawn(&event, move || {
        let _ = tx.send(());
    })
    .expect("listener");
    drop(listener);
    assert!(rx.try_recv().is_err());
}
