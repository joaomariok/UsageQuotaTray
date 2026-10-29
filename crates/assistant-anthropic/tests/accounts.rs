//! Engine seam, multiple Accounts: the real Engine and Claude Code Assistant over
//! temporary folders standing in for Claude config folders, a recording fake transport
//! and a fake clock. Nothing here reads the real ~/.claude or reaches the network.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tempfile::TempDir;
use uqt_assistant_anthropic::{ClaudeCodeAssistant, ConfigFolder, ID, keys, settings_keys};
use uqt_core::contract::{Assistant, FieldKind};
use uqt_core::engine::{Engine, RefreshReason};
use uqt_core::http::{HttpError, HttpResponse};
use uqt_core::settings::{SettingValue, Settings, SettingsEdit};
use uqt_core::testing::RecordingTransport;
use uqt_core::time::{FakeClock, Timestamp};

const HOUR: i64 = 3600;

fn start() -> Timestamp {
    Timestamp::from_utc(2026, 10, 29, 12, 0, 0)
}

fn body(session: f64) -> Result<HttpResponse, HttpError> {
    Ok(HttpResponse {
        status: 200,
        headers: Vec::new(),
        body: format!(
            r#"{{"five_hour": {{"utilization": {session}, "resets_at": "2026-10-29T15:00:00Z"}}}}"#
        )
        .into_bytes(),
    })
}

/// A config folder `<root>/<name>` with a valid login, its account file inside it.
fn folder(root: &Path, name: &str, account: &str, org: &str, email: &str) -> PathBuf {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let expires = (start().0 + 8 * HOUR) * 1000;
    std::fs::write(
        dir.join(".credentials.json"),
        format!(
            r#"{{"claudeAiOauth": {{"accessToken": "sk-ant-oat01-{name}", "expiresAt": {expires},
                "subscriptionType": "pro"}}}}"#
        ),
    )
    .unwrap();
    write_identity(&dir.join(".claude.json"), account, org, email);
    dir
}

fn write_identity(path: &Path, account: &str, org: &str, email: &str) {
    std::fs::write(
        path,
        format!(
            r#"{{"oauthAccount": {{"accountUuid": "{account}", "organizationUuid": "{org}",
                "emailAddress": "{email}"}}}}"#
        ),
    )
    .unwrap();
}

fn extra_folders(settings: &mut Settings, text: &str) {
    settings.apply(SettingsEdit::ModuleValue {
        assistant: ID.into(),
        key: settings_keys::EXTRA_FOLDERS.into(),
        value: SettingValue::Text(text.into()),
    });
}

struct Setup {
    _temp: TempDir,
    root: PathBuf,
    clock: FakeClock,
    transport: RecordingTransport,
}

impl Setup {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        Self {
            _temp: temp,
            root,
            clock: FakeClock::new(start()),
            transport: RecordingTransport::new(),
        }
    }

    fn engine(&self, default: &Path, settings: &Settings) -> Engine {
        Engine::with_settings(
            vec![Box::new(ClaudeCodeAssistant::new(vec![ConfigFolder::new(
                default.to_path_buf(),
            )]))],
            Arc::new(self.clock.clone()),
            Arc::new(self.transport.clone()),
            settings,
        )
    }
}

fn accounts(engine: &Engine) -> Vec<(String, String)> {
    engine.state().assistants[0]
        .accounts
        .iter()
        .map(|a| (a.id.clone(), a.display_name.clone()))
        .collect()
}

fn pair(id: &str, name: &str) -> (String, String) {
    (id.into(), name.into())
}

#[test]
fn extra_config_folders_from_settings_add_their_accounts() {
    let s = Setup::new();
    let default = folder(&s.root, "default", "acc-1", "org-1", "me@example.com");
    let work = folder(&s.root, "work", "acc-2", "org-2", "me@work.example");
    let other = folder(&s.root, "other", "acc-3", "org-3", "other@example.com");
    let mut settings = Settings::default();
    extra_folders(
        &mut settings,
        &format!("{} ; {};", work.display(), other.display()),
    );
    let engine = s.engine(&default, &settings);
    assert_eq!(
        accounts(&engine),
        vec![
            pair("acc-1/org-1", "me@example.com"),
            pair("acc-2/org-2", "me@work.example"),
            pair("acc-3/org-3", "other@example.com"),
        ]
    );
}

#[test]
fn a_folder_added_while_running_appears_without_a_restart() {
    let s = Setup::new();
    let default = folder(&s.root, "default", "acc-1", "org-1", "me@example.com");
    let work = folder(&s.root, "work", "acc-2", "org-2", "me@work.example");
    let mut settings = Settings::default();
    let mut engine = s.engine(&default, &settings);
    assert_eq!(accounts(&engine).len(), 1);

    extra_folders(&mut settings, &work.display().to_string());
    engine.apply_settings(&settings);
    assert_eq!(
        accounts(&engine),
        vec![
            pair("acc-1/org-1", "me@example.com"),
            pair("acc-2/org-2", "me@work.example"),
        ]
    );
    let catalog = engine.catalog();
    assert_eq!(catalog.assistants[0].accounts.len(), 2);

    // Both are fetched, each with its own login.
    s.transport.push_response(body(10.0));
    s.transport.push_response(body(20.0));
    engine.tick();
    let requests = s.transport.requests();
    assert_eq!(requests.len(), 2);
    let state = engine.state();
    let used: Vec<f64> = state.assistants[0]
        .accounts
        .iter()
        .map(|a| a.meter(keys::SESSION).unwrap().used)
        .collect();
    assert_eq!(used, vec![10.0, 20.0]);
}

#[test]
fn the_same_login_in_two_folders_is_one_account() {
    let s = Setup::new();
    let default = folder(&s.root, "default", "acc-1", "org-1", "me@example.com");
    let copy = folder(&s.root, "copy", "acc-1", "org-1", "me@example.com");
    let other_org = folder(&s.root, "team", "acc-1", "org-9", "me@example.com");
    let mut settings = Settings::default();
    extra_folders(
        &mut settings,
        &format!("{};{}", copy.display(), other_org.display()),
    );
    let mut engine = s.engine(&default, &settings);
    assert_eq!(
        accounts(&engine),
        vec![
            pair("acc-1/org-1", "me@example.com"),
            pair("acc-1/org-9", "me@example.com"),
        ]
    );
    s.transport.push_response(body(10.0));
    s.transport.push_response(body(20.0));
    engine.tick();
    assert_eq!(s.transport.requests().len(), 2, "one fetch per Account");
}

#[test]
fn a_new_identity_in_the_same_folder_is_a_new_account() {
    let s = Setup::new();
    let default = folder(&s.root, "default", "acc-1", "org-1", "me@example.com");
    let mut engine = s.engine(&default, &Settings::default());
    s.transport.push_response(body(70.0));
    engine.tick();
    assert_eq!(
        accounts(&engine),
        vec![pair("acc-1/org-1", "me@example.com")]
    );

    // Logged out and back in as someone else.
    write_identity(
        &default.join(".claude.json"),
        "acc-2",
        "org-2",
        "else@example.com",
    );
    // A scheduled fetch before anything rediscovers must not put the new login's
    // numbers under the old Account.
    s.clock.advance(5 * 60);
    s.transport.push_response(body(5.0));
    engine.tick();
    let state = engine.state();
    let old = &state.assistants[0].accounts[0];
    assert_eq!(old.meter(keys::SESSION).unwrap().used, 70.0);

    engine.request_refresh(RefreshReason::PopupOpen);
    s.transport.push_response(body(5.0));
    engine.tick();
    assert_eq!(
        accounts(&engine),
        vec![pair("acc-2/org-2", "else@example.com")]
    );
    let state = engine.state();
    assert_eq!(
        state.assistants[0].accounts[0]
            .meter(keys::SESSION)
            .unwrap()
            .used,
        5.0
    );
}

#[test]
fn a_hand_added_dot_claude_folder_finds_the_account_file_next_to_it() {
    let s = Setup::new();
    let default = folder(&s.root, "default", "acc-1", "org-1", "me@example.com");
    // The default layout of another home: <home>\.claude plus <home>\.claude.json.
    let home = s.root.join("home");
    let dot_claude = folder(&home, ".claude", "x", "y", "ignored@example.com");
    std::fs::remove_file(dot_claude.join(".claude.json")).unwrap();
    write_identity(
        &home.join(".claude.json"),
        "acc-5",
        "org-5",
        "home@example.com",
    );
    let mut settings = Settings::default();
    extra_folders(&mut settings, &dot_claude.display().to_string());
    let engine = s.engine(&default, &settings);
    assert_eq!(
        accounts(&engine)[1],
        pair("acc-5/org-5", "home@example.com")
    );
}

#[test]
fn wsl_paths_are_accepted_only_by_hand_with_a_warning() {
    let mut module = ClaudeCodeAssistant::new(Vec::new());
    let fields = module.settings_fields();
    let field = fields
        .iter()
        .find(|f| f.key == settings_keys::EXTRA_FOLDERS)
        .expect("extra folders field");
    assert!(matches!(field.kind, FieldKind::Text { .. }));
    let help = field.help.as_deref().unwrap_or_default();
    assert!(help.contains("WSL"), "{help}");
    assert!(help.contains("starts"), "{help}");

    // Nothing is found unless added; a hand-added WSL path is taken as given.
    assert_eq!(module.discover_accounts().len(), 0);
    let wsl = r"\\wsl.localhost\Ubuntu\home\me\.claude";
    let mut values = uqt_core::settings::ModuleValues::new();
    values.insert(
        settings_keys::EXTRA_FOLDERS.into(),
        SettingValue::Text(wsl.into()),
    );
    module.configure(&values);
    assert_eq!(module.folders().last().unwrap().dir, PathBuf::from(wsl));
}
