# claude-usage

A Rust CLI showing Claude plan usage limits: a live TUI, a single-shot frame,
a polybar line, or JSON.

- **What it does, and every flag** — [README.md](README.md).
- **Why it is shaped so** — [docs/design.md](docs/design.md), including the
  rejected alternatives and the known shortfalls.
- **The endpoints it talks to** — [docs/usage-api.md](docs/usage-api.md)
  (reverse-engineered, unofficial).
- **Module map** — the Layout section of the README.

## Gate

```sh
cargo test          # the whole suite; run it before and after a change
cargo clippy --all-targets
```

Tests never touch the network: the cache tests drive the fetch through a fake,
everything else parses fixtures.

## Traps

- **Fixtures must carry relative timestamps.** Reset times pinned to a literal
  date silently rot: once they are in the past every row renders "resets now",
  which passes for months and then fails. See `tui::tests::sample`.
- **Testing tail mode by hand:** `--polybar --interval N` notices a closed
  stdout only at its next write, so piping it into `head` looks like a hang for
  up to N seconds. Read one line with `timeout` instead.
- **Running the binary hits the live API** and the public status page, and
  shares both results through `~/.cache/claude-usage/{usage,incidents}.json`;
  `--no-cache` bypasses that when you need to observe a real request, and
  `--no-incidents` skips the status page. Several `--no-cache` runs in a row
  earn a 429.
- **Seeing the incident marker when nothing is wrong:** point
  `XDG_CACHE_HOME` at a scratch directory holding a hand-written
  `claude-usage/incidents.json` (`{"fetched_at": <ms>, "body": "<reply>"}`)
  with a fresh stamp, and run with `--cache-ttl 600`. The real cache is then
  untouched.
