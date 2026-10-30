//! Presentation seam: engine state in, what the icon, tooltip and popup show out.
//! No Slint, no Win32.

use std::time::Duration;

use uqt_core::contract::FetchError;
use uqt_core::engine::{
    AccountState, AssistantState, EngineState, Freshness, MeterState, Pace, SourceProblem,
};
use uqt_core::model::{BadgeSpec, Breakdown, BreakdownPart, Exactness, Notice, Unit, Window};
use uqt_core::time::Timestamp;
use uqt_ui::present::{self, Moment, Tone};

const BADGE: BadgeSpec = BadgeSpec {
    svg: "<svg/>",
    letters: "T",
    color: "#123456",
};

fn t(hh: u32, mm: u32) -> Timestamp {
    Timestamp::from_utc(2026, 10, 29, hh, mm, 0)
}

/// 12:00 UTC, shown in UTC+1.
fn moment() -> Moment {
    Moment {
        now: t(12, 0),
        utc_offset_secs: 3600,
    }
}

fn meter(key: &str, used: f64, limit: Option<f64>, unit: Unit) -> MeterState {
    MeterState {
        key: key.into(),
        label: key.into(),
        used,
        limit,
        unit,
        window: Some(Window {
            end: t(14, 5),
            start: None,
            length: Some(Duration::from_secs(5 * 3600)),
        }),
        source_name: "Test API".into(),
        exactness: Exactness::Exact,
        fetched_at: t(11, 57),
        freshness: Freshness::Fresh,
        pace: Some(Pace::OnPace),
        breakdown: None,
        unrecognized: false,
    }
}

fn state(meters: Vec<MeterState>, featured: &[&str]) -> EngineState {
    EngineState {
        now: t(12, 0),
        assistants: vec![AssistantState {
            id: "test",
            display_name: "Test Assistant".into(),
            badge: BADGE,
            usage_page: None,
            accounts: vec![AccountState {
                id: "acc".into(),
                display_name: "Work".into(),
                featured: featured.iter().map(|s| s.to_string()).collect(),
                meters,
                notices: Vec::new(),
                problem: None,
            }],
        }],
    }
}

fn popup_meter(s: &EngineState, i: usize) -> present::MeterRow {
    present::popup(s, moment()).sections[0].accounts[0].meters[i].clone()
}

// ---- Tray icon ------------------------------------------------------------------

#[test]
fn icon_shows_the_featured_meters_as_rings_with_color_bands() {
    let s = state(
        vec![
            meter("session", 42.0, Some(100.0), Unit::Percent),
            meter("weekly", 80.0, Some(100.0), Unit::Percent),
            meter("opus", 95.0, Some(100.0), Unit::Percent),
        ],
        &["session", "opus"],
    );
    let tray = present::tray(&s, moment());
    let rings: Vec<(f32, Tone)> = tray.rings.iter().map(|r| (r.fraction, r.tone)).collect();
    assert_eq!(rings, vec![(0.42, Tone::Green), (0.95, Tone::Red)]);
    assert_eq!(tray.assistant_id, Some("test"));
    assert_eq!(tray.badge, Some(BADGE));
}

#[test]
fn color_bands_switch_at_75_and_90_percent() {
    let tone = |used: f64| {
        let s = state(vec![meter("m", used, Some(100.0), Unit::Percent)], &["m"]);
        present::tray(&s, moment()).rings[0].tone
    };
    assert_eq!(tone(74.9), Tone::Green);
    assert_eq!(tone(75.0), Tone::Amber);
    assert_eq!(tone(89.9), Tone::Amber);
    assert_eq!(tone(90.0), Tone::Red);
    assert_eq!(tone(130.0), Tone::Red);
}

#[test]
fn stale_rings_are_grey_but_keep_their_value_and_reset_rings_are_empty() {
    let mut stale = meter("a", 60.0, Some(100.0), Unit::Percent);
    stale.freshness = Freshness::Stale;
    let mut reset = meter("b", 60.0, Some(100.0), Unit::Percent);
    reset.freshness = Freshness::PastReset;
    let s = state(vec![stale, reset], &["a", "b"]);
    let tray = present::tray(&s, moment());
    assert_eq!(
        (tray.rings[0].fraction, tray.rings[0].tone),
        (0.6, Tone::Grey)
    );
    assert_eq!(
        (tray.rings[1].fraction, tray.rings[1].tone),
        (0.0, Tone::Grey)
    );
}

#[test]
fn a_featured_meter_without_a_limit_keeps_its_place_as_an_empty_grey_ring() {
    let s = state(
        vec![
            meter("credits", 12.5, None, Unit::Currency("BRL".into())),
            meter("session", 42.0, Some(100.0), Unit::Percent),
        ],
        &["credits", "session"],
    );
    let rings: Vec<(f32, Tone)> = present::tray(&s, moment())
        .rings
        .iter()
        .map(|r| (r.fraction, r.tone))
        .collect();
    assert_eq!(rings, vec![(0.0, Tone::Grey), (0.42, Tone::Green)]);
}

#[test]
fn tooltip_lists_the_meters_with_percentage_and_reset() {
    let mut stale = meter("weekly", 71.0, Some(100.0), Unit::Percent);
    stale.freshness = Freshness::Stale;
    let s = state(
        vec![meter("session", 42.0, Some(100.0), Unit::Percent), stale],
        &["session", "weekly"],
    );
    assert_eq!(
        present::tray(&s, moment()).tooltip,
        "Test Assistant \u{b7} Work\nsession 42% \u{b7} resets in 2 h 5 min\nweekly 71% (as of 12:57)"
    );
}

#[test]
fn tooltip_lists_only_the_featured_meters_in_ring_order() {
    let s = state(
        vec![
            meter("session", 42.0, Some(100.0), Unit::Percent),
            meter("weekly", 22.0, Some(100.0), Unit::Percent),
            meter("opus", 0.0, Some(100.0), Unit::Percent),
        ],
        &["weekly", "session"],
    );
    let tooltip = present::tray(&s, moment()).tooltip;
    assert_eq!(
        tooltip,
        "Test Assistant \u{b7} Work\nweekly 22% \u{b7} resets in 2 h 5 min\nsession 42% \u{b7} resets in 2 h 5 min"
    );
    assert!(!tooltip.contains("opus"), "{tooltip}");
}

#[test]
fn tooltip_never_exceeds_the_windows_limit() {
    let meters = (0..20)
        .map(|i| {
            meter(
                &format!("meter number {i}"),
                10.0,
                Some(100.0),
                Unit::Percent,
            )
        })
        .collect();
    let keys: Vec<String> = (0..20).map(|i| format!("meter number {i}")).collect();
    let featured: Vec<&str> = keys.iter().map(String::as_str).collect();
    let tip = present::tray(&state(meters, &featured), moment()).tooltip;
    assert!(tip.encode_utf16().count() <= 127, "{}", tip.len());
    assert!(tip.ends_with('\u{2026}'));
}

#[test]
fn with_nothing_to_show_the_tooltip_says_so() {
    let s = EngineState {
        now: t(12, 0),
        assistants: Vec::new(),
    };
    let tray = present::tray(&s, moment());
    assert!(tray.rings.is_empty());
    assert_eq!(tray.badge, None);
    assert_eq!(tray.tooltip, "UsageQuotaTray: no Accounts found");
}

// ---- Popup ----------------------------------------------------------------------

#[test]
fn popup_groups_meters_by_assistant_then_account() {
    let s = state(
        vec![meter("session", 42.0, Some(100.0), Unit::Percent)],
        &[],
    );
    let view = present::popup(&s, moment());
    assert_eq!(view.sections.len(), 1);
    assert_eq!(view.sections[0].title, "Test Assistant");
    assert_eq!(view.sections[0].accounts[0].title, "Work");
}

#[test]
fn a_percent_meter_shows_a_bar_countdown_source_freshness_and_exactness() {
    let row = popup_meter(
        &state(
            vec![meter("session", 42.0, Some(100.0), Unit::Percent)],
            &[],
        ),
        0,
    );
    assert_eq!(row.amount, "42%");
    assert_eq!(row.limit, None);
    assert_eq!(row.fraction, Some(0.42));
    assert_eq!(row.reset.as_deref(), Some("resets in 2 h 5 min"));
    assert_eq!(row.source, "Test API");
    assert_eq!(row.updated, "updated 3 min ago");
    assert_eq!(row.exactness, "Exact");
    assert_eq!(row.pace.as_deref(), Some("on pace"));
    assert!(!row.stale);
}

#[test]
fn count_and_currency_meters_show_amount_and_limit() {
    let mut spend = meter("spend", 42.75, Some(50.0), Unit::Currency("EUR".into()));
    spend.exactness = Exactness::Estimate;
    let s = state(
        vec![
            meter(
                "requests",
                216.0,
                Some(500.0),
                Unit::Count("requests".into()),
            ),
            spend,
        ],
        &[],
    );
    let requests = popup_meter(&s, 0);
    assert_eq!(requests.amount, "216 requests");
    assert_eq!(requests.limit.as_deref(), Some("of 500 requests"));
    let spend = popup_meter(&s, 1);
    assert_eq!(spend.amount, "42.75 EUR");
    assert_eq!(spend.limit.as_deref(), Some("of 50.00 EUR"));
    assert_eq!(spend.exactness, "Estimate");
}

#[test]
fn a_meter_without_limit_or_window_shows_only_its_amount() {
    let mut m = meter("tokens", 1234.0, None, Unit::Count("tokens".into()));
    m.window = None;
    m.pace = None;
    let row = popup_meter(&state(vec![m], &[]), 0);
    assert_eq!(row.amount, "1234 tokens");
    assert_eq!(
        (row.limit, row.fraction, row.reset, row.pace),
        (None, None, None, None)
    );
}

#[test]
fn stale_meters_are_greyed_with_as_of_and_never_zero() {
    let mut m = meter("session", 42.0, Some(100.0), Unit::Percent);
    m.freshness = Freshness::Stale;
    m.pace = None;
    let row = popup_meter(&state(vec![m], &[]), 0);
    assert!(row.stale);
    assert_eq!(row.tone, Tone::Grey);
    assert_eq!(row.amount, "42%");
    assert_eq!(row.fraction, Some(0.42));
    assert_eq!(row.updated, "as of 12:57");
}

#[test]
fn a_meter_past_its_reset_shows_reset_at_with_no_new_reading() {
    let mut m = meter("session", 42.0, Some(100.0), Unit::Percent);
    m.freshness = Freshness::PastReset;
    m.window = Some(Window {
        end: t(11, 30),
        start: None,
        length: None,
    });
    m.pace = None;
    let row = popup_meter(&state(vec![m], &[]), 0);
    assert_eq!(row.reset.as_deref(), Some("reset at 12:30, no new reading"));
    assert_eq!(row.amount, "\u{2014}");
    assert_eq!(row.fraction, None);
    assert!(row.stale);
}

#[test]
fn pace_texts() {
    let text = |pace: Pace| {
        let mut m = meter("m", 10.0, Some(100.0), Unit::Percent);
        m.pace = Some(pace);
        popup_meter(&state(vec![m], &[]), 0).pace.unwrap()
    };
    assert_eq!(text(Pace::OnPace), "on pace");
    assert_eq!(text(Pace::Ahead), "ahead");
    assert_eq!(text(Pace::NotStarted), "not started");
    assert_eq!(text(Pace::LimitReached), "limit reached");
    assert_eq!(
        text(Pace::WillHitLimitAt(t(15, 40))),
        "will hit the limit at 16:40"
    );
    // On another local day the date is included.
    assert_eq!(
        text(Pace::WillHitLimitAt(Timestamp::from_utc(
            2026, 10, 30, 23, 30, 0
        ))),
        "will hit the limit at 31 Oct 00:30"
    );
}

#[test]
fn countdowns_and_ages_scale_their_units() {
    let mut m = meter("m", 10.0, Some(100.0), Unit::Percent);
    m.window = Some(Window {
        end: Timestamp::from_utc(2026, 11, 1, 0, 0, 0),
        start: None,
        length: None,
    });
    m.fetched_at = t(12, 0);
    let row = popup_meter(&state(vec![m.clone()], &[]), 0);
    assert_eq!(row.reset.as_deref(), Some("resets in 2 d 12 h"));
    assert_eq!(row.updated, "updated just now");

    m.fetched_at = t(9, 30);
    assert_eq!(
        popup_meter(&state(vec![m], &[]), 0).updated,
        "updated 2 h 30 min ago"
    );
}

#[test]
fn account_problems_and_notices_are_shown() {
    let mut s = state(vec![meter("m", 10.0, Some(100.0), Unit::Percent)], &[]);
    let acc = &mut s.assistants[0].accounts[0];
    acc.problem = Some(SourceProblem {
        source_name: "Test API".into(),
        error: FetchError::Offline,
        hint: None,
        since: t(11, 40),
    });
    acc.notices.push(Notice {
        text: "Shared with chat".into(),
    });
    let view = present::popup(&s, moment());
    let acc = &view.sections[0].accounts[0];
    assert_eq!(
        acc.problem.as_deref(),
        Some("Test API: offline since 12:40; showing the last good numbers")
    );
    assert_eq!(acc.notices, vec!["Shared with chat".to_string()]);
}

#[test]
fn every_failure_kind_has_a_message() {
    let message = |error: FetchError| {
        let mut s = state(Vec::new(), &[]);
        s.assistants[0].accounts[0].problem = Some(SourceProblem {
            source_name: "X".into(),
            error,
            hint: None,
            since: t(12, 0),
        });
        present::popup(&s, moment()).sections[0].accounts[0]
            .problem
            .clone()
            .unwrap()
    };
    assert_eq!(
        message(FetchError::NotLoggedIn),
        "X: not logged in since 13:00"
    );
    assert_eq!(message(FetchError::Expired), "X: login expired since 13:00");
    assert_eq!(
        message(FetchError::RateLimited { retry_after: None }),
        "X: rate limited since 13:00"
    );
    assert_eq!(
        message(FetchError::EndpointChanged),
        "X: the usage endpoint changed since 13:00"
    );
    assert_eq!(
        message(FetchError::Other("boom".into())),
        "X: error (boom) since 13:00"
    );
}

#[test]
fn a_module_hint_tells_the_user_what_to_do() {
    let mut s = state(Vec::new(), &[]);
    s.assistants[0].accounts[0].problem = Some(SourceProblem {
        source_name: "Claude usage".into(),
        error: FetchError::Expired,
        hint: Some("run Claude Code once".into()),
        since: t(12, 0),
    });
    assert_eq!(
        present::popup(&s, moment()).sections[0].accounts[0]
            .problem
            .as_deref(),
        Some("Claude usage: login expired since 13:00; run Claude Code once")
    );
    assert!(
        present::tray(&s, moment())
            .tooltip
            .contains("login expired: run Claude Code once")
    );

    s.assistants[0].accounts[0].problem = Some(SourceProblem {
        source_name: "Claude usage".into(),
        error: FetchError::NotInstalled,
        hint: None,
        since: t(12, 0),
    });
    assert_eq!(
        present::popup(&s, moment()).sections[0].accounts[0]
            .problem
            .as_deref(),
        Some("Claude usage: not installed since 13:00")
    );
}

// ---- Usage page -------------------------------------------------------------------

#[test]
fn the_usage_page_is_the_one_of_the_assistant_on_the_icon() {
    let mut s = state(vec![meter("m", 10.0, Some(100.0), Unit::Percent)], &["m"]);
    assert_eq!(present::usage_page(&s, moment()), None);

    let mut other = s.assistants[0].clone();
    other.id = "other";
    other.usage_page = Some("https://other.example/usage".into());
    other.accounts[0].featured.clear();
    s.assistants[0].usage_page = Some("https://test.example/usage".into());
    s.assistants.insert(0, other);
    // The icon shows "test", the only Account with Featured Meters.
    assert_eq!(
        present::usage_page(&s, moment()).as_deref(),
        Some("https://test.example/usage")
    );

    // Without an Account on the icon, the first Assistant with a page.
    s.assistants[1].accounts[0].featured.clear();
    assert_eq!(
        present::usage_page(&s, moment()).as_deref(),
        Some("https://other.example/usage")
    );

    // Only https pages are ever opened.
    s.assistants[0].usage_page = Some("file:///C:/Windows/System32/calc.exe".into());
    assert_eq!(
        present::usage_page(&s, moment()).as_deref(),
        Some("https://test.example/usage")
    );
}

// ---- Breakdown and Unrecognized ---------------------------------------------------

#[test]
fn a_meter_with_a_breakdown_shows_the_split_in_the_popup() {
    let mut weekly = meter("weekly", 14.0, Some(100.0), Unit::Percent);
    let part = |label: &str, percent: f64| BreakdownPart {
        label: label.into(),
        percent,
    };
    weekly.breakdown = Some(Breakdown {
        parts: vec![
            part("Claude Code", 13.0),
            part("Chats", 4.4),
            part("Cowork", 82.6),
        ],
    });
    let s = state(
        vec![weekly, meter("session", 5.0, Some(100.0), Unit::Percent)],
        &[],
    );
    assert_eq!(
        popup_meter(&s, 0).breakdown.as_deref(),
        Some("Claude Code 13% \u{b7} Chats 4% \u{b7} Cowork 83%")
    );
    assert_eq!(popup_meter(&s, 1).breakdown, None);
}

#[test]
fn unrecognized_meters_go_in_their_own_group_and_stay_out_of_the_tooltip() {
    let mut odd = meter("unrecognized:copper_kite", 30.0, Some(100.0), Unit::Percent);
    odd.label = "copper_kite".into();
    odd.unrecognized = true;
    let s = state(
        vec![meter("session", 5.0, Some(100.0), Unit::Percent), odd],
        &["session"],
    );
    let account = &present::popup(&s, moment()).sections[0].accounts[0];
    let labels = |rows: &[present::MeterRow]| -> Vec<String> {
        rows.iter().map(|r| r.label.clone()).collect()
    };
    assert_eq!(labels(&account.meters), vec!["session"]);
    assert_eq!(labels(&account.unrecognized), vec!["copper_kite"]);
    assert_eq!(account.unrecognized[0].amount, "30%");

    let tooltip = present::tray(&s, moment()).tooltip;
    assert!(tooltip.contains("session"), "{tooltip}");
    assert!(!tooltip.contains("copper_kite"), "{tooltip}");
}

#[test]
fn an_unrecognized_meter_at_90_percent_still_shows_the_dot() {
    let mut odd = meter("unrecognized:copper_kite", 91.0, Some(100.0), Unit::Percent);
    odd.unrecognized = true;
    let s = state(
        vec![meter("session", 5.0, Some(100.0), Unit::Percent), odd],
        &["session"],
    );
    assert!(present::tray(&s, moment()).dot);
}
