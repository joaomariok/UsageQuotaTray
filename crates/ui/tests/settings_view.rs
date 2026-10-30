//! Presentation seam: what the settings window shows, and which settings change each
//! control makes. No Slint.

use std::time::Duration;

use uqt_core::contract::{FieldKind, SettingField};
use uqt_core::engine::{
    AccountState, AssistantState, Catalog, CatalogAccount, CatalogAssistant, EngineState,
    Freshness, MeterState,
};
use uqt_core::model::{BadgeSpec, Exactness, Unit};
use uqt_core::settings::{Gauge, SettingValue, Settings, SettingsEdit};
use uqt_core::time::Timestamp;
use uqt_ui::present::{
    ActionRequest, Input, RowKind, RowTarget, SettingRow, SettingsView, settings_view,
};

const BADGE: BadgeSpec = BadgeSpec {
    svg: "<svg/>",
    letters: "T",
    color: "#123456",
};

fn meter(key: &str, limit: Option<f64>) -> MeterState {
    MeterState {
        key: key.into(),
        label: format!("{key} label"),
        used: 10.0,
        limit,
        unit: Unit::Percent,
        window: None,
        source_name: "API".into(),
        exactness: Exactness::Exact,
        fetched_at: Timestamp(0),
        freshness: Freshness::Fresh,
        pace: None,
        breakdown: None,
        unrecognized: false,
    }
}

fn fields() -> Vec<SettingField> {
    vec![
        SettingField {
            key: "wsl",
            label: "Read WSL folders".into(),
            help: Some("Starts the distro.".into()),
            kind: FieldKind::Toggle { default: false },
        },
        SettingField {
            key: "folder",
            label: "Extra folder".into(),
            help: None,
            kind: FieldKind::Text {
                default: String::new(),
                placeholder: "C:\\path".into(),
            },
        },
    ]
}

fn catalog(enabled: bool, second_account_enabled: bool) -> Catalog {
    Catalog {
        assistants: vec![
            CatalogAssistant {
                id: "t",
                display_name: "Test Assistant".into(),
                enabled: true,
                fields: fields(),
                accounts: vec![
                    CatalogAccount {
                        id: "me".into(),
                        display_name: "Me".into(),
                        enabled: true,
                    },
                    CatalogAccount {
                        id: "work".into(),
                        display_name: "Work".into(),
                        enabled: second_account_enabled,
                    },
                ],
            },
            CatalogAssistant {
                id: "off",
                display_name: "Other Assistant".into(),
                enabled,
                fields: Vec::new(),
                accounts: vec![CatalogAccount {
                    id: "o".into(),
                    display_name: "O".into(),
                    enabled: true,
                }],
            },
        ],
    }
}

fn state(featured: &[&str]) -> EngineState {
    EngineState {
        now: Timestamp(0),
        assistants: vec![AssistantState {
            id: "t",
            display_name: "Test Assistant".into(),
            badge: BADGE,
            usage_page: None,
            accounts: vec![AccountState {
                id: "me".into(),
                display_name: "Me".into(),
                featured: featured.iter().map(|s| s.to_string()).collect(),
                meters: vec![
                    meter("a", Some(100.0)),
                    meter("b", Some(100.0)),
                    meter("c", Some(100.0)),
                    meter("credits", None),
                ],
                notices: Vec::new(),
                problem: None,
            }],
        }],
    }
}

fn view(settings: &Settings) -> SettingsView {
    settings_view(&catalog(false, false), &state(&["a", "b"]), settings, false)
}

fn row<'a>(v: &'a SettingsView, label: &str) -> &'a SettingRow {
    v.rows
        .iter()
        .find(|r| r.label == label)
        .unwrap_or_else(|| panic!("no row {label}"))
}

fn labels(v: &SettingsView) -> Vec<(RowKind, String)> {
    v.rows.iter().map(|r| (r.kind, r.label.clone())).collect()
}

#[test]
fn every_assistant_and_account_has_a_switch_even_when_off() {
    let v = view(&Settings::default());
    let l = labels(&v);
    assert_eq!(l[0], (RowKind::Assistant, "Test Assistant".into()));
    assert_eq!(l[1], (RowKind::Account, "Me".into()));
    assert!(l.contains(&(RowKind::Account, "Work".into())));
    assert!(!row(&v, "Work").checked);
    let other = row(&v, "Other Assistant");
    assert_eq!(other.kind, RowKind::Assistant);
    assert!(!other.checked);
    // A disabled Assistant lists no Accounts.
    assert!(!l.contains(&(RowKind::Account, "O".into())));
    assert_eq!(
        row(&v, "Other Assistant").target,
        RowTarget::Assistant("off".into())
    );
}

#[test]
fn switching_assistants_and_accounts_makes_enable_edits() {
    let v = view(&Settings::default());
    assert_eq!(
        row(&v, "Other Assistant").edit(Input::Toggled(true)),
        Some(SettingsEdit::AssistantEnabled {
            assistant: "off".into(),
            on: true
        })
    );
    assert_eq!(
        row(&v, "Work").edit(Input::Toggled(true)),
        Some(SettingsEdit::AccountEnabled {
            assistant: "t".into(),
            account: "work".into(),
            on: true
        })
    );
}

#[test]
fn meters_of_an_enabled_account_are_listed_with_featured_checkboxes() {
    let v = view(&Settings::default());
    let a = row(&v, "a label");
    assert_eq!(a.kind, RowKind::Meter);
    assert!(a.checked && a.enabled);
    assert!(row(&v, "b label").checked);
    // Two are featured already: the others cannot be checked.
    let c = row(&v, "c label");
    assert!(!c.checked && !c.enabled);
}

#[test]
fn a_missing_featured_meter_does_not_take_a_slot() {
    let v = settings_view(
        &catalog(true, true),
        &state(&["gone", "a"]),
        &Settings::default(),
        false,
    );
    assert!(row(&v, "a label").checked);
    let b = row(&v, "b label");
    assert!(!b.checked && b.enabled);
    assert_eq!(
        b.edit(Input::Toggled(true)),
        Some(SettingsEdit::Featured {
            assistant: "t".into(),
            account: "me".into(),
            meter: "b".into(),
            on: true,
            current: vec!["a".into()],
        })
    );
}

#[test]
fn thresholds_show_the_override_or_the_default_as_a_hint() {
    let mut s = Settings::default();
    s.apply(SettingsEdit::Thresholds {
        assistant: "t".into(),
        account: "me".into(),
        meter: "b".into(),
        thresholds: vec![50],
    });
    s.apply(SettingsEdit::Thresholds {
        assistant: "t".into(),
        account: "me".into(),
        meter: "c".into(),
        thresholds: vec![],
    });
    let v = view(&s);
    let a = row(&v, "a label");
    assert!(a.editable);
    assert_eq!(
        (a.text.as_str(), a.placeholder.as_str()),
        ("", "75, 90, 100")
    );
    assert_eq!(row(&v, "b label").text, "50");
    assert_eq!(row(&v, "c label").text, "off");
    let credits = row(&v, "credits label");
    assert!(!credits.editable);
    assert!(credits.detail.contains("no limit"));
}

#[test]
fn typed_thresholds_become_an_edit_only_when_valid() {
    let v = view(&Settings::default());
    let a = row(&v, "a label");
    assert_eq!(
        a.edit(Input::Text("60, 95".into())),
        Some(SettingsEdit::Thresholds {
            assistant: "t".into(),
            account: "me".into(),
            meter: "a".into(),
            thresholds: vec![60, 95],
        })
    );
    assert_eq!(a.edit(Input::Text("60, nope".into())), None);
}

#[test]
fn module_settings_are_rendered_from_the_modules_description() {
    let mut s = Settings::default();
    s.apply(SettingsEdit::ModuleValue {
        assistant: "t".into(),
        key: "folder".into(),
        value: SettingValue::Text("D:\\claude".into()),
    });
    let v = view(&s);
    let wsl = row(&v, "Read WSL folders");
    assert_eq!(wsl.kind, RowKind::Toggle);
    assert!(!wsl.checked);
    assert_eq!(wsl.detail, "Starts the distro.");
    let folder = row(&v, "Extra folder");
    assert_eq!(folder.kind, RowKind::Text);
    assert_eq!(folder.text, "D:\\claude");
    assert_eq!(folder.placeholder, "C:\\path");
    assert!(v.rows.iter().any(|r| r.kind == RowKind::Note));

    assert_eq!(
        wsl.edit(Input::Toggled(true)),
        Some(SettingsEdit::ModuleValue {
            assistant: "t".into(),
            key: "wsl".into(),
            value: SettingValue::Bool(true),
        })
    );
    assert_eq!(
        folder.edit(Input::Text("E:\\c".into())),
        Some(SettingsEdit::ModuleValue {
            assistant: "t".into(),
            key: "folder".into(),
            value: SettingValue::Text("E:\\c".into()),
        })
    );
}

fn action_field() -> SettingField {
    SettingField {
        key: "hook",
        label: "Statusline push".into(),
        help: Some("Not installed.".into()),
        kind: FieldKind::Action {
            button: "Install".into(),
            confirm: Some("Change the file now?".into()),
        },
    }
}

#[test]
fn an_action_field_is_a_button_that_asks_before_running() {
    let mut c = catalog(false, false);
    c.assistants[0].fields.push(action_field());
    let v = settings_view(&c, &state(&["a"]), &Settings::default(), false);
    let r = row(&v, "Statusline push");
    assert_eq!(r.kind, RowKind::Action);
    assert_eq!(r.text, "Install", "the button's label");
    assert_eq!(r.detail, "Not installed.");
    assert_eq!(
        r.action(),
        Some(ActionRequest {
            assistant: "t".into(),
            key: "hook".into(),
            confirm: Some("Change the file now?".into()),
        })
    );
    assert_eq!(r.edit(Input::Toggled(true)), None, "it stores no value");
    assert_eq!(row(&v, "Read WSL folders").action(), None);
}

#[test]
fn actions_alone_need_no_apply_on_restart_note() {
    let mut c = catalog(false, false);
    c.assistants[0].fields = vec![action_field()];
    let v = settings_view(&c, &state(&["a"]), &Settings::default(), false);
    assert!(!v.rows.iter().any(|r| r.kind == RowKind::Note));
}

#[test]
fn polling_offers_the_default_and_warns_below_five_minutes() {
    let v = view(&Settings::default());
    assert_eq!(v.poll_index, 0);
    assert!(v.poll_options[0].contains("Default"));
    assert_eq!(v.poll_warning, None);

    let two = v
        .poll_options
        .iter()
        .position(|o| o == "Every 2 min")
        .expect("2 min option");
    let edit = v.poll_edit(two).expect("an edit");
    assert_eq!(
        edit,
        SettingsEdit::ActivePoll(Some(Duration::from_secs(120)))
    );
    assert_eq!(v.poll_edit(0), Some(SettingsEdit::ActivePoll(None)));

    let mut s = Settings::default();
    s.apply(edit);
    let v = view(&s);
    assert_eq!(v.poll_index, two);
    assert!(
        v.poll_warning
            .as_deref()
            .is_some_and(|w| w.contains("rate limit"))
    );

    s.apply(SettingsEdit::ActivePoll(Some(Duration::from_secs(600))));
    assert_eq!(view(&s).poll_warning, None);
}

#[test]
fn an_interval_from_a_hand_edited_file_is_still_selectable() {
    let mut s = Settings::default();
    s.apply(SettingsEdit::ActivePoll(Some(Duration::from_secs(7 * 60))));
    let v = view(&s);
    assert_eq!(v.poll_options[v.poll_index], "Every 7 min");
}

#[test]
fn the_switches_show_reset_notifications_and_start_with_windows() {
    let v = settings_view(
        &catalog(true, true),
        &state(&[]),
        &Settings::default(),
        true,
    );
    assert!(v.reset_notifications);
    assert!(v.start_with_windows);
    let mut s = Settings::default();
    s.apply(SettingsEdit::ResetNotifications(false));
    let v = settings_view(&catalog(true, true), &state(&[]), &s, false);
    assert!(!v.reset_notifications);
    assert!(!v.start_with_windows);
}

#[test]
fn featured_meters_show_their_place_and_move_up_or_down() {
    let v = view(&Settings::default());
    let move_edit = |meter: &str, up| SettingsEdit::MoveFeatured {
        assistant: "t".into(),
        account: "me".into(),
        meter: meter.into(),
        up,
        current: vec!["a".into(), "b".into()],
    };
    let a = row(&v, "a label");
    assert_eq!(a.position, Some(1));
    assert!(!a.can_move_up && a.can_move_down);
    assert_eq!(a.move_edit(false), Some(move_edit("a", false)));
    assert_eq!(a.move_edit(true), None);

    let b = row(&v, "b label");
    assert_eq!(b.position, Some(2));
    assert!(b.can_move_up && !b.can_move_down);
    assert_eq!(b.move_edit(true), Some(move_edit("b", true)));
    assert_eq!(b.move_edit(false), None);

    let c = row(&v, "c label");
    assert_eq!(c.position, None);
    assert!(!c.can_move_up && !c.can_move_down);
    assert_eq!(c.move_edit(true), None);
}

#[test]
fn a_single_featured_meter_has_nowhere_to_move() {
    let v = settings_view(
        &catalog(true, true),
        &state(&["b"]),
        &Settings::default(),
        false,
    );
    let b = row(&v, "b label");
    assert_eq!(b.position, Some(1));
    assert!(!b.can_move_up && !b.can_move_down);
}

#[test]
fn the_tray_icon_page_offers_every_gauge() {
    let v = view(&Settings::default());
    assert_eq!(
        v.gauge_options,
        vec!["Rings", "Bars", "Number", "Stacked numbers"]
    );
    assert_eq!(v.gauge_index, 0);
    assert!(v.show_badge && v.badge_enabled);
    assert_eq!(
        v.gauge_edit(3),
        Some(SettingsEdit::Gauge(Gauge::StackedNumbers))
    );
    assert_eq!(v.gauge_edit(9), None);
}

#[test]
fn the_badge_switch_is_off_limits_for_number_gauges() {
    for (gauge, index, room) in [
        (Gauge::Bars, 1, true),
        (Gauge::Number, 2, false),
        (Gauge::StackedNumbers, 3, false),
    ] {
        let mut s = Settings::default();
        s.apply(SettingsEdit::Gauge(gauge));
        s.apply(SettingsEdit::ShowBadge(false));
        let v = view(&s);
        assert_eq!(v.gauge_index, index);
        assert!(!v.show_badge);
        assert_eq!(v.badge_enabled, room, "{gauge:?}");
    }
}

#[test]
fn a_meter_without_a_limit_cannot_be_featured() {
    let v = settings_view(
        &catalog(true, true),
        &state(&["a"]),
        &Settings::default(),
        false,
    );
    let credits = row(&v, "credits label");
    assert!(!credits.checked && !credits.enabled);
    assert!(credits.detail.contains("tray icon"), "{}", credits.detail);
    // There is still a free slot for a Meter that has a limit.
    assert!(row(&v, "b label").enabled);
}

#[test]
fn a_meter_without_a_limit_featured_earlier_can_still_be_unchecked() {
    let v = settings_view(
        &catalog(true, true),
        &state(&["credits"]),
        &Settings::default(),
        false,
    );
    let credits = row(&v, "credits label");
    assert!(credits.checked && credits.enabled);
}
