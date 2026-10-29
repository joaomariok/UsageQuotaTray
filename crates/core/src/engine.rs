//! The engine: runs every Source of every Account on its schedule, keeps the newest
//! Reading per Source and Meter, and publishes the selected per-Account state.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::alerts::AlertMemory;
pub use crate::alerts::{Alert, AlertKind};
use crate::contract::{Assistant, FetchContext, FetchError, FetchOutput, SettingField, Source};
use crate::http::{HostClient, Transport};
use crate::model::{
    AccountInfo, BadgeSpec, Breakdown, Exactness, MeterKey, Notice, Reading, Unit, Window,
};
use crate::push::PushMessage;
use crate::settings::Settings;
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
    pub breakdown: Option<Breakdown>,
    /// The module could not identify this Meter (shown under its raw key).
    pub unrecognized: bool,
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
    /// Consecutive failures of the same kind (rate limited or not): each kind has its
    /// own backoff, so a 429 after being offline still starts at its first step.
    failures: u32,
    /// Whether the failures counted in `failures` are rate limits.
    rate_limited: bool,
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

/// What the schedule depends on besides each Source's own intervals.
#[derive(Clone, Copy)]
struct Pacing {
    presence: Presence,
    /// The user's active-interval override.
    active_override: Option<Duration>,
}

impl SourceRun {
    fn interval(&self, pacing: Pacing) -> i64 {
        let i = self.source.intervals();
        let d = match pacing.presence {
            Presence::Active => pacing.active_override.unwrap_or(i.active),
            Presence::Idle | Presence::Paused => i.idle,
        };
        d.as_secs() as i64
    }

    fn freshness_limit(&self, pacing: Pacing) -> i64 {
        self.source
            .freshness_limit()
            .map_or(2 * self.interval(pacing), |d| d.as_secs() as i64)
    }

    fn due_at(&self, pacing: Pacing) -> Option<Timestamp> {
        if pacing.presence == Presence::Paused || self.source.push_only() {
            return None;
        }
        let Some(last) = self.last_attempt else {
            return Some(Timestamp(i64::MIN));
        };
        let scheduled = match self.retry_at {
            Some((at, _)) => at,
            None => last.plus_secs(self.interval(pacing)),
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
    alerts: HashMap<MeterKey, AlertMemory>,
}

struct AssistantRun {
    module: Box<dyn Assistant>,
    http: HostClient,
    accounts: Vec<AccountRun>,
}

impl AssistantRun {
    /// Asks the module for its Accounts again. Accounts still found keep their
    /// Readings, schedule and alert memory; new ones start fresh; vanished ones go.
    fn rediscover(&mut self) {
        let found = self.module.discover_accounts();
        let mut old = std::mem::take(&mut self.accounts);
        for info in found {
            if self.accounts.iter().any(|a| a.info.id == info.id) {
                continue;
            }
            let run = match old.iter().position(|a| a.info.id == info.id) {
                Some(i) => {
                    let mut run = old.swap_remove(i);
                    run.info = info;
                    run
                }
                None => {
                    let mut sources: Vec<SourceRun> = self
                        .module
                        .sources(&info)
                        .into_iter()
                        .map(|source| SourceRun {
                            source,
                            last_attempt: None,
                            retry_at: None,
                            failures: 0,
                            rate_limited: false,
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
                        alerts: HashMap::new(),
                    }
                }
            };
            self.accounts.push(run);
        }
    }

    fn enabled(&self, settings: &Settings) -> bool {
        settings.assistant_enabled(self.module.id())
    }

    /// The enabled Accounts of an enabled Assistant.
    fn running<'a>(&'a self, settings: &'a Settings) -> impl Iterator<Item = &'a AccountRun> {
        let on = self.enabled(settings);
        let id = self.module.id();
        self.accounts
            .iter()
            .filter(move |acc| on && settings.account_enabled(id, &acc.info.id))
    }
}

/// Every module compiled in, with its Accounts, whether enabled or not: what the
/// settings window lists.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Catalog {
    pub assistants: Vec<CatalogAssistant>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CatalogAssistant {
    pub id: &'static str,
    pub display_name: String,
    pub enabled: bool,
    pub fields: Vec<SettingField>,
    pub accounts: Vec<CatalogAccount>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CatalogAccount {
    pub id: String,
    pub display_name: String,
    pub enabled: bool,
}

pub struct Engine {
    assistants: Vec<AssistantRun>,
    clock: Arc<dyn Clock>,
    presence: Presence,
    last_forced: Option<Timestamp>,
    settings: Settings,
    pending_alerts: Vec<Alert>,
    pending_outcomes: Vec<ActionOutcome>,
}

/// What came of one settings action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionOutcome {
    pub assistant_id: String,
    pub key: String,
    /// The module's message for the user.
    pub result: Result<String, String>,
}

impl Engine {
    /// [`Engine::with_settings`] with the default settings.
    pub fn new(
        modules: Vec<Box<dyn Assistant>>,
        clock: Arc<dyn Clock>,
        transport: Arc<dyn Transport>,
    ) -> Self {
        Self::with_settings(modules, clock, transport, &Settings::default())
    }

    /// Configures every module with the user's values, then discovers its Accounts and
    /// their Sources. Nothing is fetched until the first [`Engine::tick`].
    pub fn with_settings(
        modules: Vec<Box<dyn Assistant>>,
        clock: Arc<dyn Clock>,
        transport: Arc<dyn Transport>,
        settings: &Settings,
    ) -> Self {
        let assistants = modules
            .into_iter()
            .map(|mut module| {
                module.configure(&settings.module_values(module.id()));
                let http = HostClient::new(module.allowed_hosts(), transport.clone());
                let mut run = AssistantRun {
                    module,
                    http,
                    accounts: Vec::new(),
                };
                run.rediscover();
                run
            })
            .collect();
        Self {
            assistants,
            clock,
            presence: Presence::Active,
            last_forced: None,
            settings: settings.clone(),
            pending_alerts: Vec::new(),
            pending_outcomes: Vec::new(),
        }
    }

    /// Runs a module's settings action; the outcome waits in [`Engine::take_outcomes`].
    pub fn run_action(&mut self, assistant: &str, key: &str) {
        let result = match self
            .assistants
            .iter_mut()
            .find(|a| a.module.id() == assistant)
        {
            Some(a) => a.module.run_action(key),
            None => Err(format!("no Assistant {assistant}")),
        };
        if let Err(e) = &result {
            log::warn!("action {assistant}/{key} failed: {e}");
        }
        self.pending_outcomes.push(ActionOutcome {
            assistant_id: assistant.to_string(),
            key: key.to_string(),
            result,
        });
    }

    /// Outcomes of actions run since the last call, oldest first.
    pub fn take_outcomes(&mut self) -> Vec<ActionOutcome> {
        std::mem::take(&mut self.pending_outcomes)
    }

    /// Alerts raised by ticks since the last call, oldest first.
    pub fn take_alerts(&mut self) -> Vec<Alert> {
        std::mem::take(&mut self.pending_alerts)
    }

    /// Checks every Meter of every running Account against its thresholds.
    fn evaluate_alerts(&mut self, now: Timestamp) {
        let pacing = self.pacing();
        let settings = &self.settings;
        for a in &mut self.assistants {
            let id = a.module.id();
            if !settings.assistant_enabled(id) {
                continue;
            }
            for acc in &mut a.accounts {
                if !settings.account_enabled(id, &acc.info.id) {
                    continue;
                }
                let state = account_state(acc, &[], now, pacing);
                for m in &state.meters {
                    let thresholds = settings.thresholds(id, &acc.info.id, &m.key);
                    let kinds = acc.alerts.entry(m.key.clone()).or_default().observe(
                        m,
                        &thresholds,
                        settings.reset_notifications,
                    );
                    self.pending_alerts
                        .extend(kinds.into_iter().map(|kind| Alert {
                            assistant_id: id,
                            assistant_name: a.module.display_name().to_string(),
                            account_id: acc.info.id.clone(),
                            account_name: acc.info.display_name.clone(),
                            meter_key: m.key.clone(),
                            meter_label: m.label.clone(),
                            window_end: m.window.map(|w| w.end),
                            kind,
                        }));
                }
            }
        }
    }

    pub fn presence(&self) -> Presence {
        self.presence
    }

    /// Takes the user's new settings into account from now on. A module whose values
    /// changed is reconfigured and its Accounts are discovered again at once.
    pub fn apply_settings(&mut self, settings: &Settings) {
        for a in &mut self.assistants {
            let id = a.module.id();
            let values = settings.module_values(id);
            if values != self.settings.module_values(id) {
                a.module.configure(&values);
                a.rediscover();
            }
        }
        self.settings = settings.clone();
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Every module and Account, enabled or not.
    pub fn catalog(&self) -> Catalog {
        Catalog {
            assistants: self
                .assistants
                .iter()
                .map(|a| {
                    let id = a.module.id();
                    CatalogAssistant {
                        id,
                        display_name: a.module.display_name().to_string(),
                        enabled: a.enabled(&self.settings),
                        fields: a.module.settings_fields(),
                        accounts: a
                            .accounts
                            .iter()
                            .map(|acc| CatalogAccount {
                                id: acc.info.id.clone(),
                                display_name: acc.info.display_name.clone(),
                                enabled: self.settings.account_enabled(id, &acc.info.id),
                            })
                            .collect(),
                    }
                })
                .collect(),
        }
    }

    fn pacing(&self) -> Pacing {
        Pacing {
            presence: self.presence,
            active_override: self.settings.active_poll,
        }
    }

    pub fn set_presence(&mut self, presence: Presence) {
        self.presence = presence;
    }

    /// Discovers every module's Accounts again (a login may have changed), then asks
    /// for an immediate fetch of every Source. Automatic reasons are debounced to once
    /// a minute; rate limits are always honored.
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
        for a in &mut self.assistants {
            a.rediscover();
        }
        for source in self.sources_mut() {
            source.forced = Some((now, reason));
        }
    }

    /// When the next Source is due, or `None` while paused.
    pub fn next_wake(&self) -> Option<Timestamp> {
        let now = self.clock.now();
        let pacing = self.pacing();
        self.assistants
            .iter()
            .flat_map(|a| a.running(&self.settings))
            .flat_map(|acc| &acc.sources)
            .filter_map(|s| s.due_at(pacing))
            .min()
            .map(|t| t.max(now))
    }

    /// Fetches every due Source. Returns whether anything was attempted.
    pub fn tick(&mut self) -> bool {
        let now = self.clock.now();
        let pacing = self.pacing();
        let settings = &self.settings;
        let mut attempted = false;
        for assistant in &mut self.assistants {
            let id = assistant.module.id();
            if !settings.assistant_enabled(id) {
                continue;
            }
            let ctx = FetchContext {
                now,
                http: &assistant.http,
            };
            for account in &mut assistant.accounts {
                if !settings.account_enabled(id, &account.info.id) {
                    continue;
                }
                for run in &mut account.sources {
                    if run.due_at(pacing).is_none_or(|due| due > now) {
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
                            record(&mut account.readings, &mut account.order, run, output, now);
                        }
                        Err(error) => {
                            let rate_limited = matches!(error, FetchError::RateLimited { .. });
                            if run.rate_limited != rate_limited {
                                run.failures = 0;
                                run.rate_limited = rate_limited;
                            }
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
        self.evaluate_alerts(now);
        attempted
    }

    /// Hands a message from the push channel to the matching push Sources of every
    /// running Account. Returns whether one accepted it.
    pub fn push(&mut self, message: &PushMessage) -> bool {
        let now = self.clock.now();
        let settings = &self.settings;
        let mut accepted = false;
        for assistant in &mut self.assistants {
            let id = assistant.module.id();
            if id != message.assistant || !settings.assistant_enabled(id) {
                continue;
            }
            for account in &mut assistant.accounts {
                if !settings.account_enabled(id, &account.info.id) {
                    continue;
                }
                for run in &mut account.sources {
                    if run.source.id() != message.source {
                        continue;
                    }
                    if let Some(output) = run.source.accept_push(message) {
                        accepted = true;
                        record(&mut account.readings, &mut account.order, run, output, now);
                    }
                }
            }
        }
        if accepted {
            self.evaluate_alerts(now);
        }
        accepted
    }

    /// The published state at the clock's current time.
    pub fn state(&self) -> EngineState {
        let now = self.clock.now();
        let pacing = self.pacing();
        let assistants = self
            .assistants
            .iter()
            .filter(|a| a.enabled(&self.settings))
            .map(|a| {
                let id = a.module.id();
                AssistantState {
                    id,
                    display_name: a.module.display_name().to_string(),
                    badge: a.module.badge(),
                    usage_page: a.module.usage_page().map(str::to_string),
                    accounts: a
                        .running(&self.settings)
                        .map(|acc| {
                            let featured = self
                                .settings
                                .featured(id, &acc.info.id)
                                .unwrap_or_else(|| a.module.suggested_featured());
                            account_state(acc, &featured, now, pacing)
                        })
                        .collect(),
                }
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

/// Keeps a Source's new values as its newest Readings and its notices as current.
fn record(
    readings: &mut Vec<Reading>,
    order: &mut Vec<MeterKey>,
    run: &mut SourceRun,
    output: FetchOutput,
    now: Timestamp,
) {
    run.notices = output.notices;
    let source_id = run.source.id().to_string();
    for value in output.values {
        if !order.contains(&value.key) {
            order.push(value.key.clone());
        }
        readings.retain(|r| !(r.source_id == source_id && r.value.key == value.key));
        readings.push(Reading {
            value,
            source_id: source_id.clone(),
            source_name: run.source.display_name().to_string(),
            exactness: run.source.exactness(),
            fetched_at: now,
        });
    }
}

/// Delay before the next attempt after `failures` consecutive failures of `error`'s
/// kind.
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
    pacing: Pacing,
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
            .map_or(0, |s| s.freshness_limit(pacing));
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
        breakdown: v.breakdown.clone(),
        unrecognized: v.unrecognized,
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
