//! Live rate-limit utilization from the same source as Claude Code's `/status`:
//! `GET https://api.anthropic.com/api/oauth/usage`, authorized with the OAuth
//! token in `~/.claude/.credentials.json`.
//!
//! This is the tool's only data source. It does not read the Claude CLI session
//! logs (or any other CLI byproduct) — only the OAuth token from the
//! credentials file, which is required to authenticate the request.
//!
//! The response's `limits` array lists every active window (the 5-hour session,
//! the weekly all-models window, and any per-model weekly windows) with a
//! `percent`, `severity`, `resets_at`, and optional model `scope`.

use std::time::Duration;

use chrono::{DateTime, Local};
use serde_json::Value;

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const PROFILE_URL: &str = "https://api.anthropic.com/api/oauth/profile";
const OAUTH_BETA: &str = "oauth-2025-04-20";

/// One usage window: how much of an allowance is used and when it resets.
/// `used_dollars`/`limit_dollars` are populated only on credit-based plans.
#[derive(Debug, Clone)]
pub struct LimitWindow {
    pub label: String,
    pub group: String,
    pub percent: f64,
    pub severity: String,
    pub resets_at: Option<DateTime<Local>>,
    pub is_active: bool,
    pub used_dollars: Option<f64>,
    pub limit_dollars: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct UsageStatus {
    pub windows: Vec<LimitWindow>,
    pub fetched_at: DateTime<Local>,
}

/// Fetches the current usage windows, or an error string describing why it
/// could not (no token, offline, rate limited, expired token, ...).
pub fn fetch() -> Result<UsageStatus, String> {
    let token = read_token()?;
    let body = http_get(USAGE_URL, &token)?;
    parse(&body)
}

/// Fetches the account's plan / rate-limit tier (e.g. "Max 5x") from the
/// profile endpoint. Returns None on any failure — it is a non-essential
/// header decoration.
pub fn fetch_plan() -> Option<String> {
    let token = read_token().ok()?;
    let body = http_get(PROFILE_URL, &token).ok()?;
    let value: Value = serde_json::from_str(&body).ok()?;
    let tier = value
        .get("organization")?
        .get("rate_limit_tier")?
        .as_str()?;
    Some(plan_label(tier))
}

/// "default_claude_max_5x" -> "Max 5x", "default_claude_pro" -> "Pro", ...
fn plan_label(tier: &str) -> String {
    let base = tier.strip_prefix("default_").unwrap_or(tier);
    let base = base.strip_prefix("claude_").unwrap_or(base);
    match base {
        "pro" => "Pro".to_string(),
        "max" => "Max".to_string(),
        "max_5x" => "Max 5x".to_string(),
        "max_20x" => "Max 20x".to_string(),
        "team" => "Team".to_string(),
        "enterprise" => "Enterprise".to_string(),
        "ai" => "Free".to_string(),
        other => other
            .split('_')
            .map(title_or_multiplier)
            .collect::<Vec<_>>()
            .join(" "),
    }
}

/// "5x"/"20x" kept as-is, other words title-cased.
fn title_or_multiplier(word: &str) -> String {
    let is_multiplier =
        word.ends_with('x') && word[..word.len() - 1].chars().all(|c| c.is_ascii_digit());
    if is_multiplier {
        return word.to_string();
    }
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn read_token() -> Result<String, String> {
    let path = dirs::home_dir()
        .ok_or("no home directory")?
        .join(".claude/.credentials.json");
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("reading {}: {e}", path.display()))?;
    let json: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    json["claudeAiOauth"]["accessToken"]
        .as_str()
        .map(String::from)
        .ok_or_else(|| "no OAuth access token in credentials".to_string())
}

fn http_get(url: &str, token: &str) -> Result<String, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout_read(Duration::from_secs(6))
        .build();
    let request = agent
        .get(url)
        .set("Authorization", &format!("Bearer {token}"))
        .set("anthropic-beta", OAUTH_BETA)
        .set("Content-Type", "application/json")
        .set("User-Agent", "claude-usage-rs");
    match request.call() {
        Ok(response) => response.into_string().map_err(|e| e.to_string()),
        // Surface the API's own message (e.g. rate-limit) on a non-2xx status.
        Err(ureq::Error::Status(code, response)) => {
            let body = response.into_string().unwrap_or_default();
            Err(match error_message(&body) {
                Some(msg) => format!("HTTP {code}: {msg}"),
                None => format!("HTTP {code}"),
            })
        }
        Err(e) => Err(e.to_string()),
    }
}

fn error_message(body: &str) -> Option<String> {
    let v: Value = serde_json::from_str(body).ok()?;
    v.get("error")?
        .get("message")?
        .as_str()
        .map(String::from)
}

pub fn parse(body: &str) -> Result<UsageStatus, String> {
    let value: Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    if let Some(msg) = value
        .get("error")
        .and_then(|e| e.get("message"))
        .and_then(Value::as_str)
    {
        return Err(msg.to_string());
    }

    let mut windows: Vec<LimitWindow> = value
        .get("limits")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(|e| limit_window(e, &value)).collect())
        .unwrap_or_default();

    // Dollar spend (credit-based plans only) becomes one more window.
    if let Some(spend) = spend_window(&value) {
        windows.push(spend);
    }

    Ok(UsageStatus {
        windows,
        fetched_at: Local::now(),
    })
}

fn limit_window(value: &Value, root: &Value) -> Option<LimitWindow> {
    let kind = value.get("kind").and_then(Value::as_str)?;
    let percent = value
        .get("percent")
        .and_then(Value::as_f64)
        .or_else(|| value.get("utilization").and_then(Value::as_f64))?;
    let resets_at = value
        .get("resets_at")
        .and_then(Value::as_str)
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Local));
    let (used_dollars, limit_dollars) = window_dollars(value, root);
    Some(LimitWindow {
        label: label_for(kind, value.get("scope")),
        group: value
            .get("group")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        percent,
        severity: value
            .get("severity")
            .and_then(Value::as_str)
            .unwrap_or("normal")
            .to_string(),
        resets_at,
        is_active: value
            .get("is_active")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        used_dollars,
        limit_dollars,
    })
}

/// Dollar figures live either on the limit entry itself or on the matching
/// top-level window object (`five_hour`, `seven_day`, `seven_day_<model>`).
/// They are null on subscription plans.
fn window_dollars(entry: &Value, root: &Value) -> (Option<f64>, Option<f64>) {
    let used = entry.get("used_dollars").and_then(Value::as_f64);
    let limit = entry.get("limit_dollars").and_then(Value::as_f64);
    if used.is_some() || limit.is_some() {
        return (used, limit);
    }
    match top_level_key(entry).and_then(|k| root.get(k)) {
        Some(obj) => (
            obj.get("used_dollars").and_then(Value::as_f64),
            obj.get("limit_dollars").and_then(Value::as_f64),
        ),
        None => (None, None),
    }
}

fn top_level_key(entry: &Value) -> Option<String> {
    match entry.get("kind").and_then(Value::as_str)? {
        "session" => Some("five_hour".to_string()),
        "weekly_all" => Some("seven_day".to_string()),
        "weekly_scoped" => {
            let model = entry
                .get("scope")?
                .get("model")?
                .get("display_name")?
                .as_str()?;
            Some(format!("seven_day_{}", model.to_lowercase()))
        }
        _ => None,
    }
}

fn spend_window(root: &Value) -> Option<LimitWindow> {
    let spend = root.get("spend")?;
    if !spend.get("enabled").and_then(Value::as_bool).unwrap_or(false) {
        return None;
    }
    Some(LimitWindow {
        label: "Spend".to_string(),
        group: "spend".to_string(),
        percent: spend.get("percent").and_then(Value::as_f64).unwrap_or(0.0),
        severity: spend
            .get("severity")
            .and_then(Value::as_str)
            .unwrap_or("normal")
            .to_string(),
        resets_at: None,
        is_active: false,
        used_dollars: money(spend.get("used")),
        limit_dollars: money(spend.get("limit")),
    })
}

/// Parses a `{amount_minor, exponent, currency}` money object into dollars.
fn money(value: Option<&Value>) -> Option<f64> {
    let value = value?;
    let minor = value.get("amount_minor").and_then(Value::as_i64)?;
    let exponent = value.get("exponent").and_then(Value::as_i64).unwrap_or(2);
    Some(minor as f64 / 10f64.powi(exponent as i32))
}

fn label_for(kind: &str, scope: Option<&Value>) -> String {
    let model = scope
        .and_then(|s| s.get("model"))
        .and_then(|m| m.get("display_name"))
        .and_then(Value::as_str);
    match kind {
        "session" => "Session (5h)".to_string(),
        "weekly_all" => "Week (all models)".to_string(),
        "weekly_scoped" => match model {
            Some(m) => format!("Week \u{b7} {m}"),
            None => "Week (scoped)".to_string(),
        },
        other => {
            let pretty = humanize(other);
            match model {
                Some(m) => format!("{pretty} \u{b7} {m}"),
                None => pretty,
            }
        }
    }
}

fn humanize(kind: &str) -> String {
    let mut chars = kind.replace('_', " ").chars().collect::<Vec<_>>();
    if let Some(first) = chars.first_mut() {
        *first = first.to_ascii_uppercase();
    }
    chars.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "limits": [
            {"kind":"session","group":"session","percent":11,"severity":"normal",
             "resets_at":"2026-06-19T20:30:00+00:00","scope":null,"is_active":false},
            {"kind":"weekly_all","group":"weekly","percent":39,"severity":"normal",
             "resets_at":"2026-06-23T06:00:00+00:00","scope":null,"is_active":true},
            {"kind":"weekly_scoped","group":"weekly","percent":1,"severity":"warning",
             "resets_at":"2026-06-23T06:00:00+00:00",
             "scope":{"model":{"display_name":"Sonnet"}},"is_active":false}
        ]
    }"#;

    #[test]
    fn parses_limit_windows() {
        let status = parse(SAMPLE).unwrap();
        assert_eq!(status.windows.len(), 3);
        assert_eq!(status.windows[0].label, "Session (5h)");
        assert_eq!(status.windows[0].percent, 11.0);
        assert_eq!(status.windows[1].label, "Week (all models)");
        assert_eq!(status.windows[2].label, "Week \u{b7} Sonnet");
        assert_eq!(status.windows[2].severity, "warning");
    }

    #[test]
    fn surfaces_api_error() {
        let body = r#"{"error":{"type":"rate_limit_error","message":"Rate limited."}}"#;
        assert_eq!(parse(body).unwrap_err(), "Rate limited.");
    }

    #[test]
    fn no_limits_is_empty() {
        assert!(parse("{}").unwrap().windows.is_empty());
    }

    #[test]
    fn parses_dollars_and_spend() {
        let body = r#"{
            "five_hour": {"used_dollars": 1.5, "limit_dollars": 5.0},
            "limits": [
                {"kind":"session","group":"session","percent":30,
                 "severity":"normal","scope":null,"is_active":true}
            ],
            "spend": {"enabled": true, "percent": 12.0, "severity": "normal",
                "used": {"amount_minor": 1234, "exponent": 2, "currency": "USD"},
                "limit": {"amount_minor": 10000, "exponent": 2, "currency": "USD"}}
        }"#;
        let status = parse(body).unwrap();

        let session = &status.windows[0];
        assert!(session.is_active);
        assert_eq!(session.used_dollars, Some(1.5));
        assert_eq!(session.limit_dollars, Some(5.0));

        let spend = status.windows.iter().find(|w| w.label == "Spend").unwrap();
        assert_eq!(spend.used_dollars, Some(12.34));
        assert_eq!(spend.limit_dollars, Some(100.0));
    }

    #[test]
    fn disabled_spend_is_skipped() {
        let body = r#"{"limits": [], "spend": {"enabled": false}}"#;
        assert!(parse(body).unwrap().windows.is_empty());
    }

    #[test]
    fn plan_labels() {
        assert_eq!(plan_label("default_claude_max_5x"), "Max 5x");
        assert_eq!(plan_label("default_claude_max_20x"), "Max 20x");
        assert_eq!(plan_label("default_claude_pro"), "Pro");
        assert_eq!(plan_label("default_claude_ai"), "Free");
        assert_eq!(plan_label("claude_enterprise"), "Enterprise");
    }
}
