//! The engine: runs every Source of every Account on its schedule, keeps the newest
//! Reading per Source and Meter, and publishes the selected per-Account state.

use std::sync::Arc;
use std::time::Duration;

use crate::contract::{Assistant, FetchContext, FetchError, Source};
use crate::http::{HostClient, Transport};
use crate::model::{AccountInfo, BadgeSpec, Exactness, MeterKey, Notice, Reading, Unit, Window};
use crate::time::{Clock, Timestamp};

const MINUTE: i64 = 60;
/// Automatic refreshes (unlock, resume, popup open) run at most this often.
const REFRESH_DEBOUNCE_SECS: i64 = 60;
const BACKOFF_CAP_SECS: i64 = 30 * MINUTE;
const ERROR_BACKOFF_START_SECS: i64 = MINUTE;
const RATE_LIMIT_BACKOFF_START_SECS: i64 = 5 * MINUTE;
/// Pace is not extrapolated during the first 5% of a Window.
const PACE_WARMUP_FRACTION: f64 = 0.05;
/// Used fraction this far below the elapsed fraction counts as "ahead".
const PACE_AHEAD_MARGIN: f64 = 0.10;

/// Whether the user is at this PC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presence {
    /// Unlocked with recent input.
    Active,
    /// Unlocked, no recent input.
    Idle,
    /// Locked or asleep: no polling.
    Paused,
}

/// Why an immediate refresh was asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefreshReason {
    Unlock,
    Resume,
    PopupOpen,
    /// "Refresh now" from the menu: not debounced and skips error backoff, but still
    /// honors rate limits.
    Manual,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Freshness {
    Fresh,
    /// Older than its Source's freshness limit.
    Stale,
    /// Its Window has ended; no Reading for the new Window yet.
    PastReset,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pace {
    NotStarted,
    OnPace,
    Ahead,
    WillHitLimitAt(Timestamp),
    LimitReached,
}

/// A Meter as published: the selected Reading plus derived facts.
#[derive(Clone, Debug, PartialEq)]
pub struct MeterState {
    pub key: MeterKey,
    pub label: String,
    pub used: f64,
    pub limit: Option<f64>,
    pub unit: Unit,
    pub window: Option<Window>,
    pub source_name: String,
    pub exactness: Exactness,
    pub fetched_at: Timestamp,
    pub freshness: Freshness,
    /// Only for fresh Meters with a Limit and a Window.
    pub pace: Option<Pace>,
}

impl MeterState {
    /// Used over Limit, when there is a Limit.
    pub fn fraction(&self) -> Option<f64> {
        self.limit.filter(|l| *l > 0.0).map(|l| self.used / l)
    }
}

/// The most important failing Source of an Account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceProblem {
    pub source_name: String,
    pub error: FetchError,
    /// The module's advice for this failure, if any.
    pub hint: Option<String>,
    /// When the current run of failures began.
    pub since: Timestamp,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AccountState {
    pub id: String,
    pub display_name: String,
    pub featured: Vec<MeterKey>,
    pub meters: Vec<MeterState>,
    pub notices: Vec<Notice>,
    pub problem: Option<SourceProblem>,
}

impl AccountState {
    pub fn meter(&self, key: &str) -> Option<&MeterState> {
        self.meters.iter().find(|m| m.key == key)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AssistantState {
    pub id: &'static str,
    pub display_name: String,
    pub badge: BadgeSpec,
    pub usage_page: Option<String>,
    pub accounts: Vec<AccountState>,
}

/// Everything the UI shows, computed at `now`.
#[derive(Clone, Debug, PartialEq)]
pub struct EngineState {
    pub now: Timestamp,
    pub assistants: Vec<AssistantState>,
}

struct SourceRun {
    source: Box<dyn Source>,
    last_attempt: Option<Timestamp>,
    /// Earliest next attempt after a failure, and whether a manual refresh may skip it.
    retry_at: Option<(Timestamp, Hold)>,
    failures: u32,
    problem: Option<SourceProblem>,
    forced: Option<(Timestamp, RefreshReason)>,
    notices: Vec<Notice>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Hold {
    /// Error backoff: a manual refresh may retry early.
    Soft,
    /// Rate limit: nothing retries early.
    Hard,
}

impl SourceRun {
    fn interval(&self, presence: Presence) -> i64 {
        let i = self.source.intervals();
        let d = match presence {
            Presence::Active => i.active,
            Presence::Idle | Presence::Paused => i.idle,
        };
        d.as_secs() as i64
    }

    fn freshness_limit(&self, presence: Presence) -> i64 {
        self.source
            .freshness_limit()
            .map_or(2 * self.interval(presence), |d| d.as_secs() as i64)
    }

    fn due_at(&self, presence: Presence) -> Option<Timestamp> {
        if presence == Presence::Paused {
            return None;
        }
        let Some(last) = self.last_attempt else {
            return Some(Timestamp(i64::MIN));
        };
        let scheduled = match self.retry_at {
            Some((at, _)) => at,
            None => last.plus_secs(self.interval(presence)),
        };
        let forced = self.forced.and_then(|(at, reason)| match self.retry_at {
            Some((_, Hold::Hard)) => None,
            Some((_, Hold::Soft)) if reason != RefreshReason::Manual => None,
            _ => Some(at),
        });
        Some(forced.map_or(scheduled, |f| f.min(scheduled)))
    }
}

struct AccountRun {
    info: AccountInfo,
    sources: Vec<SourceRun>,
    /// Newest Reading per Source and Meter key.
    readings: Vec<Reading>,
    /// Meter keys in order of first appearance.
    order: Vec<MeterKey>,
}

struct AssistantRun {
    module: Box<dyn Assistant>,
    http: HostClient,
    accounts: Vec<AccountRun>,
}

pub struct Engine {
    assistants: Vec<AssistantRun>,
    clock: Arc<dyn Clock>,
    presence: Presence,
    last_forced: Option<Timestamp>,
}

impl Engine {
    /// Discovers every module's Accounts and their Sources. Nothing is fetched until
    /// the first [`Engine::tick`].
    pub fn new(
        modules: Vec<Box<dyn Assistant>>,
        clock: Arc<dyn Clock>,
        transport: Arc<dyn Transport>,
    ) -> Self {
        let assistants = modules
            .into_iter()
            .map(|mut module| {
                let http = HostClient::new(module.allowed_hosts(), transport.clone());
                let accounts = module
                    .discover_accounts()
                    .into_iter()
                    .map(|info| {
                        let mut sources: Vec<SourceRun> = module
                            .sources(&info)
                            .into_iter()
                            .map(|source| SourceRun {
                                source,
                                last_attempt: None,
                                retry_at: None,
                                failures: 0,
                                problem: None,
                                forced: None,
                                notices: Vec::new(),
                            })
                            .collect();
                        sources.sort_by_key(|s| s.source.priority());
                        AccountRun {
                            info,
                            sources,
                            readings: Vec::new(),
                            order: Vec::new(),
                        }
                    })
                    .collect();
                AssistantRun {
                    module,
                    http,
                    accounts,
                }
            })
            .collect();
        Self {
            assistants,
            clock,
            presence: Presence::Active,
            last_forced: None,
        }
    }

    pub fn presence(&self) -> Presence {
        self.presence
    }

    pub fn set_presence(&mut self, presence: Presence) {
        self.presence = presence;
    }

    /// Asks for an immediate fetch of every Source. Automatic reasons are debounced to
    /// once a minute; rate limits are always honored.
    pub fn request_refresh(&mut self, reason: RefreshReason) {
        let now = self.clock.now();
        if reason != RefreshReason::Manual
            && self
                .last_forced
                .is_some_and(|t| now.0 - t.0 < REFRESH_DEBOUNCE_SECS)
        {
            return;
        }
        self.last_forced = Some(now);
        for source in self.sources_mut() {
            source.forced = Some((now, reason));
        }
    }

    /// When the next Source is due, or `None` while paused.
    pub fn next_wake(&self) -> Option<Timestamp> {
        let now = self.clock.now();
        self.assistants
            .iter()
            .flat_map(|a| &a.accounts)
            .flat_map(|acc| &acc.sources)
            .filter_map(|s| s.due_at(self.presence))
            .min()
            .map(|t| t.max(now))
    }

    /// Fetches every due Source. Returns whether anything was attempted.
    pub fn tick(&mut self) -> bool {
        let now = self.clock.now();
        let presence = self.presence;
        let mut attempted = false;
        for assistant in &mut self.assistants {
            let ctx = FetchContext {
                now,
                http: &assistant.http,
            };
            for account in &mut assistant.accounts {
                for run in &mut account.sources {
                    if run.due_at(presence).is_none_or(|due| due > now) {
                        continue;
                    }
                    attempted = true;
                    let result = run.source.fetch(&ctx);
                    run.last_attempt = Some(now);
                    run.forced = None;
                    match result {
                        Ok(output) => {
                            run.failures = 0;
                            run.retry_at = None;
                            run.problem = None;
                            run.notices = output.notices;
                            let source_id = run.source.id().to_string();
                            for value in output.values {
                                if !account.order.contains(&value.key) {
                                    account.order.push(value.key.clone());
                                }
                                account.readings.retain(|r| {
                                    !(r.source_id == source_id && r.value.key == value.key)
                                });
                                account.readings.push(Reading {
                                    value,
                                    source_id: source_id.clone(),
                                    source_name: run.source.display_name().to_string(),
                                    exactness: run.source.exactness(),
                                    fetched_at: now,
                                });
                            }
                        }
                        Err(error) => {
                            run.failures += 1;
                            let (delay, hold) = backoff(&error, run.failures);
                            log::warn!(
                                "source {} failed ({error:?}); retrying in {} s",
                                run.source.id(),
                                delay
                            );
                            run.retry_at = Some((now.plus_secs(delay), hold));
                            let since = run.problem.as_ref().map_or(now, |p| p.since);
                            run.problem = Some(SourceProblem {
                                source_name: run.source.display_name().to_string(),
                                hint: run.source.failure_hint(&error),
                                error,
                                since,
                            });
                        }
                    }
                }
            }
        }
        attempted
    }

    /// The published state at the clock's current time.
    pub fn state(&self) -> EngineState {
        let now = self.clock.now();
        let assistants = self
            .assistants
            .iter()
            .map(|a| AssistantState {
                id: a.module.id(),
                display_name: a.module.display_name().to_string(),
                badge: a.module.badge(),
                usage_page: a.module.usage_page().map(str::to_string),
                accounts: a
                    .accounts
                    .iter()
                    .map(|acc| {
                        account_state(acc, &a.module.suggested_featured(), now, self.presence)
                    })
                    .collect(),
            })
            .collect();
        EngineState { now, assistants }
    }

    fn sources_mut(&mut self) -> impl Iterator<Item = &mut SourceRun> {
        self.assistants
            .iter_mut()
            .flat_map(|a| &mut a.accounts)
            .flat_map(|acc| &mut acc.sources)
    }
}

/// Delay before the next attempt after `failures` consecutive failures.
fn backoff(error: &FetchError, failures: u32) -> (i64, Hold) {
    let doubling = |start: i64| {
        let factor = 1_i64 << (failures.saturating_sub(1)).min(16);
        (start.saturating_mul(factor)).min(BACKOFF_CAP_SECS)
    };
    match error {
        FetchError::RateLimited {
            retry_after: Some(d),
        } => (d.as_secs() as i64, Hold::Hard),
        FetchError::RateLimited { retry_after: None } => {
            (doubling(RATE_LIMIT_BACKOFF_START_SECS), Hold::Hard)
        }
        _ => (doubling(ERROR_BACKOFF_START_SECS), Hold::Soft),
    }
}

fn account_state(
    acc: &AccountRun,
    featured: &[MeterKey],
    now: Timestamp,
    presence: Presence,
) -> AccountState {
    let rank = |r: &Reading| {
        acc.sources
            .iter()
            .position(|s| s.source.id() == r.source_id)
            .unwrap_or(usize::MAX)
    };
    let is_fresh = |r: &Reading| {
        let limit = acc
            .sources
            .iter()
            .find(|s| s.source.id() == r.source_id)
            .map_or(0, |s| s.freshness_limit(presence));
        now.0 - r.fetched_at.0 <= limit
    };
    let meters = acc
        .order
        .iter()
        .filter_map(|key| {
            let candidates: Vec<&Reading> = acc
                .readings
                .iter()
                .filter(|r| &r.value.key == key)
                .collect();
            let newest = |want: Option<Exactness>, fresh: bool| {
                candidates
                    .iter()
                    .copied()
                    .filter(|r| want.is_none_or(|e| r.exactness == e))
                    .filter(|r| !fresh || is_fresh(r))
                    // Newest first; ties go to Exact, then to source priority.
                    .min_by_key(|r| {
                        (
                            std::cmp::Reverse(r.fetched_at),
                            r.exactness != Exactness::Exact,
                            rank(r),
                        )
                    })
            };
            let chosen = newest(Some(Exactness::Exact), true)
                .or_else(|| newest(Some(Exactness::Estimate), true))
                .or_else(|| newest(None, false))?;
            Some(meter_state(chosen, is_fresh(chosen), now))
        })
        .collect();
    AccountState {
        id: acc.info.id.clone(),
        display_name: acc.info.display_name.clone(),
        featured: featured.iter().take(2).cloned().collect(),
        meters,
        notices: acc.sources.iter().flat_map(|s| s.notices.clone()).collect(),
        problem: acc.sources.iter().find_map(|s| s.problem.clone()),
    }
}

fn meter_state(r: &Reading, fresh: bool, now: Timestamp) -> MeterState {
    let v = &r.value;
    let freshness = if v.window.is_some_and(|w| w.end <= now) {
        Freshness::PastReset
    } else if fresh {
        Freshness::Fresh
    } else {
        Freshness::Stale
    };
    let pace = match (freshness, v.window) {
        (Freshness::Fresh, Some(w)) => pace(v.used, v.limit, &w, now),
        _ => None,
    };
    MeterState {
        key: v.key.clone(),
        label: v.label.clone(),
        used: v.used,
        limit: v.limit,
        unit: v.unit.clone(),
        window: v.window,
        source_name: r.source_name.clone(),
        exactness: r.exactness,
        fetched_at: r.fetched_at,
        freshness,
        pace,
    }
}

/// Compares the used fraction with the elapsed fraction of the Window and projects
/// linearly when the Limit will be reached.
fn pace(used: f64, limit: Option<f64>, window: &Window, now: Timestamp) -> Option<Pace> {
    let limit = limit.filter(|l| *l > 0.0)?;
    let start = window.effective_start()?;
    let total = window.end.0 - start.0;
    if total <= 0 {
        return None;
    }
    if now < start {
        return Some(Pace::NotStarted);
    }
    let used_fraction = used / limit;
    if used_fraction >= 1.0 {
        return Some(Pace::LimitReached);
    }
    let elapsed_secs = now.0 - start.0;
    let elapsed = elapsed_secs as f64 / total as f64;
    if elapsed < PACE_WARMUP_FRACTION {
        return Some(Pace::OnPace);
    }
    if used_fraction > elapsed {
        let to_limit = (elapsed_secs as f64 * limit / used).round() as i64;
        return Some(Pace::WillHitLimitAt(start.plus_secs(to_limit)));
    }
    if used_fraction < elapsed - PACE_AHEAD_MARGIN {
        Some(Pace::Ahead)
    } else {
        Some(Pace::OnPace)
    }
}

/// Converts a whole number of minutes into a [`Duration`].
pub const fn minutes(n: u64) -> Duration {
    Duration::from_secs(n * 60)
}
