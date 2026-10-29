//! Engine seam: the user's settings decide what runs, what is featured, how often the
//! Sources poll, and what the module is configured with.

use std::sync::Arc;
use std::time::Duration;

use uqt_assistant_fake::{FakeAssistant, FakeControl, ID, keys, settings_keys, sources};
use uqt_core::contract::FieldKind;
use uqt_core::engine::Engine;
use uqt_core::settings::{SettingValue, Settings, SettingsEdit};
use uqt_core::testing::RecordingTransport;
use uqt_core::time::{FakeClock, Timestamp};

const MIN: i64 = 60;
const ACCOUNT: &str = "fake-demo";

struct Harness {
    clock: FakeClock,
    control: FakeControl,
    engine: Engine,
}

impl Harness {
    fn new(settings: &Settings) -> Self {
        let clock = FakeClock::new(Timestamp::from_utc(2026, 10, 29, 12, 0, 0));
        let control = FakeControl::new();
        let engine = Engine::with_settings(
            vec![Box::new(FakeAssistant::with_control(control.clone()))],
            Arc::new(clock.clone()),
            Arc::new(RecordingTransport::new()),
            settings,
        );
        Self {
            clock,
            control,
            engine,
        }
    }

    fn run_for(&mut self, secs: i64) {
        let end = self.clock.now().0 + secs;
        while self.clock.now().0 < end {
            self.clock.advance(MIN);
            self.engine.tick();
        }
    }

    fn featured(&self) -> Vec<String> {
        self.engine.state().assistants[0].accounts[0]
            .featured
            .clone()
    }
}

fn with(edits: Vec<SettingsEdit>) -> Settings {
    let mut s = Settings::default();
    for e in edits {
        s.apply(e);
    }
    s
}

fn feature(meter: &str, on: bool, current: &[&str]) -> SettingsEdit {
    SettingsEdit::Featured {
        assistant: ID.into(),
        account: ACCOUNT.into(),
        meter: meter.into(),
        on,
        current: current.iter().map(|s| s.to_string()).collect(),
    }
}

#[test]
fn the_modules_suggestion_is_featured_until_the_user_chooses() {
    let mut h = Harness::new(&Settings::default());
    h.engine.tick();
    assert_eq!(h.featured(), vec![keys::REQUESTS_TODAY, keys::SPEND_MONTH]);

    let chosen = with(vec![feature(
        keys::REQUESTS_TODAY,
        false,
        &[keys::REQUESTS_TODAY, keys::SPEND_MONTH],
    )]);
    h.engine.apply_settings(&chosen);
    assert_eq!(h.featured(), vec![keys::SPEND_MONTH]);
}

#[test]
fn a_featured_meter_the_account_does_not_have_stays_chosen_but_is_not_a_meter() {
    let s = with(vec![
        feature(keys::REQUESTS_TODAY, false, &[keys::REQUESTS_TODAY]),
        feature("gone", true, &[]),
    ]);
    let mut h = Harness::new(&s);
    h.engine.tick();
    let state = h.engine.state();
    let acc = &state.assistants[0].accounts[0];
    assert_eq!(acc.featured, vec!["gone"]);
    assert!(acc.meter("gone").is_none());
}

#[test]
fn a_disabled_assistant_is_neither_fetched_nor_shown_but_stays_in_the_catalog() {
    let s = with(vec![SettingsEdit::AssistantEnabled {
        assistant: ID.into(),
        on: false,
    }]);
    let mut h = Harness::new(&s);
    h.run_for(30 * MIN);
    assert_eq!(h.control.fetch_count(sources::API), 0);
    assert!(h.engine.state().assistants.is_empty());
    assert_eq!(h.engine.next_wake(), None);

    let catalog = h.engine.catalog();
    assert_eq!(catalog.assistants.len(), 1);
    assert_eq!(catalog.assistants[0].id, ID);
    assert!(!catalog.assistants[0].enabled);
    assert_eq!(catalog.assistants[0].accounts[0].id, ACCOUNT);

    h.engine.apply_settings(&Settings::default());
    h.engine.tick();
    assert_eq!(h.control.fetch_count(sources::API), 1);
    assert_eq!(h.engine.state().assistants.len(), 1);
}

#[test]
fn a_disabled_account_is_neither_fetched_nor_shown() {
    let s = with(vec![SettingsEdit::AccountEnabled {
        assistant: ID.into(),
        account: ACCOUNT.into(),
        on: false,
    }]);
    let mut h = Harness::new(&s);
    h.run_for(30 * MIN);
    assert_eq!(h.control.fetch_count(sources::API), 0);
    let state = h.engine.state();
    assert_eq!(state.assistants.len(), 1);
    assert!(state.assistants[0].accounts.is_empty());
    let catalog = h.engine.catalog();
    assert!(catalog.assistants[0].enabled);
    assert!(!catalog.assistants[0].accounts[0].enabled);
}

#[test]
fn the_active_polling_override_replaces_the_sources_own_interval() {
    let s = with(vec![SettingsEdit::ActivePoll(Some(Duration::from_secs(
        10 * 60,
    )))]);
    let mut h = Harness::new(&s);
    h.engine.tick();
    h.run_for(9 * MIN);
    assert_eq!(h.control.fetch_count(sources::API), 1);
    h.run_for(MIN);
    assert_eq!(h.control.fetch_count(sources::API), 2);

    // Back to the module's 5 minutes.
    h.engine.apply_settings(&Settings::default());
    h.run_for(5 * MIN);
    assert_eq!(h.control.fetch_count(sources::API), 3);
}

#[test]
fn the_module_declares_its_settings_and_receives_their_values() {
    let h = Harness::new(&Settings::default());
    let catalog = h.engine.catalog();
    let fields = &catalog.assistants[0].fields;
    let name = fields
        .iter()
        .find(|f| f.key == settings_keys::ACCOUNT_NAME)
        .expect("account name field");
    assert!(matches!(name.kind, FieldKind::Text { .. }));
    assert_eq!(
        h.engine.state().assistants[0].accounts[0].display_name,
        "Demo account"
    );

    let s = with(vec![SettingsEdit::ModuleValue {
        assistant: ID.into(),
        key: settings_keys::ACCOUNT_NAME.into(),
        value: SettingValue::Text("Work".into()),
    }]);
    let h = Harness::new(&s);
    assert_eq!(
        h.engine.state().assistants[0].accounts[0].display_name,
        "Work"
    );
}
