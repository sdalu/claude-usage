# Anthropic OAuth usage API (reverse-engineered)

Notes on the undocumented `api.anthropic.com` OAuth endpoints that Claude Code's
`/status` and this tool use. Reverse-engineered from the Claude Code CLI binary
(`strings`) and confirmed by live probing with a personal account. **Unofficial
and unstable** — Anthropic can change or remove any of this without notice.

## Authentication

All requests use the Claude Code OAuth access token stored locally:

```
~/.claude/.credentials.json  →  .claudeAiOauth.accessToken
```

Required headers:

| Header | Value |
| ------ | ----- |
| `Authorization` | `Bearer <accessToken>` |
| `anthropic-beta` | `oauth-2025-04-20` |
| `Content-Type` | `application/json` |
| `User-Agent` | any (e.g. `claude-cli`) |

The token also carries `refreshToken`, `expiresAt`, `subscriptionType`, and
`rateLimitTier`, but the last two can be stale — prefer `/api/oauth/profile`
(below) for the real plan.

## Errors & rate limiting

Errors come back as:

```json
{ "error": { "type": "rate_limit_error", "message": "Rate limited. Please try again later." } }
```

The usage endpoint is **rate limited**; poll gently (this tool defaults to a
5-minute auto-refresh). A non-2xx status carries the same `error` body.

---

## `GET /api/oauth/usage` — usage windows

The only real *usage* endpoint (drives `/status` and this tool). Returns the
current utilization of each limit window. It does **not** expose a historical
per-day / per-model token breakdown — only live percentages.

```jsonc
{
  // Named windows; utilization is a percent 0..=100. *_dollars are null on
  // subscription plans, populated on credit/usage-based plans.
  "five_hour":  { "utilization": 11.0, "resets_at": "2026-…+00:00",
                  "limit_dollars": null, "used_dollars": null, "remaining_dollars": null },
  "seven_day":  { "utilization": 39.0, "resets_at": "…", "limit_dollars": null, … },

  // Per-model weekly windows (null when not applicable to the plan).
  "seven_day_opus":   null,
  "seven_day_sonnet": { "utilization": 1.0, "resets_at": "…" },
  // Other codenamed buckets, usually null:
  "seven_day_oauth_apps": null, "seven_day_cowork": null, "seven_day_omelette": null,
  "tangelo": null, "iguana_necktie": null, "omelette_promotional": null,
  "cinder_cove": null, "amber_ladder": null,

  // Overage / credit pool.
  "extra_usage": {
    "is_enabled": false, "monthly_limit": null, "used_credits": null,
    "utilization": null, "currency": null, "decimal_places": null,
    "disabled_reason": null, "daily": null, "weekly": null
  },

  // The clean, forward-compatible list of every active window. THIS is what
  // the tool parses.
  "limits": [
    { "kind": "session",       "group": "session", "percent": 11, "severity": "normal",
      "resets_at": "…", "scope": null, "is_active": false },
    { "kind": "weekly_all",    "group": "weekly",  "percent": 39, "severity": "normal",
      "resets_at": "…", "scope": null, "is_active": true },
    { "kind": "weekly_scoped", "group": "weekly",  "percent": 1,  "severity": "normal",
      "resets_at": "…",
      "scope": { "model": { "id": null, "display_name": "Sonnet" }, "surface": null },
      "is_active": false }
  ],

  // Real-currency spend (disabled on subscription plans).
  "spend": {
    "used": { "amount_minor": 0, "currency": "USD", "exponent": 2 },
    "limit": null, "percent": 0, "severity": "normal",
    "enabled": false, "disabled_reason": null
  }
}
```

Field notes:

- `limits[].kind`: `session`, `weekly_all`, `weekly_scoped` (others possible).
- `limits[].severity`: `normal`, `warning`, `critical` (used for colour).
- `limits[].is_active`: the **binding** window — the one you hit first.
- `limits[].scope.model.display_name`: the model a scoped window applies to.
- `resets_at`: RFC 3339 timestamp.
- The matching dollar figures live on the top-level window object
  (`five_hour`, `seven_day`, `seven_day_<model>`), keyed off `kind`/scope.

The headers `anthropic-ratelimit-unified-5h-utilization` /
`-7d-utilization` / `-…-reset` on a normal model API call carry the same
numbers, if you'd rather read them from a request you already make.

---

## `GET /api/oauth/profile` — account, org & plan

Identity and the authoritative plan/rate-limit tier.

```jsonc
{
  "account": {
    "uuid": "…", "full_name": "…", "display_name": "…", "email": "…",
    "has_claude_max": true, "has_claude_pro": false,
    "created_at": "2026-…Z"
  },
  "organization": {
    "uuid": "…", "name": "…",
    "organization_type": "claude_max",            // claude_pro | claude_max | team | enterprise
    "billing_type": "stripe_subscription",
    "rate_limit_tier": "default_claude_max_5x",   // the real tier
    "seat_tier": null,
    "has_extra_usage_enabled": false,
    "subscription_status": "active",
    "subscription_created_at": "2026-…Z",
    "cc_onboarding_flags": {},
    "claude_code_trial_ends_at": null,
    "claude_code_trial_duration_days": null,
    "payment_auth_hosted_invoice_url": null
  },
  "application": { "uuid": "…", "name": "…", "slug": "claude-code" },
  "enabled_plugins": []
}
```

---

## Other read-only endpoints (config, not usage)

| Endpoint | Returns |
| -------- | ------- |
| `GET /api/organization/claude_code_first_token_date` | `{ "first_token_date": "2026-…Z" }` |
| `GET /api/oauth/account/settings` | onboarding flags, dismissed banners, pinned menu items |
| `GET /api/claude_code/policy_limits` | `{ "restrictions": { … }, "compliance_taints": [] }` (feature flags) |
| `GET /api/oauth/validate` | token validity (not probed) |
| `GET /api/oauth/claude_cli/roles` | user roles (not probed) |
| `GET /api/oauth/organizations/` | organization list (not probed) |
| `GET /api/claude_cli_profile`, `/api/claude_code/{settings,skills,memory,notification/preferences}` | CLI/feature config (not probed) |

### Mutating endpoints (do not call to "read")

`POST /api/oauth/claude_cli/create_api_key`, `POST /api/oauth/file_upload`,
`/api/oauth/account/grove_notice_viewed`, …

---

## What this tool uses

Only `GET /api/oauth/usage` → `limits[]` (+ per-window dollars and `spend` when
present). Everything shown in the UI comes from that single call; the OAuth
token is the sole local dependency.
