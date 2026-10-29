//! The Fake Assistant: generated data with a Meter mix unlike Claude Code's (a daily
//! request counter and a monthly spend in a currency), so the core is exercised with
//! something other than percentages. It makes no network calls.
//!
//! Values are a deterministic function of the "server" time (the local clock plus an
//! adjustable skew). A [`FakeControl`] lets tests and the app inject failures, fixed
//! values, clock skew and missing Windows.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use uqt_core::contract::{
    Assistant, FetchContext, FetchError, FetchOutput, FieldKind, Intervals, SettingField, Source,
};
use uqt_core::engine::minutes;
use uqt_core::model::{AccountInfo, BadgeSpec, Exactness, MeterKey, MeterValue, Unit, Window};
use uqt_core::settings::{ModuleValues, SettingValue};
use uqt_core::time::{Timestamp, days_from_civil};

pub const ID: &str = "fake";

/// Meter keys.
pub mod keys {
    pub const REQUESTS_TODAY: &str = "requests_today";
    pub const SPEND_MONTH: &str = "spend_month";
}

/// Source ids.
pub mod sources {
    /// Exact, every 5 minutes while active.
    pub const API: &str = "fake-api";
    /// Estimate, every 2 minutes while active.
    pub const LOCAL: &str = "fake-local";
}

/// Keys of the module's settings.
pub mod settings_keys {
    /// Text: the demo Account's display name.
    pub const ACCOUNT_NAME: &str = "account_name";
}

const DEFAULT_ACCOUNT_NAME: &str = "Demo account";

pub const BADGE_SVG: &str = include_str!("../badge.svg");

const REQUEST_LIMIT: f64 = 500.0;
const REQUESTS_PER_HOUR: f64 = 18.0;
/// The local log misses a few requests, so its Estimate runs lower.
const ESTIMATED_REQUESTS_PER_HOUR: f64 = 17.0;
const SPEND_LIMIT: f64 = 50.0;
const SPEND_PER_DAY: f64 = 1.5;
const DAY: i64 = 86_400;

#[derive(Default)]
struct Controls {
    failures: HashMap<String, FetchError>,
    used: HashMap<MeterKey, f64>,
    no_window: HashSet<MeterKey>,
    skew_secs: i64,
    fetches: HashMap<String, usize>,
}

/// Shared knobs for the Fake Assistant. Clones control the same instance.
#[derive(Clone, Default)]
pub struct FakeControl(Arc<Mutex<Controls>>);

impl FakeControl {
    pub fn new() -> Self {
        Self::default()
    }

    /// Makes every fetch of `source` fail with `error`, or succeed again with `None`.
    pub fn fail(&self, source: &str, error: Option<FetchError>) {
        let mut c = self.lock();
        match error {
            Some(e) => c.failures.insert(source.into(), e),
            None => c.failures.remove(source),
        };
    }

    /// Serves a fixed Used value for `key` from every Source, or generated data again.
    pub fn set_used(&self, key: &str, used: Option<f64>) {
        let mut c = self.lock();
        match used {
            Some(u) => c.used.insert(key.into(), u),
            None => c.used.remove(key),
        };
    }

    /// Runs the fake server's clock `secs` ahead of (or behind, if negative) the local one.
    pub fn set_server_skew(&self, secs: i64) {
        self.lock().skew_secs = secs;
    }

    /// Serves `key` without a Window.
    pub fn drop_window(&self, key: &str) {
        self.lock().no_window.insert(key.into());
    }

    /// Fetch attempts of `source` so far, failed ones included.
    pub fn fetch_count(&self, source: &str) -> usize {
        self.lock().fetches.get(source).copied().unwrap_or(0)
    }

    fn lock(&self) -> MutexGuard<'_, Controls> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

pub struct FakeAssistant {
    control: FakeControl,
    account_name: String,
}

impl FakeAssistant {
    pub fn new() -> Self {
        Self::with_control(FakeControl::new())
    }

    pub fn with_control(control: FakeControl) -> Self {
        Self {
            control,
            account_name: DEFAULT_ACCOUNT_NAME.into(),
        }
    }
}

impl Default for FakeAssistant {
    fn default() -> Self {
        Self::new()
    }
}

impl Assistant for FakeAssistant {
    fn id(&self) -> &'static str {
        ID
    }

    fn display_name(&self) -> &str {
        "Fake Assistant"
    }

    fn allowed_hosts(&self) -> &[&'static str] {
        &[]
    }

    fn badge(&self) -> BadgeSpec {
        BadgeSpec {
            svg: BADGE_SVG,
            letters: "F",
            color: "#7A5AF8",
        }
    }

    fn suggested_featured(&self) -> Vec<MeterKey> {
        vec![keys::REQUESTS_TODAY.into(), keys::SPEND_MONTH.into()]
    }

    fn usage_page(&self) -> Option<&str> {
        None
    }

    fn settings_fields(&self) -> Vec<SettingField> {
        vec![SettingField {
            key: settings_keys::ACCOUNT_NAME,
            label: "Demo account name".into(),
            help: Some("Shown in the popup and the tooltip.".into()),
            kind: FieldKind::Text {
                default: DEFAULT_ACCOUNT_NAME.into(),
                placeholder: DEFAULT_ACCOUNT_NAME.into(),
            },
        }]
    }

    fn configure(&mut self, values: &ModuleValues) {
        self.account_name = match values.get(settings_keys::ACCOUNT_NAME) {
            Some(SettingValue::Text(t)) if !t.trim().is_empty() => t.trim().to_string(),
            _ => DEFAULT_ACCOUNT_NAME.into(),
        };
    }

    fn discover_accounts(&mut self) -> Vec<AccountInfo> {
        vec![AccountInfo {
            id: "fake-demo".into(),
            display_name: self.account_name.clone(),
        }]
    }

    fn sources(&mut self, _account: &AccountInfo) -> Vec<Box<dyn Source>> {
        vec![
            Box::new(FakeSource {
                id: sources::API,
                name: "Fake API",
                exactness: Exactness::Exact,
                priority: 0,
                intervals: Intervals {
                    active: minutes(5),
                    idle: minutes(15),
                },
                requests_per_hour: REQUESTS_PER_HOUR,
                control: self.control.clone(),
            }),
            Box::new(FakeSource {
                id: sources::LOCAL,
                name: "Fake local log",
                exactness: Exactness::Estimate,
                priority: 1,
                intervals: Intervals {
                    active: minutes(2),
                    idle: minutes(10),
                },
                requests_per_hour: ESTIMATED_REQUESTS_PER_HOUR,
                control: self.control.clone(),
            }),
        ]
    }
}

struct FakeSource {
    id: &'static str,
    name: &'static str,
    exactness: Exactness,
    priority: u32,
    intervals: Intervals,
    requests_per_hour: f64,
    control: FakeControl,
}

impl Source for FakeSource {
    fn id(&self) -> &str {
        self.id
    }

    fn display_name(&self) -> &str {
        self.name
    }

    fn exactness(&self) -> Exactness {
        self.exactness
    }

    fn priority(&self) -> u32 {
        self.priority
    }

    fn intervals(&self) -> Intervals {
        self.intervals
    }

    fn fetch(&mut self, ctx: &FetchContext<'_>) -> Result<FetchOutput, FetchError> {
        let mut c = self.control.lock();
        *c.fetches.entry(self.id.into()).or_default() += 1;
        if let Some(e) = c.failures.get(self.id) {
            return Err(e.clone());
        }
        let server = ctx.now.plus_secs(c.skew_secs);

        let day_start = Timestamp(server.0.div_euclid(DAY) * DAY);
        let hours = (server.0 - day_start.0) as f64 / 3600.0;
        let requests = MeterValue {
            key: keys::REQUESTS_TODAY.into(),
            label: "Requests today".into(),
            used: (hours * self.requests_per_hour).floor(),
            limit: Some(REQUEST_LIMIT),
            unit: Unit::Count("requests".into()),
            window: Some(Window {
                end: day_start.plus_secs(DAY),
                start: Some(day_start),
                length: Some(Duration::from_secs(DAY as u64)),
            }),
            breakdown: None,
            unrecognized: false,
        };

        let civil = server.civil(0);
        let month_start = Timestamp(days_from_civil(civil.year, civil.month, 1) * DAY);
        let (ny, nm) = if civil.month == 12 {
            (civil.year + 1, 1)
        } else {
            (civil.year, civil.month + 1)
        };
        let month_end = Timestamp(days_from_civil(ny, nm, 1) * DAY);
        let days = (server.0 - month_start.0) as f64 / DAY as f64;
        let spend = MeterValue {
            key: keys::SPEND_MONTH.into(),
            label: "Spend this month".into(),
            used: (days * SPEND_PER_DAY * 100.0).round() / 100.0,
            limit: Some(SPEND_LIMIT),
            unit: Unit::Currency("EUR".into()),
            window: Some(Window {
                end: month_end,
                start: Some(month_start),
                length: None,
            }),
            breakdown: None,
            unrecognized: false,
        };

        let values = [requests, spend]
            .into_iter()
            .map(|mut v| {
                if let Some(u) = c.used.get(&v.key) {
                    v.used = *u;
                }
                if c.no_window.contains(&v.key) {
                    v.window = None;
                }
                v
            })
            .collect();
        Ok(FetchOutput {
            values,
            notices: Vec::new(),
        })
    }
}
