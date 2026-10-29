//! The statusline push Source. Claude Code runs its `statusLine` command after every
//! response with a JSON description of the session on stdin; for claude.ai Pro and Max
//! subscribers it includes `rate_limits.five_hour` and `rate_limits.seven_day`
//! (`used_percentage` 0-100, `resets_at` in Unix epoch seconds), each possibly absent.
//! Source: https://code.claude.com/docs/en/statusline ("Available data").
//!
//! The helper (`uqt-statusline`) forwards only those numbers through Core's push
//! channel, tagged with the config folder it ran for; this Source turns them into
//! Exact Readings of the Account that folder belongs to.

pub mod chain;
pub mod installer;

use std::path::Path;
use std::time::Duration;

use installer::{Installer, Status, Uninstalled};
use serde_json::{Map, Value, json};
use uqt_core::contract::{
    FetchContext, FetchError, FetchOutput, FieldKind, Intervals, SettingField, Source,
};
use uqt_core::engine::minutes;
use uqt_core::model::{Exactness, MeterValue, Unit, Window};
use uqt_core::push::PushMessage;
use uqt_core::time::Timestamp;

use crate::{ConfigFolder, ID, keys, sources};

pub const SOURCE_NAME: &str = "Claude Code statusline";
const SESSION_LENGTH: Duration = Duration::from_secs(5 * 3600);
const WEEK_LENGTH: Duration = Duration::from_secs(7 * 86_400);
/// The statusline only runs while Claude Code is in use; after this long without a
/// push its Readings are stale.
const FRESHNESS: Duration = Duration::from_secs(15 * 60);
/// The windows the helper forwards, with their Meter keys.
const WINDOWS: [(&str, &str); 2] = [("five_hour", keys::SESSION), ("seven_day", keys::WEEKLY)];

/// The push message for one statusline input, or `None` when it has no rate limits
/// (API-key users, or before the session's first response). Only `used_percentage`
/// and `resets_at` of the known windows are kept; nothing else of the input leaves.
pub fn push_message(input: &[u8], folder: &ConfigFolder) -> Option<PushMessage> {
    let json: Value = serde_json::from_slice(input).ok()?;
    let limits = &json["rate_limits"];
    let mut kept = Map::new();
    for (window, _) in WINDOWS {
        let w = &limits[window];
        if let (Some(used), Some(resets)) = (w["used_percentage"].as_f64(), w["resets_at"].as_i64())
        {
            kept.insert(
                window.into(),
                json!({ "used_percentage": used, "resets_at": resets }),
            );
        }
    }
    if kept.is_empty() {
        return None;
    }
    Some(PushMessage {
        assistant: ID.into(),
        source: sources::STATUSLINE.into(),
        origin: folder.dir.display().to_string(),
        payload: json!({ "rate_limits": kept }),
    })
}

/// The helper's own statusline text when there is no previous command to chain:
/// `5h 24% · 7d 41%`, or nothing without rate limits.
pub fn summary(input: &[u8]) -> String {
    let Ok(json) = serde_json::from_slice::<Value>(input) else {
        return String::new();
    };
    [("5h", "five_hour"), ("7d", "seven_day")]
        .iter()
        .filter_map(|(short, window)| {
            let used = json["rate_limits"][window]["used_percentage"].as_f64()?;
            Some(format!("{short} {}%", used.round()))
        })
        .collect::<Vec<_>>()
        .join(" \u{b7} ")
}

/// The install/uninstall button in the module's settings, describing the state now.
pub(crate) fn settings_field(installer: &Installer) -> SettingField {
    let file = installer.settings_file.display();
    let (help, button, confirm) = match installer.status() {
        Status::NotInstalled => {
            let keeps = match installer.current_command() {
                Some(c) => format!(
                    " Your current statusline command ({c}) keeps working: the helper runs \
                     it with the same input and shows its output."
                ),
                None => String::new(),
            };
            (
                "Pushes fresh 5-hour and weekly usage into the tray after every Claude Code \
                 response, without using the usage endpoint."
                    .to_string(),
                "Install",
                format!(
                    "UsageQuotaTray will set the statusLine command in Claude Code's settings \
                     file:\n{file}\n\nA backup of the file is written next to it first, and \
                     every other setting is kept.{keeps}\n\nChange the file now?"
                ),
            )
        }
        Status::Installed { chained } => (
            match chained {
                Some(c) => format!("Installed. Also runs your statusline command: {c}"),
                None => "Installed.".to_string(),
            },
            "Uninstall",
            format!(
                "UsageQuotaTray will put back the statusLine setting it replaced in:\n{file}\n\n\
                 Every other setting is kept. Uninstall now?"
            ),
        ),
        Status::Replaced => (
            "The statusLine setting was changed after installing, so the helper no longer \
             runs. Forgetting the installation leaves the file as it is."
                .to_string(),
            "Forget",
            format!("Forget the statusline installation? {file} is not changed."),
        ),
    };
    SettingField {
        key: crate::STATUSLINE_FIELD,
        label: "Statusline push".into(),
        help: Some(help),
        kind: FieldKind::Action {
            button: button.into(),
            confirm: Some(confirm),
        },
    }
}

/// Installs when not installed, else uninstalls. Returns what to tell the user.
pub(crate) fn toggle(installer: &Installer) -> Result<String, String> {
    match installer.status() {
        Status::NotInstalled => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs() as i64);
            let done = installer
                .install(Timestamp(now))
                .map_err(|e| e.to_string())?;
            log::info!("statusline helper installed");
            let backup = done
                .backup
                .map(|b| format!(" The original is backed up as {}.", b.display()))
                .unwrap_or_default();
            Ok(format!(
                "Installed. Claude Code uses it from its next response.{backup}"
            ))
        }
        Status::Installed { .. } | Status::Replaced => {
            let outcome = installer.uninstall().map_err(|e| e.to_string())?;
            log::info!("statusline helper uninstalled ({outcome:?})");
            Ok(match outcome {
                Uninstalled::Restored => "Uninstalled; Claude Code's original statusLine \
                     setting is back."
                    .into(),
                Uninstalled::LeftAlone => "Forgotten; Claude Code's settings file was not \
                     changed."
                    .into(),
            })
        }
    }
}

/// The same folder however it is written (case, slash direction, trailing slash).
fn same_folder(a: &str, b: &Path) -> bool {
    let norm = |s: &str| s.replace('\\', "/").trim_end_matches('/').to_lowercase();
    norm(a) == norm(&b.display().to_string())
}

/// Readings pushed by the helper for the config folders of one Account (Exact, never
/// polled).
pub(crate) struct StatuslineSource {
    /// Every folder holding the Account's login, with its position (for the
    /// placeholder id of an unidentified login).
    pub(crate) folders: Vec<(usize, ConfigFolder)>,
    /// The Account this Source was made for.
    pub(crate) account_id: String,
}

impl StatuslineSource {
    /// Whether a push from `origin` belongs to this Account: it came from one of its
    /// folders, and that folder is still signed in to the same login. Another login in
    /// the folder is another Account; the engine picks it up on its next discovery.
    fn is_mine(&self, origin: &str) -> bool {
        self.folders.iter().any(|(index, folder)| {
            same_folder(origin, &folder.dir)
                && crate::account_info(folder, *index).id == self.account_id
        })
    }
}

impl Source for StatuslineSource {
    fn id(&self) -> &str {
        sources::STATUSLINE
    }

    fn display_name(&self) -> &str {
        SOURCE_NAME
    }

    fn exactness(&self) -> Exactness {
        Exactness::Exact
    }

    fn priority(&self) -> u32 {
        1
    }

    fn intervals(&self) -> Intervals {
        Intervals {
            active: minutes(5),
            idle: minutes(15),
        }
    }

    fn freshness_limit(&self) -> Option<Duration> {
        Some(FRESHNESS)
    }

    fn push_only(&self) -> bool {
        true
    }

    fn fetch(&mut self, _ctx: &FetchContext<'_>) -> Result<FetchOutput, FetchError> {
        Err(FetchError::Other(
            "the statusline Source is push-only".into(),
        ))
    }

    fn accept_push(&mut self, message: &PushMessage) -> Option<FetchOutput> {
        if !self.is_mine(&message.origin) {
            return None;
        }
        let limits = &message.payload["rate_limits"];
        let values: Vec<MeterValue> = WINDOWS
            .iter()
            .filter_map(|(window, key)| {
                let w = &limits[*window];
                let used = w["used_percentage"].as_f64()?;
                let end = Timestamp(w["resets_at"].as_i64()?);
                let (label, length) = if *key == keys::SESSION {
                    ("5-hour", SESSION_LENGTH)
                } else {
                    ("Weekly", WEEK_LENGTH)
                };
                Some(MeterValue {
                    key: (*key).into(),
                    label: label.into(),
                    used,
                    limit: Some(100.0),
                    unit: Unit::Percent,
                    window: Some(Window {
                        end,
                        start: None,
                        length: Some(length),
                    }),
                    breakdown: None,
                    unrecognized: false,
                })
            })
            .collect();
        (!values.is_empty()).then_some(FetchOutput {
            values,
            notices: Vec::new(),
        })
    }
}
