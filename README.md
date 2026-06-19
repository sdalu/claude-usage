# claude-usage

A live terminal monitor for Claude plan usage limits. It shows how much of each
usage window — the 5-hour session, the weekly all-models window, and any
per-model weekly windows — you have consumed, as percent-of-allowance progress
bars. These are the same numbers `claude /status` reports.

The data comes entirely from Anthropic's usage API; the tool does **not** read
the Claude CLI session logs or any other CLI byproduct. Its only local
dependency is the OAuth token in `~/.claude/.credentials.json`, which is needed
to authenticate the request.

## Screenshot

```text
┌ Claude usage limits · Max 5x ─────────────────────────────────────── ? ┐
│ Session (5h)        ━━───────────────────  11% used · resets in 37m    │
│ Week (all models) ◀ ━━━━━━━━─────────────  39% used · resets Tue 06:00 │
│ Week · Sonnet       ━━━━━━━━━━━━━────────  62% used · resets Tue 06:00 │
└────────────────────────────────────────────────────────────────────────┘
```

Press `?` at any time for the key cheat sheet. The `?` marker flashes (cycling
colour) on the top-right border for about 3 seconds at startup, then fades so
the border closes back up. The last-updated banner on the bottom border is off
by default — press `u` to notch it in (`┤ updated 18:57 / 5 ├`, where `5` is
the auto-refresh interval in minutes and the `/` keeps the border colour):

```text
┌ Keys ──────────────────────────────────────────┐
│  q / Esc       quit                            │
│  Ctrl-C        quit                            │
│  r             refresh now                     │
│  t             toggle auto-refresh (now 5m)    │
│  u             toggle the updated banner       │
│  ?             toggle this help                │
│                                                │
│  any key closes this help                      │
└────────────────────────────────────────────────┘
```

The bar is a `LineGauge` whose filled fraction (drawn here as `━` over a `─`
track) is coloured by how close the window is to its limit — green, then
yellow, then red — honouring the API's own `severity`. In a real terminal the
fill is shown by that colour rather than a separate glyph. The reset shows as a
countdown
(`resets in 1h 20m`, in d/h/m) until it is more than two days out, when it
switches to the absolute `resets Tue 06:00`. The `◀` marks the **binding**
window (the API's `is_active`) — the one you will hit first.

When the terminal is too narrow for the full labels, they shorten in steps —
first the word `resets` is dropped (`resets in 1h 20m` → `in 1h 20m`), then
`used` (`11% used` → `11%`) — applied to every row so the columns stay aligned. On credit-based plans, dollar figures (`$used / $limit` and
an overall `Spend` bar) appear automatically; on subscription plans the API
reports no dollars, so they are omitted. Regenerate this capture with
`cargo test render_monitor -- --nocapture`.

## Build & run

Requires a Rust toolchain (1.74+). Install via <https://rustup.rs> if needed.

```sh
cargo run --release           # launch the monitor
cargo run --release -- -1     # render once (coloured) and quit, no live updates
cargo run --release -- --json # print the windows as JSON, then exit
cargo test                    # run the unit tests
```

`-1` is single-shot: it prints one coloured frame inline (no startup `?` flash,
no input loop) and exits, leaving the dashboard in your terminal.

Install it onto your PATH:

```sh
cargo install --path .
```

### Keys

| Key             | Action                          |
| --------------- | ------------------------------- |
| `q` / `Esc` / `Ctrl-C` | quit                     |
| `r`             | refresh now                     |
| `t`             | toggle auto-refresh (5m ⇄ 1m)   |
| `u`             | toggle the updated banner       |
| `?`             | toggle the key cheat sheet      |

The monitor also auto-refreshes on its own — every 5 minutes by default, or
every minute after pressing `t`.

## How it works

On start (and on every `r`) the tool issues:

```
GET https://api.anthropic.com/api/oauth/usage
```

with the OAuth token from `~/.claude/.credentials.json` and the
`anthropic-beta: oauth-2025-04-20` header — exactly what `claude /status` does.
The response's `limits` array lists each window with a `percent`, `severity`,
`resets_at`, and optional model `scope`, which become the gauges. The request
has a short timeout; if it fails (offline, no token, rate limited, expired
token) the screen shows the error message instead of bars, and `r` retries.

Once at startup it also calls `GET /api/oauth/profile` to read your plan /
rate-limit tier (e.g. `Max 5x`) and show it in the header; this is more accurate
than the `subscriptionType` recorded in the credentials file. The refresh loop
only re-hits the usage endpoint.

Your token is sent only to Anthropic's own API.

### `--json`

```json
{
  "fetched_at": "2026-06-19T18:51:36+02:00",
  "windows": [
    { "label": "Session (5h)", "group": "session", "percent": 11,
      "severity": "normal", "resets_at": "2026-06-19T20:30:00+00:00",
      "is_active": false },
    { "label": "Week (all models)", "group": "weekly", "percent": 39,
      "severity": "normal", "resets_at": "2026-06-23T06:00:00+00:00",
      "is_active": true }
  ]
}
```

On a failed fetch the body is `{ "error": "..." }` and the exit code is non-zero.

## Layout

```
src/
├── main.rs        argument parsing + dispatch (UI vs --json)
├── usage_api.rs   fetches & parses GET /api/oauth/usage
├── views.rs       UsageStatus -> pre-formatted gauge rows
├── tui.rs         ratatui monitor (gauges, refresh, key handling)
└── json.rs        JSON form of the usage windows
```
