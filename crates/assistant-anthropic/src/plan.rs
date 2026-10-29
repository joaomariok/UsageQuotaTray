//! What the local files say about the login's plan, and where an API key might take
//! over billing. Nothing here reaches the network, and no secret value is ever read:
//! only whether something is there.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde::de::IgnoredAny;
use serde_json::Value;

use crate::{CREDENTIALS_FILE, ConfigFolder, read_json};

/// The variable that makes Claude Code bill an API key instead of the subscription.
pub const API_KEY_VAR: &str = "ANTHROPIC_API_KEY";
const SETTINGS_FILE: &str = "settings.json";

/// The login's plan as Claude Code's local files describe it. Every field is the
/// maker's own value, `None` when absent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// From the credentials (`pro`, `max`, ...).
    pub subscription_type: Option<String>,
    pub rate_limit_tier: Option<String>,
    pub user_rate_limit_tier: Option<String>,
    pub organization_rate_limit_tier: Option<String>,
    pub billing_type: Option<String>,
    pub organization_type: Option<String>,
    pub seat_tier: Option<String>,
    /// Whether usage credits (paid usage past the included limits) are enabled.
    pub usage_credits_enabled: Option<bool>,
}

impl Plan {
    /// Reads the plan from the folder's credentials and account file.
    pub fn detect(folder: &ConfigFolder) -> Self {
        let oauth = read_json(&folder.dir.join(CREDENTIALS_FILE))
            .map(|j| j["claudeAiOauth"].clone())
            .unwrap_or(Value::Null);
        let account = folder
            .read_account_file()
            .map(|j| j["oauthAccount"].clone())
            .unwrap_or(Value::Null);
        let text = |v: &Value, key: &str| {
            v[key]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        Self {
            subscription_type: text(&oauth, "subscriptionType"),
            rate_limit_tier: text(&oauth, "rateLimitTier"),
            user_rate_limit_tier: text(&account, "userRateLimitTier"),
            organization_rate_limit_tier: text(&account, "organizationRateLimitTier"),
            billing_type: text(&account, "billingType"),
            organization_type: text(&account, "organizationType"),
            seat_tier: text(&account, "seatTier"),
            usage_credits_enabled: account["hasExtraUsageEnabled"].as_bool(),
        }
    }

    /// A Claude subscription, whose included usage is shared with Claude chat and
    /// Cowork.
    pub fn is_subscription(&self) -> bool {
        self.subscription_type.is_some()
    }
}

/// Where an environment variable can be set on Windows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EnvScope {
    /// This (tray) process.
    Process,
    /// The user's environment (registry).
    User,
    /// The machine's environment (registry).
    Machine,
}

/// Answers whether an environment variable is set, without reading its value.
pub trait EnvProbe: Send + Sync {
    fn is_set(&self, scope: EnvScope, name: &str) -> bool;
}

/// The real process environment and registry, queried for existence only.
pub struct SystemEnv;

impl EnvProbe for SystemEnv {
    fn is_set(&self, scope: EnvScope, name: &str) -> bool {
        use windows::Win32::Foundation::ERROR_SUCCESS;
        use windows::Win32::System::Environment::GetEnvironmentVariableW;
        use windows::Win32::System::Registry::{
            HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_ANY, RegGetValueW,
        };
        use windows::core::HSTRING;

        let name = HSTRING::from(name);
        let (root, key) = match scope {
            // With no buffer, the call returns the size the value would need (2 or more
            // for a non-empty value) and copies nothing.
            EnvScope::Process => return unsafe { GetEnvironmentVariableW(&name, None) } > 1,
            EnvScope::User => (HKEY_CURRENT_USER, r"Environment"),
            EnvScope::Machine => (
                HKEY_LOCAL_MACHINE,
                r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment",
            ),
        };
        // No data or size pointer: only whether the value exists.
        let status = unsafe {
            RegGetValueW(
                root,
                &HSTRING::from(key),
                &name,
                RRF_RT_ANY,
                None,
                None,
                None,
            )
        };
        status == ERROR_SUCCESS
    }
}

/// Only the presence of the keys that matter; values are skipped unread.
#[derive(Deserialize)]
struct ClaudeSettings {
    #[serde(rename = "apiKeyHelper")]
    api_key_helper: Option<IgnoredAny>,
    env: Option<BTreeMap<String, IgnoredAny>>,
}

/// Where an API key is configured for this folder's Claude Code, in words; empty when
/// nowhere.
pub(crate) fn api_key_places(folder: &ConfigFolder, env: &dyn EnvProbe) -> Vec<&'static str> {
    let mut places = Vec::new();
    for (scope, place) in [
        (EnvScope::Process, "in this session"),
        (EnvScope::User, "in your user environment"),
        (EnvScope::Machine, "in the machine environment"),
    ] {
        if env.is_set(scope, API_KEY_VAR) {
            places.push(place);
        }
    }
    let settings = std::fs::read(folder.dir.join(SETTINGS_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<ClaudeSettings>(&bytes).ok());
    if let Some(s) = settings {
        if s.api_key_helper.is_some() {
            places.push("through apiKeyHelper in Claude Code settings");
        }
        if s.env.is_some_and(|e| e.contains_key(API_KEY_VAR)) {
            places.push("in env in Claude Code settings");
        }
    }
    places
}
