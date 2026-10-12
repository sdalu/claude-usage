# claude-usage

A live terminal monitor for Claude plan usage limits. It shows how much of each
usage window — the 5-hour session, the weekly all-models window, and any
per-model weekly windows — you have consumed, as percent-of-allowance progress
bars. These are the same numbers `claude /status` reports. It also watches
Anthropic's public status page, so an open incident there is called out beside
the numbers — an outage is then not mistaken for your own quota. See
[Open incidents](#open-incidents).

The data comes entirely from Anthropic's usage API; the tool does **not** read
the Claude CLI session logs or any other CLI byproduct. Its only local
dependency is the OAuth token in `~/.claude/.credentials.json` (or in
`CLAUDE_CODE_OAUTH_TOKEN`, see [Credential refresh](#credential-refresh)),
which is needed to authenticate the request. By default this file is treated as **read-only**:
when the access token has expired the tool just reports it and waits for Claude
Code to refresh it. Pass `--write-back` to let the tool refresh the token itself
and write the rotated credentials back. See
[Credential refresh](#credential-refresh) below.

## Screenshot

```text
┌ Claude usage limits · Max 5x ─────────────────────────────────────── ? ┐
│ Session (5h)        ━━────────────────╋──  11% used · resets in 37m    │
│ Week (all models) ◀ ━━━━━━━━────╋────────  39% used · resets Tue 06:00 │
│ Week · Sonnet       ━━━━━━━━━━━━╋────────  62% used · resets Tue 06:00 │
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

The `╋` on each bar marks the **pace**: where the fill would be by now if the
allowance were used at an even rate from the window's start to its reset. A
fill short of it is a window that lasts; a fill past it runs out before the
reset if use keeps up — above, the Sonnet week has used 62% with 57% of the
week gone. The window's length is read off its kind (5 hours for the session,
7 days for a weekly one), so a window of a kind not yet seen, the spend
window, and a window not started yet carry no mark.

When the terminal is too narrow for the full labels, they shorten in steps —
first the word `resets` is dropped (`resets in 1h 20m` → `in 1h 20m`), then
`used` (`11% used` → `11%`) — applied to every row so the columns stay aligned. On credit-based plans, dollar figures (`$used / $limit` and
an overall `Spend` bar) appear automatically; on subscription plans the API
reports no dollars, so they are omitted. Regenerate this capture with
`cargo test render_monitor -- --nocapture`.

### Open incidents

While <https://status.claude.com> lists an unresolved incident, its name is
shown on the top-right border, in red for a `major`/`critical` impact and
yellow otherwise. With several open, the worst is named and the rest counted
(`⚠ Elevated errors (+1)`):

```text
┌ Claude usage limits · Max 5x ─── ⚠ Elevated errors for Claude Sonnet 5 ┐
│ Session (5h)        ━━───────────────────  11% used · resets in 37m    │
```

The text is cut to the room the title leaves — first shortened with an
ellipsis (`⚠ Elevated err…`), then down to the bare `⚠`. Nothing is shown when
the page lists no incident, and nothing either when it could not be reached:
"unknown" is never presented as bad news, and never as an error in the usage
display. `--no-incidents` turns the check off (no marker, no request to the
status page). Regenerate the capture with
`cargo test open_incident -- --nocapture`.

The text is a terminal hyperlink (OSC 8) to the incident on the status page —
its `shortlink`, or the page's front door when it has none — so a click (often
Ctrl-click) opens it, in terminals that support hyperlinks (kitty, WezTerm,
foot, GNOME Terminal and other VTE ones, iTerm2, Windows Terminal). Elsewhere
the escape is ignored and the text is plain. The `-1` frame carries the same
link. In `--polybar` mode the marker is a click target too — see
[Polybar](#polybar).

## Build & run

Requires a Rust toolchain (1.74+). Install via <https://rustup.rs> if needed.

```sh
cargo run --release                  # launch the monitor
cargo run --release -- -1            # render once (coloured) and quit
cargo run --release -- -1 --no-border # render once without the box border
cargo run --release -- --polybar     # print one status-bar line, then exit
cargo run --release -- --polybar --interval 300   # ... or a line every 5 min
cargo run --release -- --json        # print the windows as JSON, then exit
cargo run --release -- --no-incidents # any mode, without the status-page check
cargo run --release -- --write-back  # also refresh an expired token in-place
cargo run --release -- --check-rotation  # check if the refresh token rotates
```

`-1` is single-shot: it prints one coloured frame inline (no startup `?` flash,
no input loop) and exits, leaving the dashboard in your terminal. Add
`--no-border` (only valid with `-1`) to drop the surrounding box — just the
gauge rows, with no left margin and a single trailing space — handy for
embedding the output.

Install it onto your PATH, either through cargo or under a prefix:

```sh
cargo install --path .               # into ~/.cargo/bin
make install                         # into /usr/local/bin (PREFIX, DESTDIR)
```

The Makefile wraps cargo and works under both GNU and BSD make; a bare `make`
lists its targets and the variables in force:

```sh
make check      # preflight: the release agrees with cargo and the tags; rustfmt; clippy
make tests      # the suite (cargo test); it never touches the network
make version    # the release, from Cargo.toml
make tag        # tag v<release>, after the preflight and the suite pass
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
every minute after pressing `t`. That auto-refresh goes through the shared
cache, so a monitor open beside the polybar modules costs no extra API call;
`r` always fetches for real. See
[Sharing between instances](#sharing-between-instances).

## Polybar

`--polybar` replaces the TUI with a single status-bar line — one compact
segment per usage window, coloured by severity with polybar's own format tags.
It prints that line once and exits, or, with `--interval`, keeps running and
prints one every N seconds for a `tail = true` module:

```text
%{F#50fa7b}5h 11%%{F-} · %{T2}%{F#50fa7b}wk 39%%{F-}%{T-} · %{F#f1fa8c}Sonnet 62%%{F-}
```

which renders as `5h 11% · wk 39% · Sonnet 62%`, the percentages green/yellow/red
by the same severity rule as the gauges. Window names shrink to bar size: `5h`
for the session, `wk` for the all-models week, the model name for a per-model
week, `$` for spend. The **binding** window (the one you hit first — `◀` in the
TUI) is set in polybar's second font slot, which is conventionally the bold
face, so it stands out without extra width.

A `↑` after a percentage (`Sonnet 62%↑`) marks a window running **ahead of its
pace** — the fill past the TUI's `╋`, so at that rate it runs out before it
resets. It is not judged in the first tenth of a window, where any use at all
would read as far ahead.

An open incident on the status page puts a `⚠` **in front** of the line,
coloured by the incident's impact — critical colour for `major`/`critical`,
warning colour otherwise:

```text
%{F#ff5555}⚠%{F-} %{F#50fa7b}5h 11%%{F-} · …
```

so the bar reads `⚠ 5h 11% · wk 39% · Sonnet 62%`. The tail-mode stale marker
(below) goes *after* the line; the two say different things — "Anthropic has a
problem" versus "this line is old" — so they sit at opposite ends.

**Left-clicking the `⚠` opens that incident.** The marker is wrapped in
polybar's own action tag, carrying the status page's deep link to the incident
being shown:

```text
%{A1:xdg-open 'https\://stspg.io/9xz4hhmd1jzn':}%{F#ff5555}⚠%{F-}%{A} %{F#50fa7b}5h 11%%{F-} · …
```

Only the `⚠` is the target: the tag closes before the numbers, so a left-click
anywhere else on the module is not caught. It is on the left button because
that is the one a marker invites, which leaves the right free for a module-wide
`click-right` — worth keeping, since it is the only route to the page when
there is no incident and so no marker to click:

```ini
click-right = xdg-open https://status.claude.com/
```

`--opener CMD` chooses what runs (default `xdg-open`, the URL appended and
single-quoted); `--opener=` leaves the marker unclickable, as does
`--no-color`, since the tag is polybar's syntax and means nothing to waybar or
a shell prompt. An incident whose `shortlink` is missing — or is anything but a
plain `https://` URL of printable characters — falls back to
<https://status.claude.com/>: the deep link is lost, the click is not.

```ini
[module/claude]
type = custom/script
exec = claude-usage --polybar --interval 300
tail = true
```

| Option                   | Effect                                                     |
| ------------------------ | ---------------------------------------------------------- |
| `--interval N`           | tail mode: keep running, printing a line every N seconds (minimum 30), with backoff on failure |
| `--colors OK,WARN,CRIT`  | severity colours (default `#50fa7b,#f1fa8c,#ff5555`); an empty entry keeps that default |
| `--bold-font N`          | font slot for the binding window (default `2`); `0` leaves it unemphasised |
| `--no-color`             | plain text, no format tags (and so no click action)         |
| `--opener CMD`           | command the incident marker's left-click runs, the incident's URL appended (default `xdg-open`); empty for no click |

These five are only valid with `--polybar`; `--cache-ttl` and `--no-cache`
([Sharing between instances](#sharing-between-instances)) and `--no-incidents`
([Open incidents](#open-incidents)) apply to every mode.
Both `--opt V` and `--opt=V` are accepted.

With `--no-color` (and `--bold-font 0`) the line is plain
`5h 11% · wk 39% · Sonnet 62%`, which suits waybar, i3blocks, tmux or a shell
prompt just as well.

### Tail vs. polybar's interval

`--interval N` is the preferred shape, and the snippet above uses it: one
long-lived process prints a line every N seconds (minimum 30) for a
`tail = true` module. The usage API is rate limited, and tail mode is what puts
the cadence under the tool's control rather than polybar's:

- a failed fetch **doubles** the wait — 300s, 600s, 1200s, … capped at 30
  minutes — so a rate-limited endpoint is backed off instead of re-hit on a
  fixed schedule, and the wait resets on the first success;
- while refreshes fail, the **last good line stays on the bar** with a `⚠`
  marker in the warning colour (`5h 11% · wk 39% ⚠`) rather than the module
  turning into an error message;
- no process re-exec and TLS handshake per tick.

Dropping `--interval` (and `tail`) gives the plain one-shot module, where
polybar's own `interval = 300` drives the refresh — simpler, but it re-fires on
schedule whatever the API answered:

```ini
[module/claude]
type = custom/script
exec = claude-usage --polybar
interval = 300
```

In one-shot mode a failed fetch becomes the line itself — `claude: Access token
expired` in the critical colour, the message trimmed to its first clause so it
stays bar-sized — and the exit status is still 0, so polybar keeps rendering the
module rather than treating it as a broken script. In tail mode it shows as the
stale marker instead, once anything has been fetched at all. Regenerate the
example line with `cargo test polybar_line -- --nocapture`.

### Fonts and themed colours

The bold emphasis needs a bold `font-1` in the bar itself (the tag is `%{T2}`,
one more than the config key):

```ini
[bar/main]
font-0 = "JetBrainsMono Nerd Font:size=10;2"
font-1 = "JetBrainsMono Nerd Font:style=Bold:size=10;2"
```

If you theme `--colors` from your own `[colors]` section, note that polybar
substitutes a `${section.key}` reference only when it is the *whole* value: an
inline one in `exec` is passed through verbatim and the shell answers with
`Bad substitution`. Put the call in a wrapper script that sources your palette
instead:

```sh
#!/bin/sh
. $(dirname -- "$0")/colors.sh
exec claude-usage --polybar --interval 300 \
     --bold-font 5 --colors "$green,$yellow,$red"
```

## Sharing between instances

Instances share one fetch, so running several costs one API call. This matters
because the natural setup multiplies them: polybar starts a bar — and so a
module process — **per monitor**, and a monitor (`claude-usage` with no
arguments) opened beside the bars is one more, all polling a rate-limited
endpoint on the same schedule.

Every instance reads and writes `~/.cache/claude-usage/usage.json`. One that
finds no entry younger than its TTL takes a lock file, fetches, and publishes
the result for the others; an instance that finds the lock held reuses the last
published entry rather than making the same call beside it. The status-page
check is shared the same way, in a slot of its own (`incidents.json`, with its
own lock) on the same TTL, so the two fetches never wait on each other.

| Mode                       | Cache TTL                                        |
| -------------------------- | ------------------------------------------------ |
| `--polybar --interval N`   | just under N (90%), so the other monitors ride on one fetch |
| `--polybar` (one-shot)     | 240s, sized for the usual `interval = 300`        |
| TUI auto-refresh           | just under its own 5m/1m interval                 |
| TUI `r`, `-1`, `--json`    | none — fetches fresh, and publishes it for the others |

`--cache-ttl N` overrides the TTL and `--no-cache` opts out entirely (every
instance then fetches for itself). Both work in every mode.

The entry stores the raw API response plus the time it was fetched, so every
reader parses it identically and reports the true age of the data rather than
the moment it read it — the monitor's `updated` banner shows when the numbers
were fetched, by whichever instance fetched them. Failures are published too:
three processes re-trying a rate limit in parallel is precisely what the cache
is there to prevent. A lock left behind by a killed instance is broken after 30
seconds, and an unusable cache directory is not fatal — the instance simply
fetches for itself.


## How it works

On start (and on every `r`) the tool issues — unless another instance has
already published a fresh enough result, see
[Sharing between instances](#sharing-between-instances):

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

Alongside each usage fetch (unless `--no-incidents`) it reads the unresolved
incidents from Anthropic's status page — a Statuspage instance with a
documented, unauthenticated API:

```
GET https://status.claude.com/api/v2/incidents/unresolved.json
```

No token is sent there. Each incident's `name`, `impact` and `status` become
the marker described under [Open incidents](#open-incidents); a failed check
is "unknown" and shows nothing.

Your token is sent only to Anthropic's own API.

### Credential refresh

The access token in `~/.claude/.credentials.json` is short-lived (8 hours). On
each request the tool checks the stored `expiresAt`.

**Default (read-only).** If the token has expired the tool reports
`Access token expired — run Claude Code, or pass --write-back` and leaves the
file untouched. It never modifies the credentials Claude Code owns, so the two
can never race each other. In normal use this is invisible: as long as you have
used Claude Code within the last 8 hours the token is still valid.

**`--write-back` (opt-in).** With this flag an expired token is refreshed by
exchanging the `refreshToken` via

```
POST https://console.anthropic.com/v1/oauth/token
```

(using Claude Code's public OAuth client id), and the rotated credentials are
written back to the file atomically, preserving every other field — so the
monitor keeps working even when the Claude CLI is not running to refresh the
token. A refresh failure falls back to the stored token, so the underlying API
error is still surfaced rather than a confusing local one.

The one caveat with `--write-back`: the tool and Claude Code then both write the
same file, so if Anthropic *rotates* the refresh token, a simultaneous refresh
could be raced. Whether that rotation happens is account-specific — use
`--check-rotation` (below) to find out before relying on `--write-back`.

**`CLAUDE_CODE_OAUTH_TOKEN`.** When this environment variable is set (and not
blank), its value is the token, as it is for Claude Code itself — typically a
long-lived one from `claude setup-token`, for a machine with no credentials
file. The file is then not read at all: there is no `expiresAt` to check and
no refresh token, so `--write-back` has nothing to do, and an expired or
revoked token shows as the API's own error. Whether a `setup-token` token
carries the scope the usage endpoint asks for is the API's to decide; if it
does not, that is the error you will see.

### `--check-rotation`

A passive, **read-only** probe that answers one question: does Anthropic rotate
the OAuth refresh token when it is refreshed? It never calls the network and
never writes the credentials file. It stores a non-reversible fingerprint of the
refresh token (the token itself is never written) under
`~/.cache/claude-usage/rotation-check.json`.

```sh
claude-usage --check-rotation   # first run records a baseline
# ... wait until Claude Code refreshes (around the access token's expiry) ...
claude-usage --check-rotation   # reports the verdict
```

- *No rotation* — a refresh happened and the refresh token was unchanged, so the
  credential write-back above cannot race Claude Code; it is safe.
- *Rotation detected* — the refresh token changed, so the write-back can race
  Claude Code; prefer treating the credentials as read-only.

### `--json`

```json
{
  "fetched_at": "2026-06-19T18:51:36+02:00",
  "windows": [
    { "label": "Session (5h)", "group": "session", "percent": 11,
      "severity": "normal", "resets_at": "2026-06-19T20:30:00+00:00",
      "is_active": false, "pace": 87.7, "ahead_of_pace": false },
    { "label": "Week (all models)", "group": "weekly", "percent": 39,
      "severity": "normal", "resets_at": "2026-06-23T06:00:00+00:00",
      "is_active": true, "pace": 57.1, "ahead_of_pace": false }
  ],
  "incidents": [
    { "name": "Elevated errors for Claude Sonnet 5", "impact": "major",
      "status": "investigating", "url": "https://stspg.io/jt14jk41pjsf" }
  ]
}
```

`pace` is the percent an even rate of use would have reached by now (`null`
for a window not started or of unknown length), and `ahead_of_pace` is the
polybar `↑`: used past that pace, judged from a tenth of the window on.

`incidents` lists the status page's unresolved incidents, worst first: `[]`
when it is all clear, `null` when the page was not checked (`--no-incidents`)
or could not be reached — so a script can tell "all clear" from "unknown".

On a failed fetch the body is `{ "error": "...", "incidents": … }` and the exit
code is non-zero.

## Layout

```
src/
├── main.rs        argument parsing + dispatch (UI vs --json vs --check-rotation)
├── usage_api.rs   fetches & parses GET /api/oauth/usage; OAuth token refresh
├── incidents.rs   fetches & parses the status page's unresolved incidents
├── views.rs       UsageStatus -> pre-formatted gauge rows; severity and pace
├── tui.rs         ratatui monitor (gauges, refresh, key handling)
├── polybar.rs     --polybar: one-line status-bar rendering + the tail loop
├── cache.rs       cross-process cache: one fetch (usage, incidents) serves every instance
├── rotation.rs    --check-rotation: passive refresh-token rotation probe
└── json.rs        JSON form of the usage windows and the open incidents

docs/
├── design.md      why it is shaped so: decisions, rejected alternatives
└── usage-api.md   the reverse-engineered OAuth usage endpoints

scripts/
├── gitversion.sh  the git part of `make version-full` (+N.gHASH[.dirty])
└── checktag.sh    the release in Cargo.toml agrees with the v* tags

tests/
└── check-version.sh  `make check-version`: Cargo.toml, cargo and the tags agree

Makefile           cargo driver: check, build, tests, install, version, tag
CHECKLIST.md       what closes a round of work
```
