//! The user's settings: which Assistants and Accounts run, Featured Meters, alert
//! thresholds, polling and module values. Stored as JSON in the local app-data folder
//! and written atomically (a temporary file renamed over the old one).

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::model::MeterKey;

/// Alert thresholds in percent of the Limit, unless the user overrides them per Meter.
pub const DEFAULT_THRESHOLDS: [u32; 3] = [75, 90, 100];
/// The fastest the user may poll while active.
pub const MIN_ACTIVE_POLL: Duration = Duration::from_secs(2 * 60);
/// Highest threshold accepted, in percent.
pub const MAX_THRESHOLD: u32 = 999;
/// The icon shows at most this many Featured Meters.
pub const MAX_FEATURED: usize = 2;

/// A module setting's value, as the module's settings description declares it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SettingValue {
    Bool(bool),
    Text(String),
}

/// The shape in which the tray icon draws its Featured Meters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Gauge {
    #[default]
    Rings,
    Bars,
    /// The first Featured Meter as a number.
    Number,
    /// Both Featured Meters as numbers, one above the other.
    StackedNumbers,
}

impl Gauge {
    /// Whether this Gauge leaves room for the Badge.
    pub fn has_badge(self) -> bool {
        matches!(self, Gauge::Rings | Gauge::Bars)
    }
}

/// Values of one module's settings, by key.
pub type ModuleValues = BTreeMap<String, SettingValue>;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// `None` uses each Source's own active interval.
    #[serde(
        rename = "active_poll_minutes",
        with = "minutes",
        skip_serializing_if = "Option::is_none"
    )]
    pub active_poll: Option<Duration>,
    pub reset_notifications: bool,
    #[serde(deserialize_with = "gauge_or_default")]
    pub gauge: Gauge,
    /// Applies only to the Gauges that have room for the Badge.
    pub show_badge: bool,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub assistants: BTreeMap<String, AssistantSettings>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            active_poll: None,
            reset_notifications: true,
            gauge: Gauge::default(),
            show_badge: true,
            assistants: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AssistantSettings {
    pub enabled: bool,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub accounts: BTreeMap<String, AccountSettings>,
    /// The module's own settings.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub values: ModuleValues,
}

impl Default for AssistantSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            accounts: BTreeMap::new(),
            values: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AccountSettings {
    pub enabled: bool,
    /// `None` until the user chooses; then the module's suggestion no longer applies.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub featured: Option<Vec<MeterKey>>,
    /// Per-Meter overrides of [`DEFAULT_THRESHOLDS`]; an empty list turns alerts off.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub thresholds: BTreeMap<MeterKey, Vec<u32>>,
}

impl Default for AccountSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            featured: None,
            thresholds: BTreeMap::new(),
        }
    }
}

/// One change the settings window makes.
#[derive(Clone, Debug, PartialEq)]
pub enum SettingsEdit {
    AssistantEnabled {
        assistant: String,
        on: bool,
    },
    AccountEnabled {
        assistant: String,
        account: String,
        on: bool,
    },
    /// Features or un-features `meter`. `current` is the Account's Featured Meters as
    /// shown (the module's suggestion until the user first chooses).
    Featured {
        assistant: String,
        account: String,
        meter: MeterKey,
        on: bool,
        current: Vec<MeterKey>,
    },
    /// Moves `meter` one place up or down the Account's Featured Meters, which
    /// `current` holds as shown.
    MoveFeatured {
        assistant: String,
        account: String,
        meter: MeterKey,
        up: bool,
        current: Vec<MeterKey>,
    },
    Thresholds {
        assistant: String,
        account: String,
        meter: MeterKey,
        thresholds: Vec<u32>,
    },
    ModuleValue {
        assistant: String,
        key: String,
        value: SettingValue,
    },
    ActivePoll(Option<Duration>),
    ResetNotifications(bool),
    Gauge(Gauge),
    ShowBadge(bool),
}

impl Settings {
    pub fn assistant_enabled(&self, assistant: &str) -> bool {
        self.assistants.get(assistant).is_none_or(|a| a.enabled)
    }

    pub fn account_enabled(&self, assistant: &str, account: &str) -> bool {
        self.account(assistant, account).is_none_or(|a| a.enabled)
    }

    /// The user's Featured Meters for an Account, or `None` for the module's suggestion.
    pub fn featured(&self, assistant: &str, account: &str) -> Option<Vec<MeterKey>> {
        self.account(assistant, account)
            .and_then(|a| a.featured.clone())
    }

    /// The alert thresholds for one Meter, ascending.
    pub fn thresholds(&self, assistant: &str, account: &str, meter: &str) -> Vec<u32> {
        self.account(assistant, account)
            .and_then(|a| a.thresholds.get(meter).cloned())
            .unwrap_or_else(|| DEFAULT_THRESHOLDS.to_vec())
    }

    pub fn module_values(&self, assistant: &str) -> ModuleValues {
        self.assistants
            .get(assistant)
            .map(|a| a.values.clone())
            .unwrap_or_default()
    }

    fn account(&self, assistant: &str, account: &str) -> Option<&AccountSettings> {
        self.assistants.get(assistant)?.accounts.get(account)
    }

    fn account_mut(&mut self, assistant: &str, account: &str) -> &mut AccountSettings {
        self.assistants
            .entry(assistant.into())
            .or_default()
            .accounts
            .entry(account.into())
            .or_default()
    }

    pub fn apply(&mut self, edit: SettingsEdit) {
        match edit {
            SettingsEdit::AssistantEnabled { assistant, on } => {
                self.assistants.entry(assistant).or_default().enabled = on;
            }
            SettingsEdit::AccountEnabled {
                assistant,
                account,
                on,
            } => self.account_mut(&assistant, &account).enabled = on,
            SettingsEdit::Featured {
                assistant,
                account,
                meter,
                on,
                current,
            } => {
                let mut list: Vec<MeterKey> = current.into_iter().take(MAX_FEATURED).collect();
                if on {
                    if !list.contains(&meter) && list.len() < MAX_FEATURED {
                        list.push(meter);
                    }
                } else {
                    list.retain(|k| *k != meter);
                }
                self.account_mut(&assistant, &account).featured = Some(list);
            }
            SettingsEdit::MoveFeatured {
                assistant,
                account,
                meter,
                up,
                current,
            } => {
                let mut list: Vec<MeterKey> = current.into_iter().take(MAX_FEATURED).collect();
                if let Some(i) = list.iter().position(|k| *k == meter) {
                    let j = if up { i.checked_sub(1) } else { Some(i + 1) };
                    if let Some(j) = j.filter(|&j| j < list.len()) {
                        list.swap(i, j);
                    }
                }
                self.account_mut(&assistant, &account).featured = Some(list);
            }
            SettingsEdit::Thresholds {
                assistant,
                account,
                meter,
                thresholds,
            } => {
                let thresholds = normalize_thresholds(thresholds);
                let acc = self.account_mut(&assistant, &account);
                if thresholds == DEFAULT_THRESHOLDS {
                    acc.thresholds.remove(&meter);
                } else {
                    acc.thresholds.insert(meter, thresholds);
                }
            }
            SettingsEdit::ModuleValue {
                assistant,
                key,
                value,
            } => {
                self.assistants
                    .entry(assistant)
                    .or_default()
                    .values
                    .insert(key, value);
            }
            SettingsEdit::ActivePoll(poll) => self.active_poll = poll.map(clamp_poll),
            SettingsEdit::ResetNotifications(on) => self.reset_notifications = on,
            SettingsEdit::Gauge(gauge) => self.gauge = gauge,
            SettingsEdit::ShowBadge(on) => self.show_badge = on,
        }
        self.prune();
    }

    /// Drops entries equal to the defaults so the file stays small and readable.
    fn prune(&mut self) {
        for a in self.assistants.values_mut() {
            a.accounts
                .retain(|_, acc| *acc != AccountSettings::default());
        }
        self.assistants
            .retain(|_, a| *a != AssistantSettings::default());
    }

    /// Corrects values a hand-edited file may hold.
    fn normalize(mut self) -> Self {
        self.active_poll = self.active_poll.map(clamp_poll);
        for a in self.assistants.values_mut() {
            for acc in a.accounts.values_mut() {
                if let Some(f) = &mut acc.featured {
                    f.truncate(MAX_FEATURED);
                }
                for t in acc.thresholds.values_mut() {
                    *t = normalize_thresholds(std::mem::take(t));
                }
            }
        }
        self.prune();
        self
    }
}

/// A Gauge this version does not know, say from a newer one, falls back to the default
/// instead of discarding the whole file.
fn gauge_or_default<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Gauge, D::Error> {
    let value = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}

fn clamp_poll(d: Duration) -> Duration {
    d.max(MIN_ACTIVE_POLL)
}

fn normalize_thresholds(mut t: Vec<u32>) -> Vec<u32> {
    t.retain(|v| (1..=MAX_THRESHOLD).contains(v));
    t.sort_unstable();
    t.dedup();
    t
}

/// Parses what the user typed: numbers separated by commas or spaces, `off` for no
/// alerts, empty for the defaults. `None` when it is not valid.
pub fn parse_thresholds(text: &str) -> Option<Vec<u32>> {
    let text = text.trim();
    if text.is_empty() {
        return Some(DEFAULT_THRESHOLDS.to_vec());
    }
    if text.eq_ignore_ascii_case("off") {
        return Some(Vec::new());
    }
    let mut values = Vec::new();
    for part in text.split([',', ' ', ';']).filter(|p| !p.is_empty()) {
        let v: u32 = part.parse().ok()?;
        if !(1..=MAX_THRESHOLD).contains(&v) {
            return None;
        }
        values.push(v);
    }
    Some(normalize_thresholds(values))
}

/// How thresholds are shown in a text field.
pub fn thresholds_text(thresholds: &[u32]) -> String {
    if thresholds.is_empty() {
        return "off".into();
    }
    thresholds
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Reads and writes `settings.json`.
#[derive(Clone, Debug)]
pub struct SettingsStore {
    path: PathBuf,
}

impl SettingsStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The stored settings, or the defaults when there are none. A file that does not
    /// parse is kept next to it as `settings.json.bad` so nothing the user wrote is lost.
    pub fn load(&self) -> Settings {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(t) => t,
            Err(e) => {
                if e.kind() != std::io::ErrorKind::NotFound {
                    log::warn!("settings unreadable ({e}); using defaults");
                }
                return Settings::default();
            }
        };
        match serde_json::from_str::<Settings>(&text) {
            Ok(s) => s.normalize(),
            Err(e) => {
                log::warn!("settings file does not parse ({e}); using defaults");
                let _ = std::fs::rename(&self.path, self.path.with_extension("json.bad"));
                Settings::default()
            }
        }
    }

    /// Writes a temporary file next to the settings and renames it over them, so a
    /// crash never leaves a half-written file.
    pub fn save(&self, settings: &Settings) -> std::io::Result<()> {
        let dir = self.path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(dir)?;
        let tmp = self.path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(settings).map_err(std::io::Error::other)?;
        {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(text.as_bytes())?;
            f.sync_all()?;
        }
        // On Windows this is MoveFileExW with MOVEFILE_REPLACE_EXISTING.
        std::fs::rename(&tmp, &self.path)
    }
}

mod minutes {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(d: &Option<Duration>, s: S) -> Result<S::Ok, S::Error> {
        match d {
            Some(d) => s.serialize_u64(d.as_secs() / 60),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Duration>, D::Error> {
        Ok(Option::<u64>::deserialize(d)?.map(|m| Duration::from_secs(m * 60)))
    }
}
