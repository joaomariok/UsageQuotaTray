//! Engine and scheduler-thread seam for module actions (a settings button such as
//! "Install"): the module runs the action, the outcome is published once, and the
//! catalog shows the module's settings as they are afterwards.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use uqt_core::contract::{
    Assistant, FetchContext, FetchError, FetchOutput, FieldKind, Intervals, SettingField, Source,
};
use uqt_core::engine::{ActionOutcome, Engine, minutes};
use uqt_core::model::{AccountInfo, BadgeSpec, Exactness, MeterKey, MeterValue, Unit};
use uqt_core::push::PushMessage;
use uqt_core::runner::EngineThread;
use uqt_core::testing::RecordingTransport;
use uqt_core::time::{FakeClock, Timestamp};

/// A module with one button that flips between "Install" and "Uninstall".
struct Switchy {
    installed: Arc<Mutex<bool>>,
}

impl Assistant for Switchy {
    fn id(&self) -> &'static str {
        "switchy"
    }
    fn display_name(&self) -> &str {
        "Switchy"
    }
    fn allowed_hosts(&self) -> &[&'static str] {
        &[]
    }
    fn badge(&self) -> BadgeSpec {
        BadgeSpec {
            svg: "",
            letters: "S",
            color: "#000000",
        }
    }
    fn suggested_featured(&self) -> Vec<MeterKey> {
        Vec::new()
    }
    fn usage_page(&self) -> Option<&str> {
        None
    }
    fn settings_fields(&self) -> Vec<SettingField> {
        let on = *self.installed.lock().unwrap();
        vec![SettingField {
            key: "hook",
            label: "Hook".into(),
            help: Some(if on { "Installed." } else { "Not installed." }.into()),
            kind: FieldKind::Action {
                button: if on { "Uninstall" } else { "Install" }.into(),
                confirm: (!on).then(|| "Install the hook?".to_string()),
            },
        }]
    }
    fn run_action(&mut self, key: &str) -> Result<String, String> {
        if key != "hook" {
            return Err(format!("no action {key}"));
        }
        let mut on = self.installed.lock().unwrap();
        *on = !*on;
        Ok(if *on { "Installed." } else { "Removed." }.into())
    }
    fn discover_accounts(&mut self) -> Vec<AccountInfo> {
        vec![AccountInfo {
            id: "a".into(),
            display_name: "A".into(),
        }]
    }
    fn sources(&mut self, _account: &AccountInfo) -> Vec<Box<dyn Source>> {
        vec![Box::new(Hook)]
    }
}

struct Hook;

impl Source for Hook {
    fn id(&self) -> &str {
        "hook"
    }
    fn display_name(&self) -> &str {
        "Hook"
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
    fn push_only(&self) -> bool {
        true
    }
    fn fetch(&mut self, _ctx: &FetchContext<'_>) -> Result<FetchOutput, FetchError> {
        Err(FetchError::Other("never polled".into()))
    }
    fn accept_push(&mut self, message: &PushMessage) -> Option<FetchOutput> {
        Some(FetchOutput {
            values: vec![MeterValue {
                key: "n".into(),
                label: "N".into(),
                used: message.payload["n"].as_f64()?,
                limit: None,
                unit: Unit::Count("things".into()),
                window: None,
            }],
            notices: Vec::new(),
        })
    }
}

fn switchy() -> Switchy {
    Switchy {
        installed: Arc::new(Mutex::new(false)),
    }
}

fn engine(module: Switchy) -> (Engine, FakeClock) {
    let clock = FakeClock::new(Timestamp::from_utc(2026, 10, 29, 12, 0, 0));
    let engine = Engine::new(
        vec![Box::new(module)],
        Arc::new(clock.clone()),
        Arc::new(RecordingTransport::new()),
    );
    (engine, clock)
}

fn button(engine: &Engine) -> String {
    match &engine.catalog().assistants[0].fields[0].kind {
        FieldKind::Action { button, .. } => button.clone(),
        other => panic!("not an action: {other:?}"),
    }
}

#[test]
fn an_action_runs_in_the_module_and_its_outcome_is_published_once() {
    let (mut engine, _) = engine(switchy());
    assert_eq!(button(&engine), "Install");

    engine.run_action("switchy", "hook");
    assert_eq!(
        engine.take_outcomes(),
        vec![ActionOutcome {
            assistant_id: "switchy".into(),
            key: "hook".into(),
            result: Ok("Installed.".into()),
        }]
    );
    assert!(engine.take_outcomes().is_empty());
    assert_eq!(button(&engine), "Uninstall");
}

#[test]
fn an_unknown_module_or_action_reports_an_error() {
    let (mut engine, _) = engine(switchy());
    engine.run_action("nobody", "hook");
    engine.run_action("switchy", "other");
    let outcomes = engine.take_outcomes();
    assert_eq!(outcomes.len(), 2);
    assert!(outcomes.iter().all(|o| o.result.is_err()), "{outcomes:?}");
}

#[test]
fn the_scheduler_thread_takes_pushes_and_actions() {
    let (engine, clock) = engine(switchy());
    let (tx, rx) = mpsc::channel();
    let thread = EngineThread::spawn(engine, Arc::new(clock), move |u| {
        let _ = tx.send(u);
    });
    let first = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(first.outcomes.is_empty());

    let handle = thread.handle();
    handle.push(PushMessage {
        assistant: "switchy".into(),
        source: "hook".into(),
        origin: String::new(),
        payload: json!({ "n": 3 }),
    });
    let pushed = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let account = &pushed.state.assistants[0].accounts[0];
    assert_eq!(account.meter("n").map(|m| m.used), Some(3.0));

    handle.run_action("switchy".into(), "hook".into());
    let acted = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(acted.outcomes.len(), 1);
    assert_eq!(acted.outcomes[0].result, Ok("Installed.".into()));
    assert!(matches!(
        &acted.catalog.assistants[0].fields[0].kind,
        FieldKind::Action { button, .. } if button == "Uninstall"
    ));
    thread.shutdown();
}
