//! The scheduler thread: owns the engine, takes commands, publishes state.

use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use uqt_assistant_fake::{FakeAssistant, FakeControl, ID, keys, sources};
use uqt_core::engine::{AlertKind, Engine, RefreshReason};
use uqt_core::runner::EngineThread;
use uqt_core::settings::{Settings, SettingsEdit};
use uqt_core::testing::RecordingTransport;
use uqt_core::time::{FakeClock, Timestamp};

fn engine(clock: &FakeClock, control: &FakeControl) -> Engine {
    Engine::new(
        vec![Box::new(FakeAssistant::with_control(control.clone()))],
        Arc::new(clock.clone()),
        Arc::new(RecordingTransport::new()),
    )
}

#[test]
fn publishes_after_start_and_after_each_command_then_shuts_down() {
    let clock = FakeClock::new(Timestamp::from_utc(2026, 10, 29, 12, 0, 0));
    let control = FakeControl::new();
    let (tx, rx) = mpsc::channel();
    let thread = EngineThread::spawn(
        engine(&clock, &control),
        Arc::new(clock.clone()),
        move |update| {
            let _ = tx.send(update);
        },
    );
    let wait = Duration::from_secs(5);

    let first = rx.recv_timeout(wait).expect("initial state");
    assert_eq!(first.state.assistants[0].accounts[0].meters.len(), 2);
    assert_eq!(first.catalog.assistants[0].id, ID);
    assert_eq!(control.fetch_count(sources::API), 1);

    clock.advance(10);
    thread.handle().refresh(RefreshReason::Manual);
    rx.recv_timeout(wait).expect("state after refresh");
    assert_eq!(control.fetch_count(sources::API), 2);

    thread.shutdown();
}

#[test]
fn alerts_are_published_once_and_settings_reach_the_engine() {
    let clock = FakeClock::new(Timestamp::from_utc(2026, 10, 29, 12, 0, 0));
    let control = FakeControl::new();
    control.set_used(keys::SPEND_MONTH, Some(0.0));
    control.set_used(keys::REQUESTS_TODAY, Some(480.0));
    let (tx, rx) = mpsc::channel();
    let thread = EngineThread::spawn(
        engine(&clock, &control),
        Arc::new(clock.clone()),
        move |update| {
            let _ = tx.send(update);
        },
    );
    let wait = Duration::from_secs(5);

    let first = rx.recv_timeout(wait).expect("initial state");
    assert_eq!(first.alerts.len(), 1);
    assert!(matches!(
        first.alerts[0].kind,
        AlertKind::Threshold { percent: 90, .. }
    ));

    let mut off = Settings::default();
    off.apply(SettingsEdit::AssistantEnabled {
        assistant: ID.into(),
        on: false,
    });
    thread.handle().apply_settings(off);
    let next = rx.recv_timeout(wait).expect("state after settings");
    assert!(next.alerts.is_empty());
    assert!(next.state.assistants.is_empty());
    assert!(!next.catalog.assistants[0].enabled);

    thread.shutdown();
}
