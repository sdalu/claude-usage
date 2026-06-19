//! Machine-readable form of the live usage windows for scripting and
//! status-line integration.

use serde_json::{json, Value};

use crate::usage_api::{LimitWindow, UsageStatus};

pub fn build(status: &Result<UsageStatus, String>) -> Value {
    match status {
        Ok(s) => json!({
            "fetched_at": s.fetched_at.to_rfc3339(),
            "windows": s.windows.iter().map(window).collect::<Vec<_>>(),
        }),
        Err(e) => json!({ "error": e }),
    }
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
