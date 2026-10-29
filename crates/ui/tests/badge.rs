//! Badge rules for SVGs shipped in the repo, and the user's local override.

use std::fs;

use uqt_ui::badge::{BadgeOrigin, BadgeRuleError, resolve_badge, validate_badge_svg};

const GOOD: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><g><path fill="#2BA3A0" d="M2 2h20v20H2z"/></g></svg>"##;

fn rejects(svg: &str) -> Vec<BadgeRuleError> {
    validate_badge_svg(svg).expect_err("should be rejected")
}

#[test]
fn accepts_filled_paths_in_a_square_view_box() {
    assert_eq!(validate_badge_svg(GOOD), Ok(()));
}

#[test]
fn requires_a_square_view_box() {
    assert_eq!(
        rejects(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 20"><path d="M0 0h1v1z"/></svg>"#
        ),
        vec![BadgeRuleError::ViewBoxNotSquare]
    );
    assert_eq!(
        rejects(r#"<svg xmlns="http://www.w3.org/2000/svg"><path d="M0 0h1v1z"/></svg>"#),
        vec![BadgeRuleError::ViewBoxMissing]
    );
}

#[test]
fn rejects_text_gradients_filters_images_and_strokes() {
    let wrap = |inner: &str| {
        format!(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">{inner}</svg>"#)
    };
    for (inner, element) in [
        ("<text>A</text>", "text"),
        ("<linearGradient id=\"g\"/>", "linearGradient"),
        ("<filter id=\"f\"/>", "filter"),
        ("<image href=\"x.png\"/>", "image"),
        ("<circle r=\"3\"/>", "circle"),
    ] {
        assert!(
            rejects(&wrap(inner)).contains(&BadgeRuleError::ElementNotAllowed(element.into())),
            "{element}"
        );
    }
    assert_eq!(
        rejects(&wrap(r##"<path stroke="#000" d="M0 0h1v1z"/>"##)),
        vec![BadgeRuleError::AttributeNotAllowed("stroke".into())]
    );
    assert_eq!(
        rejects(&wrap(r#"<path style="stroke:red" d="M0 0h1v1z"/>"#)),
        vec![BadgeRuleError::AttributeNotAllowed("style".into())]
    );
    assert_eq!(
        rejects(&wrap(r#"<path fill="url(#g)" d="M0 0h1v1z"/>"#)),
        vec![BadgeRuleError::FillNotSolid]
    );
}

#[test]
fn rejects_external_references() {
    let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" viewBox="0 0 24 24"><path xlink:href="https://example.com/x.svg" d="M0 0h1v1z"/></svg>"#;
    assert_eq!(
        rejects(svg),
        vec![BadgeRuleError::AttributeNotAllowed("href".into())]
    );
}

#[test]
fn rejects_malformed_xml() {
    assert!(matches!(
        rejects("<svg")[..],
        [BadgeRuleError::Malformed(_)]
    ));
}

#[test]
fn a_local_override_replaces_the_shipped_badge() {
    let dir = tempfile::tempdir().unwrap();
    // Override SVGs keep their own colors and are not held to the repo rules.
    let custom = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><circle cx="5" cy="5" r="5" fill="#f00"/></svg>"##;
    fs::write(dir.path().join("fake.svg"), custom).unwrap();

    let chosen = resolve_badge("fake", GOOD, Some(dir.path()));
    assert_eq!(chosen.origin, BadgeOrigin::Override);
    assert_eq!(chosen.svg, custom);

    let other = resolve_badge("claude-code", GOOD, Some(dir.path()));
    assert_eq!(other.origin, BadgeOrigin::Shipped);
    assert_eq!(other.svg, GOOD);
}

#[test]
fn an_unreadable_override_falls_back_to_the_shipped_badge() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("fake.svg"), "not svg at all").unwrap();
    let chosen = resolve_badge("fake", GOOD, Some(dir.path()));
    assert_eq!(chosen.origin, BadgeOrigin::Shipped);
}

#[test]
fn without_a_badges_folder_the_shipped_badge_is_used() {
    assert_eq!(
        resolve_badge("fake", GOOD, None).origin,
        BadgeOrigin::Shipped
    );
}
