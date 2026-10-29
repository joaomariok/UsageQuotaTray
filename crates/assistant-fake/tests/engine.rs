//! Engine seam: the real Engine running the real Fake Assistant, with a fake clock and a
//! recording fake transport. Tests advance time, change what the Fake Assistant serves,
//! and assert the published per-Account state.

use std::sync::Arc;
use std::time::Duration;

use uqt_assistant_fake::{FakeAssistant, FakeControl, keys, sources};
use uqt_core::contract::FetchError;
use uqt_core::engine::{Engine, Freshness, MeterState, Pace, Presence, RefreshReason};
use uqt_core::model::{Exactness, Unit};
use uqt_core::testing::RecordingTransport;
use uqt_core::time::{FakeClock, Timestamp};

const MIN: i64 = 60;
const HOUR: i64 = 3600;

struct Harness {
    clock: FakeClock,
    transport: RecordingTransport,
    control: FakeControl,
    engine: Engine,
}

impl Harness {
    /// Starts at 2026-10-29 12:00:00 UTC, user present.
    fn new() -> Self {
        Self::at(Timestamp::from_utc(2026, 10, 29, 12, 0, 0))
    }

    fn at(start: Timestamp) -> Self {
        let clock = FakeClock::new(start);
        let transport = RecordingTransport::new();
        let control = FakeControl::new();
        let engine = Engine::new(
            vec![Box::new(FakeAssistant::with_control(control.clone()))],
            Arc::new(clock.clone()),
            Arc::new(transport.clone()),
        );
        Self {
            clock,
            transport,
            control,
            engine,
        }
    }

    fn meter(&self, key: &str) -> MeterState {
        let state = self.engine.state();
        state.assistants[0].accounts[0]
            .meter(key)
            .unwrap_or_else(|| panic!("meter {key} missing"))
            .clone()
    }

    /// Advances the clock minute by minute, ticking the engine whenever it is due.
    fn run_for(&mut self, secs: i64) {
        let end = self.clock.now().0 + secs;
        while self.clock.now().0 < end {
            self.clock.advance(MIN);
            self.engine.tick();
        }
    }

    fn api_fetches(&self) -> usize {
        self.control.fetch_count(sources::API)
    }
}

#[test]
fn fake_assistant_publishes_a_daily_request_counter_and_a_monthly_spend() {
    let mut h = Harness::new();
    h.engine.tick();

    let state = h.engine.state();
    assert_eq!(state.assistants.len(), 1);
    assert_eq!(state.assistants[0].display_name, "Fake Assistant");
    assert_eq!(state.assistants[0].accounts.len(), 1);

    let requests = h.meter(keys::REQUESTS_TODAY);
    // 12 hours into the day at 18 requests an hour.
    assert_eq!(requests.used, 216.0);
    assert_eq!(requests.limit, Some(500.0));
    assert_eq!(requests.unit, Unit::Count("requests".into()));
    assert_eq!(requests.exactness, Exactness::Exact);
    assert_eq!(requests.source_name, "Fake API");
    assert_eq!(requests.freshness, Freshness::Fresh);
    let window = requests.window.expect("daily window");
    assert_eq!(window.end, Timestamp::from_utc(2026, 10, 30, 0, 0, 0));

    let spend = h.meter(keys::SPEND_MONTH);
    // 28.5 days into October at 1.50 EUR a day.
    assert_eq!(spend.used, 42.75);
    assert_eq!(spend.limit, Some(50.0));
    assert_eq!(spend.unit, Unit::Currency("EUR".into()));
    assert_eq!(
        spend.window.expect("monthly window").end,
        Timestamp::from_utc(2026, 11, 1, 0, 0, 0)
    );
}

#[test]
fn the_fake_assistant_never_touches_the_network() {
    let mut h = Harness::new();
    h.run_for(2 * HOUR);
    assert!(h.transport.requests().is_empty());
}

// ---- Reading selection -------------------------------------------------------------

#[test]
fn a_fresh_exact_reading_beats_a_newer_fresh_estimate() {
    let mut h = Harness::new();
    h.engine.tick();
    // The Estimate Source polls more often, so its Reading becomes the newest.
    h.run_for(3 * MIN);
    let requests = h.meter(keys::REQUESTS_TODAY);
    assert_eq!(requests.exactness, Exactness::Exact);
    assert_eq!(requests.freshness, Freshness::Fresh);
}

#[test]
fn a_fresh_estimate_wins_once_the_exact_reading_goes_stale() {
    let mut h = Harness::new();
    h.engine.tick();
    h.control.fail(sources::API, Some(FetchError::Offline));
    // The API Source is fresh for twice its 5-minute interval.
    h.run_for(11 * MIN);
    let requests = h.meter(keys::REQUESTS_TODAY);
    assert_eq!(requests.exactness, Exactness::Estimate);
    assert_eq!(requests.source_name, "Fake local log");
    assert_eq!(requests.freshness, Freshness::Fresh);
}

#[test]
fn when_every_source_fails_the_newest_reading_stays_but_is_marked_stale() {
    let mut h = Harness::new();
    h.engine.tick();
    let before = h.meter(keys::REQUESTS_TODAY);
    h.control.fail(sources::API, Some(FetchError::Offline));
    h.control
        .fail(sources::LOCAL, Some(FetchError::Other("disk".into())));
    h.run_for(30 * MIN);

    let requests = h.meter(keys::REQUESTS_TODAY);
    assert_eq!(requests.freshness, Freshness::Stale);
    // Never zeroed: the last good value is kept with its fetch time.
    assert_eq!(requests.used, before.used);
    assert!(requests.used > 0.0);
    // The Estimate was fetched in the same first tick; the newest of any kind wins.
    assert_eq!(requests.fetched_at, before.fetched_at);

    let state = h.engine.state();
    let problem = state.assistants[0].accounts[0]
        .problem
        .clone()
        .expect("account reports the failure");
    assert_eq!(problem.error, FetchError::Offline);
}

#[test]
fn a_reading_past_its_window_end_is_reported_as_reset_with_no_new_reading() {
    // 23:50, ten minutes before the daily Window ends.
    let mut h = Harness::at(Timestamp::from_utc(2026, 10, 29, 23, 50, 0));
    h.engine.tick();
    h.control.fail(sources::API, Some(FetchError::Offline));
    h.control.fail(sources::LOCAL, Some(FetchError::Offline));
    h.run_for(15 * MIN);
    let requests = h.meter(keys::REQUESTS_TODAY);
    assert_eq!(requests.freshness, Freshness::PastReset);
    assert_eq!(requests.pace, None);
}

#[test]
fn recovery_replaces_stale_readings_with_fresh_ones() {
    let mut h = Harness::new();
    h.engine.tick();
    h.control.fail(sources::API, Some(FetchError::Offline));
    h.control.fail(sources::LOCAL, Some(FetchError::Offline));
    h.run_for(30 * MIN);
    h.control.fail(sources::API, None);
    h.control.fail(sources::LOCAL, None);
    h.run_for(40 * MIN);
    let requests = h.meter(keys::REQUESTS_TODAY);
    assert_eq!(requests.freshness, Freshness::Fresh);
    assert_eq!(requests.exactness, Exactness::Exact);
    assert!(h.engine.state().assistants[0].accounts[0].problem.is_none());
}

// ---- Pace ------------------------------------------------------------------------

fn pace_at(hh: u32, mm: u32, used: Option<f64>) -> Option<Pace> {
    let mut h = Harness::at(Timestamp::from_utc(2026, 10, 29, hh, mm, 0));
    h.control.set_used(keys::REQUESTS_TODAY, used);
    h.engine.tick();
    h.meter(keys::REQUESTS_TODAY).pace
}

#[test]
fn pace_is_on_pace_when_usage_tracks_elapsed_time() {
    // Half the day gone, 45% of the limit used.
    assert_eq!(pace_at(12, 0, Some(225.0)), Some(Pace::OnPace));
}

#[test]
fn pace_is_ahead_when_usage_is_well_below_elapsed_time() {
    assert_eq!(pace_at(12, 0, Some(100.0)), Some(Pace::Ahead));
}

#[test]
fn pace_projects_the_time_the_limit_will_be_hit() {
    // 300 of 500 after 12 hours: 25 an hour, the limit is hit after 20 hours.
    assert_eq!(
        pace_at(12, 0, Some(300.0)),
        Some(Pace::WillHitLimitAt(Timestamp::from_utc(
            2026, 10, 29, 20, 0, 0
        )))
    );
}

#[test]
fn pace_at_zero_percent_is_ahead() {
    assert_eq!(pace_at(12, 0, Some(0.0)), Some(Pace::Ahead));
}

#[test]
fn pace_at_one_hundred_percent_is_limit_reached() {
    assert_eq!(pace_at(12, 0, Some(500.0)), Some(Pace::LimitReached));
    assert_eq!(pace_at(12, 0, Some(650.0)), Some(Pace::LimitReached));
}

#[test]
fn pace_just_after_a_reset_does_not_extrapolate_from_a_few_minutes() {
    // Two minutes into the day, 10 requests would project to 7200 by midnight.
    assert_eq!(pace_at(0, 2, Some(10.0)), Some(Pace::OnPace));
}

#[test]
fn pace_shows_not_started_when_the_window_has_not_begun() {
    // The Fake Assistant's server clock runs 10 minutes ahead: its new day starts at
    // 23:50 local time, so at 23:55 local the server says the Window began in the future.
    let mut h = Harness::at(Timestamp::from_utc(2026, 10, 29, 23, 45, 0));
    h.control.set_server_skew(10 * MIN);
    h.clock.advance(10 * MIN);
    h.engine.tick();
    // Server time is 00:05 on the 30th; locally it is 23:55 on the 29th.
    assert_eq!(h.meter(keys::REQUESTS_TODAY).pace, Some(Pace::NotStarted));
}

#[test]
fn pace_tolerates_a_server_clock_behind_the_local_clock() {
    // Server 10 minutes behind: the server still reports yesterday's Window at 00:05.
    let mut h = Harness::at(Timestamp::from_utc(2026, 10, 30, 0, 5, 0));
    h.control.set_server_skew(-10 * MIN);
    h.engine.tick();
    let requests = h.meter(keys::REQUESTS_TODAY);
    assert_eq!(requests.freshness, Freshness::PastReset);
    assert_eq!(requests.pace, None);
}

#[test]
fn meters_without_a_window_get_no_pace() {
    let mut h = Harness::new();
    h.control.drop_window(keys::SPEND_MONTH);
    h.engine.tick();
    assert_eq!(h.meter(keys::SPEND_MONTH).pace, None);
}

// ---- Polling ---------------------------------------------------------------------

#[test]
fn polls_on_the_active_interval_while_the_user_is_present() {
    let mut h = Harness::new();
    h.engine.tick();
    h.run_for(30 * MIN);
    // First fetch plus one every 5 minutes.
    assert_eq!(h.api_fetches(), 7);
}

#[test]
fn polls_on_the_idle_interval_when_the_user_is_away() {
    let mut h = Harness::new();
    h.engine.tick();
    h.engine.set_presence(Presence::Idle);
    h.run_for(30 * MIN);
    // One every 15 minutes.
    assert_eq!(h.api_fetches(), 3);
}

#[test]
fn polling_pauses_while_locked_and_refreshes_on_unlock() {
    let mut h = Harness::new();
    h.engine.tick();
    h.engine.set_presence(Presence::Paused);
    assert_eq!(h.engine.next_wake(), None);
    h.run_for(2 * HOUR);
    assert_eq!(h.api_fetches(), 1);

    h.engine.set_presence(Presence::Active);
    h.engine.request_refresh(RefreshReason::Unlock);
    h.engine.tick();
    assert_eq!(h.api_fetches(), 2);
}

#[test]
fn automatic_refreshes_are_debounced_to_once_a_minute() {
    let mut h = Harness::new();
    h.engine.tick();
    h.clock.advance(70);
    h.engine.request_refresh(RefreshReason::PopupOpen);
    h.engine.tick();
    assert_eq!(h.api_fetches(), 2);

    h.clock.advance(20);
    h.engine.request_refresh(RefreshReason::Resume);
    h.engine.tick();
    assert_eq!(h.api_fetches(), 2);

    h.clock.advance(45);
    h.engine.request_refresh(RefreshReason::PopupOpen);
    h.engine.tick();
    assert_eq!(h.api_fetches(), 3);
}

#[test]
fn refresh_now_is_not_debounced() {
    let mut h = Harness::new();
    h.engine.tick();
    h.clock.advance(5);
    h.engine.request_refresh(RefreshReason::Manual);
    h.engine.tick();
    assert_eq!(h.api_fetches(), 2);
}

#[test]
fn errors_back_off_exponentially_up_to_thirty_minutes() {
    let mut h = Harness::new();
    h.control.fail(sources::API, Some(FetchError::Offline));
    h.engine.tick();
    let mut attempts = vec![h.clock.now()];
    let mut seen = 1;
    while attempts.len() < 8 {
        h.clock.advance(MIN);
        h.engine.tick();
        if h.api_fetches() > seen {
            seen = h.api_fetches();
            attempts.push(h.clock.now());
        }
    }
    let gaps: Vec<i64> = attempts
        .windows(2)
        .map(|w| (w[1].0 - w[0].0) / MIN)
        .collect();
    assert_eq!(gaps, vec![1, 2, 4, 8, 16, 30, 30]);
}

#[test]
fn rate_limits_honor_retry_after() {
    let mut h = Harness::new();
    h.control.fail(
        sources::API,
        Some(FetchError::RateLimited {
            retry_after: Some(Duration::from_secs(17 * 60)),
        }),
    );
    h.engine.tick();
    h.control.fail(sources::API, None);
    // Even a manual refresh waits for Retry-After.
    h.clock.advance(MIN);
    h.engine.request_refresh(RefreshReason::Manual);
    h.engine.tick();
    h.run_for(15 * MIN);
    assert_eq!(h.api_fetches(), 1);
    h.run_for(2 * MIN);
    assert_eq!(h.api_fetches(), 2);
}

#[test]
fn rate_limits_without_retry_after_start_at_five_minutes() {
    let mut h = Harness::new();
    h.control.fail(
        sources::API,
        Some(FetchError::RateLimited { retry_after: None }),
    );
    h.engine.tick();
    h.run_for(4 * MIN);
    assert_eq!(h.api_fetches(), 1);
    h.run_for(MIN);
    assert_eq!(h.api_fetches(), 2);
    // Then 10 minutes.
    h.run_for(9 * MIN);
    assert_eq!(h.api_fetches(), 2);
    h.run_for(MIN);
    assert_eq!(h.api_fetches(), 3);
}

#[test]
fn next_wake_points_at_the_earliest_due_source() {
    let mut h = Harness::new();
    h.engine.tick();
    let start = h.clock.now();
    // The local log Source polls every 2 minutes while active.
    assert_eq!(h.engine.next_wake(), Some(Timestamp(start.0 + 2 * MIN)));
}
