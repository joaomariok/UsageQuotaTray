//! The Assistant-neutral Meter model (vocabulary in GLOSSARY.md).

use std::time::Duration;

use crate::time::Timestamp;

/// A Meter's stable key within its Account, chosen by the module.
pub type MeterKey = String;

/// What a Meter counts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unit {
    /// Percent of an allowance the maker does not disclose; the Limit is 100.
    Percent,
    /// Money in an ISO 4217 currency code.
    Currency(String),
    /// A count of something, with its unit name ("requests").
    Count(String),
}

/// The span over which a Meter accumulates before it resets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    /// When the Meter resets.
    pub end: Timestamp,
    pub start: Option<Timestamp>,
    pub length: Option<Duration>,
}

impl Window {
    /// The start as given, or derived from end minus length.
    pub fn effective_start(&self) -> Option<Timestamp> {
        self.start.or_else(|| {
            self.length
                .map(|l| Timestamp(self.end.0 - l.as_secs() as i64))
        })
    }
}

/// Whether a value came from the Assistant's maker or was derived locally.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exactness {
    Exact,
    Estimate,
}

/// One Meter's value as a Source reports it.
#[derive(Clone, Debug, PartialEq)]
pub struct MeterValue {
    pub key: MeterKey,
    pub label: String,
    pub used: f64,
    pub limit: Option<f64>,
    pub unit: Unit,
    pub window: Option<Window>,
}

/// One Meter's value at one moment from one Source.
#[derive(Clone, Debug, PartialEq)]
pub struct Reading {
    pub value: MeterValue,
    pub source_id: String,
    pub source_name: String,
    pub exactness: Exactness,
    pub fetched_at: Timestamp,
}

/// An Account-level message supplied by a module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub text: String,
}

/// One login to an Assistant. `id` is the maker's identity for the login.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountInfo {
    pub id: String,
    pub display_name: String,
}

/// How the tray icon marks an Assistant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BadgeSpec {
    /// SVG source following the Badge rules (square viewBox, filled paths only).
    pub svg: &'static str,
    /// One or two letters for places an SVG cannot go.
    pub letters: &'static str,
    /// `#RRGGBB`.
    pub color: &'static str,
}
