//! Machine-readable form of the live usage windows, and of the status page's
//! open incidents, for scripting and status-line integration.

use serde_json::{json, Value};

use crate::incidents::{Incident, Incidents};
use crate::usage_api::{LimitWindow, UsageStatus};

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
    json!({
        "label": w.label,
        "group": w.group,
        "percent": w.percent,
        "severity": w.severity,
        "resets_at": w.resets_at.map(|t| t.to_rfc3339()),
        "is_active": w.is_active,
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
