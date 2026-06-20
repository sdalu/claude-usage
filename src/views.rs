//! Turns the live [`UsageStatus`] into the pre-formatted gauge rows the TUI
//! renders. There is no local data involved — every value here comes from the
//! `/api/oauth/usage` response.

use chrono::{DateTime, Local};

use crate::usage_api::{LimitWindow, UsageStatus};

/// Beyond this, show the absolute reset time instead of a countdown.
const ABSOLUTE_THRESHOLD: i64 = 2 * 86_400;

/// Shown in the reset column for a limit window whose clock hasn't started yet
/// — the API reports no `resets_at` until the window is first used. The TUI
/// recognises this value and renders it without the usual "resets " prefix.
pub const NOT_STARTED: &str = "not started";

/// One usage window rendered as a progress bar. The text pieces are kept
/// separate so the TUI can drop words (`resets`, then `used`) on narrow
/// terminals; `reset` holds only the suffix (e.g. `in 1h 20m`, `Tue 06:00`).
pub struct GaugeView {
    pub name: String,
    pub percent: i64,
    pub dollars: String,
    pub reset: String,
    pub ratio: f64,
    pub severity: String,
    pub active: bool,
}

pub struct Views {
    pub windows: Vec<GaugeView>,
    /// Set when the last fetch failed; shown in place of the gauges.
    pub error: Option<String>,
}

pub fn build(status: &Result<UsageStatus, String>) -> Views {
    match status {
        Ok(s) => {
            let now = Local::now();
            Views {
                windows: s.windows.iter().map(|w| gauge(w, now)).collect(),
                error: None,
            }
        }
        Err(e) => Views {
            windows: Vec::new(),
            error: Some(e.clone()),
        },
    }
}

fn gauge(window: &LimitWindow, now: DateTime<Local>) -> GaugeView {
    GaugeView {
        name: window.label.clone(),
        percent: window.percent.round() as i64,
        dollars: dollars(window),
        reset: match window.resets_at {
            Some(t) => reset_label(t, now),
            // The Spend window legitimately has no reset; everything else with
            // no reset is a window that hasn't started yet.
            None if window.group == "spend" => String::new(),
            None => NOT_STARTED.to_string(),
        },
        ratio: (window.percent / 100.0).clamp(0.0, 1.0),
        severity: window.severity.clone(),
        active: window.is_active,
    }
}

fn dollars(window: &LimitWindow) -> String {
    match (window.used_dollars, window.limit_dollars) {
        (Some(u), Some(l)) => format!("${u:.2} / ${l:.2}"),
        (Some(u), None) => format!("${u:.2}"),
        _ => String::new(),
    }
}

/// Reset suffix (the TUI prepends "resets " when it has room): a countdown
/// ("in 1h 20m") until more than two days out, then the absolute weekday/time
/// ("Tue 06:00").
fn reset_label(resets_at: DateTime<Local>, now: DateTime<Local>) -> String {
    let secs = (resets_at - now).num_seconds();
    if secs <= 0 {
        "now".to_string()
    } else if secs > ABSOLUTE_THRESHOLD {
        resets_at.format("%a %H:%M").to_string()
    } else {
        format!("in {}", duration_dhm(secs))
    }
}

/// Two most-significant units, no seconds: "1d 3h", "2h 15m", or "45m".
fn duration_dhm(secs: i64) -> String {
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3600;
    let minutes = (secs % 3600) / 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn duration_uses_two_units_no_seconds() {
        assert_eq!(duration_dhm(45 * 60 + 30), "45m");
        assert_eq!(duration_dhm(2 * 3600 + 15 * 60), "2h 15m");
        assert_eq!(duration_dhm(86_400 + 3 * 3600), "1d 3h");
    }

    #[test]
    fn reset_is_a_countdown_within_two_days() {
        let now = Local::now();
        assert_eq!(reset_label(now + Duration::minutes(90), now), "in 1h 30m");
        assert_eq!(reset_label(now - Duration::minutes(1), now), "now");
    }

    #[test]
    fn reset_is_absolute_beyond_two_days() {
        let now = Local::now();
        let label = reset_label(now + Duration::days(3), now);
        assert!(!label.starts_with("in "));
        assert!(label.contains(':'));
    }

    fn window(group: &str, resets_at: Option<DateTime<Local>>) -> LimitWindow {
        LimitWindow {
            label: "x".into(),
            group: group.into(),
            percent: 0.0,
            severity: "normal".into(),
            resets_at,
            is_active: false,
            used_dollars: None,
            limit_dollars: None,
        }
    }

    #[test]
    fn unstarted_window_shows_not_started() {
        let now = Local::now();
        assert_eq!(gauge(&window("session", None), now).reset, NOT_STARTED);
    }

    #[test]
    fn spend_window_has_no_reset_label() {
        let now = Local::now();
        assert_eq!(gauge(&window("spend", None), now).reset, "");
    }
}
