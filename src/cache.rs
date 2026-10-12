//! A cross-process cache of the last responses, so several claude-usage
//! instances cost one API call between them.
//!
//! This exists because the natural setup multiplies instances: polybar starts a
//! bar (and so a module process) per monitor, and a TUI opened alongside them
//! is one more. Each would otherwise poll `/api/oauth/usage` on its own against
//! a rate-limited endpoint. Instead every instance reads
//! `~/.cache/claude-usage/usage.json`, and one that finds no entry younger than
//! its TTL takes a lock file, fetches, and publishes the result for the others
//! — including a failure, which is worth sharing precisely because retrying it
//! from three processes is what a rate limit punishes.
//!
//! The status-page check (`incidents.json`) is shared the same way, in a slot
//! of its own with its own lock, so the two fetches never wait on each other.
//!
//! An entry stores the raw response body, so every reader parses it exactly as
//! the fetcher would, and carries the original fetch time rather than the time
//! it was read.

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{Local, TimeZone, Utc};
use serde_json::{json, Value};

use crate::incidents::{self, Incidents};
use crate::usage_api::{self, UsageStatus};

/// A lock older than this is treated as abandoned (a crashed or killed
/// instance). Comfortably longer than the HTTP timeouts in `usage_api`.
const LOCK_STALE: Duration = Duration::from_secs(30);

/// How long to wait for the instance holding the lock to publish, when we have
/// nothing cached at all to fall back on.
const LOCK_WAIT: Duration = Duration::from_secs(8);
const LOCK_POLL: Duration = Duration::from_millis(100);

/// The cache slots: each is `<name>.json` plus `<name>.lock` in the cache dir.
const USAGE: &str = "usage";
const INCIDENTS: &str = "incidents";

/// When false, every instance fetches for itself (`--no-cache`).
static ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Turns cross-process sharing off. Set once at startup.
pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, std::sync::atomic::Ordering::Relaxed);
}

/// Whether cross-process sharing is on.
pub fn enabled() -> bool {
    ENABLED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Returns the shared usage status, fetching only if no published entry is
/// younger than `ttl`. A zero TTL forces a fetch (and publishes it), which is
/// what an explicit refresh should do.
pub fn fetch(ttl: Duration) -> Result<UsageStatus, String> {
    if !enabled() {
        return usage_api::fetch();
    }
    match dir() {
        Ok(dir) => shared(&dir, ttl, usage_api::fetch_raw),
        // No usable cache directory: sharing is a bonus, not a requirement.
        Err(_) => usage_api::fetch(),
    }
}

/// The shared status-page check, on the same terms as [`fetch`]. None when
/// the check is disabled or failed: an outage we cannot confirm is shown as
/// nothing, never as an error in the usage display.
pub fn fetch_incidents(ttl: Duration) -> Option<Incidents> {
    if !incidents::enabled() {
        return None;
    }
    if !enabled() {
        return incidents::fetch().ok();
    }
    match dir() {
        Ok(dir) => shared_incidents(&dir, ttl, incidents::fetch_raw).ok(),
        Err(_) => incidents::fetch().ok(),
    }
}

fn dir() -> Result<PathBuf, String> {
    let dir = dirs::cache_dir()
        .ok_or("no cache directory")?
        .join("claude-usage");
    std::fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    Ok(dir)
}

/// A raw response (or the failure) and when it was fetched, in ms since the
/// epoch — by whichever instance fetched it.
struct Stamped {
    body: Result<String, String>,
    at_ms: i64,
}

fn shared(
    dir: &Path,
    ttl: Duration,
    fetch_raw: impl Fn() -> Result<String, String>,
) -> Result<UsageStatus, String> {
    let stamped = shared_raw(dir, USAGE, ttl, fetch_raw);
    stamped
        .body
        .and_then(|body| stamp(usage_api::parse(&body), stamped.at_ms))
}

fn shared_incidents(
    dir: &Path,
    ttl: Duration,
    fetch_raw: impl Fn() -> Result<String, String>,
) -> Result<Incidents, String> {
    let stamped = shared_raw(dir, INCIDENTS, ttl, fetch_raw);
    stamped.body.and_then(|body| {
        incidents::parse(&body).map(|mut incidents| {
            if let Some(at) = local_time(stamped.at_ms) {
                incidents.fetched_at = at;
            }
            incidents
        })
    })
}

/// The slot's entry if it is younger than `ttl`, else a fresh fetch published
/// for the others — or, when another instance is already fetching, whatever it
/// published last.
fn shared_raw(
    dir: &Path,
    name: &str,
    ttl: Duration,
    fetch_raw: impl Fn() -> Result<String, String>,
) -> Stamped {
    let now = Utc::now().timestamp_millis();
    if let Some((age, stamped)) = read(dir, name, now) {
        if age < ttl {
            return stamped;
        }
    }

    match lock(dir, name) {
        Some(guard) => {
            let body = fetch_raw();
            let at_ms = Utc::now().timestamp_millis();
            publish(dir, name, &body, at_ms);
            drop(guard);
            Stamped { body, at_ms }
        }
        // Another instance is already fetching. Use whatever it published last
        // rather than making the same call beside it; with nothing published at
        // all, wait briefly for its result before falling back to our own.
        None => match read(dir, name, now) {
            Some((_, stamped)) => stamped,
            None => match wait_for_entry(dir, name) {
                Some(stamped) => stamped,
                None => Stamped {
                    body: fetch_raw(),
                    at_ms: Utc::now().timestamp_millis(),
                },
            },
        },
    }
}

/// The published entry and its age, or None when there is none (or it is
/// unreadable, which is treated the same way).
fn read(dir: &Path, name: &str, now_ms: i64) -> Option<(Duration, Stamped)> {
    let text = std::fs::read_to_string(entry_path(dir, name)).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    let at_ms = value.get("fetched_at").and_then(Value::as_i64)?;
    let age = Duration::from_millis(now_ms.saturating_sub(at_ms).max(0) as u64);

    let body = match value.get("error").and_then(Value::as_str) {
        Some(error) => Err(error.to_string()),
        None => Ok(value.get("body").and_then(Value::as_str)?.to_string()),
    };
    Some((age, Stamped { body, at_ms }))
}

/// Stamps a parsed usage status with when it was actually fetched (not when it
/// was read) so an age shown anywhere stays honest.
fn stamp(status: Result<UsageStatus, String>, at_ms: i64) -> Result<UsageStatus, String> {
    let mut status = status?;
    if let Some(at) = local_time(at_ms) {
        status.fetched_at = at;
    }
    Ok(status)
}

fn local_time(at_ms: i64) -> Option<chrono::DateTime<Local>> {
    Local.timestamp_millis_opt(at_ms).single()
}

/// Publishes a result for the other instances. Best effort: a cache we cannot
/// write costs an extra API call, nothing more.
fn publish(dir: &Path, name: &str, body: &Result<String, String>, at_ms: i64) {
    let entry = match body {
        Ok(body) => json!({ "fetched_at": at_ms, "body": body }),
        // Failures are shared too: three instances re-trying a rate limit in
        // parallel is exactly what the cache is here to prevent.
        Err(e) => json!({ "fetched_at": at_ms, "error": e }),
    };
    let text = match serde_json::to_string(&entry) {
        Ok(t) => t,
        Err(_) => return,
    };
    // Write-then-rename, so a reader never sees a half-written entry.
    let tmp = dir.join(format!("{name}.json.{}", std::process::id()));
    if std::fs::write(&tmp, text).is_ok() && std::fs::rename(&tmp, entry_path(dir, name)).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// Polls for an entry published by the lock holder, up to [`LOCK_WAIT`].
fn wait_for_entry(dir: &Path, name: &str) -> Option<Stamped> {
    let deadline = std::time::Instant::now() + LOCK_WAIT;
    while std::time::Instant::now() < deadline {
        std::thread::sleep(LOCK_POLL);
        if let Some((_, stamped)) = read(dir, name, Utc::now().timestamp_millis()) {
            return Some(stamped);
        }
    }
    None
}

fn entry_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.json"))
}

fn lock_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.lock"))
}

/// Removes the lock file when the fetch is done (or the fetcher panics).
struct Lock(PathBuf);

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Takes the slot's fetch lock, or None if another instance holds it. A lock
/// left behind by a dead instance is broken after [`LOCK_STALE`].
fn lock(dir: &Path, name: &str) -> Option<Lock> {
    let path = lock_path(dir, name);
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(_) => Some(Lock(path)),
        Err(_) if is_stale(&path) => {
            let _ = std::fs::remove_file(&path);
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .ok()
                .map(|_| Lock(path))
        }
        Err(_) => None,
    }
}

fn is_stale(path: &Path) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .map(|t| t.elapsed().map(|age| age > LOCK_STALE).unwrap_or(false))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const BODY: &str = r#"{"limits":[{"kind":"session","group":"session","percent":11,
        "severity":"normal","scope":null,"is_active":false}]}"#;

    const INCIDENTS_BODY: &str = r#"{"incidents":[{"name":"Elevated errors",
        "status":"investigating","impact":"major"}]}"#;

    /// A scratch cache directory of its own, so the tests never touch the real
    /// one and can run in parallel.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("claude-usage-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn second_instance_reuses_the_published_entry() {
        let dir = scratch("reuse");
        let calls = AtomicUsize::new(0);
        let fetch = || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(BODY.to_string())
        };

        let first = shared(&dir, Duration::from_secs(300), fetch).unwrap();
        let second = shared(&dir, Duration::from_secs(300), fetch).unwrap();

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(first.windows[0].percent, second.windows[0].percent);
        // The reader reports when it was fetched, not when it was read.
        assert_eq!(first.fetched_at, second.fetched_at);
    }

    #[test]
    fn an_expired_entry_is_refetched() {
        let dir = scratch("expired");
        let calls = AtomicUsize::new(0);
        let fetch = || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(BODY.to_string())
        };

        shared(&dir, Duration::from_secs(300), fetch).unwrap();
        // A zero TTL is how an explicit refresh forces a real fetch.
        shared(&dir, Duration::ZERO, fetch).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn failures_are_shared_too() {
        let dir = scratch("failure");
        let calls = AtomicUsize::new(0);
        let fetch = || {
            calls.fetch_add(1, Ordering::SeqCst);
            Err("Rate limited by the usage API".to_string())
        };

        let first = shared(&dir, Duration::from_secs(300), fetch);
        let second = shared(&dir, Duration::from_secs(300), fetch);

        // The second instance takes the first one's error rather than making
        // the same call into the same rate limit.
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(first.unwrap_err(), second.unwrap_err());
    }

    #[test]
    fn a_held_lock_yields_the_last_published_entry() {
        let dir = scratch("locked");
        let calls = AtomicUsize::new(0);
        let fetch = || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(BODY.to_string())
        };

        shared(&dir, Duration::from_secs(300), fetch).unwrap();
        let held = lock(&dir, USAGE).expect("lock is free after the first fetch");

        // Entry is stale (zero TTL) but another instance is fetching: reuse
        // what it published rather than fetching alongside it.
        shared(&dir, Duration::ZERO, fetch).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        drop(held);
        shared(&dir, Duration::ZERO, fetch).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn the_lock_is_exclusive_and_released_on_drop() {
        let dir = scratch("lock");
        let held = lock(&dir, USAGE).unwrap();
        assert!(lock(&dir, USAGE).is_none());
        drop(held);
        assert!(lock(&dir, USAGE).is_some());
    }

    #[test]
    fn an_abandoned_lock_is_broken() {
        let dir = scratch("stale-lock");
        let held = lock(&dir, USAGE).unwrap();
        // Back-date it past the staleness window, as a killed instance leaves it.
        let path = lock_path(&dir, USAGE);
        let old = std::time::SystemTime::now() - LOCK_STALE - Duration::from_secs(5);
        filetime_set(&path, old);
        assert!(is_stale(&path));
        assert!(lock(&dir, USAGE).is_some());
        drop(held);
    }

    #[test]
    fn slots_are_independent() {
        let dir = scratch("slots");
        let usage_calls = AtomicUsize::new(0);
        let incident_calls = AtomicUsize::new(0);
        let fetch_usage = || {
            usage_calls.fetch_add(1, Ordering::SeqCst);
            Ok(BODY.to_string())
        };
        let fetch_incidents = || {
            incident_calls.fetch_add(1, Ordering::SeqCst);
            Ok(INCIDENTS_BODY.to_string())
        };

        // A fresh usage entry does not stand in for the incidents check, and
        // holding one slot's lock does not block the other.
        shared(&dir, Duration::from_secs(300), fetch_usage).unwrap();
        let held = lock(&dir, USAGE).unwrap();
        let incidents = shared_incidents(&dir, Duration::from_secs(300), fetch_incidents).unwrap();
        drop(held);

        assert_eq!(usage_calls.load(Ordering::SeqCst), 1);
        assert_eq!(incident_calls.load(Ordering::SeqCst), 1);
        assert_eq!(incidents.worst().unwrap().name, "Elevated errors");

        // ... and the incidents entry is reused like the usage one.
        shared_incidents(&dir, Duration::from_secs(300), fetch_incidents).unwrap();
        assert_eq!(incident_calls.load(Ordering::SeqCst), 1);
        assert!(entry_path(&dir, INCIDENTS).exists());
    }

    /// Back-dates a file's mtime without pulling in a dependency for it.
    /// `touch -t` is the form both GNU and BSD userlands accept.
    fn filetime_set(path: &Path, when: std::time::SystemTime) {
        let secs = when
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let stamp = Local
            .timestamp_opt(secs, 0)
            .single()
            .unwrap()
            .format("%Y%m%d%H%M.%S")
            .to_string();
        let status = std::process::Command::new("touch")
            .arg("-t")
            .arg(stamp)
            .arg(path)
            .status()
            .expect("touch");
        assert!(status.success());
    }
}
