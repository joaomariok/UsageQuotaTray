//! The Claude Code Assistant (Anthropic first-party billing).
//!
//! It finds Claude Code's config folder, reads its login read-only (never refreshing
//! or rewriting it), and calls the OAuth usage endpoint that Claude Code's own usage
//! view uses, through Core's host client. The access token only ever goes into the
//! request's Authorization header; it is never logged, stored or shown.

pub mod statusline;
mod usage;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;
use statusline::installer::Installer;
use uqt_core::contract::{
    Assistant, FetchContext, FetchError, FetchOutput, Intervals, SettingField, Source,
};
use uqt_core::engine::minutes;
use uqt_core::http::HttpError;
use uqt_core::model::{AccountInfo, BadgeSpec, Exactness, MeterKey};
use uqt_core::time::Timestamp;

pub const ID: &str = "claude-code";
/// The only host this module talks to.
pub const API_HOST: &str = "api.anthropic.com";
pub const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
pub const OAUTH_BETA: &str = "oauth-2025-04-20";
/// The Claude usage settings page.
pub const USAGE_PAGE: &str = "https://claude.ai/settings/usage";
pub const BADGE_SVG: &str = include_str!("../badge.svg");

/// Meter keys.
pub mod keys {
    /// The 5-hour session window.
    pub const SESSION: &str = "session";
    /// The weekly window shared by every model and surface.
    pub const WEEKLY: &str = "weekly";
}

/// Source ids.
pub mod sources {
    pub const USAGE: &str = "claude-usage";
    /// Pushed by the statusline helper after every Claude Code response.
    pub const STATUSLINE: &str = "claude-statusline";
}

const CREDENTIALS_FILE: &str = ".credentials.json";
const ACCOUNT_FILE: &str = ".claude.json";

/// Where one Claude Code login lives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigFolder {
    /// Holds `.credentials.json`.
    pub dir: PathBuf,
    /// Claude Code's account file (`.claude.json`) with the login's identity.
    pub account_file: PathBuf,
}

impl ConfigFolder {
    /// A folder set through `CLAUDE_CONFIG_DIR` (or added by hand): the account file
    /// sits inside it.
    pub fn new(dir: PathBuf) -> Self {
        let account_file = dir.join(ACCOUNT_FILE);
        Self { dir, account_file }
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

/// The settings key of the statusline install/uninstall button.
pub const STATUSLINE_FIELD: &str = "statusline";

pub struct ClaudeCodeAssistant {
    folders: Vec<ConfigFolder>,
    statusline: Option<Installer>,
}

impl ClaudeCodeAssistant {
    pub fn new(folders: Vec<ConfigFolder>) -> Self {
        Self {
            folders,
            statusline: None,
        }
    }

    /// Offers installing the statusline helper in settings, with these files.
    pub fn with_statusline(mut self, installer: Installer) -> Self {
        self.statusline = Some(installer);
        self
    }

    /// The default config folder of this process.
    pub fn from_env() -> Self {
        Self::new(ConfigFolder::discover_default().into_iter().collect())
    }

    fn folder_of(&self, account: &AccountInfo) -> Option<&ConfigFolder> {
        self.folders
            .iter()
            .enumerate()
            .find(|(i, f)| account_info(f, *i).id == account.id)
            .map(|(_, f)| f)
    }
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
        self.statusline
            .as_ref()
            .map(statusline::settings_field)
            .into_iter()
            .collect()
    }

    fn run_action(&mut self, key: &str) -> Result<String, String> {
        match (&self.statusline, key) {
            (Some(installer), STATUSLINE_FIELD) => statusline::toggle(installer),
            _ => Err(format!("Claude Code has no action {key}")),
        }
    }

    /// One Account per config folder, even when Claude Code is missing or logged out,
    /// so that state is shown instead of nothing.
    fn discover_accounts(&mut self) -> Vec<AccountInfo> {
        self.folders
            .iter()
            .enumerate()
            .map(|(i, f)| account_info(f, i))
            .collect()
    }

    fn sources(&mut self, account: &AccountInfo) -> Vec<Box<dyn Source>> {
        let Some(folder) = self.folder_of(account) else {
            return Vec::new();
        };
        vec![
            Box::new(UsageSource {
                folder: folder.clone(),
            }),
            Box::new(statusline::StatuslineSource {
                folder: folder.clone(),
            }),
        ]
    }
}

/// The login's identity (account plus organization) from the account file, or a
/// placeholder per folder while there is none.
fn account_info(folder: &ConfigFolder, index: usize) -> AccountInfo {
    let account = read_json(&folder.account_file)
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
                .map(|values| FetchOutput {
                    values,
                    notices: Vec::new(),
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
