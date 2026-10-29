//! Engine seam, plan detection and Notices: the real Engine and Claude Code Assistant
//! over a temporary config folder, a recording fake transport, a fake clock and a fake
//! environment probe. Nothing here reads the real ~/.claude, the real environment or
//! registry, or reaches the network.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tempfile::TempDir;
use uqt_assistant_anthropic::{ClaudeCodeAssistant, ConfigFolder, EnvProbe, EnvScope, Plan};
use uqt_core::engine::Engine;
use uqt_core::http::{HttpError, HttpResponse};
use uqt_core::testing::RecordingTransport;
use uqt_core::time::{FakeClock, Timestamp};

const HOUR: i64 = 3600;
const RECORDED: &str = include_str!("fixtures/usage-recorded.json");

fn start() -> Timestamp {
    Timestamp::from_utc(2026, 10, 29, 12, 0, 0)
}

fn ok(body: &str) -> Result<HttpResponse, HttpError> {
    Ok(HttpResponse {
        status: 200,
        headers: Vec::new(),
        body: body.as_bytes().to_vec(),
    })
}

/// Which variables the fake environment has, by scope.
#[derive(Default)]
struct FakeEnv(HashSet<(EnvScope, &'static str)>);

impl EnvProbe for FakeEnv {
    fn is_set(&self, scope: EnvScope, name: &str) -> bool {
        self.0.iter().any(|(s, n)| *s == scope && *n == name)
    }
}

struct Folder {
    _temp: TempDir,
    dir: PathBuf,
}

/// A logged-in config folder. `oauth` and `account` are extra JSON members of the
/// credentials' `claudeAiOauth` and of the account file's `oauthAccount`.
fn folder(oauth: &str, account: &str) -> Folder {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("claude");
    std::fs::create_dir(&dir).unwrap();
    let expires = (start().0 + 8 * HOUR) * 1000;
    std::fs::write(
        dir.join(".credentials.json"),
        format!(
            r#"{{"claudeAiOauth": {{"accessToken": "sk-ant-oat01-x", "expiresAt": {expires}{oauth}}}}}"#
        ),
    )
    .unwrap();
    std::fs::write(
        dir.join(".claude.json"),
        format!(
            r#"{{"oauthAccount": {{"accountUuid": "acc-1", "organizationUuid": "org-1",
                "emailAddress": "me@example.com"{account}}}}}"#
        ),
    )
    .unwrap();
    Folder { _temp: temp, dir }
}

const MAX: &str = r#", "subscriptionType": "max", "rateLimitTier": "default_claude_max_5x""#;
const CREDITS_ON: &str = r#", "billingType": "stripe_subscription", "organizationType": "claude_max",
    "seatTier": null, "hasExtraUsageEnabled": true"#;

/// Runs one fetch with `body` and returns the Account's Notices.
fn notices(dir: &Path, env: FakeEnv, body: &str) -> Vec<String> {
    let transport = RecordingTransport::new();
    transport.push_response(ok(body));
    let module = ClaudeCodeAssistant::new(vec![ConfigFolder::new(dir.to_path_buf())])
        .with_env(Arc::new(env));
    let mut engine = Engine::new(
        vec![Box::new(module)],
        Arc::new(FakeClock::new(start())),
        Arc::new(transport),
    );
    engine.tick();
    engine.state().assistants[0].accounts[0]
        .notices
        .iter()
        .map(|n| n.text.clone())
        .collect()
}

fn session_at(percent: f64) -> String {
    format!(
        r#"{{"five_hour": {{"utilization": {percent}, "resets_at": "2026-10-29T15:00:00Z"}},
            "seven_day": {{"utilization": 30.0, "resets_at": "2026-11-03T00:00:00Z"}}}}"#
    )
}

#[test]
fn plan_is_detected_from_local_files() {
    let f = folder(
        MAX,
        r#", "billingType": "stripe_subscription", "organizationType": "claude_max",
        "seatTier": "standard", "hasExtraUsageEnabled": true,
        "userRateLimitTier": "u-tier", "organizationRateLimitTier": "o-tier""#,
    );
    let plan = Plan::detect(&ConfigFolder::new(f.dir.clone()));
    assert_eq!(
        plan,
        Plan {
            subscription_type: Some("max".into()),
            rate_limit_tier: Some("default_claude_max_5x".into()),
            user_rate_limit_tier: Some("u-tier".into()),
            organization_rate_limit_tier: Some("o-tier".into()),
            billing_type: Some("stripe_subscription".into()),
            organization_type: Some("claude_max".into()),
            seat_tier: Some("standard".into()),
            usage_credits_enabled: Some(true),
        }
    );
    assert!(plan.is_subscription());

    let empty = Plan::detect(&ConfigFolder::new(f.dir.join("missing")));
    assert_eq!(empty, Plan::default());
    assert!(!empty.is_subscription());
}

#[test]
fn a_subscription_shows_that_the_quota_is_shared_with_chat_and_cowork() {
    let f = folder(MAX, "");
    let texts = notices(&f.dir, FakeEnv::default(), RECORDED);
    assert_eq!(
        texts,
        vec![
            "Included usage is shared with Claude chat and Cowork \
             (this week: Claude Code 13%, Chats 4%, Cowork 83%, Other 0%)"
                .to_string()
        ]
    );
}

#[test]
fn no_shared_quota_notice_without_a_subscription() {
    let f = folder("", "");
    assert!(notices(&f.dir, FakeEnv::default(), &session_at(10.0)).is_empty());
}

#[test]
fn an_api_key_in_any_environment_is_noticed() {
    for scope in [EnvScope::Process, EnvScope::User, EnvScope::Machine] {
        let f = folder("", "");
        let env = FakeEnv([(scope, "ANTHROPIC_API_KEY")].into_iter().collect());
        let texts = notices(&f.dir, env, &session_at(10.0));
        assert_eq!(texts.len(), 1, "{scope:?}: {texts:?}");
        assert!(
            texts[0].starts_with("ANTHROPIC_API_KEY is set"),
            "{}",
            texts[0]
        );
        assert!(texts[0].contains("instead of your subscription"));
    }
}

#[test]
fn an_api_key_helper_or_env_entry_in_claude_code_settings_is_noticed() {
    for settings in [
        r#"{"apiKeyHelper": "C:\\tools\\key.cmd", "theme": "dark"}"#,
        r#"{"env": {"ANTHROPIC_API_KEY": "sk-ant-api03-not-read"}}"#,
    ] {
        let f = folder("", "");
        std::fs::write(f.dir.join("settings.json"), settings).unwrap();
        let texts = notices(&f.dir, FakeEnv::default(), &session_at(10.0));
        assert_eq!(texts.len(), 1, "{settings}: {texts:?}");
        assert!(texts[0].contains("Claude Code settings"), "{}", texts[0]);
        assert!(!texts[0].contains("sk-ant"), "the value is never read");
    }
    let f = folder("", "");
    std::fs::write(
        f.dir.join("settings.json"),
        r#"{"env": {"OTHER": "1"}, "apiKeyHelper": null}"#,
    )
    .unwrap();
    assert!(notices(&f.dir, FakeEnv::default(), &session_at(10.0)).is_empty());
}

#[test]
fn a_reached_limit_with_usage_credits_enabled_says_credits_may_apply() {
    let f = folder("", CREDITS_ON);
    let texts = notices(&f.dir, FakeEnv::default(), &session_at(100.0));
    assert_eq!(
        texts,
        vec!["An included limit was reached; usage credits may apply".to_string()]
    );
    // Below the limit, or with credits off, nothing is said.
    assert!(notices(&f.dir, FakeEnv::default(), &session_at(99.0)).is_empty());
    let off = folder("", r#", "hasExtraUsageEnabled": false"#);
    assert!(notices(&off.dir, FakeEnv::default(), &session_at(100.0)).is_empty());
}
