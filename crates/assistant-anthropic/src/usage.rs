//! Maps the usage endpoint's response onto Meters.
//!
//! The self-describing `limits` list is the primary source; the legacy `five_hour` and
//! `seven_day` objects are the fallback for each Meter. Window lengths are module
//! knowledge (5 hours, 7 days); the Weekly start and Breakdown come from
//! `seven_day_breakdown`. Objects under keys this module does not know become
//! Unrecognized Meters when they have a utilization and a reset time.

use std::time::Duration;

use serde_json::Value;
use uqt_core::model::{Breakdown, BreakdownPart, MeterValue, Unit, Window};
use uqt_core::time::Timestamp;

use crate::keys;

const SESSION_LENGTH: Duration = Duration::from_secs(5 * 3600);
const WEEK_LENGTH: Duration = Duration::from_secs(7 * 86_400);

/// Top-level keys this module reads or deliberately ignores. Anything else is
/// unrecognized.
const KNOWN_KEYS: &[&str] = &[
    "five_hour",
    "seven_day",
    "limits",
    "spend",
    "extra_usage",
    "seven_day_breakdown",
    "weekly_scoped_shares",
    "member_dashboard_available",
    "pace",
];

/// Prefix of Unrecognized Meter keys, followed by the raw key.
const UNRECOGNIZED_PREFIX: &str = "unrecognized:";

/// What a usage response says.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Usage {
    pub values: Vec<MeterValue>,
    /// Whether usage credits are enabled, when the response says.
    pub credits_enabled: Option<bool>,
}

/// The usage in a response, or `None` when it has neither the 5-hour nor the Weekly
/// value in any known shape.
pub(crate) fn parse(body: &[u8]) -> Option<Usage> {
    let json: Value = serde_json::from_slice(body).ok()?;
    if !json.is_object() {
        return None;
    }
    let weekly_start = json["seven_day_breakdown"]["window_started_at"]
        .as_str()
        .and_then(Timestamp::parse_rfc3339);

    let session = from_limits(&json, "session")
        .or_else(|| from_legacy(&json, "five_hour"))
        .map(|(used, end)| {
            percent_meter(
                keys::SESSION.into(),
                "5-hour".into(),
                used,
                end,
                SESSION_LENGTH,
            )
        });
    let weekly = from_limits(&json, "weekly_all")
        .or_else(|| from_legacy(&json, "seven_day"))
        .map(|(used, end)| {
            let mut m = percent_meter(keys::WEEKLY.into(), "Weekly".into(), used, end, WEEK_LENGTH);
            if let Some(w) = &mut m.window {
                w.start = weekly_start;
            }
            m.breakdown = breakdown(&json["seven_day_breakdown"]);
            m
        });

    let mut values: Vec<MeterValue> = [session, weekly].into_iter().flatten().collect();
    if values.is_empty() {
        return None;
    }
    values.extend(model_weekly(&json));
    values.extend(usage_credits(&json));
    values.extend(unrecognized(&json));
    log_raw_signals(&json);
    let credits_enabled = json["spend"]["enabled"]
        .as_bool()
        .or_else(|| json["extra_usage"]["is_enabled"].as_bool());
    Some(Usage {
        values,
        credits_enabled,
    })
}

fn percent_meter(
    key: String,
    label: String,
    used: f64,
    end: Option<Timestamp>,
    length: Duration,
) -> MeterValue {
    MeterValue {
        key,
        label,
        used,
        limit: Some(100.0),
        unit: Unit::Percent,
        window: end.map(|end| Window {
            end,
            start: None,
            length: Some(length),
        }),
        breakdown: None,
        unrecognized: false,
    }
}

/// One Meter per `weekly_scoped` limit, labeled with the model's display name.
fn model_weekly(json: &Value) -> Vec<MeterValue> {
    let Some(limits) = json["limits"].as_array() else {
        return Vec::new();
    };
    limits
        .iter()
        .filter(|l| l["kind"].as_str() == Some("weekly_scoped"))
        .filter_map(|l| {
            let name = l["scope"]["model"]["display_name"].as_str()?;
            Some(percent_meter(
                keys::weekly_model(name),
                format!("Weekly \u{b7} {name}"),
                l["percent"].as_f64()?,
                reset(&l["resets_at"]),
                WEEK_LENGTH,
            ))
        })
        .collect()
}

/// Credits used against the monthly spend limit, from the structured `spend` object or,
/// when that is missing or cannot be read, from `extra_usage`. Absent while credits are
/// disabled. No reset date is given, so there is no Window.
fn usage_credits(json: &Value) -> Option<MeterValue> {
    let spend = &json["spend"];
    if spend["enabled"].as_bool() == Some(false) {
        return None;
    }
    let (used, limit, currency) = spend_credits(spend).or_else(|| extra_credits(json))?;
    Some(MeterValue {
        key: keys::USAGE_CREDITS.into(),
        label: "Usage credits".into(),
        used,
        limit,
        unit: Unit::Currency(currency),
        window: None,
        breakdown: None,
        unrecognized: false,
    })
}

/// Used, limit and currency from the structured `spend` object.
fn spend_credits(spend: &Value) -> Option<(f64, Option<f64>, String)> {
    let (used, currency) = money(&spend["used"])?;
    Some((used, money(&spend["limit"]).map(|l| l.0), currency))
}

/// Used, limit and currency from `extra_usage` (amounts scaled by `decimal_places`).
fn extra_credits(json: &Value) -> Option<(f64, Option<f64>, String)> {
    let extra = &json["extra_usage"];
    if extra["is_enabled"].as_bool() != Some(true) {
        return None;
    }
    let scale = 10f64.powi(extra["decimal_places"].as_i64().unwrap_or(2) as i32);
    let currency = extra["currency"].as_str()?.to_string();
    let used = extra["used_credits"].as_f64().unwrap_or(0.0) / scale;
    let limit = extra["monthly_limit"].as_f64().map(|l| l / scale);
    Some((used, limit, currency))
}

/// A structured money value: amount in minor units with its exponent, and currency.
fn money(value: &Value) -> Option<(f64, String)> {
    let minor = value["amount_minor"].as_f64()?;
    let exponent = value["exponent"].as_i64().unwrap_or(0) as i32;
    let currency = value["currency"].as_str()?.to_string();
    Some((minor / 10f64.powi(exponent), currency))
}

/// The per-surface split of the weekly usage (`seven_day_breakdown.rows`).
fn breakdown(value: &Value) -> Option<Breakdown> {
    let parts: Vec<BreakdownPart> = value["rows"]
        .as_array()?
        .iter()
        .filter_map(|row| {
            Some(BreakdownPart {
                label: row["display_name"].as_str()?.to_string(),
                percent: row["percent"].as_f64()?,
            })
        })
        .collect();
    (!parts.is_empty()).then_some(Breakdown { parts })
}

/// Non-null objects under unknown keys with a utilization and a reset time, labeled
/// with their raw key. Null code-named keys are ignored.
fn unrecognized(json: &Value) -> Vec<MeterValue> {
    let Some(map) = json.as_object() else {
        return Vec::new();
    };
    map.iter()
        .filter(|(key, value)| !KNOWN_KEYS.contains(&key.as_str()) && value.is_object())
        .filter_map(|(key, value)| {
            let used = value["utilization"].as_f64()?;
            let end = reset(&value["resets_at"])?;
            Some(MeterValue {
                key: format!("{UNRECOGNIZED_PREFIX}{key}"),
                label: key.clone(),
                used,
                limit: Some(100.0),
                unit: Unit::Percent,
                window: Some(Window {
                    end,
                    start: None,
                    length: None,
                }),
                breakdown: None,
                unrecognized: true,
            })
        })
        .collect()
}

/// `pace`, and each limit's `severity` and `is_active`, are not used yet; they are
/// logged raw so their meaning can be learned. None of them identify the user.
fn log_raw_signals(json: &Value) {
    let limits: Vec<String> = json["limits"]
        .as_array()
        .map(|limits| {
            limits
                .iter()
                .map(|l| {
                    format!(
                        "{}: severity={} is_active={}",
                        l["kind"], l["severity"], l["is_active"]
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    log::info!(
        "usage signals: pace={} limits=[{}]",
        json["pace"],
        limits.join(", ")
    );
}

/// Percent used and reset time of the first `limits` entry of `kind`.
fn from_limits(json: &Value, kind: &str) -> Option<(f64, Option<Timestamp>)> {
    let entry = json["limits"]
        .as_array()?
        .iter()
        .find(|l| l["kind"].as_str() == Some(kind))?;
    Some((entry["percent"].as_f64()?, reset(&entry["resets_at"])))
}

/// Utilization (0-100) and reset time of a legacy object such as `five_hour`.
fn from_legacy(json: &Value, key: &str) -> Option<(f64, Option<Timestamp>)> {
    let entry = &json[key];
    Some((entry["utilization"].as_f64()?, reset(&entry["resets_at"])))
}

fn reset(value: &Value) -> Option<Timestamp> {
    value.as_str().and_then(Timestamp::parse_rfc3339)
}
