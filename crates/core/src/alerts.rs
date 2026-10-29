//! Alert rules: thresholds in percent of the Limit, each raised at most once per
//! Window, and a reset alert only for Windows that had crossed the first threshold.

use crate::engine::{Freshness, MeterState};
use crate::model::MeterKey;
use crate::time::Timestamp;

/// Reset times a Source reports may move by a few seconds between fetches; a Window
/// end that moves further than this is a new Window.
const WINDOW_JITTER_SECS: i64 = 60;
/// Rounding slack so 75.0 % of a float Limit counts as reaching 75 %.
const EPSILON_PERCENT: f64 = 1e-9;

/// Something the user should be told about one Meter.
#[derive(Clone, Debug, PartialEq)]
pub struct Alert {
    pub assistant_id: &'static str,
    pub assistant_name: String,
    pub account_id: String,
    pub account_name: String,
    pub meter_key: MeterKey,
    pub meter_label: String,
    /// When the Meter's current Window ends, if it has one.
    pub window_end: Option<Timestamp>,
    pub kind: AlertKind,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AlertKind {
    /// Usage reached `percent` of the Limit; when it jumped past several thresholds at
    /// once, this is the highest of them.
    Threshold { percent: u32, used_fraction: f64 },
    /// The Window ended after usage had crossed the first threshold.
    Reset,
}

/// What has been raised for one Meter in its current Window.
#[derive(Clone, Debug, Default)]
pub(crate) struct AlertMemory {
    /// The Window being tracked; `None` before the first Reading and after a reset.
    window_end: Option<Timestamp>,
    fired: Vec<u32>,
    crossed_first: bool,
}

impl AlertMemory {
    /// Takes in one Meter's published state and returns what to raise. `thresholds`
    /// are ascending; empty means alerts are off for the Meter.
    pub(crate) fn observe(
        &mut self,
        m: &MeterState,
        thresholds: &[u32],
        reset_alerts: bool,
    ) -> Vec<AlertKind> {
        let mut out = Vec::new();
        let Some(fraction) = m.fraction() else {
            return out;
        };
        match m.freshness {
            Freshness::Stale => return out,
            Freshness::PastReset => {
                if self.window_end.is_some() {
                    if self.crossed_first && reset_alerts {
                        out.push(AlertKind::Reset);
                    }
                    *self = Self::default();
                }
                return out;
            }
            Freshness::Fresh => {}
        }
        let percent = fraction * 100.0 + EPSILON_PERCENT;
        match (m.window, self.window_end) {
            (Some(w), Some(old)) if (w.end.0 - old.0).abs() > WINDOW_JITTER_SECS => {
                if self.crossed_first && reset_alerts {
                    out.push(AlertKind::Reset);
                }
                self.fired.clear();
                self.crossed_first = false;
            }
            // Without a Window, a drop below the first threshold is the only sign of a
            // new period.
            (None, _) if thresholds.first().is_some_and(|t| percent < f64::from(*t)) => {
                self.fired.clear();
                self.crossed_first = false;
            }
            _ => {}
        }
        self.window_end = m.window.map(|w| w.end);

        let newly: Vec<u32> = thresholds
            .iter()
            .copied()
            .filter(|t| percent >= f64::from(*t) && !self.fired.contains(t))
            .collect();
        if let Some(&highest) = newly.last() {
            out.push(AlertKind::Threshold {
                percent: highest,
                used_fraction: fraction,
            });
            self.fired.extend(newly);
        }
        if thresholds.first().is_some_and(|t| percent >= f64::from(*t)) {
            self.crossed_first = true;
        }
        out
    }
}
