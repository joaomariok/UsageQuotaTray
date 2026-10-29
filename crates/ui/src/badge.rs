//! Badge rules for SVGs shipped in the repo, and the user's local override.
//!
//! Shipped Badges must have a square viewBox and contain only filled paths: no text,
//! strokes, gradients, filters, images or external references. A user may drop their
//! own `<assistant id>.svg` in the app-data badges folder; it keeps its own colors and
//! only has to render.

use std::path::Path;

use resvg::usvg::{self, roxmltree};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BadgeRuleError {
    Malformed(String),
    ViewBoxMissing,
    ViewBoxNotSquare,
    ElementNotAllowed(String),
    AttributeNotAllowed(String),
    FillNotSolid,
}

const ALLOWED_ELEMENTS: &[&str] = &["svg", "g", "path"];

/// Checks a shipped Badge against the rules, listing every violation.
pub fn validate_badge_svg(svg: &str) -> Result<(), Vec<BadgeRuleError>> {
    let doc = roxmltree::Document::parse(svg)
        .map_err(|e| vec![BadgeRuleError::Malformed(e.to_string())])?;
    let mut errors = Vec::new();
    let mut push = |e: BadgeRuleError| {
        if !errors.contains(&e) {
            errors.push(e);
        }
    };
    let root = doc.root_element();
    match root.attribute("viewBox").map(parse_view_box) {
        None => push(BadgeRuleError::ViewBoxMissing),
        Some(None) => push(BadgeRuleError::Malformed("viewBox".into())),
        Some(Some((w, h))) if (w - h).abs() > f64::EPSILON || w <= 0.0 => {
            push(BadgeRuleError::ViewBoxNotSquare)
        }
        Some(Some(_)) => {}
    }
    for node in root.descendants().filter(|n| n.is_element()) {
        let name = node.tag_name().name();
        if !ALLOWED_ELEMENTS.contains(&name) {
            push(BadgeRuleError::ElementNotAllowed(name.into()));
        }
        for attr in node.attributes() {
            let attr_name = attr.name();
            if attr_name.starts_with("stroke")
                || matches!(
                    attr_name,
                    "style" | "filter" | "href" | "mask" | "clip-path"
                )
            {
                push(BadgeRuleError::AttributeNotAllowed(attr_name.into()));
            }
            if attr_name == "fill" && attr.value().trim_start().starts_with("url(") {
                push(BadgeRuleError::FillNotSolid);
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn parse_view_box(value: &str) -> Option<(f64, f64)> {
    let numbers: Vec<f64> = value
        .split([' ', ','])
        .filter(|s| !s.is_empty())
        .map(|s| s.parse().ok())
        .collect::<Option<_>>()?;
    match numbers[..] {
        [_, _, w, h] => Some((w, h)),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BadgeOrigin {
    Shipped,
    Override,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChosenBadge {
    pub svg: String,
    pub origin: BadgeOrigin,
}

/// The user's `<badges_dir>/<assistant_id>.svg` when it exists and renders, otherwise
/// the shipped Badge.
pub fn resolve_badge(assistant_id: &str, shipped: &str, badges_dir: Option<&Path>) -> ChosenBadge {
    let custom = badges_dir
        .map(|d| d.join(format!("{assistant_id}.svg")))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .filter(|svg| usvg::Tree::from_str(svg, &usvg::Options::default()).is_ok());
    match custom {
        Some(svg) => ChosenBadge {
            svg,
            origin: BadgeOrigin::Override,
        },
        None => ChosenBadge {
            svg: shipped.to_string(),
            origin: BadgeOrigin::Shipped,
        },
    }
}
