//! The scheduler thread: owns the engine, takes commands, publishes state.

use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use uqt_assistant_fake::{FakeAssistant, FakeControl, sources};
use uqt_core::engine::{Engine, RefreshReason};
use uqt_core::runner::EngineThread;
use uqt_core::testing::RecordingTransport;
use uqt_core::time::{FakeClock, Timestamp};

#[test]
fn publishes_after_start_and_after_each_command_then_shuts_down() {
    let clock = FakeClock::new(Timestamp::from_utc(2026, 10, 29, 12, 0, 0));
    let control = FakeControl::new();
    let engine = Engine::new(
        vec![Box::new(FakeAssistant::with_control(control.clone()))],
        Arc::new(clock.clone()),
        Arc::new(RecordingTransport::new()),
    );
    let (tx, rx) = mpsc::channel();
    let thread = EngineThread::spawn(engine, Arc::new(clock.clone()), move |state| {
        let _ = tx.send(state);
    });
    let wait = Duration::from_secs(5);

    let first = rx.recv_timeout(wait).expect("initial state");
    assert_eq!(first.assistants[0].accounts[0].meters.len(), 2);
    assert_eq!(control.fetch_count(sources::API), 1);

    clock.advance(10);
    thread.handle().refresh(RefreshReason::Manual);
    rx.recv_timeout(wait).expect("state after refresh");
    assert_eq!(control.fetch_count(sources::API), 2);

    thread.shutdown();
}
