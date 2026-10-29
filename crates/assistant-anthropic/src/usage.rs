//! Maps the usage endpoint's response onto Meters.
//!
//! The self-describing `limits` list is the primary source; the legacy `five_hour` and
//! `seven_day` objects are the fallback for each Meter. Window lengths are module
//! knowledge (5 hours, 7 days); the Weekly start comes from `seven_day_breakdown`.

use std::time::Duration;

use serde_json::Value;
use uqt_core::model::{MeterValue, Unit, Window};
use uqt_core::time::Timestamp;

use crate::keys;

const SESSION_LENGTH: Duration = Duration::from_secs(5 * 3600);
const WEEK_LENGTH: Duration = Duration::from_secs(7 * 86_400);

/// The Meters in a usage response, or `None` when it has neither the 5-hour nor the
/// Weekly value in any known shape.
pub(crate) fn parse(body: &[u8]) -> Option<Vec<MeterValue>> {
    let json: Value = serde_json::from_slice(body).ok()?;
    if !json.is_object() {
        return None;
    }
    let weekly_start = json["seven_day_breakdown"]["window_started_at"]
        .as_str()
        .and_then(Timestamp::parse_rfc3339);

    let session = from_limits(&json, "session")
        .or_else(|| from_legacy(&json, "five_hour"))
        .map(|(used, end)| MeterValue {
            key: keys::SESSION.into(),
            label: "5-hour".into(),
            used,
            limit: Some(100.0),
            unit: Unit::Percent,
            window: end.map(|end| Window {
                end,
                start: None,
                length: Some(SESSION_LENGTH),
            }),
        });
    let weekly = from_limits(&json, "weekly_all")
        .or_else(|| from_legacy(&json, "seven_day"))
        .map(|(used, end)| MeterValue {
            key: keys::WEEKLY.into(),
            label: "Weekly".into(),
            used,
            limit: Some(100.0),
            unit: Unit::Percent,
            window: end.map(|end| Window {
                end,
                start: weekly_start,
                length: Some(WEEK_LENGTH),
            }),
        });

    let values: Vec<MeterValue> = [session, weekly].into_iter().flatten().collect();
    (!values.is_empty()).then_some(values)
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
