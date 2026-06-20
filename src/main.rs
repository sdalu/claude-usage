//! claude-usage: a live terminal monitor for Claude plan usage.
//!
//! It shows the percent of each usage window (5-hour session, weekly, and any
//! per-model weekly windows) that has been consumed, fetched live from the same
//! endpoint as `claude /status`. Its only local dependency is the OAuth token
//! in `~/.claude/.credentials.json`; it does not read the Claude CLI session
//! logs or any other CLI byproduct.

mod json;
mod rotation;
mod tui;
mod usage_api;
mod views;

const USAGE: &str = "\
Usage: claude-usage [-1 [--no-border]] [--json] [--write-back] [--check-rotation] [--help]

A live monitor of your Claude plan usage windows (percent of allowance used),
fetched from https://api.anthropic.com/api/oauth/usage using the OAuth token in
~/.claude/.credentials.json.

Options:
  -1           render the dashboard once and exit (no live updates)
  --no-border  with -1, drop the box border (just the gauge rows)
  --json       print the usage windows as JSON and exit (no UI)
  --write-back refresh an expired OAuth token and write the rotated
               credentials back to ~/.claude/.credentials.json. Default
               is read-only: an expired token is reported, never rewritten.
  --check-rotation
               passively check whether the OAuth refresh token rotates
               (read-only; no network). Run once to baseline, then again
               after Claude Code refreshes.
  -h, --help   show this help

Keys (interactive UI):
  q / Esc / Ctrl-C   quit
  r / Enter          refresh now
  t                  toggle auto-refresh (5m / 1m)
  u                  toggle the updated banner
  ?                  toggle the key help overlay

The monitor auto-refreshes every 5 minutes (every minute after pressing t).";

fn main() {
    std::process::exit(real_main());
}

fn real_main() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut json_mode = false;
    let mut oneshot = false;
    let mut no_border = false;
    for arg in &args {
        match arg.as_str() {
            "--json" => json_mode = true,
            "-1" => oneshot = true,
            "--no-border" => no_border = true,
            "--write-back" => usage_api::set_write_back(true),
            // Standalone diagnostic: runs and exits, ignoring the other flags.
            "--check-rotation" => return rotation::run(),
            "-h" | "--help" => {
                println!("{USAGE}");
                return 0;
            }
            other => {
                eprintln!("claude-usage: unknown option: {other}");
                eprintln!("{USAGE}");
                return 1;
            }
        }
    }

    if no_border && !oneshot {
        eprintln!("claude-usage: --no-border is only valid with -1");
        eprintln!("{USAGE}");
        return 1;
    }

    let status = usage_api::fetch();

    if json_mode {
        match serde_json::to_string_pretty(&json::build(&status)) {
            Ok(s) => println!("{s}"),
            Err(e) => {
                eprintln!("claude-usage: {e}");
                return 1;
            }
        }
        // A failed fetch is reported in the JSON body; still exit non-zero so
        // scripts can detect it.
        return if status.is_err() { 1 } else { 0 };
    }

    // Plan/tier is account info; fetch it once (the refresh loop only re-hits
    // the usage endpoint).
    let plan = usage_api::fetch_plan();

    // Single-shot: render one frame inline and quit (no "?" flash, no input).
    if oneshot {
        let mut app = tui::App::new(status, plan).without_intro();
        if no_border {
            app = app.without_border();
        }
        return match app.render_once() {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("claude-usage: {e}");
                1
            }
        };
    }

    let app = tui::App::new(status, plan);
    let mut terminal = ratatui::init();
    let result = app.run(&mut terminal);
    ratatui::restore();

    if let Err(e) = result {
        eprintln!("claude-usage: {e}");
        return 1;
    }
    0
}
