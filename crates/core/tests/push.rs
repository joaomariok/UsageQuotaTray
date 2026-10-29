//! Engine seam for push Sources: a module-neutral test Assistant with one polled Source
//! and one push Source. Push messages are fed to the engine the way the push channel
//! delivers them; nothing here knows any real Assistant.

use std::sync::Arc;

use serde_json::json;
use uqt_core::contract::{Assistant, FetchContext, FetchError, FetchOutput, Intervals, Source};
use uqt_core::engine::{Engine, minutes};
use uqt_core::model::{AccountInfo, BadgeSpec, Exactness, MeterKey, MeterValue, Unit};
use uqt_core::push::PushMessage;
use uqt_core::settings::{Settings, SettingsEdit};
use uqt_core::testing::RecordingTransport;
use uqt_core::time::{FakeClock, Timestamp};

struct Pushy;

impl Assistant for Pushy {
    fn id(&self) -> &'static str {
        "pushy"
    }
    fn display_name(&self) -> &str {
        "Pushy"
    }
    fn allowed_hosts(&self) -> &[&'static str] {
        &[]
    }
    fn badge(&self) -> BadgeSpec {
        BadgeSpec {
            svg: "",
            letters: "P",
            color: "#000000",
        }
    }
    fn suggested_featured(&self) -> Vec<MeterKey> {
        vec!["count".into()]
    }
    fn usage_page(&self) -> Option<&str> {
        None
    }
    fn discover_accounts(&mut self) -> Vec<AccountInfo> {
        ["a", "b"]
            .into_iter()
            .map(|id| AccountInfo {
                id: id.into(),
                display_name: id.to_uppercase(),
            })
            .collect()
    }
    fn sources(&mut self, account: &AccountInfo) -> Vec<Box<dyn Source>> {
        vec![Box::new(Hook {
            origin: account.id.clone(),
        })]
    }
}

/// Accepts `{"count": n}` from its own origin only.
struct Hook {
    origin: String,
}

impl Source for Hook {
    fn id(&self) -> &str {
        "hook"
    }
    fn display_name(&self) -> &str {
        "Hook"
    }
    fn exactness(&self) -> Exactness {
        Exactness::Exact
    }
    fn priority(&self) -> u32 {
        0
    }
    fn intervals(&self) -> Intervals {
        Intervals {
            active: minutes(5),
            idle: minutes(15),
        }
    }
    fn push_only(&self) -> bool {
        true
    }
    fn fetch(&mut self, _ctx: &FetchContext<'_>) -> Result<FetchOutput, FetchError> {
        panic!("a push-only Source is never polled");
    }
    fn accept_push(&mut self, message: &PushMessage) -> Option<FetchOutput> {
        if message.origin != self.origin {
            return None;
        }
        let used = message.payload["count"].as_f64()?;
        Some(FetchOutput {
            values: vec![MeterValue {
                key: "count".into(),
                label: "Count".into(),
                used,
                limit: Some(10.0),
                unit: Unit::Count("pushes".into()),
                window: None,
            }],
            notices: Vec::new(),
        })
    }
}

fn message(source: &str, origin: &str, count: u32) -> PushMessage {
    PushMessage {
        assistant: "pushy".into(),
        source: source.into(),
        origin: origin.into(),
        payload: json!({ "count": count }),
    }
}

fn engine(settings: &Settings) -> (Engine, FakeClock) {
    let clock = FakeClock::new(Timestamp::from_utc(2026, 10, 29, 12, 0, 0));
    let engine = Engine::with_settings(
        vec![Box::new(Pushy)],
        Arc::new(clock.clone()),
        Arc::new(RecordingTransport::new()),
        settings,
    );
    (engine, clock)
}

#[test]
fn a_push_message_becomes_a_reading_of_the_account_it_belongs_to() {
    let (mut engine, clock) = engine(&Settings::default());
    clock.advance(30);
    assert!(engine.push(&message("hook", "b", 4)));

    let state = engine.state();
    let accounts = &state.assistants[0].accounts;
    assert!(accounts[0].meters.is_empty(), "account a got nothing");
    let m = accounts[1]
        .meter("count")
        .expect("account b has the Reading");
    assert_eq!(m.used, 4.0);
    assert_eq!(m.source_name, "Hook");
    assert_eq!(m.exactness, Exactness::Exact);
    assert_eq!(m.fetched_at, clock.now());
}

#[test]
fn messages_for_another_assistant_source_or_origin_are_ignored() {
    let (mut engine, _) = engine(&Settings::default());
    let mut other_assistant = message("hook", "a", 1);
    other_assistant.assistant = "someone-else".into();
    assert!(!engine.push(&other_assistant));
    assert!(!engine.push(&message("other-source", "a", 1)));
    assert!(!engine.push(&message("hook", "nobody", 1)));
    assert!(
        engine.state().assistants[0]
            .accounts
            .iter()
            .all(|a| a.meters.is_empty())
    );
}

#[test]
fn push_only_sources_are_never_polled_or_scheduled() {
    let (mut engine, _) = engine(&Settings::default());
    assert!(!engine.tick(), "nothing to fetch");
    assert_eq!(engine.next_wake(), None);
}

#[test]
fn a_disabled_account_ignores_pushes() {
    let mut settings = Settings::default();
    settings.apply(SettingsEdit::AccountEnabled {
        assistant: "pushy".into(),
        account: "a".into(),
        on: false,
    });
    let (mut engine, _) = engine(&settings);
    assert!(!engine.push(&message("hook", "a", 3)));
}

#[test]
fn pushed_readings_raise_alerts_like_polled_ones() {
    let (mut engine, _) = engine(&Settings::default());
    assert!(engine.push(&message("hook", "a", 8)));
    let alerts = engine.take_alerts();
    assert_eq!(alerts.len(), 1, "{alerts:?}");
    assert_eq!(alerts[0].meter_key, "count");
    assert_eq!(alerts[0].account_id, "a");
}

#[test]
fn messages_survive_the_wire_format() {
    let m = message("hook", "C:/Users/me/.claude", 7);
    let decoded = PushMessage::decode(&m.encode()).expect("round trip");
    assert_eq!(decoded, m);
    assert_eq!(PushMessage::decode(b"not json"), None);
    assert_eq!(PushMessage::decode(br#"{"v": 99}"#), None);
}
