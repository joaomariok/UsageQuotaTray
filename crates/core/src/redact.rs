//! Redaction, carried over from the Phase 0 spike. Every log line goes through [`mask`],
//! so no token, key, email address or account identifier reaches a log file.

use serde_json::Value;

const REDACTED: &str = "<redacted>";

// `=` is left out so `key=value` splits; base64 padding then stays behind harmlessly.
fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '~' | '+' | '/' | '-')
}

/// A run of token characters is treated as a secret when it contains an Anthropic key or
/// token (`sk-ant-...`) or is long and mixed like an opaque token, JWT, UUID or prefixed
/// identifier (`wrkspc_01...`, 30 characters, is why the bar is 24 and not 32).
fn looks_secret(run: &str) -> bool {
    run.contains("sk-ant-")
        || (run.len() >= 24
            && run.chars().any(|c| c.is_ascii_digit())
            && run.chars().any(|c| c.is_ascii_alphabetic()))
}

/// Replaces runs of `keep` characters for which `secret` holds.
fn replace_runs(
    text: &str,
    keep: fn(char) -> bool,
    secret: fn(&str) -> bool,
    with: &str,
) -> String {
    let mut out = String::with_capacity(text.len());
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut String| {
        if !run.is_empty() {
            out.push_str(if secret(run) { with } else { run });
            run.clear();
        }
    };
    for c in text.chars() {
        if keep(c) {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
            out.push(c);
        }
    }
    flush(&mut run, &mut out);
    out
}

fn is_email_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '%' | '+' | '-' | '@')
}

fn looks_email(run: &str) -> bool {
    match run.split_once('@') {
        Some((local, domain)) => !local.is_empty() && domain.contains('.') && !domain.contains('@'),
        None => false,
    }
}

/// Masks email addresses, then anything that looks like a token, key or identifier.
pub fn mask(text: &str) -> String {
    let no_emails = replace_runs(text, is_email_char, looks_email, "<email>");
    replace_runs(&no_emails, is_token_char, looks_secret, REDACTED)
}

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Lists every leaf of `value` as `path: type`, adding ` = value` (masked) only for keys
/// in `allowed_value_keys`. Modules pass the keys that describe a plan, not a person.
pub fn outline(value: &Value, allowed_value_keys: &[&str]) -> Vec<String> {
    let mut lines = Vec::new();
    walk(value, "", None, allowed_value_keys, &mut lines);
    lines
}

fn walk(value: &Value, path: &str, key: Option<&str>, allowed: &[&str], lines: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                let child = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                walk(v, &child, Some(k), allowed, lines);
            }
        }
        Value::Array(items) => {
            for (i, v) in items.iter().enumerate() {
                walk(v, &format!("{path}[{i}]"), key, allowed, lines);
            }
        }
        leaf => {
            if key.is_some_and(|k| allowed.contains(&k)) {
                lines.push(format!(
                    "{path}: {} = {}",
                    type_name(leaf),
                    mask(&leaf.to_string())
                ));
            } else {
                lines.push(format!("{path}: {}", type_name(leaf)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const PLAN_KEYS: &[&str] = &["subscriptionType", "expiresAt", "scopes", "rateLimitTier"];

    #[test]
    fn masks_anthropic_tokens_and_keys() {
        assert_eq!(mask("Bearer sk-ant-oat01-AbC_123-xyz"), "Bearer <redacted>");
        assert_eq!(mask("key=sk-ant-api03-short"), "key=<redacted>");
    }

    #[test]
    fn masks_long_opaque_tokens_jwts_and_uuids() {
        let opaque = "a".repeat(30) + "1234567890";
        assert_eq!(mask(&format!("token {opaque} end")), "token <redacted> end");
        assert_eq!(
            mask("eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.sig"),
            "<redacted>"
        );
        assert_eq!(
            mask("org 3f2b8c1e-9a4d-4e6f-8b2a-1c3d5e7f9a0b"),
            "org <redacted>"
        );
        assert_eq!(
            mask("anthropic-workspace-id: wrkspc_01AbCdEfGhIjKlMnOpQrStUv"),
            "anthropic-workspace-id: <redacted>"
        );
    }

    #[test]
    fn keeps_usage_shaped_text() {
        let text = r#"{"five_hour":{"utilization":42.0,"resets_at":"2026-10-29T14:20:00.123456+00:00"}} https://api.anthropic.com/api/oauth/usage"#;
        assert_eq!(mask(text), text);
    }

    #[test]
    fn masks_email_addresses() {
        assert_eq!(
            mask("Logged in as jane.doe+cc@example.co.uk (Max)"),
            "Logged in as <email> (Max)"
        );
        assert_eq!(mask("not an email: a@b"), "not an email: a@b");
    }

    #[test]
    fn outline_prints_values_only_for_allowlisted_keys() {
        let creds = json!({
            "claudeAiOauth": {
                "accessToken": "sk-ant-oat01-secret",
                "subscriptionType": "max",
                "expiresAt": 1767225600000_i64,
                "scopes": ["user:inference", "user:profile"]
            },
            "organizationUuid": "3f2b8c1e-9a4d-4e6f-8b2a-1c3d5e7f9a0b"
        });
        let lines = outline(&creds, PLAN_KEYS);
        assert!(lines.contains(&"claudeAiOauth.accessToken: string".to_string()));
        assert!(lines.contains(&r#"claudeAiOauth.subscriptionType: string = "max""#.to_string()));
        assert!(lines.contains(&"claudeAiOauth.expiresAt: number = 1767225600000".to_string()));
        assert!(lines.contains(&r#"claudeAiOauth.scopes[1]: string = "user:profile""#.to_string()));
        assert!(lines.contains(&"organizationUuid: string".to_string()));
        assert!(
            lines
                .iter()
                .all(|l| !l.contains("secret") && !l.contains("3f2b8c1e"))
        );
    }

    #[test]
    fn outline_still_masks_secret_looking_allowlisted_values() {
        let lines = outline(
            &json!({ "rateLimitTier": "sk-ant-oat01-leaked" }),
            PLAN_KEYS,
        );
        assert_eq!(
            lines,
            vec![r#"rateLimitTier: string = "<redacted>""#.to_string()]
        );
    }
}
