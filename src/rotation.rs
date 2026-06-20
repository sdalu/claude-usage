//! `--check-rotation`: a passive, read-only probe answering one question —
//! does Anthropic rotate the OAuth refresh token when Claude Code refreshes it?
//!
//! That single fact decides whether claude-usage's credential write-back can
//! ever race Claude Code: if the refresh token is *not* rotated, both processes
//! can refresh freely and there is nothing to fight over; if it *is* rotated,
//! the loser of a simultaneous refresh can be logged out.
//!
//! This probe never calls the network and never writes the credentials file. On
//! first run it records a fingerprint of the current refresh token (a
//! non-reversible hash — the token itself is never stored) plus the access
//! token's expiry. On later runs it compares: once a refresh has actually
//! happened (the expiry moved forward), it reports whether the refresh token
//! changed. Re-run it after Claude Code has refreshed on its own.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use chrono::{Local, TimeZone};
use serde_json::{json, Value};

use crate::usage_api;

struct Snapshot {
    refresh_fp: u64,
    expires_at: i64,
}

pub fn run() -> i32 {
    let current = match read_snapshot() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("claude-usage: {e}");
            return 1;
        }
    };

    let path = match state_path() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("claude-usage: {e}");
            return 1;
        }
    };

    match load_state(&path) {
        None => {
            // First run: lay down the baseline to compare against later.
            println!("Recorded baseline:");
            print_snapshot(&current);
            println!(
                "\nRe-run `claude-usage --check-rotation` after Claude Code refreshes\n\
                 (the access-token expiry will have moved forward) to learn whether\n\
                 the refresh token rotates."
            );
            persist(&path, &current)
        }
        Some(prev) => report(&prev, &current, &path),
    }
}

/// Compares a fresh snapshot against the recorded baseline and reports the
/// verdict. Updates the baseline only once something actually changed, so
/// repeated runs before a refresh keep comparing against the original.
fn report(prev: &Snapshot, current: &Snapshot, path: &std::path::Path) -> i32 {
    if current.refresh_fp != prev.refresh_fp {
        println!("ROTATION DETECTED: the refresh token changed.");
        println!("  baseline fp: {:016x}", prev.refresh_fp);
        println!("  current  fp: {:016x}", current.refresh_fp);
        println!(
            "\nAnthropic rotates refresh tokens, so claude-usage's write-back can\n\
             race Claude Code. Prefer read-only (or a check-and-swap on write)."
        );
        return persist(path, current);
    }
    if current.expires_at != prev.expires_at {
        println!("No rotation: a refresh happened but the refresh token is unchanged.");
        println!("  refresh fp : {:016x} (stable across the refresh)", current.refresh_fp);
        println!(
            "  expiry moved {} -> {}",
            fmt_expiry(prev.expires_at),
            fmt_expiry(current.expires_at)
        );
        println!(
            "\nThe refresh token is not rotated, so claude-usage and Claude Code\n\
             cannot fight over it — the write-back is safe."
        );
        return persist(path, current);
    }
    println!("No refresh observed yet — the access token is unchanged.");
    println!("  refresh fp : {:016x}", current.refresh_fp);
    println!("  expiry     : {}", fmt_expiry(current.expires_at));
    println!("\nRun this again after Claude Code refreshes (around the expiry above).");
    0
}

fn read_snapshot() -> Result<Snapshot, String> {
    let path = usage_api::credentials_path()?;
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("reading {}: {e}", path.display()))?;
    let json: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let oauth = json
        .get("claudeAiOauth")
        .ok_or("no claudeAiOauth in credentials")?;
    let refresh = oauth
        .get("refreshToken")
        .and_then(Value::as_str)
        .ok_or("no refresh token in credentials")?;
    let expires_at = oauth.get("expiresAt").and_then(Value::as_i64).unwrap_or(0);
    Ok(Snapshot {
        refresh_fp: fingerprint(refresh),
        expires_at,
    })
}

/// A non-cryptographic, deterministic hash. We only need to detect *change*
/// without ever persisting the secret; `DefaultHasher` is seeded with fixed
/// keys, so the same input hashes identically across runs of this binary.
fn fingerprint(secret: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    secret.hash(&mut hasher);
    hasher.finish()
}

fn print_snapshot(s: &Snapshot) {
    println!("  refresh fp : {:016x}", s.refresh_fp);
    println!("  expiry     : {}", fmt_expiry(s.expires_at));
}

fn fmt_expiry(ms: i64) -> String {
    match Local.timestamp_millis_opt(ms).single() {
        Some(dt) => dt.format("%Y-%m-%d %H:%M:%S").to_string(),
        None => "unknown".to_string(),
    }
}

fn state_path() -> Result<PathBuf, String> {
    let dir = dirs::cache_dir()
        .ok_or("no cache directory")?
        .join("claude-usage");
    std::fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    Ok(dir.join("rotation-check.json"))
}

fn load_state(path: &std::path::Path) -> Option<Snapshot> {
    let text = std::fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    let refresh_fp = value
        .get("refresh_fp")
        .and_then(Value::as_str)
        .and_then(|s| u64::from_str_radix(s, 16).ok())?;
    let expires_at = value.get("expires_at").and_then(Value::as_i64)?;
    Some(Snapshot {
        refresh_fp,
        expires_at,
    })
}

/// Writes the snapshot best-effort; a failure to persist is a warning, not a
/// fatal error, since the probe still printed its verdict.
fn persist(path: &std::path::Path, s: &Snapshot) -> i32 {
    let body = json!({
        "refresh_fp": format!("{:016x}", s.refresh_fp),
        "expires_at": s.expires_at,
        "recorded_at": Local::now().to_rfc3339(),
    });
    if let Err(e) = serde_json::to_string_pretty(&body)
        .map_err(|e| e.to_string())
        .and_then(|t| std::fs::write(path, t).map_err(|e| e.to_string()))
    {
        eprintln!("claude-usage: warning: could not save rotation state: {e}");
    }
    0
}
