//! Machine-readable form of the live usage windows, and of the status page's
//! open incidents, for scripting and status-line integration.

use serde_json::{json, Value};

use crate::incidents::{Incident, Incidents};
use crate::usage_api::{LimitWindow, UsageStatus};
use crate::views;

pub fn build(status: &Result<UsageStatus, String>, incidents: Option<&Incidents>) -> Value {
    let mut value = match status {
        Ok(s) => json!({
            "fetched_at": s.fetched_at.to_rfc3339(),
            "windows": s.windows.iter().map(window).collect::<Vec<_>>(),
        }),
        Err(e) => json!({ "error": e }),
    };
    // Null when the page was not checked (or could not be), so a script can
    // tell "all clear" from "unknown".
    value["incidents"] = match incidents {
        Some(i) => Value::Array(i.open.iter().map(incident).collect()),
        None => Value::Null,
    };
    value
}

fn window(w: &LimitWindow) -> Value {
    let now = chrono::Local::now();
    json!({
        "label": w.label,
        "group": w.group,
        "percent": w.percent,
        "severity": w.severity,
        "resets_at": w.resets_at.map(|t| t.to_rfc3339()),
        "is_active": w.is_active,
        // The percent an even rate of use would have reached by now; null when
        // the window has not started or its length is unknown.
        "pace": views::pace(w, now).map(|p| p * 100.0),
        "ahead_of_pace": views::ahead_of_pace(w, now),
        "used_dollars": w.used_dollars,
        "limit_dollars": w.limit_dollars,
    })
}

fn incident(i: &Incident) -> Value {
    json!({
        "name": i.name,
        "impact": i.impact.as_str(),
        "status": i.status,
        "url": i.url,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage_api;

    const USAGE: &str = r#"{"limits":[{"kind":"session","group":"session","percent":11,
        "severity":"normal","scope":null,"is_active":false}]}"#;

    #[test]
    fn windows_carry_their_pace() {
        let reset = (chrono::Local::now() + chrono::Duration::minutes(150)).to_rfc3339();
        let body = format!(
            r#"{{"limits":[{{"kind":"session","group":"session","percent":62,
                "severity":"normal","resets_at":"{reset}","is_active":false}}]}}"#
        );
        let value = build(&usage_api::parse(&body), None);
        let pace = value["windows"][0]["pace"].as_f64().unwrap();
        assert!((pace - 50.0).abs() < 0.1, "{pace}");
        assert_eq!(value["windows"][0]["ahead_of_pace"], json!(true));
        // No reset time (not started): no pace, and never ahead.
        let value = build(&usage_api::parse(USAGE), None);
        assert!(value["windows"][0]["pace"].is_null());
        assert_eq!(value["windows"][0]["ahead_of_pace"], json!(false));
    }

    #[test]
    fn incidents_are_listed_or_null() {
        let status = usage_api::parse(USAGE);
        let open = crate::incidents::parse(
            r#"{"incidents":[{"name":"Elevated errors","status":"investigating",
                "impact":"major","shortlink":"https://stspg.io/x"}]}"#,
        )
        .unwrap();

        let value = build(&status, Some(&open));
        assert_eq!(value["windows"][0]["percent"], json!(11.0));
        assert_eq!(value["incidents"][0]["name"], json!("Elevated errors"));
        assert_eq!(value["incidents"][0]["impact"], json!("major"));
        assert_eq!(value["incidents"][0]["url"], json!("https://stspg.io/x"));

        let all_clear = crate::incidents::parse(r#"{"incidents":[]}"#).unwrap();
        assert_eq!(build(&status, Some(&all_clear))["incidents"], json!([]));
        assert!(build(&status, None)["incidents"].is_null());
        // The failure shape carries it too.
        assert!(build(&Err("offline".into()), None)["incidents"].is_null());
    }
}
