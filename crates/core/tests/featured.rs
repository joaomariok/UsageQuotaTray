//! Engine seam for the Featured Meters of an Account that lacks the ones its module
//! suggests: a module-neutral test Assistant whose one Source returns the given Meters.

use std::sync::Arc;

use uqt_core::contract::{Assistant, FetchContext, FetchError, FetchOutput, Intervals, Source};
use uqt_core::engine::{Engine, minutes};
use uqt_core::model::{AccountInfo, BadgeSpec, Exactness, MeterKey, MeterValue, Unit};
use uqt_core::settings::{Settings, SettingsEdit};
use uqt_core::testing::RecordingTransport;
use uqt_core::time::{FakeClock, Timestamp};

const ID: &str = "mixed";
const ACCOUNT: &str = "me";

/// Suggests `first` and `second`, and has whatever Meters it is given.
struct Mixed(Vec<MeterValue>);

impl Assistant for Mixed {
    fn id(&self) -> &'static str {
        ID
    }
    fn display_name(&self) -> &str {
        "Mixed"
    }
    fn allowed_hosts(&self) -> &[&'static str] {
        &[]
    }
    fn badge(&self) -> BadgeSpec {
        BadgeSpec {
            svg: "",
            letters: "M",
            color: "#000000",
        }
    }
    fn suggested_featured(&self) -> Vec<MeterKey> {
        vec!["first".into(), "second".into()]
    }
    fn usage_page(&self) -> Option<&str> {
        None
    }
    fn discover_accounts(&mut self) -> Vec<AccountInfo> {
        vec![AccountInfo {
            id: ACCOUNT.into(),
            display_name: "Me".into(),
        }]
    }
    fn sources(&mut self, _account: &AccountInfo) -> Vec<Box<dyn Source>> {
        vec![Box::new(Fixed(self.0.clone()))]
    }
}

struct Fixed(Vec<MeterValue>);

impl Source for Fixed {
    fn id(&self) -> &str {
        "fixed"
    }
    fn display_name(&self) -> &str {
        "Fixed"
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
    fn fetch(&mut self, _ctx: &FetchContext<'_>) -> Result<FetchOutput, FetchError> {
        Ok(FetchOutput {
            values: self.0.clone(),
            notices: Vec::new(),
        })
    }
}

fn meter(key: &str) -> MeterValue {
    MeterValue {
        key: key.into(),
        label: key.into(),
        used: 1.0,
        limit: Some(10.0),
        unit: Unit::Percent,
        window: None,
        breakdown: None,
        unrecognized: false,
    }
}

fn unrecognized(key: &str) -> MeterValue {
    MeterValue {
        unrecognized: true,
        ..meter(key)
    }
}

/// The Featured Meters after one fetch of `meters`.
fn featured(meters: Vec<MeterValue>, settings: &Settings) -> Vec<MeterKey> {
    let mut engine = Engine::with_settings(
        vec![Box::new(Mixed(meters))],
        Arc::new(FakeClock::new(Timestamp::from_utc(2026, 10, 29, 12, 0, 0))),
        Arc::new(RecordingTransport::new()),
        settings,
    );
    engine.tick();
    engine.state().assistants[0].accounts[0].featured.clone()
}

/// Settings in which the user unchecked `meter` while `current` was featured.
fn unchecked(meter: &str, current: &[&str]) -> Settings {
    let mut s = Settings::default();
    s.apply(SettingsEdit::Featured {
        assistant: ID.into(),
        account: ACCOUNT.into(),
        meter: meter.into(),
        on: false,
        current: current.iter().map(|k| k.to_string()).collect(),
    });
    s
}

#[test]
fn without_any_suggested_meter_the_first_two_recognized_ones_are_featured() {
    let meters = vec![
        unrecognized("raw"),
        meter("a"),
        unrecognized("raw2"),
        meter("b"),
        meter("c"),
    ];
    assert_eq!(featured(meters, &Settings::default()), vec!["a", "b"]);
}

#[test]
fn one_suggested_meter_is_enough_to_keep_the_suggestion() {
    let meters = vec![meter("a"), meter("second")];
    assert_eq!(
        featured(meters, &Settings::default()),
        vec!["first", "second"]
    );
}

#[test]
fn unrecognized_meters_are_never_featured_by_default() {
    let chosen = featured(vec![unrecognized("raw")], &Settings::default());
    assert!(!chosen.contains(&"raw".to_string()), "{chosen:?}");
}

#[test]
fn the_users_choice_wins_even_when_empty_or_missing_from_the_account() {
    let meters = || vec![meter("a"), meter("b")];
    assert!(featured(meters(), &unchecked("a", &["a"])).is_empty());
    assert_eq!(
        featured(meters(), &unchecked("a", &["a", "gone"])),
        vec!["gone"]
    );
}
