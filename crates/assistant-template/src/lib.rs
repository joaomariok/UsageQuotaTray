//! Template for a new Assistant module. Copy this crate, rename `Example` and fill in
//! the marked places (see docs/adding-an-assistant.md). It is a workspace member so it
//! always compiles and its tests run in the gate, but it is NOT registered in the app.
//!
//! It models an imaginary service at `api.example.com` whose usage endpoint returns:
//! `{"meters": {"<key>": {"used", "limit", "resets_at" (epoch seconds), "by_model"?}}}`.

use std::time::Duration;

use serde_json::Value;
use uqt_core::contract::{Assistant, FetchContext, FetchError, FetchOutput, Intervals, Source};
use uqt_core::engine::minutes;
use uqt_core::http::HttpError;
use uqt_core::model::{
    AccountInfo, BadgeSpec, Breakdown, BreakdownPart, Exactness, MeterKey, MeterValue, Unit, Window,
};
use uqt_core::time::Timestamp;

/// Stable id; also the Badge override file name (`<id>.svg`) and the settings key.
pub const ID: &str = "example";
/// The only host this module may call. The HostClient refuses everything else.
pub const API_HOST: &str = "api.example.com";
pub const USAGE_URL: &str = "https://api.example.com/v1/usage";

/// Meter keys the module recognizes.
pub mod keys {
    pub const REQUESTS: &str = "requests";
}

/// Source ids.
pub mod sources {
    pub const USAGE: &str = "example-usage";
}

pub const BADGE_SVG: &str = include_str!("../badge.svg");

pub struct ExampleAssistant;

impl ExampleAssistant {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ExampleAssistant {
    fn default() -> Self {
        Self::new()
    }
}

impl Assistant for ExampleAssistant {
    fn id(&self) -> &'static str {
        ID
    }

    fn display_name(&self) -> &str {
        "Example Assistant"
    }

    fn allowed_hosts(&self) -> &[&'static str] {
        &[API_HOST]
    }

    fn badge(&self) -> BadgeSpec {
        BadgeSpec {
            svg: BADGE_SVG,
            letters: "E",
            color: "#7A5AF8",
        }
    }

    fn suggested_featured(&self) -> Vec<MeterKey> {
        vec![keys::REQUESTS.into()]
    }

    fn usage_page(&self) -> Option<&str> {
        Some("https://example.com/usage")
    }

    /// One Account per login. The id must be the maker's identity for the login and
    /// stable between calls. A real module reads it from the Assistant's local files.
    fn discover_accounts(&mut self) -> Vec<AccountInfo> {
        vec![AccountInfo {
            id: "example-account".into(),
            display_name: "Example account".into(),
        }]
    }

    fn sources(&mut self, _account: &AccountInfo) -> Vec<Box<dyn Source>> {
        vec![Box::new(UsageSource)]
    }
}

/// The usage endpoint (Exact).
struct UsageSource;

impl Source for UsageSource {
    fn id(&self) -> &str {
        sources::USAGE
    }

    fn display_name(&self) -> &str {
        "Example usage API"
    }

    fn exactness(&self) -> Exactness {
        Exactness::Exact
    }

    fn priority(&self) -> u32 {
        0
    }

    fn intervals(&self) -> Intervals {
        Intervals {
            active: minutes(5),
            idle: minutes(15),
        }
    }

    fn fetch(&mut self, ctx: &FetchContext<'_>) -> Result<FetchOutput, FetchError> {
        // A real module adds credentials here, read-only, e.g. ("Authorization", &bearer).
        let response = match ctx.http.get(USAGE_URL, &[("Accept", "application/json")]) {
            Ok(r) => r,
            Err(HttpError::Transport(e)) => {
                log::info!("usage request failed: {e}");
                return Err(FetchError::Offline);
            }
            Err(e) => return Err(FetchError::Other(format!("{e:?}"))),
        };
        match response.status {
            200 => parse(&response.body)
                .map(|values| FetchOutput {
                    values,
                    notices: Vec::new(),
                })
                .ok_or(FetchError::EndpointChanged),
            401 => Err(FetchError::Expired),
            429 => Err(FetchError::RateLimited {
                retry_after: response
                    .header("retry-after")
                    .and_then(|v| v.trim().parse::<u64>().ok())
                    .map(Duration::from_secs),
            }),
            other => Err(FetchError::Other(format!("HTTP {other}"))),
        }
    }
}

/// Maps the response body to Meters; `None` when it does not have the expected shape
/// (which the engine shows as "endpoint changed" instead of wrong numbers).
fn parse(body: &[u8]) -> Option<Vec<MeterValue>> {
    let json: Value = serde_json::from_slice(body).ok()?;
    let meters = json.get("meters")?.as_object()?;
    let mut out = Vec::new();
    for (raw_key, meter) in meters {
        if meter.is_null() {
            continue;
        }
        let used = meter.get("used")?.as_f64()?;
        let window = meter
            .get("resets_at")
            .and_then(Value::as_i64)
            .map(|end| Window {
                end: Timestamp(end),
                start: None,
                length: None,
            });
        let known = raw_key == keys::REQUESTS;
        out.push(MeterValue {
            // Unknown Meters stay visible under their raw key, never featured.
            key: if known {
                raw_key.clone()
            } else {
                format!("unrecognized:{raw_key}")
            },
            label: if known {
                "Requests".into()
            } else {
                raw_key.clone()
            },
            used,
            limit: meter.get("limit").and_then(Value::as_f64),
            unit: Unit::Count("requests".into()),
            window,
            breakdown: breakdown(meter),
            unrecognized: !known,
        });
    }
    Some(out)
}

fn breakdown(meter: &Value) -> Option<Breakdown> {
    let parts: Vec<BreakdownPart> = meter
        .get("by_model")?
        .as_object()?
        .iter()
        .filter_map(|(label, percent)| {
            Some(BreakdownPart {
                label: label.clone(),
                percent: percent.as_f64()?,
            })
        })
        .collect();
    (!parts.is_empty()).then_some(Breakdown { parts })
}
