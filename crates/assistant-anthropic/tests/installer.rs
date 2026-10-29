//! The settings-file installer seam: installing and uninstalling the statusline helper
//! on temporary files standing in for Claude Code's settings.json. Nothing here
//! touches the real ~/.claude.

use std::path::PathBuf;

use serde_json::{Value, json};
use tempfile::TempDir;
use uqt_assistant_anthropic::statusline::chain::chained_command;
use uqt_assistant_anthropic::statusline::installer::{
    InstallError, Installer, Status, Uninstalled,
};
use uqt_core::time::Timestamp;

const ORIGINAL: &str = r#"{
  "model": "opus",
  "permissions": {
    "allow": ["Bash(git status)"],
    "deny": []
  },
  "env": {"FOO": "bar"},
  "statusLine": {
    "type": "command",
    "command": "~/.claude/statusline.sh",
    "padding": 2
  },
  "alwaysThinkingEnabled": true
}
"#;

struct Files {
    _temp: TempDir,
    installer: Installer,
}

impl Files {
    fn new(settings: Option<&str>) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let claude = temp.path().join(".claude");
        std::fs::create_dir(&claude).unwrap();
        let data = temp.path().join("UsageQuotaTray");
        let settings_file = claude.join("settings.json");
        if let Some(text) = settings {
            std::fs::write(&settings_file, text).unwrap();
        }
        let installer = Installer {
            settings_file,
            state_file: data.join("statusline.json"),
            helper: PathBuf::from(r"C:\Apps\UsageQuotaTray\uqt-statusline.exe"),
        };
        Self {
            _temp: temp,
            installer,
        }
    }

    fn settings_text(&self) -> String {
        std::fs::read_to_string(&self.installer.settings_file).unwrap()
    }

    fn settings(&self) -> Value {
        serde_json::from_str(&self.settings_text()).unwrap()
    }
}

fn now() -> Timestamp {
    Timestamp::from_utc(2026, 10, 29, 12, 0, 0)
}

fn without_status_line(mut v: Value) -> Value {
    v.as_object_mut().unwrap().remove("statusLine");
    v
}

const HELPER_COMMAND: &str = "C:/Apps/UsageQuotaTray/uqt-statusline.exe";

#[test]
fn installing_points_status_line_at_the_helper_and_keeps_every_other_key() {
    let f = Files::new(Some(ORIGINAL));
    assert_eq!(f.installer.status(), Status::NotInstalled);
    let done = f.installer.install(now()).expect("installs");

    let after = f.settings();
    let before: Value = serde_json::from_str(ORIGINAL).unwrap();
    assert_eq!(
        without_status_line(after.clone()),
        without_status_line(before)
    );
    assert_eq!(
        after["statusLine"],
        json!({"type": "command", "command": HELPER_COMMAND, "padding": 2}),
        "the user's padding stays"
    );
    assert_eq!(done.chained.as_deref(), Some("~/.claude/statusline.sh"));
    assert_eq!(
        f.installer.status(),
        Status::Installed {
            chained: Some("~/.claude/statusline.sh".into())
        }
    );
}

#[test]
fn installing_keeps_the_key_order() {
    let f = Files::new(Some(ORIGINAL));
    f.installer.install(now()).unwrap();
    let keys: Vec<String> = f.settings().as_object().unwrap().keys().cloned().collect();
    assert_eq!(
        keys,
        [
            "model",
            "permissions",
            "env",
            "statusLine",
            "alwaysThinkingEnabled"
        ]
    );
}

#[test]
fn installing_backs_up_the_settings_file_first() {
    let f = Files::new(Some(ORIGINAL));
    let done = f.installer.install(now()).unwrap();
    let backup = done.backup.expect("a backup");
    assert_eq!(backup.parent(), f.installer.settings_file.parent());
    assert_eq!(std::fs::read_to_string(&backup).unwrap(), ORIGINAL);
}

#[test]
fn the_helper_chains_the_previous_command() {
    let f = Files::new(Some(ORIGINAL));
    assert_eq!(chained_command(&f.installer.state_file), None);
    f.installer.install(now()).unwrap();
    assert_eq!(
        chained_command(&f.installer.state_file).as_deref(),
        Some("~/.claude/statusline.sh")
    );
    f.installer.uninstall().unwrap();
    assert_eq!(chained_command(&f.installer.state_file), None);
}

#[test]
fn uninstalling_right_after_installing_restores_the_original_bytes() {
    let f = Files::new(Some(ORIGINAL));
    f.installer.install(now()).unwrap();
    assert_eq!(f.installer.uninstall().unwrap(), Uninstalled::Restored);
    assert_eq!(f.settings_text(), ORIGINAL);
    assert_eq!(f.installer.status(), Status::NotInstalled);
}

#[test]
fn uninstalling_after_other_changes_restores_only_the_status_line() {
    let f = Files::new(Some(ORIGINAL));
    f.installer.install(now()).unwrap();
    // Claude Code (or the user) changes another setting while the helper is installed.
    let mut changed = f.settings();
    changed["model"] = json!("sonnet");
    std::fs::write(
        &f.installer.settings_file,
        serde_json::to_string_pretty(&changed).unwrap(),
    )
    .unwrap();

    assert_eq!(f.installer.uninstall().unwrap(), Uninstalled::Restored);
    let after = f.settings();
    assert_eq!(after["model"], "sonnet", "the later change stays");
    let before: Value = serde_json::from_str(ORIGINAL).unwrap();
    assert_eq!(after["statusLine"], before["statusLine"]);
}

#[test]
fn uninstalling_leaves_a_status_line_someone_else_set_since() {
    let f = Files::new(Some(ORIGINAL));
    f.installer.install(now()).unwrap();
    let mut changed = f.settings();
    changed["statusLine"] = json!({"type": "command", "command": "my-new-line"});
    let text = serde_json::to_string_pretty(&changed).unwrap();
    std::fs::write(&f.installer.settings_file, &text).unwrap();

    assert_eq!(f.installer.status(), Status::Replaced);
    assert_eq!(f.installer.uninstall().unwrap(), Uninstalled::LeftAlone);
    assert_eq!(f.settings_text(), text);
    assert_eq!(f.installer.status(), Status::NotInstalled);
}

#[test]
fn without_a_settings_file_installing_creates_one_and_uninstalling_removes_it() {
    let f = Files::new(None);
    let done = f.installer.install(now()).unwrap();
    assert_eq!(done.backup, None);
    assert_eq!(done.chained, None);
    assert_eq!(
        f.settings(),
        json!({"statusLine": {"type": "command", "command": HELPER_COMMAND}})
    );
    f.installer.uninstall().unwrap();
    assert!(!f.installer.settings_file.exists());
}

#[test]
fn without_a_previous_status_line_uninstalling_removes_the_key() {
    let original = "{\n  \"model\": \"opus\"\n}";
    let f = Files::new(Some(original));
    f.installer.install(now()).unwrap();
    let mut changed = f.settings();
    changed["env"] = json!({});
    std::fs::write(&f.installer.settings_file, changed.to_string()).unwrap();
    f.installer.uninstall().unwrap();
    assert_eq!(f.settings(), json!({"model": "opus", "env": {}}));
}

#[test]
fn installing_twice_changes_nothing_the_second_time() {
    let f = Files::new(Some(ORIGINAL));
    f.installer.install(now()).unwrap();
    let once = f.settings_text();
    let again = f.installer.install(Timestamp(now().0 + 60)).unwrap();
    assert_eq!(f.settings_text(), once);
    assert_eq!(again.chained.as_deref(), Some("~/.claude/statusline.sh"));
    // Uninstalling still gets back to the true original.
    f.installer.uninstall().unwrap();
    assert_eq!(f.settings_text(), ORIGINAL);
}

#[test]
fn a_settings_file_that_is_not_json_is_never_touched() {
    let broken = "{ \"model\": \"opus\", oops";
    let f = Files::new(Some(broken));
    assert!(matches!(
        f.installer.install(now()),
        Err(InstallError::NotJson(_))
    ));
    assert_eq!(f.settings_text(), broken);
    assert!(!f.installer.state_file.exists());
    let dir = f.installer.settings_file.parent().unwrap();
    assert_eq!(
        std::fs::read_dir(dir).unwrap().count(),
        1,
        "no backup either"
    );
}

#[test]
fn a_helper_path_the_shell_would_split_is_refused() {
    let mut f = Files::new(Some(ORIGINAL));
    f.installer.helper = PathBuf::from(r"C:\Program Files\UsageQuotaTray\uqt-statusline.exe");
    assert!(matches!(
        f.installer.install(now()),
        Err(InstallError::UnsafeHelperPath(_))
    ));
    assert_eq!(f.settings_text(), ORIGINAL);
}

#[test]
fn a_previous_command_that_is_the_helper_itself_is_not_chained() {
    let mine = format!(r#"{{"statusLine": {{"type": "command", "command": "{HELPER_COMMAND}"}}}}"#);
    let f = Files::new(Some(&mine));
    // Installed by an earlier copy whose record was lost: shown as installed.
    assert_eq!(f.installer.status(), Status::Installed { chained: None });
    let done = f.installer.install(now()).unwrap();
    assert_eq!(done.chained, None);
    f.installer.uninstall().unwrap();
    assert_eq!(f.settings(), json!({}));
}

#[test]
fn helper_command_uses_forward_slashes() {
    let f = Files::new(None);
    assert_eq!(f.installer.helper_command().unwrap(), HELPER_COMMAND);
}

mod from_settings {
    //! The same installer driven the way the settings window drives it: through the
    //! module's Action field and the engine.

    use std::sync::Arc;

    use uqt_assistant_anthropic::{ClaudeCodeAssistant, ConfigFolder, ID, STATUSLINE_FIELD};
    use uqt_core::contract::FieldKind;
    use uqt_core::engine::Engine;
    use uqt_core::testing::RecordingTransport;
    use uqt_core::time::FakeClock;

    use super::{Files, ORIGINAL, now};

    fn engine_with(f: &Files, installer: bool) -> Engine {
        let folder = ConfigFolder::new(f.installer.settings_file.parent().unwrap().into());
        let mut module = ClaudeCodeAssistant::new(vec![folder]);
        if installer {
            module = module.with_statusline(f.installer.clone());
        }
        Engine::new(
            vec![Box::new(module)],
            Arc::new(FakeClock::new(now())),
            Arc::new(RecordingTransport::new()),
        )
    }

    /// Button, help and confirmation of the statusline field.
    fn field(engine: &Engine) -> (String, String, Option<String>) {
        let a = &engine.catalog().assistants[0];
        let f = a
            .fields
            .iter()
            .find(|f| f.key == STATUSLINE_FIELD)
            .expect("statusline field");
        match &f.kind {
            FieldKind::Action { button, confirm } => (
                button.clone(),
                f.help.clone().unwrap_or_default(),
                confirm.clone(),
            ),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn install_asks_first_then_uninstall_restores_the_file() {
        let f = Files::new(Some(ORIGINAL));
        let mut engine = engine_with(&f, true);

        let (button, _, confirm) = field(&engine);
        assert_eq!(button, "Install");
        let confirm = confirm.expect("installing asks first");
        let path = f.installer.settings_file.display().to_string();
        assert!(confirm.contains(&path), "{confirm}");
        assert!(confirm.contains("backup"), "{confirm}");
        assert!(confirm.contains("~/.claude/statusline.sh"), "{confirm}");
        assert_eq!(f.settings_text(), ORIGINAL, "looking changes nothing");

        engine.run_action(ID, STATUSLINE_FIELD);
        let outcome = engine.take_outcomes().remove(0);
        assert!(outcome.result.is_ok(), "{outcome:?}");
        assert_ne!(f.settings_text(), ORIGINAL);
        let (button, help, confirm) = field(&engine);
        assert_eq!(button, "Uninstall");
        assert!(help.contains("~/.claude/statusline.sh"), "{help}");
        assert!(confirm.is_some(), "uninstalling asks too");

        engine.run_action(ID, STATUSLINE_FIELD);
        assert!(engine.take_outcomes()[0].result.is_ok());
        assert_eq!(f.settings_text(), ORIGINAL);
        assert_eq!(field(&engine).0, "Install");
    }

    #[test]
    fn a_failed_install_is_reported_and_changes_nothing() {
        let f = Files::new(Some("{ broken"));
        let mut engine = engine_with(&f, true);
        engine.run_action(ID, STATUSLINE_FIELD);
        let outcome = engine.take_outcomes().remove(0);
        let error = outcome.result.unwrap_err();
        assert!(error.contains("not valid JSON"), "{error}");
        assert_eq!(f.settings_text(), "{ broken");
    }

    #[test]
    fn without_an_installer_there_is_no_statusline_field() {
        let f = Files::new(None);
        let engine = engine_with(&f, false);
        let fields = &engine.catalog().assistants[0].fields;
        assert!(
            fields
                .iter()
                .all(|f| f.key != STATUSLINE_FIELD && !matches!(f.kind, FieldKind::Action { .. })),
            "{fields:?}"
        );
    }
}
