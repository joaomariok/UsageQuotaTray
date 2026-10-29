//! Phase 0 spike for UsageQuotaTray: inspects Claude Code's local login on this machine and,
//! with `--call`, sends one request to the undocumented OAuth usage endpoint.
//!
//! Every value it prints goes through `redact`, so no token, key, email address or account
//! identifier reaches the terminal. It only reads Claude Code's files: it never writes,
//! refreshes or rotates its credentials.

mod clock;
mod redact;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::Value;
use ureq::Agent;
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const OAUTH_BETA: &str = "oauth-2025-04-20";
const AUTH_STATUS_TIMEOUT: Duration = Duration::from_secs(30);

/// Deliberately not `Debug`, so the token cannot end up in a `{:?}`.
enum Token {
    Missing,
    Expired,
    Usable(String),
}

fn main() {
    let call = std::env::args().skip(1).any(|a| a == "--call");
    println!(
        "anthropic-usage-spike {} (no secrets are printed{})",
        env!("CARGO_PKG_VERSION"),
        if call {
            ""
        } else {
            "; dry run, add --call to request usage"
        }
    );

    let dir = config_dir();
    let token = credentials(&dir);
    account_file(&dir);
    settings_file(&dir);
    auth_status();
    environment();

    section("OAuth usage endpoint");
    match (call, token) {
        (false, _) => println!("skipped: run with --call to send one GET {USAGE_URL}"),
        (true, Token::Missing) => {
            println!("skipped: no access token found; sign in with `claude` first")
        }
        (true, Token::Expired) => println!(
            "skipped: the access token has expired; run `claude` once so it renews its login \
             (this tool never refreshes tokens)"
        ),
        (true, Token::Usable(token)) => call_usage(&token),
    }
}

fn section(title: &str) {
    println!("\n== {title} ==");
}

fn home() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_default()
}

fn config_dir() -> PathBuf {
    section("Config directory");
    let dir = match std::env::var_os("CLAUDE_CONFIG_DIR") {
        Some(dir) if !dir.is_empty() => {
            println!("CLAUDE_CONFIG_DIR is set in this process");
            PathBuf::from(dir)
        }
        _ => {
            println!("CLAUDE_CONFIG_DIR is not set in this process; using %USERPROFILE%\\.claude");
            home().join(".claude")
        }
    };
    println!(
        "{} ({})",
        dir.display(),
        if dir.is_dir() { "exists" } else { "missing" }
    );
    dir
}

/// Reads and parses a JSON file, reporting size or the reason it could not be read.
fn read_json(path: &Path) -> Option<Value> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) => {
            println!("{}: {e}", path.display());
            return None;
        }
    };
    println!("{}: {} bytes", path.display(), text.len());
    match serde_json::from_str(&text) {
        Ok(value) => Some(value),
        Err(e) => {
            println!("  not valid JSON: {e}");
            None
        }
    }
}

fn print_time(label: &str, value: &Value) {
    if let Some(n) = value.as_i64() {
        let secs = clock::epoch_to_secs(n);
        println!(
            "  {label}: {} ({})",
            clock::format_utc(secs),
            clock::relative(secs, clock::now_secs())
        );
    }
}

fn credentials(dir: &Path) -> Token {
    section("Credentials file (key names and types; plan fields and expiry only)");
    let Some(json) = read_json(&dir.join(".credentials.json")) else {
        return Token::Missing;
    };
    for line in redact::outline(&json) {
        println!("  {line}");
    }
    let oauth = &json["claudeAiOauth"];
    print_time("access token expires", &oauth["expiresAt"]);
    print_time("refresh token expires", &oauth["refreshTokenExpiresAt"]);

    match oauth["accessToken"].as_str() {
        None | Some("") => {
            println!("  no claudeAiOauth.accessToken");
            Token::Missing
        }
        Some(token) => match oauth["expiresAt"].as_i64().map(clock::epoch_to_secs) {
            Some(expires) if expires <= clock::now_secs() => Token::Expired,
            _ => Token::Usable(token.to_owned()),
        },
    }
}

fn account_file(dir: &Path) {
    section("Account file .claude.json (oauthAccount: key names and types; plan fields only)");
    let mut candidates = vec![dir.join(".claude.json"), home().join(".claude.json")];
    candidates.dedup();
    for path in candidates {
        let Some(json) = read_json(&path) else {
            continue;
        };
        println!(
            "  top-level keys: {}",
            json.as_object().map_or(0, |m| m.len())
        );
        match json.get("oauthAccount") {
            Some(account) => {
                for line in redact::outline(account) {
                    println!("  oauthAccount.{line}");
                }
            }
            None => println!("  no oauthAccount"),
        }
    }
}

fn settings_file(dir: &Path) {
    section("Claude Code settings.json (key names only)");
    let Some(json) = read_json(&dir.join("settings.json")) else {
        return;
    };
    let Some(map) = json.as_object() else { return };
    println!(
        "  top-level keys: {}",
        map.keys().cloned().collect::<Vec<_>>().join(", ")
    );
    if map.contains_key("apiKeyHelper") {
        println!("  apiKeyHelper is set: Claude Code may authenticate with an API key");
    }
    if let Some(env) = map.get("env").and_then(Value::as_object) {
        println!(
            "  env keys: {}",
            env.keys().cloned().collect::<Vec<_>>().join(", ")
        );
    }
    if map.contains_key("statusLine") {
        println!("  statusLine is configured (Phase 2 must chain to it)");
    }
}

fn auth_status() {
    section("claude auth status (emails and identifiers masked)");
    let child = Command::new("claude")
        .args(["auth", "status"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let child = match child {
        Ok(child) => child,
        Err(e) => {
            println!("could not run `claude`: {e}");
            return;
        }
    };
    let pid = child.id();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    match rx.recv_timeout(AUTH_STATUS_TIMEOUT) {
        Ok(Ok(output)) => {
            println!("exit: {}", output.status);
            print_masked_output("stdout", &output.stdout);
            print_masked_output("stderr", &output.stderr);
        }
        Ok(Err(e)) => println!("failed while waiting for `claude`: {e}"),
        Err(_) => {
            println!(
                "timed out after {} s; stopping it",
                AUTH_STATUS_TIMEOUT.as_secs()
            );
            let _ = Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .output();
        }
    }
}

fn print_masked_output(label: &str, bytes: &[u8]) {
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    let shown = match serde_json::from_str::<Value>(text) {
        Ok(json) => serde_json::to_string_pretty(&json).unwrap_or_default(),
        Err(_) => text.to_owned(),
    };
    println!("{label}:\n{}", redact::mask(&shown));
}

/// True when the registry value exists. The value itself is never read into this process.
fn reg_value_exists(key: &str, name: &str) -> bool {
    Command::new("reg")
        .args(["query", key, "/v", name])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn environment() {
    section("Environment (set / not set only)");
    let label = |set: bool| if set { "set" } else { "not set" };
    for name in [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "CLAUDE_CONFIG_DIR",
    ] {
        let process = std::env::var_os(name).is_some_and(|v| !v.is_empty());
        let user = reg_value_exists(r"HKCU\Environment", name);
        let machine = reg_value_exists(
            r"HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Environment",
            name,
        );
        println!(
            "{name}: this process {}, user environment {}, machine environment {}",
            label(process),
            label(user),
            label(machine)
        );
    }
    println!(
        "(If ANTHROPIC_API_KEY is set where Claude Code runs, it uses the key instead of the subscription login.)"
    );
}

fn call_usage(token: &str) {
    println!("GET {USAGE_URL} (anthropic-beta: {OAUTH_BETA})");
    let tls = TlsConfig::builder()
        .provider(TlsProvider::NativeTls)
        .root_certs(RootCerts::PlatformVerifier)
        .build();
    let agent: Agent = Agent::config_builder()
        .tls_config(tls)
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(30)))
        .user_agent(concat!("UsageQuotaTray-spike/", env!("CARGO_PKG_VERSION")))
        .build()
        .into();

    let started = Instant::now();
    let result = agent
        .get(USAGE_URL)
        .header("Authorization", format!("Bearer {token}"))
        .header("anthropic-beta", OAUTH_BETA)
        .header("Accept", "application/json")
        .call();
    let elapsed = started.elapsed().as_millis();

    let mut response = match result {
        Ok(response) => response,
        Err(e) => {
            println!(
                "network error after {elapsed} ms: {}",
                redact::mask(&e.to_string())
            );
            println!("hint: offline, blocked by a proxy or firewall, or TLS intercepted?");
            return;
        }
    };

    let status = response.status();
    println!("HTTP {status} in {elapsed} ms");
    let mut headers: Vec<(String, String)> = response
        .headers()
        .iter()
        .map(|(name, value)| {
            let shown = if name.as_str() == "set-cookie" {
                "<omitted>".to_owned()
            } else {
                redact::mask(value.to_str().unwrap_or("<not ASCII>"))
            };
            (name.as_str().to_owned(), shown)
        })
        .collect();
    headers.sort();
    let header_text: String = headers
        .iter()
        .map(|(k, v)| format!("  {k}: {v}\n"))
        .collect();
    print!("headers:\n{header_text}");

    let body = match response.body_mut().read_to_string() {
        Ok(body) => body,
        Err(e) => {
            println!("could not read the body: {}", redact::mask(&e.to_string()));
            String::new()
        }
    };
    let shown = match serde_json::from_str::<Value>(&body) {
        Ok(json) => serde_json::to_string_pretty(&json).unwrap_or_default(),
        Err(_) => body.chars().take(4000).collect(),
    };
    println!("body:\n{}", redact::mask(&shown));

    let retry_after = headers
        .iter()
        .find(|(k, _)| k == "retry-after")
        .map(|(_, v)| v.as_str());
    println!("\n{}", hint(status.as_u16(), retry_after));
    save(status.as_u16(), &header_text, &body);
}

fn hint(status: u16, retry_after: Option<&str>) -> String {
    match status {
        200..=299 => "OK: map the body's keys onto Meters.".to_owned(),
        401 => "401: the token was rejected (expired or revoked). Run `claude` once so it renews its \
                login; this tool never refreshes tokens."
            .to_owned(),
        403 => "403: forbidden. The token may lack a scope this endpoint needs, or the endpoint refuses \
                this client."
            .to_owned(),
        404 => "404: the endpoint may have moved or been removed.".to_owned(),
        429 => format!("429: rate limited. Retry-After: {}", retry_after.unwrap_or("(none)")),
        500..=599 => format!("{status}: server error; try again later."),
        _ => format!("{status}: unexpected status."),
    }
}

/// Saves the raw body (usage data, kept out of git by .gitignore) and the masked headers.
fn save(status: u16, header_text: &str, body: &str) {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("out");
    let stamp = clock::now_secs();
    let body_path = dir.join(format!("usage-{stamp}.json"));
    let headers_path = dir.join(format!("usage-{stamp}.headers.txt"));
    let result = std::fs::create_dir_all(&dir)
        .and_then(|()| std::fs::write(&body_path, body))
        .and_then(|()| std::fs::write(&headers_path, format!("HTTP {status}\n{header_text}")));
    match result {
        Ok(()) => println!(
            "saved {} and {}",
            body_path.display(),
            headers_path.display()
        ),
        Err(e) => println!("could not save the response: {e}"),
    }
}
