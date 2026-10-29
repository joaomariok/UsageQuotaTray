//! Pure mapping from engine state to what the tray icon, tooltip and popup show.
//! No Slint and no Win32 here, so all of it is tested directly.

mod settings;

pub use settings::{
    ActionRequest, Input, RowKind, RowTarget, SettingRow, SettingsView, settings_view,
};
use uqt_core::contract::FetchError;
use uqt_core::engine::{
    AccountState, Alert, AlertKind, AssistantState, EngineState, Freshness, MeterState, Pace,
    SourceProblem,
};
use uqt_core::model::{BadgeSpec, Exactness, Unit};
use uqt_core::time::Timestamp;

/// Windows truncates tray tooltips at 127 UTF-16 units.
const TOOLTIP_MAX: usize = 127;
const AMBER_FROM: f64 = 0.75;
const RED_FROM: f64 = 0.90;
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// The time a view is computed at, and the local offset for clock times.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Moment {
    pub now: Timestamp,
    pub utc_offset_secs: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Green,
    Amber,
    Red,
    /// Stale or reset: the value is not current.
    Grey,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ring {
    /// Used over Limit, clamped to 0..=1 when drawn.
    pub fraction: f32,
    pub tone: Tone,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TrayView {
    /// At most two, outer first.
    pub rings: Vec<Ring>,
    pub assistant_id: Option<&'static str>,
    pub badge: Option<BadgeSpec>,
    /// A Meter that is not featured is at 90% or more.
    pub dot: bool,
    pub tooltip: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PopupView {
    pub sections: Vec<AssistantSection>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AssistantSection {
    pub title: String,
    pub accounts: Vec<AccountSection>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AccountSection {
    pub title: String,
    pub problem: Option<String>,
    pub notices: Vec<String>,
    pub meters: Vec<MeterRow>,
    /// Meters the module could not identify, shown collapsed under "Unrecognized".
    pub unrecognized: Vec<MeterRow>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MeterRow {
    pub label: String,
    pub amount: String,
    pub limit: Option<String>,
    /// For a bar; `None` when there is no Limit or no current value.
    pub fraction: Option<f32>,
    pub tone: Tone,
    pub reset: Option<String>,
    pub pace: Option<String>,
    pub source: String,
    pub exactness: &'static str,
    pub updated: String,
    pub stale: bool,
    /// The Breakdown ("Claude Code 13% · Chats 4%"), if the Meter has one.
    pub breakdown: Option<String>,
}

/// How constrained an Account's Featured Meters are: the highest fresh Used/Limit
/// (in tenths of a percent, so near-equal values tie), then the soonest projected
/// exhaustion among the Meters at that highest value. Greater is more constrained.
fn constraint(acc: &AccountState, now: Timestamp) -> Option<(i64, std::cmp::Reverse<i64>)> {
    let tenths = |m: &MeterState| m.fraction().map(|f| (f * 1000.0).round() as i64);
    let fresh: Vec<(i64, &MeterState)> = featured(acc)
        .filter(|m| m.freshness == Freshness::Fresh)
        .filter_map(|m| Some((tenths(m)?, m)))
        .collect();
    let highest = fresh.iter().map(|(t, _)| *t).max()?;
    let soonest = fresh
        .iter()
        .filter(|(t, _)| *t == highest)
        .filter_map(|(_, m)| match m.pace {
            Some(Pace::LimitReached) => Some(now.0),
            Some(Pace::WillHitLimitAt(t)) => Some(t.0),
            _ => None,
        })
        .min()
        .unwrap_or(i64::MAX);
    Some((highest, std::cmp::Reverse(soonest)))
}

/// The icon shows the Featured Meters of the most constrained Account; with nothing
/// fresh anywhere, the first Account that has Featured Meters.
pub fn tray(state: &EngineState, at: Moment) -> TrayView {
    let candidates = || {
        state.assistants.iter().flat_map(|a| {
            a.accounts
                .iter()
                .filter(|acc| featured(acc).next().is_some())
                .map(move |acc| (a, acc))
        })
    };
    let mut shown = None;
    let mut best = None;
    for (a, acc) in candidates() {
        // Strictly greater, so earlier Accounts win exact ties.
        if let Some(c) = constraint(acc, at.now)
            && best.as_ref().is_none_or(|b| c > *b)
        {
            best = Some(c);
            shown = Some((a, acc));
        }
    }
    let shown = shown.or_else(|| candidates().next());
    let rings = shown.map_or_else(Vec::new, |(_, acc)| {
        featured(acc)
            .filter_map(|m| {
                let fraction = m.fraction()?;
                Some(match m.freshness {
                    Freshness::PastReset => Ring {
                        fraction: 0.0,
                        tone: Tone::Grey,
                    },
                    Freshness::Stale => Ring {
                        fraction: fraction as f32,
                        tone: Tone::Grey,
                    },
                    Freshness::Fresh => Ring {
                        fraction: fraction as f32,
                        tone: band(fraction),
                    },
                })
            })
            .take(2)
            .collect()
    });
    let dot = state
        .assistants
        .iter()
        .flat_map(|a| &a.accounts)
        .any(|acc| {
            acc.meters.iter().any(|m| {
                !acc.featured.contains(&m.key)
                    && m.freshness != Freshness::PastReset
                    && m.fraction().is_some_and(|f| f >= RED_FROM)
            })
        });
    TrayView {
        rings,
        assistant_id: shown.map(|(a, _)| a.id),
        badge: shown.map(|(a, _)| a.badge),
        dot,
        tooltip: tooltip(state, at),
    }
}

/// A toast's text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlertMessage {
    pub title: String,
    pub body: String,
}

pub fn alert_message(alert: &Alert, at: Moment) -> AlertMessage {
    let mut body = format!("{} \u{b7} {}", alert.assistant_name, alert.account_name);
    let title = match alert.kind {
        AlertKind::Threshold {
            percent,
            used_fraction,
        } => {
            if let Some(end) = alert.window_end {
                body.push_str(&format!(" \u{b7} {}", countdown(end, at)));
            }
            if percent >= 100 {
                format!("{} limit reached", alert.meter_label)
            } else {
                format!("{} at {}", alert.meter_label, self::percent(used_fraction))
            }
        }
        AlertKind::Reset => format!("{} has reset", alert.meter_label),
    };
    AlertMessage { title, body }
}

/// The toast's XML for the Windows notification API (generic template, no actions).
pub fn toast_xml(m: &AlertMessage) -> String {
    let escape = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&apos;")
    };
    format!(
        "<toast launch=\"open-popup\"><visual><binding template=\"ToastGeneric\">\
         <text>{}</text><text>{}</text></binding></visual></toast>",
        escape(&m.title),
        escape(&m.body)
    )
}

fn featured(acc: &AccountState) -> impl Iterator<Item = &MeterState> {
    acc.featured.iter().filter_map(|k| acc.meter(k))
}

fn band(fraction: f64) -> Tone {
    if fraction >= RED_FROM {
        Tone::Red
    } else if fraction >= AMBER_FROM {
        Tone::Amber
    } else {
        Tone::Green
    }
}

fn tooltip(state: &EngineState, at: Moment) -> String {
    let mut lines = Vec::new();
    for a in &state.assistants {
        for acc in &a.accounts {
            lines.push(format!("{} \u{b7} {}", a.display_name, acc.display_name));
            if acc.meters.is_empty()
                && let Some(p) = &acc.problem
            {
                lines.push(problem_line(p));
            }
            for m in featured(acc).filter(|m| !m.unrecognized) {
                lines.push(tooltip_line(m, at));
            }
        }
    }
    if lines.is_empty() {
        return "UsageQuotaTray: no Accounts found".into();
    }
    truncate_utf16(&lines.join("\n"), TOOLTIP_MAX)
}

fn tooltip_line(m: &MeterState, at: Moment) -> String {
    let value = match m.fraction() {
        Some(f) => percent(f),
        None => amount(m.used, &m.unit),
    };
    match m.freshness {
        Freshness::PastReset => format!(
            "{} {}",
            m.label,
            m.window.map_or_else(String::new, |w| reset_past(w.end, at))
        ),
        Freshness::Stale => format!(
            "{} {value} (as of {})",
            m.label,
            clock_time(m.fetched_at, at)
        ),
        Freshness::Fresh => match m.window {
            Some(w) => format!("{} {value} \u{b7} {}", m.label, countdown(w.end, at)),
            None => format!("{} {value}", m.label),
        },
    }
}

fn truncate_utf16(text: &str, max: usize) -> String {
    if text.encode_utf16().count() <= max {
        return text.to_string();
    }
    let mut out = String::new();
    let mut units = 0;
    for c in text.chars() {
        if units + c.len_utf16() > max - 1 {
            break;
        }
        units += c.len_utf16();
        out.push(c);
    }
    out.push('\u{2026}');
    out
}

pub fn popup(state: &EngineState, at: Moment) -> PopupView {
    PopupView {
        sections: state
            .assistants
            .iter()
            .map(|a| AssistantSection {
                title: a.display_name.clone(),
                accounts: a
                    .accounts
                    .iter()
                    .map(|acc| AccountSection {
                        title: acc.display_name.clone(),
                        problem: acc.problem.as_ref().map(|p| {
                            let mut text = format!(
                                "{}: {} since {}",
                                p.source_name,
                                failure_text(&p.error),
                                clock_time(p.since, at)
                            );
                            if let Some(hint) = &p.hint {
                                text.push_str("; ");
                                text.push_str(hint);
                            }
                            if !acc.meters.is_empty() {
                                text.push_str("; showing the last good numbers");
                            }
                            text
                        }),
                        notices: acc.notices.iter().map(|n| n.text.clone()).collect(),
                        meters: acc
                            .meters
                            .iter()
                            .filter(|m| !m.unrecognized)
                            .map(|m| meter_row(m, at))
                            .collect(),
                        unrecognized: acc
                            .meters
                            .iter()
                            .filter(|m| m.unrecognized)
                            .map(|m| meter_row(m, at))
                            .collect(),
                    })
                    .collect(),
            })
            .collect(),
    }
}

fn meter_row(m: &MeterState, at: Moment) -> MeterRow {
    let past_reset = m.freshness == Freshness::PastReset;
    let stale = m.freshness != Freshness::Fresh;
    let fraction = m.fraction().filter(|_| !past_reset);
    MeterRow {
        label: m.label.clone(),
        amount: if past_reset {
            "\u{2014}".into()
        } else {
            match m.unit {
                Unit::Percent => percent(m.used / 100.0),
                _ => amount(m.used, &m.unit),
            }
        },
        limit: match (&m.unit, m.limit) {
            (Unit::Percent, _) | (_, None) => None,
            (unit, Some(l)) => Some(format!("of {}", amount(l, unit))),
        },
        fraction: fraction.map(|f| f as f32),
        tone: match fraction {
            Some(f) if !stale => band(f),
            Some(_) => Tone::Grey,
            None if stale => Tone::Grey,
            None => Tone::Green,
        },
        reset: m.window.map(|w| {
            if past_reset {
                reset_past(w.end, at)
            } else {
                format!("resets {}", in_span(w.end.0 - at.now.0))
            }
        }),
        pace: m.pace.map(|p| match p {
            Pace::NotStarted => "not started".into(),
            Pace::OnPace => "on pace".into(),
            Pace::Ahead => "ahead".into(),
            Pace::LimitReached => "limit reached".into(),
            Pace::WillHitLimitAt(t) => format!("will hit the limit at {}", clock_time(t, at)),
        }),
        source: m.source_name.clone(),
        exactness: match m.exactness {
            Exactness::Exact => "Exact",
            Exactness::Estimate => "Estimate",
        },
        updated: if stale {
            format!("as of {}", clock_time(m.fetched_at, at))
        } else {
            let age = at.now.0 - m.fetched_at.0;
            if age < 60 {
                "updated just now".into()
            } else {
                format!("updated {} ago", span(age))
            }
        },
        stale,
        breakdown: m.breakdown.as_ref().map(|b| {
            b.parts
                .iter()
                .map(|p| format!("{} {}%", p.label, p.percent.round() as i64))
                .collect::<Vec<_>>()
                .join(" \u{b7} ")
        }),
    }
}

/// The generic failure text plus the module's hint, for the tooltip.
fn problem_line(p: &SourceProblem) -> String {
    match &p.hint {
        Some(hint) => format!("{}: {hint}", failure_text(&p.error)),
        None => failure_text(&p.error),
    }
}

/// The usage page "Open usage page" opens: that of the Assistant on the icon, else the
/// first Assistant with one. Only `https://` pages qualify.
pub fn usage_page(state: &EngineState, at: Moment) -> Option<String> {
    let page = |a: &AssistantState| {
        a.usage_page
            .clone()
            .filter(|p| p.to_ascii_lowercase().starts_with("https://"))
    };
    let shown = tray(state, at).assistant_id;
    state
        .assistants
        .iter()
        .filter(|a| Some(a.id) == shown)
        .find_map(page)
        .or_else(|| state.assistants.iter().find_map(page))
}

fn failure_text(error: &FetchError) -> String {
    match error {
        FetchError::NotInstalled => "not installed".into(),
        FetchError::NotLoggedIn => "not logged in".into(),
        FetchError::Expired => "login expired".into(),
        FetchError::Offline => "offline".into(),
        FetchError::RateLimited { .. } => "rate limited".into(),
        FetchError::EndpointChanged => "the usage endpoint changed".into(),
        FetchError::Other(detail) => format!("error ({detail})"),
    }
}

fn percent(fraction: f64) -> String {
    format!("{}%", (fraction * 100.0).round() as i64)
}

fn amount(value: f64, unit: &Unit) -> String {
    match unit {
        Unit::Percent => format!("{}%", value.round() as i64),
        Unit::Currency(code) => format!("{value:.2} {code}"),
        Unit::Count(name) => {
            if value.fract() == 0.0 {
                format!("{} {name}", value as i64)
            } else {
                format!("{value:.1} {name}")
            }
        }
    }
}

fn countdown(end: Timestamp, at: Moment) -> String {
    format!("resets {}", in_span(end.0 - at.now.0))
}

fn reset_past(end: Timestamp, at: Moment) -> String {
    format!("reset at {}, no new reading", clock_time(end, at))
}

fn in_span(secs: i64) -> String {
    if secs < 60 {
        "in under a minute".into()
    } else {
        format!("in {}", span(secs))
    }
}

/// "3 min", "2 h 5 min", "2 d 12 h".
fn span(secs: i64) -> String {
    let minutes = secs.max(0) / 60;
    let (d, h, m) = (minutes / 1440, minutes % 1440 / 60, minutes % 60);
    if d > 0 {
        format!("{d} d {h} h")
    } else if h > 0 {
        format!("{h} h {m} min")
    } else {
        format!("{m} min")
    }
}

/// Local "HH:MM", with "D Mon " in front when it is not today.
fn clock_time(t: Timestamp, at: Moment) -> String {
    let c = t.civil(at.utc_offset_secs);
    let today = at.now.civil(at.utc_offset_secs);
    let hm = format!("{:02}:{:02}", c.hour, c.minute);
    if (c.year, c.month, c.day) == (today.year, today.month, today.day) {
        hm
    } else {
        format!("{} {} {hm}", c.day, MONTHS[(c.month - 1) as usize])
    }
}
