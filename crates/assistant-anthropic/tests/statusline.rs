//! Engine seam for the statusline push Source: the real Engine and Claude Code
//! Assistant over a temporary config folder, a recording fake transport and a fake
//! clock. The helper's input (the JSON Claude Code pipes to a statusLine command) is
//! turned into push messages exactly as the helper does and fed to the engine.
//!
//! Input shape: https://code.claude.com/docs/en/statusline ("Available data":
//! `rate_limits.five_hour` / `seven_day` with `used_percentage` 0-100 and `resets_at`
//! in Unix epoch seconds; each window may be absent).

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tempfile::TempDir;
use uqt_assistant_anthropic::{ClaudeCodeAssistant, ConfigFolder, ID, keys, sources, statusline};
use uqt_core::contract::FetchError;
use uqt_core::engine::{Engine, Freshness, MeterState};
use uqt_core::http::{HttpError, HttpResponse};
use uqt_core::model::{Exactness, Unit};
use uqt_core::testing::RecordingTransport;
use uqt_core::time::{FakeClock, Timestamp};

const MIN: i64 = 60;
const HOUR: i64 = 3600;

fn start() -> Timestamp {
    Timestamp::from_utc(2026, 10, 29, 12, 0, 0)
}

/// 2026-10-29T14:50:00Z and 2026-11-04T02:00:00Z.
const SESSION_RESET: i64 = 1_793_285_400;
const WEEK_RESET: i64 = 1_793_757_600;

/// A statusline input in the documented shape, with the other fields Claude Code
/// sends (which must never reach the push channel).
fn input(five_hour: f64, seven_day: f64) -> String {
    format!(
        r#"{{
  "hook_event_name": "Status",
  "session_id": "abc123",
  "transcript_path": "C:/Users/me/.claude/projects/x/abc123.jsonl",
  "cwd": "D:/secret/project",
  "model": {{"id": "claude-opus-4-1", "display_name": "Opus"}},
  "workspace": {{"current_dir": "D:/secret/project", "project_dir": "D:/secret/project"}},
  "version": "2.1.300",
  "cost": {{"total_cost_usd": 0.01234}},
  "context_window": {{"used_percentage": 8}},
  "rate_limits": {{
    "five_hour": {{"used_percentage": {five_hour}, "resets_at": {SESSION_RESET}}},
    "seven_day": {{"used_percentage": {seven_day}, "resets_at": {WEEK_RESET}}}
  }}
}}"#
    )
}

const USAGE_BODY: &str = r#"{
  "five_hour": {"utilization": 50.0, "resets_at": "2026-10-29T14:50:00+00:00"},
  "seven_day": {"utilization": 10.0, "resets_at": "2026-11-04T02:00:00+00:00"}
}"#;

fn ok(body: &str) -> Result<HttpResponse, HttpError> {
    Ok(HttpResponse {
        status: 200,
        headers: Vec::new(),
        body: body.as_bytes().to_vec(),
    })
}

struct Harness {
    _temp: TempDir,
    folder: ConfigFolder,
    clock: FakeClock,
    transport: RecordingTransport,
    engine: Engine,
}

impl Harness {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join(".claude");
        std::fs::create_dir(&dir).unwrap();
        write_login(&dir);
        let folder = ConfigFolder::new(dir);
        let clock = FakeClock::new(start());
        let transport = RecordingTransport::new();
        let engine = Engine::new(
            vec![Box::new(ClaudeCodeAssistant::new(vec![folder.clone()]))],
            Arc::new(clock.clone()),
            Arc::new(transport.clone()),
        );
        Self {
            _temp: temp,
            folder,
            clock,
            transport,
            engine,
        }
    }

    /// What the helper does with one statusline input.
    fn push(&mut self, input: &str) -> bool {
        match statusline::push_message(input.as_bytes(), &self.folder) {
            Some(m) => self.engine.push(&m),
            None => false,
        }
    }

    fn meter(&self, key: &str) -> MeterState {
        self.engine.state().assistants[0].accounts[0]
            .meter(key)
            .unwrap_or_else(|| panic!("meter {key} missing"))
            .clone()
    }
}

fn write_login(dir: &Path) {
    let expires = (start().0 + 8 * HOUR) * 1000;
    std::fs::write(
        dir.join(".credentials.json"),
        format!(
            r#"{{"claudeAiOauth": {{"accessToken": "sk-ant-oat01-test", "expiresAt": {expires}}}}}"#
        ),
    )
    .unwrap();
}

#[test]
fn the_statusline_rate_limits_become_exact_session_and_weekly_readings() {
    let mut h = Harness::new();
    assert!(h.push(&input(23.5, 41.2)));

    let session = h.meter(keys::SESSION);
    assert_eq!(session.used, 23.5);
    assert_eq!(session.limit, Some(100.0));
    assert_eq!(session.unit, Unit::Percent);
    assert_eq!(session.exactness, Exactness::Exact);
    assert_eq!(session.source_name, "Claude Code statusline");
    let w = session.window.expect("session window");
    assert_eq!(w.end, Timestamp(SESSION_RESET));
    assert_eq!(w.length, Some(Duration::from_secs(5 * 3600)));

    let weekly = h.meter(keys::WEEKLY);
    assert_eq!(weekly.used, 41.2);
    let w = weekly.window.expect("weekly window");
    assert_eq!(w.end, Timestamp(WEEK_RESET));
    assert_eq!(w.length, Some(Duration::from_secs(7 * 86_400)));
    assert!(
        h.transport.requests().is_empty(),
        "pushing costs no request"
    );
}

#[test]
fn the_push_message_carries_only_the_rate_limits() {
    let h = Harness::new();
    let m = statusline::push_message(input(1.0, 2.0).as_bytes(), &h.folder).unwrap();
    assert_eq!(m.assistant, ID);
    assert_eq!(m.source, sources::STATUSLINE);
    let wire = String::from_utf8(m.encode()).unwrap();
    for private in ["secret", "abc123", "transcript", "cost", "Opus"] {
        assert!(!wire.contains(private), "{private} leaked: {wire}");
    }
}

#[test]
fn a_missing_window_is_skipped_and_no_rate_limits_sends_nothing() {
    let mut h = Harness::new();
    let only_week = format!(
        r#"{{"rate_limits": {{"seven_day": {{"used_percentage": 12, "resets_at": {WEEK_RESET}}}}}}}"#
    );
    assert!(h.push(&only_week));
    let state = h.engine.state();
    let account = &state.assistants[0].accounts[0];
    assert!(account.meter(keys::SESSION).is_none());
    assert_eq!(account.meter(keys::WEEKLY).map(|m| m.used), Some(12.0));

    // API-key users and the first moments of a session have no rate_limits at all.
    let folder = h.folder.clone();
    for nothing in [
        r#"{"model": {"id": "x"}}"#,
        r#"{"rate_limits": {}}"#,
        "not json",
        "",
    ] {
        assert!(
            statusline::push_message(nothing.as_bytes(), &folder).is_none(),
            "{nothing}"
        );
    }
}

#[test]
fn a_push_from_another_config_folder_is_not_this_accounts() {
    let mut h = Harness::new();
    let elsewhere = ConfigFolder::new(h.folder.dir.with_file_name("other-claude"));
    let m = statusline::push_message(input(5.0, 5.0).as_bytes(), &elsewhere).unwrap();
    assert!(!h.engine.push(&m));
}

#[test]
fn the_same_folder_written_differently_still_matches() {
    let mut h = Harness::new();
    let shown = h.folder.dir.display().to_string();
    let written = ConfigFolder::new(shown.replace('\\', "/").to_uppercase().into());
    let m = statusline::push_message(input(5.0, 5.0).as_bytes(), &written).unwrap();
    assert!(h.engine.push(&m));
}

#[test]
fn the_newest_fresh_exact_reading_wins_between_poll_and_push() {
    let mut h = Harness::new();
    h.transport.push_response(ok(USAGE_BODY));
    h.engine.tick();
    assert_eq!(h.meter(keys::SESSION).used, 50.0);
    assert_eq!(h.meter(keys::SESSION).source_name, "Claude usage");

    h.clock.advance(MIN);
    assert!(h.push(&input(52.0, 11.0)));
    assert_eq!(h.meter(keys::SESSION).used, 52.0, "the push is newer");
    assert_eq!(h.meter(keys::SESSION).source_name, "Claude Code statusline");

    // The next poll is newer again.
    h.transport.push_response(ok(
        r#"{"five_hour": {"utilization": 55.0, "resets_at": "2026-10-29T14:50:00+00:00"}}"#,
    ));
    h.clock.advance(4 * MIN);
    h.engine.tick();
    assert_eq!(h.meter(keys::SESSION).used, 55.0);
    assert_eq!(h.meter(keys::SESSION).source_name, "Claude usage");
    // The poll had no weekly value; the push's is still the newest fresh one.
    assert_eq!(h.meter(keys::WEEKLY).used, 11.0);
}

#[test]
fn pushes_keep_the_meters_fresh_while_polling_fails() {
    let mut h = Harness::new();
    h.transport.push_response(ok(USAGE_BODY));
    h.engine.tick();
    for _ in 0..6 {
        h.transport
            .push_response(Err(HttpError::Transport("offline".into())));
    }
    h.clock.advance(30 * MIN);
    h.engine.tick();
    assert_eq!(h.meter(keys::SESSION).freshness, Freshness::Stale);
    assert!(h.push(&input(60.0, 12.0)));
    let session = h.meter(keys::SESSION);
    assert_eq!(session.freshness, Freshness::Fresh);
    assert_eq!(session.used, 60.0);
    let problem = h.engine.state().assistants[0].accounts[0].problem.clone();
    assert_eq!(problem.map(|p| p.error), Some(FetchError::Offline));
}

#[test]
fn a_pushed_reading_goes_stale_when_claude_code_stops_sending() {
    let mut h = Harness::new();
    // No login problem for polling either: keep it failing so only the push counts.
    for _ in 0..20 {
        h.transport
            .push_response(Err(HttpError::Transport("offline".into())));
    }
    assert!(h.push(&input(10.0, 10.0)));
    h.clock.advance(14 * MIN);
    assert_eq!(h.meter(keys::SESSION).freshness, Freshness::Fresh);
    h.clock.advance(2 * MIN);
    assert_eq!(h.meter(keys::SESSION).freshness, Freshness::Stale);
}

#[test]
fn the_statusline_source_is_never_polled() {
    let mut h = Harness::new();
    h.transport.push_response(ok(USAGE_BODY));
    h.engine.tick();
    assert_eq!(h.transport.requests().len(), 1, "only the usage endpoint");
}

#[test]
fn the_helpers_own_line_shows_both_windows() {
    assert_eq!(
        statusline::summary(input(23.5, 41.2).as_bytes()),
        "5h 24% \u{b7} 7d 41%"
    );
    assert_eq!(statusline::summary(br#"{"model": {}}"#), "");
}

fn write_identity(dir: &Path, account: &str) {
    std::fs::write(
        dir.join(".claude.json"),
        format!(
            r#"{{"oauthAccount": {{"accountUuid": "{account}", "organizationUuid": "org",
                "emailAddress": "{account}@example.com"}}}}"#
        ),
    )
    .unwrap();
}

/// An engine over these config folders, each with a login of the given account.
fn engine_over(folders: &[(&Path, &str)]) -> (Engine, Vec<ConfigFolder>) {
    let found: Vec<ConfigFolder> = folders
        .iter()
        .map(|(dir, account)| {
            std::fs::create_dir_all(dir).unwrap();
            write_login(dir);
            write_identity(dir, account);
            ConfigFolder::new(dir.to_path_buf())
        })
        .collect();
    let engine = Engine::new(
        vec![Box::new(ClaudeCodeAssistant::new(found.clone()))],
        Arc::new(FakeClock::new(start())),
        Arc::new(RecordingTransport::new()),
    );
    (engine, found)
}

#[test]
fn a_push_after_the_folder_signed_in_to_another_account_is_refused() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join(".claude");
    let (mut engine, folders) = engine_over(&[(&dir, "alice")]);
    write_identity(&dir, "bob");
    let m = statusline::push_message(input(5.0, 5.0).as_bytes(), &folders[0]).unwrap();
    assert!(
        !engine.push(&m),
        "bob's numbers must not be filed under alice's Account"
    );
    let state = engine.state();
    assert!(
        state.assistants[0].accounts[0]
            .meter(keys::SESSION)
            .is_none()
    );
}

#[test]
fn a_push_from_any_folder_of_the_same_login_is_that_accounts() {
    let temp = tempfile::tempdir().unwrap();
    let first = temp.path().join("first");
    let second = temp.path().join("second");
    let (mut engine, folders) = engine_over(&[(&first, "alice"), (&second, "alice")]);
    assert_eq!(
        engine.state().assistants[0].accounts.len(),
        1,
        "one Account"
    );
    let m = statusline::push_message(input(7.0, 8.0).as_bytes(), &folders[1]).unwrap();
    assert!(engine.push(&m));
    let state = engine.state();
    let account = &state.assistants[0].accounts[0];
    assert_eq!(account.meter(keys::SESSION).map(|m| m.used), Some(7.0));
}
