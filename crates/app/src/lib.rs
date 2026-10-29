//! The composition root: the only crate that names Assistant modules.

use uqt_assistant_fake::{FakeAssistant, FakeControl, sources};
use uqt_core::contract::{Assistant, FetchError};

/// Which modules run, decided at startup.
#[derive(Clone, Debug, Default)]
pub struct ModuleOptions {
    /// On in debug builds, or with `UQT_FAKE_ASSISTANT=1`.
    pub fake_assistant: bool,
    /// Every Fake Source fails with this, to see failure states (`UQT_FAKE_FAIL`).
    pub fake_failure: Option<FetchError>,
}

impl ModuleOptions {
    pub fn from_env() -> Self {
        Self {
            fake_assistant: cfg!(debug_assertions)
                || std::env::var("UQT_FAKE_ASSISTANT").is_ok_and(|v| v == "1"),
            fake_failure: std::env::var("UQT_FAKE_FAIL")
                .ok()
                .and_then(|v| fake_failure(&v)),
        }
    }
}

/// Every module compiled in, enabled or not. Register new Assistants here.
pub fn all_modules() -> Vec<Box<dyn Assistant>> {
    vec![Box::new(FakeAssistant::new())]
}

/// The modules to run.
pub fn enabled_modules(options: &ModuleOptions) -> Vec<Box<dyn Assistant>> {
    let mut modules: Vec<Box<dyn Assistant>> = Vec::new();
    if options.fake_assistant {
        let control = FakeControl::new();
        if let Some(error) = &options.fake_failure {
            control.fail(sources::API, Some(error.clone()));
            control.fail(sources::LOCAL, Some(error.clone()));
        }
        modules.push(Box::new(FakeAssistant::with_control(control)));
    }
    modules
}

/// Parses a `UQT_FAKE_FAIL` value.
pub fn fake_failure(name: &str) -> Option<FetchError> {
    Some(match name {
        "offline" => FetchError::Offline,
        "expired" => FetchError::Expired,
        "not-logged-in" => FetchError::NotLoggedIn,
        "changed" => FetchError::EndpointChanged,
        "rate-limited" => FetchError::RateLimited { retry_after: None },
        _ => return None,
    })
}
