//! Engine seam: alerts at 75/90/100% of the Limit for every Meter that has one, each
//! threshold at most once per Window, and a reset alert only for Windows that had
//! crossed the first threshold.

use std::sync::Arc;

use uqt_assistant_fake::{FakeAssistant, FakeControl, ID, keys, sources};
use uqt_core::contract::FetchError;
use uqt_core::engine::{Alert, AlertKind, Engine};
use uqt_core::settings::{Settings, SettingsEdit};
use uqt_core::testing::RecordingTransport;
use uqt_core::time::{FakeClock, Timestamp};

const MIN: i64 = 60;
const HOUR: i64 = 3600;
const ACCOUNT: &str = "fake-demo";
/// The daily request Meter's Limit.
const LIMIT: f64 = 500.0;

struct Harness {
    clock: FakeClock,
    control: FakeControl,
    engine: Engine,
}

impl Harness {
    /// 2026-10-29 12:00 UTC. The monthly spend is held at zero so only the request
    /// Meter moves.
    fn new(settings: &Settings) -> Self {
        let clock = FakeClock::new(Timestamp::from_utc(2026, 10, 29, 12, 0, 0));
        let control = FakeControl::new();
        control.set_used(keys::SPEND_MONTH, Some(0.0));
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

    /// Serves `percent` of the request Limit from the next fetch on, waits for one, and
    /// returns the alerts raised meanwhile.
    fn requests_at(&mut self, percent: f64) -> Vec<Alert> {
        self.control
            .set_used(keys::REQUESTS_TODAY, Some(LIMIT * percent / 100.0));
        self.run_for(5 * MIN)
    }

    fn run_for(&mut self, secs: i64) -> Vec<Alert> {
        let end = self.clock.now().0 + secs;
        while self.clock.now().0 < end {
            self.clock.advance(MIN);
            self.engine.tick();
        }
        self.engine.take_alerts()
    }

    /// Moves to just after the next UTC midnight, when the daily Window resets.
    fn past_midnight(&mut self) -> Vec<Alert> {
        let now = self.clock.now().0;
        let midnight = (now / 86_400 + 1) * 86_400;
        self.run_for(midnight - now + 5 * MIN)
    }
}

fn thresholds(alerts: &[Alert]) -> Vec<(String, u32)> {
    alerts
        .iter()
        .filter_map(|a| match a.kind {
            AlertKind::Threshold { percent, .. } => Some((a.meter_key.clone(), percent)),
            AlertKind::Reset => None,
        })
        .collect()
}

fn resets(alerts: &[Alert]) -> Vec<String> {
    alerts
        .iter()
        .filter(|a| a.kind == AlertKind::Reset)
        .map(|a| a.meter_key.clone())
        .collect()
}

fn req(percent: u32) -> Vec<(String, u32)> {
    vec![(keys::REQUESTS_TODAY.to_string(), percent)]
}

fn settings(edits: Vec<SettingsEdit>) -> Settings {
    let mut s = Settings::default();
    for e in edits {
        s.apply(e);
    }
    s
}

fn request_thresholds(t: Vec<u32>) -> SettingsEdit {
    SettingsEdit::Thresholds {
        assistant: ID.into(),
        account: ACCOUNT.into(),
        meter: keys::REQUESTS_TODAY.into(),
        thresholds: t,
    }
}

#[test]
fn each_threshold_fires_once_as_usage_climbs() {
    let mut h = Harness::new(&Settings::default());
    assert!(h.requests_at(60.0).is_empty());
    assert_eq!(thresholds(&h.requests_at(76.0)), req(75));
    assert!(h.requests_at(80.0).is_empty());
    assert!(h.run_for(HOUR).is_empty());
    assert_eq!(thresholds(&h.requests_at(92.0)), req(90));
    assert_eq!(thresholds(&h.requests_at(100.0)), req(100));
    assert!(h.requests_at(104.0).is_empty());
}

#[test]
fn a_jump_past_several_thresholds_raises_one_alert_for_the_highest() {
    let mut h = Harness::new(&Settings::default());
    h.requests_at(50.0);
    assert_eq!(thresholds(&h.requests_at(95.0)), req(90));
    assert_eq!(thresholds(&h.requests_at(100.0)), req(100));
}

#[test]
fn an_alert_names_the_assistant_account_meter_and_window() {
    let mut h = Harness::new(&Settings::default());
    let alerts = h.requests_at(91.0);
    assert_eq!(alerts.len(), 1);
    let a = &alerts[0];
    assert_eq!(a.assistant_id, ID);
    assert_eq!(a.assistant_name, "Fake Assistant");
    assert_eq!(a.account_id, ACCOUNT);
    assert_eq!(a.account_name, "Demo account");
    assert_eq!(a.meter_label, "Requests today");
    assert_eq!(
        a.window_end,
        Some(Timestamp::from_utc(2026, 10, 30, 0, 0, 0))
    );
    match a.kind {
        AlertKind::Threshold {
            percent,
            used_fraction,
        } => {
            assert_eq!(percent, 90);
            assert!((used_fraction - 0.91).abs() < 1e-9);
        }
        AlertKind::Reset => panic!("expected a threshold alert"),
    }
}

#[test]
fn meters_that_are_not_featured_alert_too() {
    let s = settings(vec![SettingsEdit::Featured {
        assistant: ID.into(),
        account: ACCOUNT.into(),
        meter: keys::REQUESTS_TODAY.into(),
        on: false,
        current: vec![keys::REQUESTS_TODAY.into()],
    }]);
    let mut h = Harness::new(&s);
    assert_eq!(h.engine.state().assistants[0].accounts[0].featured.len(), 0);
    assert_eq!(thresholds(&h.requests_at(80.0)), req(75));
}

#[test]
fn the_monthly_spend_alerts_in_its_own_unit() {
    let mut h = Harness::new(&Settings::default());
    h.control.set_used(keys::SPEND_MONTH, Some(45.5));
    let alerts = h.run_for(5 * MIN);
    assert_eq!(
        thresholds(&alerts),
        vec![(keys::SPEND_MONTH.to_string(), 90)]
    );
}

#[test]
fn per_meter_thresholds_replace_the_defaults() {
    let mut h = Harness::new(&settings(vec![request_thresholds(vec![50, 85])]));
    assert_eq!(thresholds(&h.requests_at(55.0)), req(50));
    assert!(h.requests_at(80.0).is_empty());
    assert_eq!(thresholds(&h.requests_at(86.0)), req(85));
    assert!(h.requests_at(100.0).is_empty());
}

#[test]
fn a_meter_with_alerts_off_never_alerts() {
    let mut h = Harness::new(&settings(vec![request_thresholds(vec![])]));
    assert!(h.requests_at(100.0).is_empty());
    assert!(h.past_midnight().is_empty());
}

#[test]
fn a_new_window_rearms_the_thresholds_and_reports_the_reset() {
    let mut h = Harness::new(&Settings::default());
    assert_eq!(thresholds(&h.requests_at(80.0)), req(75));

    h.control.set_used(keys::REQUESTS_TODAY, Some(LIMIT * 0.02));
    let alerts = h.past_midnight();
    assert_eq!(resets(&alerts), vec![keys::REQUESTS_TODAY.to_string()]);
    assert!(thresholds(&alerts).is_empty());

    assert!(h.run_for(HOUR).is_empty());
    assert_eq!(thresholds(&h.requests_at(80.0)), req(75));
}

#[test]
fn a_window_that_never_crossed_the_first_threshold_resets_silently() {
    let mut h = Harness::new(&Settings::default());
    assert!(h.requests_at(70.0).is_empty());
    h.control.set_used(keys::REQUESTS_TODAY, Some(LIMIT * 0.02));
    assert!(h.past_midnight().is_empty());
}

#[test]
fn reset_alerts_can_be_switched_off() {
    let mut h = Harness::new(&settings(vec![SettingsEdit::ResetNotifications(false)]));
    assert_eq!(thresholds(&h.requests_at(80.0)), req(75));
    h.control.set_used(keys::REQUESTS_TODAY, Some(LIMIT * 0.02));
    assert!(h.past_midnight().is_empty());
    assert_eq!(thresholds(&h.requests_at(80.0)), req(75));
}

#[test]
fn a_reset_with_no_new_reading_is_reported_once() {
    let mut h = Harness::new(&Settings::default());
    assert_eq!(thresholds(&h.requests_at(95.0)), req(90));

    h.control.fail(sources::API, Some(FetchError::Offline));
    h.control.fail(sources::LOCAL, Some(FetchError::Offline));
    let alerts = h.past_midnight();
    assert_eq!(resets(&alerts), vec![keys::REQUESTS_TODAY.to_string()]);
    assert!(h.run_for(HOUR).is_empty());

    h.control.fail(sources::API, None);
    h.control.fail(sources::LOCAL, None);
    h.control.set_used(keys::REQUESTS_TODAY, Some(LIMIT * 0.02));
    assert!(h.run_for(HOUR).is_empty());
    assert_eq!(thresholds(&h.requests_at(80.0)), req(75));
}

#[test]
fn stale_numbers_do_not_alert_until_they_are_fresh_again() {
    let mut h = Harness::new(&settings(vec![request_thresholds(vec![90])]));
    assert!(h.requests_at(80.0).is_empty());
    h.control.fail(sources::API, Some(FetchError::Offline));
    h.control.fail(sources::LOCAL, Some(FetchError::Offline));
    h.run_for(HOUR);

    h.engine
        .apply_settings(&settings(vec![request_thresholds(vec![75])]));
    assert!(h.run_for(HOUR).is_empty());

    h.control.fail(sources::API, None);
    h.control.fail(sources::LOCAL, None);
    assert_eq!(thresholds(&h.run_for(HOUR)), req(75));
}
