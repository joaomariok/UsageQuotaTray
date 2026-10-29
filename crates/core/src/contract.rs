//! The contracts an Assistant module implements. Modules are registered at compile time
//! by the app crate; nothing here names a concrete Assistant.

use std::time::Duration;

use crate::http::HostClient;
use crate::model::{AccountInfo, BadgeSpec, Exactness, MeterKey, MeterValue, Notice};
use crate::push::PushMessage;
use crate::settings::ModuleValues;
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
    /// The module's own settings, rendered by the settings window.
    fn settings_fields(&self) -> Vec<SettingField> {
        Vec::new()
    }
    /// The user's values for [`Assistant::settings_fields`]; keys the user never set are
    /// absent. Called before the first discovery and after every settings change.
    fn configure(&mut self, _values: &ModuleValues) {}
    /// Runs the [`FieldKind::Action`] field `key` (the UI has already asked the user
    /// to confirm, when the field asks for it). Returns what to tell the user.
    fn run_action(&mut self, key: &str) -> Result<String, String> {
        Err(format!("{} has no action {key}", self.display_name()))
    }
    fn discover_accounts(&mut self) -> Vec<AccountInfo>;
    fn sources(&mut self, account: &AccountInfo) -> Vec<Box<dyn Source>>;
}

/// One entry of a module's settings description.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingField {
    /// Stable key the value is stored under.
    pub key: &'static str,
    pub label: String,
    pub help: Option<String>,
    pub kind: FieldKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldKind {
    Toggle {
        default: bool,
    },
    Text {
        default: String,
        placeholder: String,
    },
    /// A button that runs [`Assistant::run_action`] at once; it stores no value. The
    /// label and help may describe the current state, since the settings description
    /// is read again after every change.
    Action {
        button: String,
        /// Asked as a yes/no question before the action runs.
        confirm: Option<String>,
    },
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
    /// A push Source that only receives [`PushMessage`]s and is never polled.
    fn push_only(&self) -> bool {
        false
    }
    /// The values in a message from the push channel, or `None` when the message is
    /// not for this Source and Account (or carries nothing usable). The engine routes
    /// only messages whose assistant and source ids match.
    fn accept_push(&mut self, _message: &PushMessage) -> Option<FetchOutput> {
        None
    }
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
