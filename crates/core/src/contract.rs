//! The contracts an Assistant module implements. Modules are registered at compile time
//! by the app crate; nothing here names a concrete Assistant.

use std::time::Duration;

use crate::http::HostClient;
use crate::model::{AccountInfo, BadgeSpec, Exactness, MeterKey, MeterValue, Notice};
use crate::time::Timestamp;

/// One Assistant module.
pub trait Assistant: Send {
    /// Stable id, also the Badge override file name (`<id>.svg`).
    fn id(&self) -> &'static str;
    fn display_name(&self) -> &str;
    /// Hosts the module's Sources may call through the [`HostClient`]; nothing else.
    fn allowed_hosts(&self) -> &[&'static str];
    fn badge(&self) -> BadgeSpec;
    /// Up to two Meter keys featured on the tray icon until the user chooses.
    fn suggested_featured(&self) -> Vec<MeterKey>;
    /// The maker's usage page, if any.
    fn usage_page(&self) -> Option<&str>;
    fn discover_accounts(&mut self) -> Vec<AccountInfo>;
    fn sources(&mut self, account: &AccountInfo) -> Vec<Box<dyn Source>>;
}

/// Polling intervals a Source declares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Intervals {
    /// While the user is present on this PC.
    pub active: Duration,
    /// While the user is away but the session is unlocked.
    pub idle: Duration,
}

/// One way of obtaining Meter values for an Account.
pub trait Source: Send {
    fn id(&self) -> &str;
    /// Shown next to each Reading.
    fn display_name(&self) -> &str;
    fn exactness(&self) -> Exactness;
    /// Lower is fetched first.
    fn priority(&self) -> u32;
    fn intervals(&self) -> Intervals;
    /// How old a Reading may get before it is stale; `None` means twice the current
    /// poll interval.
    fn freshness_limit(&self) -> Option<Duration> {
        None
    }
    fn fetch(&mut self, ctx: &FetchContext<'_>) -> Result<FetchOutput, FetchError>;
    /// What the user can do about `error`, in the Assistant's own words
    /// ("run Claude Code once"); shown next to the generic failure text.
    fn failure_hint(&self, _error: &FetchError) -> Option<String> {
        None
    }
}

/// What a Source gets for one fetch.
pub struct FetchContext<'a> {
    pub now: Timestamp,
    /// The only way a Source may reach the network.
    pub http: &'a HostClient,
}

/// A successful fetch.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FetchOutput {
    pub values: Vec<MeterValue>,
    pub notices: Vec<Notice>,
}

/// Why a fetch failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FetchError {
    /// The Assistant itself is not installed on this PC.
    NotInstalled,
    NotLoggedIn,
    Expired,
    Offline,
    RateLimited {
        retry_after: Option<Duration>,
    },
    EndpointChanged,
    Other(String),
}
