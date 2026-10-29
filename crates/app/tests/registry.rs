//! The composition root's module registry.

use uqt_app::{ModuleOptions, all_modules, enabled_modules, fake_failure};
use uqt_core::contract::FetchError;
use uqt_ui::badge::validate_badge_svg;

#[test]
fn every_registered_module_badge_follows_the_badge_rules() {
    let modules = all_modules();
    assert!(!modules.is_empty());
    for m in &modules {
        assert_eq!(validate_badge_svg(m.badge().svg), Ok(()), "{}", m.id());
        let letters = m.badge().letters.chars().count();
        assert!((1..=2).contains(&letters), "{}", m.id());
    }
}

#[test]
fn module_ids_are_unique() {
    let mut ids: Vec<&str> = all_modules().iter().map(|m| m.id()).collect();
    let count = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), count);
}

#[test]
fn the_fake_assistant_runs_only_when_enabled() {
    let ids = |fake| {
        enabled_modules(&ModuleOptions {
            claude_code: false,
            fake_assistant: fake,
            fake_failure: None,
        })
        .iter()
        .map(|m| m.id())
        .collect::<Vec<_>>()
    };
    assert!(ids(true).contains(&"fake"));
    assert!(!ids(false).contains(&"fake"));
}

#[test]
fn fake_failures_can_be_simulated_by_name() {
    assert_eq!(fake_failure("offline"), Some(FetchError::Offline));
    assert_eq!(fake_failure("expired"), Some(FetchError::Expired));
    assert_eq!(fake_failure("not-logged-in"), Some(FetchError::NotLoggedIn));
    assert_eq!(fake_failure("changed"), Some(FetchError::EndpointChanged));
    assert_eq!(
        fake_failure("rate-limited"),
        Some(FetchError::RateLimited { retry_after: None })
    );
    assert_eq!(fake_failure("nonsense"), None);
}

#[test]
fn claude_code_is_registered_and_runs_unless_disabled() {
    assert!(all_modules().iter().any(|m| m.id() == "claude-code"));
    let ids = |claude_code| {
        enabled_modules(&ModuleOptions {
            claude_code,
            fake_assistant: false,
            fake_failure: None,
        })
        .iter()
        .map(|m| m.id())
        .collect::<Vec<_>>()
    };
    assert_eq!(ids(true), vec!["claude-code"]);
    assert!(ids(false).is_empty());
}
