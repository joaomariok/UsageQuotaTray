//! What the settings window shows, and which settings change each control makes.

use std::time::Duration;

use uqt_core::contract::FieldKind;
use uqt_core::engine::{Catalog, EngineState};
use uqt_core::model::MeterKey;
use uqt_core::settings::{
    DEFAULT_THRESHOLDS, MAX_FEATURED, SettingValue, Settings, SettingsEdit, parse_thresholds,
    thresholds_text,
};

/// Polling choices offered besides the default, in minutes.
const POLL_MINUTES: [u64; 5] = [2, 3, 5, 10, 15];
/// Faster than this shows the shared-rate-limit warning.
const POLL_WARN_BELOW: Duration = Duration::from_secs(5 * 60);
const POLL_WARNING: &str = "Some usage endpoints share their rate limit with the \
    Assistant's own usage display. Polling more often than every 5 minutes can make \
    both fail for a while.";
const MODULE_NOTE: &str = "Changes to these settings apply the next time UsageQuotaTray starts.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    /// A switch turning an Assistant on or off.
    Assistant,
    /// A switch turning an Account on or off.
    Account,
    /// A Featured checkbox and a thresholds field.
    Meter,
    /// A module setting with a switch.
    Toggle,
    /// A module setting with a text field.
    Text,
    /// Explanatory text.
    Note,
    /// A module action: a button (labelled with `text`) with `detail` beside it.
    Action,
}

/// What pressing an action row's button asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionRequest {
    pub assistant: String,
    pub key: String,
    /// Ask this yes/no question first; run only on yes.
    pub confirm: Option<String>,
}

/// What a row's control changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowTarget {
    None,
    Assistant(String),
    Account {
        assistant: String,
        account: String,
    },
    Meter {
        assistant: String,
        account: String,
        meter: MeterKey,
        /// The Account's Featured Meters as shown.
        featured: Vec<MeterKey>,
    },
    Field {
        assistant: String,
        key: String,
    },
    Action(ActionRequest),
}

#[derive(Clone, Debug, PartialEq)]
pub struct SettingRow {
    pub kind: RowKind,
    pub label: String,
    pub detail: String,
    /// Switch or checkbox state.
    pub checked: bool,
    /// Whether the switch or checkbox can be changed.
    pub enabled: bool,
    /// Whether the text field can be edited.
    pub editable: bool,
    pub text: String,
    pub placeholder: String,
    pub target: RowTarget,
}

/// A change the user made to a row's control.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Input {
    Toggled(bool),
    Text(String),
}

impl SettingRow {
    fn new(kind: RowKind, label: impl Into<String>, target: RowTarget) -> Self {
        Self {
            kind,
            label: label.into(),
            detail: String::new(),
            checked: false,
            enabled: true,
            editable: false,
            text: String::new(),
            placeholder: String::new(),
            target,
        }
    }

    /// What the row's button runs, for action rows.
    pub fn action(&self) -> Option<ActionRequest> {
        match &self.target {
            RowTarget::Action(request) => Some(request.clone()),
            _ => None,
        }
    }

    /// The settings change `input` makes, or `None` when it makes none (such as
    /// thresholds that do not parse).
    pub fn edit(&self, input: Input) -> Option<SettingsEdit> {
        Some(match (&self.target, input) {
            (RowTarget::Assistant(assistant), Input::Toggled(on)) => {
                SettingsEdit::AssistantEnabled {
                    assistant: assistant.clone(),
                    on,
                }
            }
            (RowTarget::Account { assistant, account }, Input::Toggled(on)) => {
                SettingsEdit::AccountEnabled {
                    assistant: assistant.clone(),
                    account: account.clone(),
                    on,
                }
            }
            (
                RowTarget::Meter {
                    assistant,
                    account,
                    meter,
                    featured,
                },
                input,
            ) => match input {
                Input::Toggled(on) => SettingsEdit::Featured {
                    assistant: assistant.clone(),
                    account: account.clone(),
                    meter: meter.clone(),
                    on,
                    current: featured.clone(),
                },
                Input::Text(text) => SettingsEdit::Thresholds {
                    assistant: assistant.clone(),
                    account: account.clone(),
                    meter: meter.clone(),
                    thresholds: parse_thresholds(&text)?,
                },
            },
            (RowTarget::Field { assistant, key }, input) => SettingsEdit::ModuleValue {
                assistant: assistant.clone(),
                key: key.clone(),
                value: match input {
                    Input::Toggled(on) => SettingValue::Bool(on),
                    Input::Text(t) => SettingValue::Text(t),
                },
            },
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SettingsView {
    /// The Assistants page, top to bottom.
    pub rows: Vec<SettingRow>,
    pub poll_options: Vec<String>,
    pub poll_index: usize,
    /// `None` uses each Source's default.
    pub poll_choices: Vec<Option<Duration>>,
    pub poll_warning: Option<String>,
    pub reset_notifications: bool,
    pub start_with_windows: bool,
}

impl SettingsView {
    pub fn poll_edit(&self, index: usize) -> Option<SettingsEdit> {
        self.poll_choices
            .get(index)
            .map(|c| SettingsEdit::ActivePoll(*c))
    }
}

pub fn settings_view(
    catalog: &Catalog,
    state: &EngineState,
    settings: &Settings,
    start_with_windows: bool,
) -> SettingsView {
    let mut rows = Vec::new();
    for a in &catalog.assistants {
        let mut header = SettingRow::new(
            RowKind::Assistant,
            &a.display_name,
            RowTarget::Assistant(a.id.into()),
        );
        header.checked = a.enabled;
        rows.push(header);
        if !a.enabled {
            continue;
        }
        let shown = state.assistants.iter().find(|s| s.id == a.id);
        for acc in &a.accounts {
            let mut row = SettingRow::new(
                RowKind::Account,
                &acc.display_name,
                RowTarget::Account {
                    assistant: a.id.into(),
                    account: acc.id.clone(),
                },
            );
            row.checked = acc.enabled;
            row.detail = "Check up to two Meters for the tray icon. Alerts are in percent \
                of the limit; type off for none."
                .into();
            rows.push(row);
            if !acc.enabled {
                continue;
            }
            let Some(live) = shown.and_then(|s| s.accounts.iter().find(|x| x.id == acc.id)) else {
                continue;
            };
            if live.meters.is_empty() {
                rows.push(SettingRow::new(
                    RowKind::Note,
                    "No Meters yet.",
                    RowTarget::None,
                ));
            }
            let featured: Vec<MeterKey> = live
                .featured
                .iter()
                .filter(|k| live.meter(k).is_some())
                .take(MAX_FEATURED)
                .cloned()
                .collect();
            for m in &live.meters {
                let mut row = SettingRow::new(
                    RowKind::Meter,
                    &m.label,
                    RowTarget::Meter {
                        assistant: a.id.into(),
                        account: acc.id.clone(),
                        meter: m.key.clone(),
                        featured: featured.clone(),
                    },
                );
                row.checked = featured.contains(&m.key);
                row.enabled = row.checked || featured.len() < MAX_FEATURED;
                row.editable = m.limit.is_some();
                row.placeholder = thresholds_text(&DEFAULT_THRESHOLDS);
                if row.editable {
                    let t = settings.thresholds(a.id, &acc.id, &m.key);
                    if t != DEFAULT_THRESHOLDS {
                        row.text = thresholds_text(&t);
                    }
                } else {
                    row.detail = "This Meter has no limit, so no alerts.".into();
                }
                rows.push(row);
            }
        }
        let values = settings.module_values(a.id);
        for f in &a.fields {
            let target = RowTarget::Field {
                assistant: a.id.into(),
                key: f.key.into(),
            };
            let mut row = match &f.kind {
                FieldKind::Toggle { default } => {
                    let mut r = SettingRow::new(RowKind::Toggle, &f.label, target);
                    r.checked = match values.get(f.key) {
                        Some(SettingValue::Bool(b)) => *b,
                        _ => *default,
                    };
                    r
                }
                FieldKind::Text {
                    default,
                    placeholder,
                } => {
                    let mut r = SettingRow::new(RowKind::Text, &f.label, target);
                    r.editable = true;
                    r.text = match values.get(f.key) {
                        Some(SettingValue::Text(t)) => t.clone(),
                        _ => default.clone(),
                    };
                    r.placeholder = placeholder.clone();
                    r
                }
                FieldKind::Action { button, confirm } => {
                    let mut r = SettingRow::new(
                        RowKind::Action,
                        &f.label,
                        RowTarget::Action(ActionRequest {
                            assistant: a.id.into(),
                            key: f.key.into(),
                            confirm: confirm.clone(),
                        }),
                    );
                    r.text = button.clone();
                    r
                }
            };
            row.detail = f.help.clone().unwrap_or_default();
            rows.push(row);
        }
        // Actions apply at once; stored values only on the next start.
        if a.fields
            .iter()
            .any(|f| !matches!(f.kind, FieldKind::Action { .. }))
        {
            rows.push(SettingRow::new(RowKind::Note, MODULE_NOTE, RowTarget::None));
        }
    }

    let mut poll_choices: Vec<Option<Duration>> = std::iter::once(None)
        .chain(
            POLL_MINUTES
                .iter()
                .map(|m| Some(Duration::from_secs(m * 60))),
        )
        .collect();
    if let Some(p) = settings.active_poll
        && !poll_choices.contains(&Some(p))
    {
        poll_choices.push(Some(p));
        poll_choices[1..].sort();
    }
    let poll_index = poll_choices
        .iter()
        .position(|c| *c == settings.active_poll)
        .unwrap_or(0);
    let poll_options = poll_choices
        .iter()
        .map(|c| match c {
            None => "Default (each Source's own interval)".to_string(),
            Some(d) => format!("Every {} min", d.as_secs() / 60),
        })
        .collect();
    SettingsView {
        rows,
        poll_options,
        poll_index,
        poll_choices,
        poll_warning: settings
            .active_poll
            .filter(|p| *p < POLL_WARN_BELOW)
            .map(|_| POLL_WARNING.to_string()),
        reset_notifications: settings.reset_notifications,
        start_with_windows,
    }
}
