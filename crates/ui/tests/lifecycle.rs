//! The app's Windows lifecycle over a registry seam: Start with Windows, the
//! notification identity, removal from the PC, and one instance per session. Tests use
//! the in-memory registry and temporary folders; nothing touches the real HKCU.

use std::cell::RefCell;
use std::io::Write;
use std::path::Path;
use std::rc::Rc;

use uqt_ui::lifecycle::{
    APP_ID, AUMID_KEY, Autostart, MemoryRegistry, OutsideChange, RUN_KEY, Registry, SingleInstance,
    instance_name, register_notifications, removal_question, remove_from_pc,
    remove_from_pc_undoing,
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
}

impl FakeChange {
    fn new(registry: &Rc<MemoryRegistry>, pending: Option<&'static str>, fails: bool) -> Self {
        Self {
            pending,
            fails,
            undone_with_run_value: RefCell::new(None),
            registry: registry.clone(),
        }
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
