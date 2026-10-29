//! Engine seam: the real Engine running the template Assistant against a recorded
//! fixture, a recording fake transport and a fake clock. No real network, no real files.

use std::sync::Arc;

use uqt_assistant_template::{ExampleAssistant, USAGE_URL, keys};
use uqt_core::contract::FetchError;
use uqt_core::engine::Engine;
use uqt_core::http::{HttpError, HttpResponse};
use uqt_core::model::Unit;
use uqt_core::testing::RecordingTransport;
use uqt_core::time::{FakeClock, Timestamp};

const FIXTURE: &str = include_str!("fixtures/usage.json");

fn respond(status: u16, body: &str) -> Result<HttpResponse, HttpError> {
    Ok(HttpResponse {
        status,
        headers: Vec::new(),
        body: body.as_bytes().to_vec(),
    })
}

fn engine_serving(response: Result<HttpResponse, HttpError>) -> (Engine, RecordingTransport) {
    let transport = RecordingTransport::new();
    transport.push_response(response);
    let clock = FakeClock::new(Timestamp::from_utc(2026, 10, 29, 12, 0, 0));
    let engine = Engine::new(
        vec![Box::new(ExampleAssistant::new())],
        Arc::new(clock),
        Arc::new(transport.clone()),
    );
    (engine, transport)
}

fn problem_of(response: Result<HttpResponse, HttpError>) -> FetchError {
    let (mut engine, _) = engine_serving(response);
    engine.tick();
    let state = engine.state();
    state.assistants[0].accounts[0]
        .problem
        .as_ref()
        .expect("a problem is shown")
        .error
        .clone()
}

#[test]
fn the_fixture_becomes_meters_with_a_breakdown_and_an_unrecognized_one() {
    let (mut engine, transport) = engine_serving(respond(200, FIXTURE));
    engine.tick();

    let state = engine.state();
    let account = &state.assistants[0].accounts[0];
    let requests = account.meter(keys::REQUESTS).expect("requests meter");
    assert_eq!(requests.used, 120.0);
    assert_eq!(requests.limit, Some(500.0));
    assert_eq!(requests.unit, Unit::Count("requests".into()));
    assert_eq!(requests.window.expect("window").end.0, 1_793_283_600);
    assert!(!requests.unrecognized);
    let parts = &requests.breakdown.as_ref().expect("breakdown").parts;
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0].label, "small");
    assert_eq!(parts[0].percent, 70.0);

    let extra = account.meter("unrecognized:burst_credits").expect("kept");
    assert!(extra.unrecognized);
    assert_eq!(extra.label, "burst_credits");

    assert_eq!(transport.requests().len(), 1);
    assert_eq!(transport.requests()[0].url, USAGE_URL);
}

#[test]
fn an_unexpected_shape_is_reported_as_a_changed_endpoint() {
    assert_eq!(
        problem_of(respond(200, r#"{"surprise": true}"#)),
        FetchError::EndpointChanged
    );
}

#[test]
fn a_401_means_the_login_expired_and_a_429_is_rate_limited() {
    assert_eq!(problem_of(respond(401, "")), FetchError::Expired);
    assert!(matches!(
        problem_of(respond(429, "")),
        FetchError::RateLimited { .. }
    ));
}

#[test]
fn a_transport_failure_is_offline() {
    assert_eq!(
        problem_of(Err(HttpError::Transport("down".into()))),
        FetchError::Offline
    );
}
