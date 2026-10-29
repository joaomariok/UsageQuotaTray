//! Settings: stored as JSON in the app-data folder, written atomically, and edited
//! through domain rules (at most two Featured Meters, polling no faster than 2 min).
//! Every test works in a temporary folder.

use std::time::Duration;

use uqt_core::settings::{
    DEFAULT_THRESHOLDS, MIN_ACTIVE_POLL, SettingValue, Settings, SettingsEdit, SettingsStore,
    parse_thresholds, thresholds_text,
};

fn store() -> (tempfile::TempDir, SettingsStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = SettingsStore::new(dir.path().join("UsageQuotaTray").join("settings.json"));
    (dir, store)
}

fn edited(settings: &Settings, edit: SettingsEdit) -> Settings {
    let mut s = settings.clone();
    s.apply(edit);
    s
}

#[test]
fn a_missing_file_gives_the_defaults() {
    let (_dir, store) = store();
    let s = store.load();
    assert_eq!(s, Settings::default());
    assert!(s.reset_notifications);
    assert_eq!(s.active_poll, None);
    assert!(s.assistant_enabled("fake"));
    assert!(s.account_enabled("fake", "acc"));
    assert_eq!(s.featured("fake", "acc"), None);
    assert_eq!(
        s.thresholds("fake", "acc", "m"),
        DEFAULT_THRESHOLDS.to_vec()
    );
}

#[test]
fn saved_settings_load_back_and_leave_no_temporary_file() {
    let (_dir, store) = store();
    let mut s = Settings::default();
    s.apply(SettingsEdit::AssistantEnabled {
        assistant: "fake".into(),
        on: false,
    });
    s.apply(SettingsEdit::AccountEnabled {
        assistant: "claude".into(),
        account: "work".into(),
        on: false,
    });
    s.apply(SettingsEdit::Thresholds {
        assistant: "claude".into(),
        account: "me".into(),
        meter: "weekly".into(),
        thresholds: vec![50, 80],
    });
    s.apply(SettingsEdit::ActivePoll(Some(Duration::from_secs(180))));
    s.apply(SettingsEdit::ResetNotifications(false));
    s.apply(SettingsEdit::ModuleValue {
        assistant: "claude".into(),
        key: "folders".into(),
        value: SettingValue::Text("C:\\x".into()),
    });
    store.save(&s).unwrap();

    assert_eq!(store.load(), s);
    let dir = store.path().parent().unwrap();
    let names: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names, vec!["settings.json".to_string()]);
}

#[test]
fn saving_replaces_an_existing_file() {
    let (_dir, store) = store();
    store.save(&Settings::default()).unwrap();
    let s = edited(
        &Settings::default(),
        SettingsEdit::ResetNotifications(false),
    );
    store.save(&s).unwrap();
    assert!(!store.load().reset_notifications);
}

#[test]
fn the_file_is_plain_json_people_can_read() {
    let (_dir, store) = store();
    store
        .save(&edited(
            &Settings::default(),
            SettingsEdit::ActivePoll(Some(Duration::from_secs(120))),
        ))
        .unwrap();
    let text = std::fs::read_to_string(store.path()).unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(json["active_poll_minutes"], 2);
    assert_eq!(json["reset_notifications"], true);
}

#[test]
fn a_corrupt_file_gives_the_defaults_and_is_kept_aside() {
    let (_dir, store) = store();
    std::fs::create_dir_all(store.path().parent().unwrap()).unwrap();
    std::fs::write(store.path(), "{ not json").unwrap();

    assert_eq!(store.load(), Settings::default());
    let aside = store.path().with_extension("json.bad");
    assert_eq!(std::fs::read_to_string(aside).unwrap(), "{ not json");
}

#[test]
fn unknown_and_missing_keys_are_tolerated() {
    let (_dir, store) = store();
    std::fs::create_dir_all(store.path().parent().unwrap()).unwrap();
    std::fs::write(
        store.path(),
        r#"{"reset_notifications": false, "from_the_future": 1}"#,
    )
    .unwrap();
    let s = store.load();
    assert!(!s.reset_notifications);
    assert_eq!(s.active_poll, None);
}

#[test]
fn values_out_of_range_in_the_file_are_corrected_on_load() {
    let (_dir, store) = store();
    std::fs::create_dir_all(store.path().parent().unwrap()).unwrap();
    std::fs::write(
        store.path(),
        r#"{"active_poll_minutes": 1, "assistants": {"a": {"accounts": {"x": {
            "featured": ["m1", "m2", "m3"], "thresholds": {"m": [100, 0, 75, 75]}}}}}}"#,
    )
    .unwrap();
    let s = store.load();
    assert_eq!(s.active_poll, Some(MIN_ACTIVE_POLL));
    assert_eq!(
        s.featured("a", "x"),
        Some(vec!["m1".to_string(), "m2".to_string()])
    );
    assert_eq!(s.thresholds("a", "x", "m"), vec![75, 100]);
}

#[test]
fn at_most_two_meters_are_featured_starting_from_the_current_choice() {
    let current = vec!["five_hour".to_string(), "weekly".to_string()];
    let feature = |s: &Settings, meter: &str, on: bool, current: &[String]| {
        edited(
            s,
            SettingsEdit::Featured {
                assistant: "c".into(),
                account: "me".into(),
                meter: meter.into(),
                on,
                current: current.to_vec(),
            },
        )
    };
    let s = feature(&Settings::default(), "opus", true, &current);
    assert_eq!(s.featured("c", "me"), Some(current.clone()));

    let s = feature(&s, "weekly", false, &current);
    assert_eq!(s.featured("c", "me"), Some(vec!["five_hour".to_string()]));

    let now = s.featured("c", "me").unwrap();
    let s = feature(&s, "opus", true, &now);
    assert_eq!(
        s.featured("c", "me"),
        Some(vec!["five_hour".to_string(), "opus".to_string()])
    );
}

#[test]
fn active_polling_cannot_go_below_two_minutes() {
    let s = edited(
        &Settings::default(),
        SettingsEdit::ActivePoll(Some(Duration::from_secs(30))),
    );
    assert_eq!(s.active_poll, Some(MIN_ACTIVE_POLL));
    assert_eq!(MIN_ACTIVE_POLL, Duration::from_secs(120));
    let s = edited(&s, SettingsEdit::ActivePoll(None));
    assert_eq!(s.active_poll, None);
}

#[test]
fn thresholds_back_at_the_default_drop_the_override() {
    let set = |s: &Settings, t: Vec<u32>| {
        edited(
            s,
            SettingsEdit::Thresholds {
                assistant: "c".into(),
                account: "me".into(),
                meter: "weekly".into(),
                thresholds: t,
            },
        )
    };
    let s = set(&Settings::default(), vec![]);
    assert_eq!(s.thresholds("c", "me", "weekly"), Vec::<u32>::new());
    let s = set(&s, DEFAULT_THRESHOLDS.to_vec());
    assert_eq!(s, Settings::default());
}

#[test]
fn thresholds_are_typed_as_a_comma_list_or_off() {
    assert_eq!(parse_thresholds("75, 90, 100"), Some(vec![75, 90, 100]));
    assert_eq!(parse_thresholds(" 90 50 "), Some(vec![50, 90]));
    assert_eq!(parse_thresholds("120"), Some(vec![120]));
    assert_eq!(parse_thresholds(""), Some(DEFAULT_THRESHOLDS.to_vec()));
    assert_eq!(parse_thresholds("off"), Some(vec![]));
    assert_eq!(parse_thresholds("Off"), Some(vec![]));
    assert_eq!(parse_thresholds("75, x"), None);
    assert_eq!(parse_thresholds("0"), None);
    assert_eq!(parse_thresholds("1000"), None);

    assert_eq!(thresholds_text(&[75, 90, 100]), "75, 90, 100");
    assert_eq!(thresholds_text(&[]), "off");
}
