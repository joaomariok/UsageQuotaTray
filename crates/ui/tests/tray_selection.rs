//! Presentation seam: which Account the single tray icon shows, and the dot for
//! non-featured Meters at 90% or more.

use std::time::Duration;

use uqt_core::engine::{AccountState, AssistantState, EngineState, Freshness, MeterState, Pace};
use uqt_core::model::{BadgeSpec, Exactness, Unit, Window};
use uqt_core::time::Timestamp;
use uqt_ui::present::{self, Moment};

const BADGE_A: BadgeSpec = BadgeSpec {
    svg: "<svg/>",
    letters: "A",
    color: "#111111",
};
const BADGE_B: BadgeSpec = BadgeSpec {
    svg: "<svg/>",
    letters: "B",
    color: "#222222",
};

fn t(hh: u32, mm: u32) -> Timestamp {
    Timestamp::from_utc(2026, 10, 29, hh, mm, 0)
}

fn moment() -> Moment {
    Moment {
        now: t(12, 0),
        utc_offset_secs: 0,
    }
}

fn meter(key: &str, percent: f64) -> MeterState {
    MeterState {
        key: key.into(),
        label: key.into(),
        used: percent,
        limit: Some(100.0),
        unit: Unit::Percent,
        window: Some(Window {
            end: t(17, 0),
            start: None,
            length: Some(Duration::from_secs(5 * 3600)),
        }),
        source_name: "API".into(),
        exactness: Exactness::Exact,
        fetched_at: t(11, 58),
        freshness: Freshness::Fresh,
        pace: Some(Pace::OnPace),
        breakdown: None,
        unrecognized: false,
    }
}

fn with(mut m: MeterState, f: impl FnOnce(&mut MeterState)) -> MeterState {
    f(&mut m);
    m
}

fn account(id: &str, meters: Vec<MeterState>, featured: &[&str]) -> AccountState {
    AccountState {
        id: id.into(),
        display_name: id.into(),
        featured: featured.iter().map(|s| s.to_string()).collect(),
        meters,
        notices: Vec::new(),
        problem: None,
    }
}

fn assistant(id: &'static str, badge: BadgeSpec, accounts: Vec<AccountState>) -> AssistantState {
    AssistantState {
        id,
        display_name: id.into(),
        badge,
        usage_page: None,
        accounts,
    }
}

fn state(assistants: Vec<AssistantState>) -> EngineState {
    EngineState {
        now: t(12, 0),
        assistants,
    }
}

fn rings(s: &EngineState) -> Vec<f32> {
    present::tray(s, moment())
        .rings
        .iter()
        .map(|r| r.fraction)
        .collect()
}

#[test]
fn the_icon_shows_the_account_whose_featured_meters_are_most_constrained() {
    let s = state(vec![assistant(
        "a",
        BADGE_A,
        vec![
            account(
                "calm",
                vec![meter("s", 40.0), meter("w", 50.0)],
                &["s", "w"],
            ),
            account(
                "busy",
                vec![meter("s", 80.0), meter("w", 10.0)],
                &["s", "w"],
            ),
        ],
    )]);
    assert_eq!(rings(&s), vec![0.8, 0.1]);
}

#[test]
fn the_most_constrained_account_brings_its_assistants_badge() {
    let s = state(vec![
        assistant(
            "a",
            BADGE_A,
            vec![account("x", vec![meter("s", 40.0)], &["s"])],
        ),
        assistant(
            "b",
            BADGE_B,
            vec![account("y", vec![meter("s", 70.0)], &["s"])],
        ),
    ]);
    let tray = present::tray(&s, moment());
    assert_eq!(tray.assistant_id, Some("b"));
    assert_eq!(tray.badge, Some(BADGE_B));
}

#[test]
fn a_tie_goes_to_the_meter_projected_to_hit_its_limit_soonest() {
    let later = with(meter("s", 60.0), |m| {
        m.pace = Some(Pace::WillHitLimitAt(t(15, 0)))
    });
    let sooner = with(meter("s", 60.0), |m| {
        m.pace = Some(Pace::WillHitLimitAt(t(13, 0)))
    });
    let s = state(vec![assistant(
        "a",
        BADGE_A,
        vec![
            account("later", vec![later, meter("w", 5.0)], &["s", "w"]),
            account("sooner", vec![sooner, meter("w", 1.0)], &["s", "w"]),
        ],
    )]);
    assert_eq!(rings(&s), vec![0.6, 0.01]);
}

#[test]
fn only_fresh_readings_count_as_constrained() {
    let stale = with(meter("s", 95.0), |m| m.freshness = Freshness::Stale);
    let s = state(vec![assistant(
        "a",
        BADGE_A,
        vec![
            account("stale", vec![stale], &["s"]),
            account("fresh", vec![meter("s", 30.0)], &["s"]),
        ],
    )]);
    assert_eq!(rings(&s), vec![0.3]);
}

#[test]
fn with_nothing_fresh_the_first_account_with_featured_meters_is_shown() {
    let stale = |v| with(meter("s", v), |m| m.freshness = Freshness::Stale);
    let s = state(vec![assistant(
        "a",
        BADGE_A,
        vec![
            account("none", vec![meter("s", 99.0)], &[]),
            account("first", vec![stale(20.0)], &["s"]),
            account("second", vec![stale(70.0)], &["s"]),
        ],
    )]);
    assert_eq!(rings(&s), vec![0.2]);
}

#[test]
fn a_featured_meter_the_account_lacks_is_skipped() {
    let s = state(vec![assistant(
        "a",
        BADGE_A,
        vec![account("x", vec![meter("w", 33.0)], &["gone", "w"])],
    )]);
    assert_eq!(rings(&s), vec![0.33]);
}

#[test]
fn a_dot_appears_when_a_non_featured_meter_reaches_90_percent() {
    let dot = |meters: Vec<MeterState>| {
        let s = state(vec![assistant(
            "a",
            BADGE_A,
            vec![account("x", meters, &["s"])],
        )]);
        present::tray(&s, moment()).dot
    };
    assert!(!dot(vec![meter("s", 20.0), meter("opus", 89.0)]));
    assert!(dot(vec![meter("s", 20.0), meter("opus", 90.0)]));
    assert!(dot(vec![
        meter("s", 20.0),
        with(meter("opus", 96.0), |m| m.freshness = Freshness::Stale)
    ]));
    assert!(!dot(vec![
        meter("s", 20.0),
        with(meter("opus", 96.0), |m| m.freshness = Freshness::PastReset)
    ]));
    // A featured Meter already shows as a red ring.
    assert!(!dot(vec![meter("s", 99.0)]));
}

#[test]
fn the_dot_covers_meters_of_accounts_the_icon_is_not_showing() {
    let s = state(vec![assistant(
        "a",
        BADGE_A,
        vec![
            account("shown", vec![meter("s", 80.0)], &["s"]),
            account("other", vec![meter("s", 10.0), meter("opus", 93.0)], &["s"]),
        ],
    )]);
    let tray = present::tray(&s, moment());
    assert_eq!(tray.rings[0].fraction, 0.8);
    assert!(tray.dot);
}
