//! Open incidents on Anthropic's public status page, so an outage shows up
//! beside the usage numbers instead of being mistaken for one's own quota.
//!
//! The page (<https://status.claude.com>) is a Statuspage instance, whose
//! documented, unauthenticated API v2 lists the unresolved incidents:
//! `GET /api/v2/incidents/unresolved.json`. Only that list is read — the
//! component states and scheduled maintenances are not — because the question
//! the monitor answers is "is something wrong right now", and an open incident
//! is the page's own answer to it.
//!
//! The check is a decoration, like the plan name: it is skipped with
//! `--no-incidents`, and a failed fetch is "unknown", shown as nothing, never as
//! an error in the usage display. Colour follows the incident's `impact`:
//! `critical` and `major` are critical, the rest warning.

use std::time::Duration;

use chrono::{DateTime, Local};
use serde_json::Value;

use crate::views::Level;

const URL: &str = "https://status.claude.com/api/v2/incidents/unresolved.json";

/// Where an incident links to when it carries no shortlink of its own, or one
/// that did not survive [`is_safe_url`].
pub const STATUS_PAGE: &str = "https://status.claude.com/";

/// When false, the status page is never contacted (`--no-incidents`). Set once
/// at startup.
static ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Turns the status-page check off.
pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, std::sync::atomic::Ordering::Relaxed);
}

/// Whether the status page is consulted at all.
pub fn enabled() -> bool {
    ENABLED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Statuspage's incident impact levels, worst last so they order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Impact {
    None,
    Minor,
    Major,
    Critical,
}

impl Impact {
    fn parse(text: &str) -> Impact {
        match text {
            "critical" => Impact::Critical,
            "major" => Impact::Major,
            "minor" => Impact::Minor,
            _ => Impact::None,
        }
    }

    /// The severity colour an incident of this impact is shown in.
    pub fn level(self) -> Level {
        match self {
            Impact::Critical | Impact::Major => Level::Crit,
            Impact::Minor | Impact::None => Level::Warn,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Impact::Critical => "critical",
            Impact::Major => "major",
            Impact::Minor => "minor",
            Impact::None => "none",
        }
    }
}

/// One unresolved incident, e.g. "Elevated errors for Claude Sonnet 5".
#[derive(Debug, Clone)]
pub struct Incident {
    pub name: String,
    pub impact: Impact,
    /// Statuspage's lifecycle stage: investigating, identified, monitoring.
    pub status: String,
    pub url: Option<String>,
}

impl Incident {
    /// Where the incident links to — polybar's click, the TUI's hyperlink: its
    /// own status-page shortlink when the reply carried a usable one, else the
    /// page front door.
    pub fn link(&self) -> &str {
        match self.url.as_deref() {
            Some(url) if is_safe_url(url) => url,
            _ => STATUS_PAGE,
        }
    }
}

/// Whether a shortlink is fit to pass on. Narrow on purpose: the value arrives
/// over the network and ends up inside a command a shell will run (polybar) or
/// inside a terminal escape sequence (the TUI), so the test is "a plain https
/// URL of printable characters", not "parses as a URL". Everything else falls
/// back to [`STATUS_PAGE`], which loses the deep link and nothing else.
fn is_safe_url(url: &str) -> bool {
    url.starts_with("https://")
        && url.len() <= 300
        // No spaces, no control characters (which would end an OSC 8 escape),
        // nothing needing an encoding guess.
        && url.chars().all(|c| c.is_ascii_graphic())
        // Quoting in polybar's command keeps the shell out of it; these three
        // are the characters that would escape the quoting or polybar's tag
        // syntax.
        && !url.contains('\'')
        && !url.contains('{')
        && !url.contains('}')
}

/// The unresolved incidents, worst first, and when they were checked.
#[derive(Debug, Clone)]
pub struct Incidents {
    pub open: Vec<Incident>,
    pub fetched_at: DateTime<Local>,
}

impl Incidents {
    /// The incident to headline: the highest-impact one, the newest of a tie.
    pub fn worst(&self) -> Option<&Incident> {
        self.open.first()
    }
}

/// Fetches the unresolved incidents, or why it could not.
pub fn fetch() -> Result<Incidents, String> {
    parse(&fetch_raw()?)
}

/// The raw status-page reply. Separate from [`fetch`] for the same reason as
/// `usage_api::fetch_raw`: the shared cache stores exactly what the page said.
pub fn fetch_raw() -> Result<String, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout_read(Duration::from_secs(6))
        .build();
    match agent.get(URL).set("User-Agent", "claude-usage-rs").call() {
        Ok(response) => response.into_string().map_err(|e| e.to_string()),
        Err(ureq::Error::Status(code, _)) => Err(format!("status page HTTP {code}")),
        Err(e) => Err(e.to_string()),
    }
}

pub fn parse(body: &str) -> Result<Incidents, String> {
    let value: Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    let list = value
        .get("incidents")
        .and_then(Value::as_array)
        .ok_or("no incidents list in the status page reply")?;
    let mut open: Vec<Incident> = list.iter().filter_map(incident).collect();
    // Worst first; the sort is stable, so within one impact the page's own
    // order (newest first) is kept.
    open.sort_by_key(|incident| std::cmp::Reverse(incident.impact));
    Ok(Incidents {
        open,
        fetched_at: Local::now(),
    })
}

fn incident(value: &Value) -> Option<Incident> {
    let name = value.get("name").and_then(Value::as_str)?.trim();
    if name.is_empty() {
        return None;
    }
    let status = value
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    // The unresolved endpoint should not list these; belt and braces.
    if matches!(status.as_str(), "resolved" | "postmortem") {
        return None;
    }
    Some(Incident {
        name: name.to_string(),
        impact: Impact::parse(value.get("impact").and_then(Value::as_str).unwrap_or("")),
        status,
        url: value
            .get("shortlink")
            .and_then(Value::as_str)
            .map(String::from),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed from a live reply (2026-09-03), plus a second, lesser incident.
    pub const SAMPLE: &str = r#"{
        "page": {"id": "tymt9n04zgry", "name": "Claude"},
        "incidents": [
            {"name": "Degraded file uploads", "status": "monitoring", "impact": "minor",
             "shortlink": "https://stspg.io/minor1"},
            {"name": "Elevated errors for Claude Sonnet 5", "status": "investigating",
             "impact": "major", "shortlink": "https://stspg.io/jt14jk41pjsf",
             "incident_updates": [{"body": "We are investigating elevated errors."}]}
        ]
    }"#;

    #[test]
    fn incidents_are_listed_worst_first() {
        let incidents = parse(SAMPLE).unwrap();
        assert_eq!(incidents.open.len(), 2);
        let worst = incidents.worst().unwrap();
        assert_eq!(worst.name, "Elevated errors for Claude Sonnet 5");
        assert_eq!(worst.impact, Impact::Major);
        assert_eq!(worst.status, "investigating");
        assert_eq!(worst.url.as_deref(), Some("https://stspg.io/jt14jk41pjsf"));
        assert_eq!(incidents.open[1].impact, Impact::Minor);
    }

    #[test]
    fn an_empty_list_is_all_clear() {
        let incidents = parse(r#"{"incidents": []}"#).unwrap();
        assert!(incidents.open.is_empty());
        assert!(incidents.worst().is_none());
    }

    #[test]
    fn resolved_and_nameless_entries_are_dropped() {
        let body = r#"{"incidents": [
            {"name": "Old outage", "status": "resolved", "impact": "critical"},
            {"name": "  ", "status": "investigating", "impact": "major"},
            {"name": "Still open", "status": "identified", "impact": "none"}
        ]}"#;
        let incidents = parse(body).unwrap();
        assert_eq!(incidents.open.len(), 1);
        assert_eq!(incidents.open[0].name, "Still open");
        assert_eq!(incidents.open[0].impact, Impact::None);
    }

    #[test]
    fn a_reply_without_the_list_is_an_error() {
        assert!(parse("{}").is_err());
        assert!(parse("not json").is_err());
    }

    #[test]
    fn impact_maps_to_severity() {
        assert_eq!(Impact::Critical.level(), Level::Crit);
        assert_eq!(Impact::Major.level(), Level::Crit);
        assert_eq!(Impact::Minor.level(), Level::Warn);
        assert_eq!(Impact::None.level(), Level::Warn);
        assert!(Impact::Critical > Impact::Major && Impact::Major > Impact::Minor);
    }

    #[test]
    fn toggle_defaults_on() {
        // Restored afterwards: the flag is process-wide and tests share it.
        assert!(enabled());
        set_enabled(false);
        assert!(!enabled());
        set_enabled(true);
    }
}
