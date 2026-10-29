//! The Claude Code Assistant (Anthropic first-party billing).
//!
//! It finds Claude Code's config folder, reads its login read-only (never refreshing
//! or rewriting it), and calls the OAuth usage endpoint that Claude Code's own usage
//! view uses, through Core's host client. The access token only ever goes into the
//! request's Authorization header; it is never logged, stored or shown.

mod plan;
pub mod statusline;
mod usage;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

pub use plan::{API_KEY_VAR, EnvProbe, EnvScope, Plan, SystemEnv};
use serde_json::Value;
use statusline::installer::Installer;
use uqt_core::contract::{
    Assistant, FetchContext, FetchError, FetchOutput, FieldKind, Intervals, SettingField, Source,
};
use uqt_core::engine::minutes;
use uqt_core::http::HttpError;
use uqt_core::model::{AccountInfo, BadgeSpec, Exactness, MeterKey};
use uqt_core::model::{MeterValue, Notice};
use uqt_core::settings::{ModuleValues, SettingValue};
use uqt_core::time::Timestamp;

pub const ID: &str = "claude-code";
/// The only host this module talks to.
pub const API_HOST: &str = "api.anthropic.com";
pub const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
pub const OAUTH_BETA: &str = "oauth-2025-04-20";
/// The Claude usage settings page.
pub const USAGE_PAGE: &str = "https://claude.ai/settings/usage";
/// The Badge: the Claude logo, unaltered. It has no fill of its own, so the tray
/// draws it in the Badge color.
pub const BADGE_SVG: &str = include_str!("../badge.svg");

/// Meter keys.
pub mod keys {
    /// The 5-hour session window.
    pub const SESSION: &str = "session";
    /// The weekly window shared by every model and surface.
    pub const WEEKLY: &str = "weekly";
    /// Paid usage credits against the monthly spend limit; no Window.
    pub const USAGE_CREDITS: &str = "usage-credits";

    /// Prefix of the model-specific weekly limits' keys.
    pub const WEEKLY_MODEL_PREFIX: &str = "weekly:";

    /// A model-specific weekly limit, keyed by the model's display name
    /// (`weekly:fable`).
    pub fn weekly_model(display_name: &str) -> String {
        format!("{WEEKLY_MODEL_PREFIX}{}", display_name.to_lowercase())
    }
}

/// Source ids.
pub mod sources {
    pub const USAGE: &str = "claude-usage";
    /// Pushed by the statusline helper after every Claude Code response.
    pub const STATUSLINE: &str = "claude-statusline";
}

const SIGNED_IN_ELSEWHERE: &str = "this folder is now signed in to another account";
const CREDENTIALS_FILE: &str = ".credentials.json";
const ACCOUNT_FILE: &str = ".claude.json";

/// Where one Claude Code login lives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigFolder {
    /// Holds `.credentials.json`.
    pub dir: PathBuf,
    /// Claude Code's account file (`.claude.json`) with the login's identity.
    pub account_file: PathBuf,
    /// Read when `account_file` does not exist: for a hand-added `...\.claude` folder,
    /// the `.claude.json` next to it (Claude Code's default layout).
    pub account_file_fallback: Option<PathBuf>,
}

impl ConfigFolder {
    /// A folder set through `CLAUDE_CONFIG_DIR`: the account file sits inside it.
    pub fn new(dir: PathBuf) -> Self {
        let account_file = dir.join(ACCOUNT_FILE);
        Self {
            dir,
            account_file,
            account_file_fallback: None,
        }
    }

    /// A folder the user added by hand. The account file is looked for inside it, then,
    /// when the folder is named `.claude`, next to it. Nothing is read here, so adding a
    /// WSL path does not start its distro until the folder is actually read.
    pub fn added(dir: PathBuf) -> Self {
        let fallback = (dir.file_name().is_some_and(|n| n == ".claude"))
            .then(|| dir.parent().map(|p| p.join(ACCOUNT_FILE)))
            .flatten();
        Self {
            account_file_fallback: fallback,
            ..Self::new(dir)
        }
    }

    /// Whether reading this folder goes through WSL (and so starts its distro).
    pub fn is_wsl(&self) -> bool {
        let s = self.dir.to_string_lossy().to_ascii_lowercase();
        s.starts_with(r"\\wsl$\") || s.starts_with(r"\\wsl.localhost\")
    }

    fn read_account_file(&self) -> Option<Value> {
        read_json(&self.account_file).or_else(|| {
            self.account_file_fallback
                .as_deref()
                .filter(|_| !self.account_file.exists())
                .and_then(read_json)
        })
    }

    /// The default folder from `CLAUDE_CONFIG_DIR` and the home folder as given:
    /// `CLAUDE_CONFIG_DIR` when set and not empty, else `<home>\.claude` with the
    /// account file at `<home>\.claude.json`.
    pub fn default_from(
        claude_config_dir: Option<OsString>,
        home: Option<PathBuf>,
    ) -> Option<Self> {
        match claude_config_dir {
            Some(dir) if !dir.is_empty() => Some(Self::new(PathBuf::from(dir))),
            _ => home.map(|home| Self {
                dir: home.join(".claude"),
                account_file: home.join(ACCOUNT_FILE),
                account_file_fallback: None,
            }),
        }
    }

    /// The default folder as this (tray) process sees `CLAUDE_CONFIG_DIR`.
    pub fn discover_default() -> Option<Self> {
        Self::default_from(
            std::env::var_os("CLAUDE_CONFIG_DIR"),
            std::env::var_os("USERPROFILE").map(PathBuf::from),
        )
    }
}

/// Keys of this module's settings.
pub mod settings_keys {
    /// Extra config folders, separated by `;`.
    pub const EXTRA_FOLDERS: &str = "extra_config_folders";
}

/// The settings key of the statusline install/uninstall button.
pub const STATUSLINE_FIELD: &str = "statusline";

pub struct ClaudeCodeAssistant {
    /// Found without the user's help (the default folder).
    base: Vec<ConfigFolder>,
    /// Added by hand in settings.
    extra: Vec<ConfigFolder>,
    env: Arc<dyn EnvProbe>,
    statusline: Option<Installer>,
}

impl ClaudeCodeAssistant {
    /// `folders` are the ones found without the user's help; settings add more. The
    /// API key check uses the real environment ([`SystemEnv`]).
    pub fn new(folders: Vec<ConfigFolder>) -> Self {
        Self {
            base: folders,
            extra: Vec::new(),
            env: Arc::new(SystemEnv),
            statusline: None,
        }
    }

    /// Offers installing the statusline helper in settings, with these files.
    pub fn with_statusline(mut self, installer: Installer) -> Self {
        self.statusline = Some(installer);
        self
    }

    /// Checks for an API key through `env` instead of the real environment.
    pub fn with_env(self, env: Arc<dyn EnvProbe>) -> Self {
        Self { env, ..self }
    }

    /// The default config folder of this process. WSL folders are never looked for.
    pub fn from_env() -> Self {
        Self::new(ConfigFolder::discover_default().into_iter().collect())
    }

    /// Every config folder read: the default ones, then those added in settings.
    pub fn folders(&self) -> impl Iterator<Item = &ConfigFolder> {
        self.base.iter().chain(&self.extra)
    }

    /// Each folder with the Account found in it, dropping later folders holding a login
    /// already seen.
    fn accounts(&self) -> Vec<(AccountInfo, &ConfigFolder)> {
        let mut found: Vec<(AccountInfo, &ConfigFolder)> = Vec::new();
        for (i, folder) in self.folders().enumerate() {
            let info = account_info(folder, i);
            if !found.iter().any(|(a, _)| a.id == info.id) {
                found.push((info, folder));
            }
        }
        found
    }
}

/// Splits the extra-folders setting into folders.
fn parse_folders(text: &str) -> Vec<ConfigFolder> {
    text.split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| ConfigFolder::added(PathBuf::from(s)))
        .collect()
}

impl Assistant for ClaudeCodeAssistant {
    fn id(&self) -> &'static str {
        ID
    }

    fn display_name(&self) -> &str {
        "Claude Code"
    }

    fn allowed_hosts(&self) -> &[&'static str] {
        &[API_HOST]
    }

    fn badge(&self) -> BadgeSpec {
        BadgeSpec {
            svg: BADGE_SVG,
            letters: "C",
            color: "#D97757",
        }
    }

    fn suggested_featured(&self) -> Vec<MeterKey> {
        vec![keys::SESSION.into(), keys::WEEKLY.into()]
    }

    fn usage_page(&self) -> Option<&str> {
        Some(USAGE_PAGE)
    }

    fn settings_fields(&self) -> Vec<SettingField> {
        let mut fields = vec![SettingField {
            key: settings_keys::EXTRA_FOLDERS,
            label: "Extra config folders".into(),
            help: Some(
                "More Claude Code config folders to monitor, separated by ;. The same \
                 login in two folders is shown once. WSL folders are only read when added \
                 here, and reading one starts its WSL distro."
                    .into(),
            ),
            kind: FieldKind::Text {
                default: String::new(),
                placeholder: r"D:\claude-work; \wsl.localhost\Ubuntu\home\me\.claude".into(),
            },
        }];
        fields.extend(self.statusline.as_ref().map(statusline::settings_field));
        fields
    }

    fn configure(&mut self, values: &ModuleValues) {
        self.extra = match values.get(settings_keys::EXTRA_FOLDERS) {
            Some(SettingValue::Text(text)) => parse_folders(text),
            _ => Vec::new(),
        };
    }

    fn run_action(&mut self, key: &str) -> Result<String, String> {
        match (&self.statusline, key) {
            (Some(installer), STATUSLINE_FIELD) => statusline::toggle(installer),
            _ => Err(format!("Claude Code has no action {key}")),
        }
    }

    /// One Account per login, even when Claude Code is missing or logged out in a
    /// folder, so that state is shown instead of nothing. The same login (account plus
    /// organization) found in several folders is one Account.
    fn discover_accounts(&mut self) -> Vec<AccountInfo> {
        self.accounts().into_iter().map(|(info, _)| info).collect()
    }

    fn sources(&mut self, account: &AccountInfo) -> Vec<Box<dyn Source>> {
        // Every folder holding this login: polling reads the first, while the
        // statusline helper may run for any of them.
        let found: Vec<(usize, ConfigFolder)> = self
            .folders()
            .enumerate()
            .filter(|(i, f)| account_info(f, *i).id == account.id)
            .map(|(i, f)| (i, f.clone()))
            .collect();
        let Some((index, folder)) = found.first().cloned() else {
            return Vec::new();
        };
        vec![
            Box::new(UsageSource {
                folder,
                index,
                account_id: account.id.clone(),
                env: self.env.clone(),
            }),
            Box::new(statusline::StatuslineSource {
                folders: found,
                account_id: account.id.clone(),
            }),
        ]
    }
}

/// The login's identity (account plus organization) from the account file, or a
/// placeholder per folder while there is none.
pub(crate) fn account_info(folder: &ConfigFolder, index: usize) -> AccountInfo {
    let account = folder
        .read_account_file()
        .and_then(|json| json.get("oauthAccount").cloned())
        .and_then(|a| {
            let id = a.get("accountUuid")?.as_str()?.to_string();
            let org = a
                .get("organizationUuid")
                .and_then(Value::as_str)
                .unwrap_or("");
            let name = a
                .get("emailAddress")
                .and_then(Value::as_str)
                .map_or_else(|| "Claude account".to_string(), str::to_string);
            Some(AccountInfo {
                id: format!("{id}/{org}"),
                display_name: name,
            })
        });
    account.unwrap_or_else(|| AccountInfo {
        id: format!("unidentified-{index}"),
        display_name: "Claude Code".into(),
    })
}

fn read_json(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Deliberately not `Debug` or `Display`, so the token cannot be formatted by accident.
struct AccessToken(String);

/// Reads Claude Code's credentials without changing them.
fn read_token(folder: &ConfigFolder, now: Timestamp) -> Result<AccessToken, FetchError> {
    if !folder.dir.is_dir() {
        return Err(FetchError::NotInstalled);
    }
    let Some(json) = read_json(&folder.dir.join(CREDENTIALS_FILE)) else {
        return Err(FetchError::NotLoggedIn);
    };
    let oauth = &json["claudeAiOauth"];
    let token = match oauth["accessToken"].as_str() {
        Some(t) if !t.is_empty() => AccessToken(t.to_string()),
        _ => return Err(FetchError::NotLoggedIn),
    };
    match oauth["expiresAt"].as_i64().map(epoch_to_secs) {
        Some(expires) if expires <= now.0 => Err(FetchError::Expired),
        _ => Ok(token),
    }
}

/// Claude Code stores some times as Unix milliseconds; accept seconds or milliseconds.
fn epoch_to_secs(n: i64) -> i64 {
    if n.abs() >= 100_000_000_000 {
        n / 1000
    } else {
        n
    }
}

/// The OAuth usage endpoint (Exact).
struct UsageSource {
    folder: ConfigFolder,
    /// Position of the folder, for the placeholder id of an unidentified login.
    index: usize,
    /// The Account this Source was made for.
    account_id: String,
    env: Arc<dyn EnvProbe>,
}

impl UsageSource {
    fn api_key_places(&self) -> Vec<&'static str> {
        plan::api_key_places(&self.folder, self.env.as_ref())
    }
}

/// The Account's Notices after a successful fetch: the shared quota (for a
/// subscription, with this week's Breakdown), an API key that takes over billing, and
/// a reached included limit while usage credits are enabled.
fn notices(
    plan: &Plan,
    credits_enabled: Option<bool>,
    values: &[MeterValue],
    api_key_places: &[&str],
) -> Vec<Notice> {
    let mut out = Vec::new();
    if plan.is_subscription() {
        let mut text = "Included usage is shared with Claude chat and Cowork".to_string();
        let split = values
            .iter()
            .find(|v| v.key == keys::WEEKLY)
            .and_then(|v| v.breakdown.as_ref());
        if let Some(b) = split {
            let parts: Vec<String> = b
                .parts
                .iter()
                .map(|p| format!("{} {}%", p.label, p.percent.round() as i64))
                .collect();
            text.push_str(&format!(" (this week: {})", parts.join(", ")));
        }
        out.push(Notice { text });
    }
    if !api_key_places.is_empty() {
        out.push(Notice {
            text: format!(
                "{API_KEY_VAR} is set {}; Claude Code bills that key instead of your \
                 subscription",
                api_key_places.join(" and ")
            ),
        });
    }
    let included_reached = values.iter().any(|v| {
        let included = v.key == keys::SESSION
            || v.key == keys::WEEKLY
            || v.key.starts_with(keys::WEEKLY_MODEL_PREFIX);
        included && v.limit.is_some_and(|l| v.used >= l)
    });
    if included_reached && credits_enabled == Some(true) {
        out.push(Notice {
            text: "An included limit was reached; usage credits may apply".into(),
        });
    }
    out
}

impl Source for UsageSource {
    fn id(&self) -> &str {
        sources::USAGE
    }

    fn display_name(&self) -> &str {
        "Claude usage"
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
        // Another login in this folder is another Account; its numbers must never be
        // filed under this one. The engine picks it up on its next discovery.
        if account_info(&self.folder, self.index).id != self.account_id {
            return Err(FetchError::Other(SIGNED_IN_ELSEWHERE.into()));
        }
        let token = read_token(&self.folder, ctx.now)?;
        let authorization = format!("Bearer {}", token.0);
        let result = ctx.http.get(
            USAGE_URL,
            &[
                ("Authorization", &authorization),
                ("anthropic-beta", OAUTH_BETA),
                ("Accept", "application/json"),
            ],
        );
        let response = match result {
            Ok(r) => r,
            Err(HttpError::Transport(e)) => {
                log::info!("usage request failed: {e}");
                return Err(FetchError::Offline);
            }
            Err(e) => return Err(FetchError::Other(format!("{e:?}"))),
        };
        match response.status {
            200 => usage::parse(&response.body)
                .map(|usage| {
                    let plan = Plan::detect(&self.folder);
                    let credits = usage.credits_enabled.or(plan.usage_credits_enabled);
                    let notices = notices(&plan, credits, &usage.values, &self.api_key_places());
                    FetchOutput {
                        values: usage.values,
                        notices,
                    }
                })
                .ok_or_else(|| {
                    log::warn!("the usage response has an unknown shape");
                    FetchError::EndpointChanged
                }),
            401 => Err(FetchError::Expired),
            429 => Err(FetchError::RateLimited {
                retry_after: response
                    .header("retry-after")
                    .and_then(|v| v.trim().parse::<u64>().ok())
                    .map(Duration::from_secs),
            }),
            300..=399 | 404 | 410 => Err(FetchError::EndpointChanged),
            status => Err(FetchError::Other(format!("HTTP {status}"))),
        }
    }

    fn failure_hint(&self, error: &FetchError) -> Option<String> {
        Some(
            match error {
                FetchError::NotInstalled => "Claude Code was not found; install it and sign in",
                FetchError::NotLoggedIn => "sign in to Claude Code",
                FetchError::Expired => "run Claude Code once",
                FetchError::EndpointChanged => {
                    "the usage endpoint moved or changed; UsageQuotaTray needs an update"
                }
                _ => return None,
            }
            .into(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_config_dir_wins_when_set() {
        let f = ConfigFolder::default_from(
            Some(OsString::from(r"D:\claude-work")),
            Some(PathBuf::from(r"C:\Users\me")),
        )
        .unwrap();
        assert_eq!(f.dir, PathBuf::from(r"D:\claude-work"));
        assert_eq!(
            f.account_file,
            PathBuf::from(r"D:\claude-work\.claude.json")
        );
    }

    #[test]
    fn otherwise_the_home_folder_is_used() {
        let home = Some(PathBuf::from(r"C:\Users\me"));
        for unset in [None, Some(OsString::new())] {
            let f = ConfigFolder::default_from(unset, home.clone()).unwrap();
            assert_eq!(f.dir, PathBuf::from(r"C:\Users\me\.claude"));
            assert_eq!(f.account_file, PathBuf::from(r"C:\Users\me\.claude.json"));
        }
        assert_eq!(ConfigFolder::default_from(None, None), None);
    }

    #[test]
    fn expiry_accepts_seconds_or_milliseconds() {
        assert_eq!(epoch_to_secs(1_793_277_249), 1_793_277_249);
        assert_eq!(epoch_to_secs(1_793_277_249_000), 1_793_277_249);
    }
}
