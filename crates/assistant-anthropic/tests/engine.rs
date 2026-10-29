//! Engine seam: the real Engine running the real Claude Code Assistant against a
//! temporary folder standing in for a Claude config folder, a recording fake transport
//! serving hand-written responses in the shape recorded in Phase 0, and a fake clock.
//! Nothing here reads the real ~/.claude or reaches the network.

use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use tempfile::TempDir;
use uqt_assistant_anthropic::{
    ClaudeCodeAssistant, ConfigFolder, OAUTH_BETA, USAGE_PAGE, USAGE_URL, keys,
};
use uqt_core::contract::FetchError;
use uqt_core::engine::{Engine, Freshness, MeterState, Presence, SourceProblem};
use uqt_core::http::{HttpError, HttpResponse};
use uqt_core::model::{Breakdown, BreakdownPart, Exactness, Unit};
use uqt_core::testing::RecordingTransport;
use uqt_core::time::{FakeClock, Timestamp};

const MIN: i64 = 60;
const HOUR: i64 = 3600;
const TOKEN: &str = "sk-ant-oat01-test-token-0123456789abcdef";

/// Shaped like the recorded Phase 0 response: the limits list plus the legacy objects
/// (deliberately different numbers, to show which one wins) and the weekly breakdown.
const LIMITS_BODY: &str = r#"{
  "five_hour": {"utilization": 99.0, "resets_at": "2026-10-29T13:00:00+00:00"},
  "seven_day": {"utilization": 99.0, "resets_at": "2026-11-01T00:00:00+00:00"},
  "seven_day_opus": null,
  "limits": [
    {"kind": "session", "group": "session", "percent": 58, "severity": "normal",
     "resets_at": "2026-10-29T14:50:00.247834+00:00", "scope": null, "is_active": true},
    {"kind": "weekly_all", "group": "weekly", "percent": 14, "severity": "normal",
     "resets_at": "2026-11-04T02:00:00.247855+00:00", "scope": null, "is_active": false},
    {"kind": "weekly_scoped", "group": "weekly", "percent": 0, "severity": "normal",
     "resets_at": "2026-11-04T02:00:00+00:00",
     "scope": {"model": {"id": null, "display_name": "Fable"}, "surface": null},
     "is_active": false}
  ],
  "seven_day_breakdown": {
    "as_of": "2026-10-29T12:34:10.308422+00:00",
    "window_started_at": "2026-10-28T02:00:00.247855+00:00",
    "rows": [{"key": "claude_code", "display_name": "Claude Code", "percent": 13}]
  }
}"#;

/// An older response shape: only the legacy objects.
const LEGACY_BODY: &str = r#"{
  "five_hour": {"utilization": 42.5, "resets_at": "2026-10-29T15:00:00+00:00"},
  "seven_day": {"utilization": 7.0, "resets_at": "2026-11-03T00:00:00Z"}
}"#;

fn ok(body: &str) -> Result<HttpResponse, HttpError> {
    Ok(HttpResponse {
        status: 200,
        headers: vec![("content-type".into(), "application/json".into())],
        body: body.as_bytes().to_vec(),
    })
}

fn status(code: u16, headers: &[(&str, &str)]) -> Result<HttpResponse, HttpError> {
    Ok(HttpResponse {
        status: code,
        headers: headers
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect(),
        body: Vec::new(),
    })
}

fn start() -> Timestamp {
    Timestamp::from_utc(2026, 10, 29, 12, 0, 0)
}

/// Unix milliseconds, as Claude Code stores `expiresAt`.
fn millis(t: Timestamp) -> i64 {
    t.0 * 1000
}

fn write_credentials(dir: &Path, token: &str, expires: Timestamp) {
    let json = format!(
        r#"{{"claudeAiOauth": {{"accessToken": "{token}", "refreshToken": "sk-ant-ort01-never-used",
            "expiresAt": {}, "scopes": ["user:inference", "user:profile"],
            "subscriptionType": "max", "rateLimitTier": "default_claude_max_5x"}}}}"#,
        millis(expires)
    );
    std::fs::write(dir.join(".credentials.json"), json).unwrap();
}

fn write_account(dir: &Path, account: &str, org: &str, email: &str) {
    let json = format!(
        r#"{{"numStartups": 3, "oauthAccount": {{"accountUuid": "{account}",
            "organizationUuid": "{org}", "emailAddress": "{email}"}}}}"#
    );
    std::fs::write(dir.join(".claude.json"), json).unwrap();
}

struct Harness {
    _temp: TempDir,
    dir: std::path::PathBuf,
    clock: FakeClock,
    transport: RecordingTransport,
    engine: Engine,
}

impl Harness {
    /// A config folder with a valid login (token valid for 8 hours) and an account file.
    fn logged_in() -> Self {
        Self::with(|dir| {
            write_credentials(dir, TOKEN, start().plus_secs(8 * HOUR));
            write_account(dir, "acc-1", "org-1", "dev@example.com");
        })
    }

    fn with(setup: impl FnOnce(&Path)) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join(".claude");
        std::fs::create_dir(&dir).unwrap();
        setup(&dir);
        Self::in_folder(temp, dir)
    }

    fn in_folder(temp: TempDir, dir: std::path::PathBuf) -> Self {
        let clock = FakeClock::new(start());
        let transport = RecordingTransport::new();
        let engine = Engine::new(
            vec![Box::new(ClaudeCodeAssistant::new(vec![ConfigFolder::new(
                dir.clone(),
            )]))],
            Arc::new(clock.clone()),
            Arc::new(transport.clone()),
        );
        Self {
            _temp: temp,
            dir,
            clock,
            transport,
            engine,
        }
    }

    fn meter(&self, key: &str) -> MeterState {
        self.engine.state().assistants[0].accounts[0]
            .meter(key)
            .unwrap_or_else(|| panic!("meter {key} missing"))
            .clone()
    }

    fn problem(&self) -> SourceProblem {
        self.engine.state().assistants[0].accounts[0]
            .problem
            .clone()
            .expect("a problem is shown")
    }

    /// Advances the clock minute by minute, ticking whenever due; returns request times
    /// as minutes after the start.
    fn run_for(&mut self, secs: i64) -> Vec<i64> {
        let end = self.clock.now().0 + secs;
        let mut fetched = Vec::new();
        while self.clock.now().0 < end {
            self.clock.advance(MIN);
            let before = self.transport.requests().len();
            self.engine.tick();
            if self.transport.requests().len() > before {
                fetched.push((self.clock.now().0 - start().0) / MIN);
            }
        }
        fetched
    }
}

// ---- Numbers ---------------------------------------------------------------------

#[test]
fn shows_the_5_hour_and_weekly_meters_from_the_limits_list() {
    let mut h = Harness::logged_in();
    h.transport.push_response(ok(LIMITS_BODY));
    h.engine.tick();

    let session = h.meter(keys::SESSION);
    assert_eq!(session.label, "5-hour");
    assert_eq!(session.used, 58.0);
    assert_eq!(session.limit, Some(100.0));
    assert_eq!(session.unit, Unit::Percent);
    assert_eq!(session.exactness, Exactness::Exact);
    assert_eq!(session.freshness, Freshness::Fresh);
    let w = session.window.expect("session window");
    assert_eq!(w.end, Timestamp::from_utc(2026, 10, 29, 14, 50, 0));
    assert_eq!(w.length, Some(Duration::from_secs(5 * 3600)));
    assert_eq!(w.start, None);

    let weekly = h.meter(keys::WEEKLY);
    assert_eq!(weekly.label, "Weekly");
    assert_eq!(weekly.used, 14.0);
    assert_eq!(weekly.limit, Some(100.0));
    let w = weekly.window.expect("weekly window");
    assert_eq!(w.end, Timestamp::from_utc(2026, 11, 4, 2, 0, 0));
    assert_eq!(w.length, Some(Duration::from_secs(7 * 86_400)));
    assert_eq!(w.start, Some(Timestamp::from_utc(2026, 10, 28, 2, 0, 0)));

    let state = h.engine.state();
    assert_eq!(state.assistants[0].display_name, "Claude Code");
    assert_eq!(state.assistants[0].usage_page.as_deref(), Some(USAGE_PAGE));
    assert_eq!(
        state.assistants[0].accounts[0].featured,
        vec![keys::SESSION.to_string(), keys::WEEKLY.to_string()]
    );
}

#[test]
fn falls_back_to_the_legacy_five_hour_and_seven_day_objects() {
    let mut h = Harness::logged_in();
    h.transport.push_response(ok(LEGACY_BODY));
    h.engine.tick();

    let session = h.meter(keys::SESSION);
    assert_eq!(session.used, 42.5);
    assert_eq!(
        session.window.unwrap().end,
        Timestamp::from_utc(2026, 10, 29, 15, 0, 0)
    );
    let weekly = h.meter(keys::WEEKLY);
    assert_eq!(weekly.used, 7.0);
    let w = weekly.window.unwrap();
    assert_eq!(w.end, Timestamp::from_utc(2026, 11, 3, 0, 0, 0));
    assert_eq!(w.length, Some(Duration::from_secs(7 * 86_400)));
    assert_eq!(w.start, None, "no breakdown, so the start is derived");
}

#[test]
fn calls_the_oauth_usage_endpoint_with_the_bearer_token_and_beta_header() {
    let mut h = Harness::logged_in();
    h.transport.push_response(ok(LIMITS_BODY));
    h.engine.tick();

    let requests = h.transport.requests();
    assert_eq!(requests.len(), 1);
    let r = &requests[0];
    assert_eq!(r.method, "GET");
    assert_eq!(r.url, USAGE_URL);
    assert!(USAGE_URL.starts_with("https://api.anthropic.com/"));
    let header = |name: &str| {
        r.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    };
    assert_eq!(header("authorization"), Some(format!("Bearer {TOKEN}")));
    assert_eq!(header("anthropic-beta").as_deref(), Some(OAUTH_BETA));
    assert_eq!(OAUTH_BETA, "oauth-2025-04-20");
}

#[test]
fn the_account_is_the_login_identity_from_the_account_file() {
    let h = Harness::logged_in();
    let state = h.engine.state();
    let account = &state.assistants[0].accounts[0];
    assert_eq!(account.id, "acc-1/org-1");
    assert_eq!(account.display_name, "dev@example.com");
}

// ---- Credentials -------------------------------------------------------------------

#[test]
fn credentials_are_read_fresh_every_fetch_and_never_written() {
    let mut h = Harness::logged_in();
    let path = h.dir.join(".credentials.json");
    // Claude Code renews its own login; the next fetch must use the renewed token.
    h.transport.push_response(ok(LIMITS_BODY));
    h.engine.tick();
    write_credentials(&h.dir, "sk-ant-oat01-renewed", start().plus_secs(9 * HOUR));
    let before = std::fs::read(&path).unwrap();
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();

    h.transport.push_response(ok(LIMITS_BODY));
    h.run_for(5 * MIN);

    let requests = h.transport.requests();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[1]
            .headers
            .iter()
            .any(|(_, v)| v == "Bearer sk-ant-oat01-renewed")
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(
        std::fs::metadata(&path).unwrap().modified().unwrap(),
        modified
    );
    let names: Vec<_> = std::fs::read_dir(&h.dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(
        names.len(),
        2,
        "nothing was added to the config folder: {names:?}"
    );
}

/// Captures every log line of this test binary.
struct Capture(Mutex<Vec<String>>);

impl log::Log for Capture {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        true
    }
    fn log(&self, record: &log::Record<'_>) {
        self.0.lock().unwrap().push(record.args().to_string());
    }
    fn flush(&self) {}
}

fn captured_logs() -> &'static Capture {
    static CAPTURE: OnceLock<&'static Capture> = OnceLock::new();
    CAPTURE.get_or_init(|| {
        let capture: &'static Capture = Box::leak(Box::new(Capture(Mutex::new(Vec::new()))));
        log::set_logger(capture).unwrap();
        log::set_max_level(log::LevelFilter::Trace);
        capture
    })
}

#[test]
fn the_token_never_reaches_the_log() {
    let logs = captured_logs();
    let mut h = Harness::logged_in();
    h.transport.push_response(ok(LIMITS_BODY));
    h.transport.push_response(status(401, &[]));
    h.transport.push_response(ok(r#"{"unexpected": true}"#));
    h.transport
        .push_response(Err(HttpError::Transport("connection refused".into())));
    h.run_for(2 * HOUR);
    assert!(h.transport.requests().len() >= 4);
    let lines = logs.0.lock().unwrap();
    assert!(!lines.is_empty(), "failures are logged");
    for line in lines.iter() {
        assert!(!line.contains("test-token"), "{line}");
    }
}

// ---- States ------------------------------------------------------------------------

#[test]
fn claude_code_not_installed() {
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join(".claude");
    let mut h = Harness::in_folder(temp, missing);
    h.engine.tick();
    let p = h.problem();
    assert_eq!(p.error, FetchError::NotInstalled);
    assert_eq!(
        p.hint.as_deref(),
        Some("Claude Code was not found; install it and sign in")
    );
    assert!(h.transport.requests().is_empty());
    assert_eq!(h.engine.state().assistants[0].accounts.len(), 1);
}

#[test]
fn never_logged_in() {
    let mut h = Harness::with(|_| {});
    h.engine.tick();
    let p = h.problem();
    assert_eq!(p.error, FetchError::NotLoggedIn);
    assert_eq!(p.hint.as_deref(), Some("sign in to Claude Code"));
    assert!(h.transport.requests().is_empty());
}

#[test]
fn an_expired_token_asks_to_run_claude_code_once_without_calling_the_endpoint() {
    let mut h = Harness::with(|dir| write_credentials(dir, TOKEN, start().plus_secs(-MIN)));
    h.engine.tick();
    let p = h.problem();
    assert_eq!(p.error, FetchError::Expired);
    assert_eq!(p.hint.as_deref(), Some("run Claude Code once"));
    assert!(h.transport.requests().is_empty());
}

#[test]
fn a_rejected_token_is_expired_too() {
    let mut h = Harness::logged_in();
    h.transport.push_response(status(401, &[]));
    h.engine.tick();
    assert_eq!(h.problem().error, FetchError::Expired);
}

#[test]
fn offline_keeps_the_last_good_numbers_and_recovers_by_itself() {
    let mut h = Harness::logged_in();
    h.transport.push_response(ok(LIMITS_BODY));
    h.engine.tick();
    h.transport
        .push_response(Err(HttpError::Transport("dns lookup failed".into())));
    h.run_for(5 * MIN);
    assert_eq!(h.problem().error, FetchError::Offline);
    assert_eq!(h.meter(keys::SESSION).used, 58.0);

    h.transport.push_response(ok(LEGACY_BODY));
    h.run_for(5 * MIN);
    let state = h.engine.state();
    assert_eq!(state.assistants[0].accounts[0].problem, None);
    assert_eq!(h.meter(keys::SESSION).used, 42.5);
}

#[test]
fn a_404_means_the_endpoint_changed() {
    let mut h = Harness::logged_in();
    h.transport.push_response(status(404, &[]));
    h.engine.tick();
    let p = h.problem();
    assert_eq!(p.error, FetchError::EndpointChanged);
    assert!(p.hint.is_some());
}

#[test]
fn a_changed_response_shape_shows_no_numbers() {
    let mut h = Harness::logged_in();
    h.transport
        .push_response(ok(r#"{"limits": "moved", "five_hour": {"pct": 10}}"#));
    h.engine.tick();
    assert_eq!(h.problem().error, FetchError::EndpointChanged);
    assert!(h.engine.state().assistants[0].accounts[0].meters.is_empty());

    let mut h = Harness::logged_in();
    h.transport.push_response(ok("<html>maintenance</html>"));
    h.engine.tick();
    assert_eq!(h.problem().error, FetchError::EndpointChanged);
}

// ---- Polling -----------------------------------------------------------------------

#[test]
fn polls_every_5_minutes_while_active_and_every_15_while_idle() {
    let mut h = Harness::logged_in();
    for _ in 0..20 {
        h.transport.push_response(ok(LIMITS_BODY));
    }
    h.engine.tick();
    assert_eq!(h.run_for(20 * MIN), vec![5, 10, 15, 20]);
    h.engine.set_presence(Presence::Idle);
    assert_eq!(h.run_for(45 * MIN), vec![35, 50, 65]);
}

#[test]
fn a_429_honors_retry_after() {
    let mut h = Harness::logged_in();
    h.transport
        .push_response(status(429, &[("Retry-After", "1200")]));
    h.engine.tick();
    assert_eq!(
        h.problem().error,
        FetchError::RateLimited {
            retry_after: Some(Duration::from_secs(1200))
        }
    );
    h.transport.push_response(ok(LIMITS_BODY));
    assert_eq!(h.run_for(25 * MIN), vec![20, 25]);
}

#[test]
fn a_429_with_a_short_retry_after_still_waits_the_rate_limit_backoff() {
    let mut h = Harness::logged_in();
    for _ in 0..3 {
        h.transport
            .push_response(status(429, &[("Retry-After", "0")]));
    }
    h.transport.push_response(ok(LIMITS_BODY));
    h.engine.tick();
    // 5, then 10 and 20 minutes apart, as without Retry-After; never right away.
    assert_eq!(h.run_for(39 * MIN), vec![5, 15, 35]);
}

#[test]
fn a_429_without_retry_after_waits_5_minutes_doubling_up_to_30() {
    let mut h = Harness::logged_in();
    for _ in 0..6 {
        h.transport.push_response(status(429, &[]));
    }
    h.engine.tick();
    assert_eq!(
        h.problem().error,
        FetchError::RateLimited { retry_after: None }
    );
    // 5, then 10, 20, 30, 30 minutes apart.
    assert_eq!(h.run_for(100 * MIN), vec![5, 15, 35, 65, 95]);
}

#[test]
fn a_429_after_other_failures_still_waits_5_minutes_first() {
    let mut h = Harness::logged_in();
    for _ in 0..2 {
        h.transport
            .push_response(Err(HttpError::Transport("offline".into())));
    }
    h.transport.push_response(status(429, &[]));
    h.transport
        .push_response(Err(HttpError::Transport("offline".into())));
    h.transport.push_response(ok(LIMITS_BODY));
    h.engine.tick();
    // Offline at 0 and 1 (1, then 2 minutes), rate limited at 3 (5 minutes, not the
    // third step of a shared doubling), offline again at 8 (1 minute, a new run).
    assert_eq!(h.run_for(10 * MIN), vec![1, 3, 8, 9]);
}

// ---- The recorded Phase 0 response -------------------------------------------------

/// The usage response recorded in Phase 0 (body as recorded, headers excluded).
const RECORDED: &str = include_str!("fixtures/usage-recorded.json");

fn recorded() -> Harness {
    let mut h = Harness::logged_in();
    h.transport.push_response(ok(RECORDED));
    h.engine.tick();
    h
}

#[test]
fn the_recorded_response_shows_the_model_specific_weekly_limit_by_display_name() {
    let h = recorded();
    let fable = h.meter("weekly:fable");
    assert_eq!(fable.label, "Weekly \u{b7} Fable");
    assert_eq!(fable.used, 0.0);
    assert_eq!(fable.limit, Some(100.0));
    assert_eq!(fable.unit, Unit::Percent);
    let w = fable.window.expect("model window");
    assert_eq!(w.end, Timestamp::from_utc(2026, 11, 4, 2, 0, 0));
    assert_eq!(w.length, Some(Duration::from_secs(7 * 86_400)));
    assert_eq!(h.meter(keys::SESSION).used, 58.0);
    assert_eq!(h.meter(keys::WEEKLY).used, 14.0);
}

#[test]
fn the_weekly_meter_carries_the_per_surface_breakdown() {
    let h = recorded();
    let weekly = h.meter(keys::WEEKLY);
    let part = |label: &str, percent: f64| BreakdownPart {
        label: label.into(),
        percent,
    };
    assert_eq!(
        weekly.breakdown,
        Some(Breakdown {
            parts: vec![
                part("Claude Code", 13.0),
                part("Chats", 4.0),
                part("Cowork", 83.0),
                part("Other", 0.0),
            ]
        })
    );
    assert_eq!(h.meter(keys::SESSION).breakdown, None);
}

#[test]
fn the_recorded_response_shows_usage_credits_in_the_accounts_currency_without_a_window() {
    let h = recorded();
    let credits = h.meter(keys::USAGE_CREDITS);
    assert_eq!(credits.label, "Usage credits");
    assert_eq!(credits.used, 0.0);
    assert_eq!(credits.limit, Some(100.0));
    assert_eq!(credits.unit, Unit::Currency("BRL".into()));
    assert_eq!(credits.window, None);
    assert_eq!(credits.pace, None);
}

#[test]
fn usage_credits_come_from_the_structured_spend_object_in_minor_units() {
    let mut h = Harness::logged_in();
    h.transport.push_response(ok(r#"{
      "five_hour": {"utilization": 10.0, "resets_at": "2026-10-29T15:00:00Z"},
      "spend": {"used": {"amount_minor": 1234, "currency": "JPY", "exponent": 0},
                "limit": {"amount_minor": 5000, "currency": "JPY", "exponent": 0},
                "percent": 25, "enabled": true},
      "extra_usage": {"is_enabled": true, "monthly_limit": 1, "used_credits": 1.0,
                      "currency": "USD", "decimal_places": 2}
    }"#));
    h.engine.tick();
    let credits = h.meter(keys::USAGE_CREDITS);
    assert_eq!(credits.used, 1234.0);
    assert_eq!(credits.limit, Some(5000.0));
    assert_eq!(credits.unit, Unit::Currency("JPY".into()));
}

#[test]
fn usage_credits_fall_back_to_extra_usage() {
    let mut h = Harness::logged_in();
    h.transport.push_response(ok(r#"{
      "five_hour": {"utilization": 10.0, "resets_at": "2026-10-29T15:00:00Z"},
      "extra_usage": {"is_enabled": true, "monthly_limit": 2000, "used_credits": 550.0,
                      "currency": "USD", "decimal_places": 2}
    }"#));
    h.engine.tick();
    let credits = h.meter(keys::USAGE_CREDITS);
    assert_eq!(credits.used, 5.5);
    assert_eq!(credits.limit, Some(20.0));
    assert_eq!(credits.unit, Unit::Currency("USD".into()));
    assert_eq!(credits.window, None);
}

#[test]
fn usage_credits_fall_back_to_extra_usage_when_spend_cannot_be_read() {
    let mut h = Harness::logged_in();
    h.transport.push_response(ok(r#"{
      "five_hour": {"utilization": 10.0, "resets_at": "2026-10-29T15:00:00Z"},
      "spend": {"used": {"amount": "12.34"}, "limit": null, "enabled": true},
      "extra_usage": {"is_enabled": true, "monthly_limit": 2000, "used_credits": 550.0,
                      "currency": "USD", "decimal_places": 2}
    }"#));
    h.engine.tick();
    let credits = h.meter(keys::USAGE_CREDITS);
    assert_eq!(credits.used, 5.5);
    assert_eq!(credits.limit, Some(20.0));
    assert_eq!(credits.unit, Unit::Currency("USD".into()));
}

#[test]
fn no_usage_credits_meter_while_credits_are_disabled() {
    let mut h = Harness::logged_in();
    h.transport.push_response(ok(r#"{
      "five_hour": {"utilization": 10.0, "resets_at": "2026-10-29T15:00:00Z"},
      "spend": {"used": {"amount_minor": 0, "currency": "USD", "exponent": 2},
                "limit": null, "enabled": false},
      "extra_usage": {"is_enabled": false, "monthly_limit": null, "used_credits": null,
                      "currency": null, "decimal_places": 2}
    }"#));
    h.engine.tick();
    let state = h.engine.state();
    assert!(
        state.assistants[0].accounts[0]
            .meter(keys::USAGE_CREDITS)
            .is_none()
    );
}

// ---- Unrecognized keys -------------------------------------------------------------

#[test]
fn an_unrecognized_object_with_utilization_and_reset_becomes_a_meter_under_its_raw_key() {
    let h = recorded();
    let m = h.meter("unrecognized:iguana_necktie");
    assert_eq!(m.label, "iguana_necktie");
    assert!(m.unrecognized);
    assert_eq!(m.used, 0.0);
    assert_eq!(m.limit, Some(100.0));
    assert_eq!(m.unit, Unit::Percent);
    assert_eq!(
        m.window.map(|w| w.end),
        Some(Timestamp::from_utc(2026, 11, 5, 7, 59, 0))
    );
    assert!(!h.meter(keys::WEEKLY).unrecognized);

    let state = h.engine.state();
    let account = &state.assistants[0].accounts[0];
    assert!(
        !account
            .featured
            .iter()
            .any(|k| k.starts_with("unrecognized:"))
    );
}

#[test]
fn null_code_named_keys_are_ignored() {
    let h = recorded();
    let state = h.engine.state();
    let mut keys: Vec<&str> = state.assistants[0].accounts[0]
        .meters
        .iter()
        .map(|m| m.key.as_str())
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec![
            "session",
            "unrecognized:iguana_necktie",
            "usage-credits",
            "weekly",
            "weekly:fable"
        ]
    );
}

#[test]
fn unrecognized_meters_keep_alerts_on() {
    let mut h = Harness::logged_in();
    h.transport.push_response(ok(r#"{
      "five_hour": {"utilization": 10.0, "resets_at": "2026-10-29T15:00:00Z"},
      "copper_kite": {"utilization": 91.0, "resets_at": "2026-11-02T00:00:00Z"},
      "brass_thimble": {"note": "no utilization, no Meter"}
    }"#));
    h.engine.tick();
    let alerted: Vec<String> = h
        .engine
        .take_alerts()
        .into_iter()
        .map(|a| a.meter_key)
        .collect();
    assert!(alerted.contains(&"unrecognized:copper_kite".to_string()));
    let state = h.engine.state();
    assert!(
        state.assistants[0].accounts[0]
            .meter("unrecognized:brass_thimble")
            .is_none()
    );
}

#[test]
fn pace_severity_and_is_active_are_logged_raw() {
    let logs = captured_logs();
    let _h = recorded();
    let lines = logs.0.lock().unwrap();
    assert!(
        lines
            .iter()
            .any(|l| l.contains("pace") && l.contains("severity") && l.contains("is_active")),
        "{lines:?}"
    );
}
