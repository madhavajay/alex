# Credentials — feature plan

Target: make the vault a first-class, multi-account, budgeted credential manager with full UI.

## 1. Credentials view in the Mac app (CRUD)
- New "Credentials" tab in Preferences (or window): lists every vault entry — provider icon,
  name, kind (oauth / api-key / run-key / harness-key), status (active / paused / expired),
  last refresh, expiry countdown, last used, fingerprint.
- Create (import token / paste key / start login flow), edit (name, description, settings),
  delete (with plan+approve modal like harness actions), re-auth button on expired oauth.

## 2. Identity: name + description
- `name`: short slug (`[a-z0-9-_]+`), unique per vault, e.g. `codex-work`.
- `description`: free text ("work ChatGPT account, expires with contract in Sept").
- Names become the stable reference everywhere: config, CLI flags, UI, traces.

## 3. Controls per credential
- **Pause/resume** (disable temporarily; paused = never selected, shown greyed).
- **Budgets**: optional token and/or $ ceilings with a window (per-day / per-week / per-month /
  lifetime). Default: unlimited. At threshold: auto-pause + notification (and failover to the
  next account per policy). Show utilization bar in the list.
- **Model allow-list**: per credential, models on/off; default all models on.
- Special kinds keep their flows: run-keys (mint/list/revoke, TTL), harness keys (never-expire,
  tagged) — minting UI lives here too.

## 4. Multiple accounts per subscription (PRIORITY #1 — codex first)
- Vault supports N accounts per provider: `accounts/openai-oauth-<name>.json` (name inside the
  file too; legacy unsuffixed files auto-migrate to name `default`).
- `alex auth login codex --name work`, `alex auth import --name personal`, `alex auth list`.
- Selection policy per provider in config:
  - `priority` (ordered list; first healthy+unpaused+under-budget wins)
  - `round_robin` (rotate per request)
  - `threshold` (use #1 until its rate-limit window utilization ≥ X%, then #2, …; falls back
    to #1 when its window resets)
- Automatic failover on 429/quota/auth errors: mark account cooling-down, try next per policy.
- Traces record which account served each request (already have account_id — surface it).

## 5. OpenRouter provider
- New provider kind `openrouter`: one or more API tokens (named, like all credentials).
- Per-token model allow-list (which OpenRouter models get exposed via /v1/models and routing).
- Routes: `openrouter/<model>` prefix + optional bare-model routing for allow-listed models.
- Uses the generic openai-compatible upstream (foundation for routes.toml later).

## 6. Copy & reveal UX
- Every credential row: copy-to-clipboard button (copies the export block or bare key).
- Reveal toggle: values masked by default, shown in selectable text areas when revealed.
- "Copy env exports" per credential and per provider (same shape as `alex env`).

## 7. Extras (proposed)
- **Usage stats per credential**: requests/tokens/$ (7d), sparkline, "view traces" jump
  (trace search by key fingerprint already exists).
- **Audit log**: minted/revoked/paused/budget-hit/refresh-failed events with timestamps.
- **Health per account**: last heartbeat result per account (not just per provider).
- **Encrypted export/import** of the vault — groundwork for peer credential sync.
- **Budget alerts** into the menu bar (approaching threshold, auto-paused).
- **Scoping**: restrict a credential to specific harnesses/tags (run-key style but persistent).

## Build order
1. Multi-account vault + selection policies + failover (codex, now) — CLI + daemon only.
2. Credentials tab: list + pause + name/description + copy/reveal (CRUD on top of 1).
3. Budgets + model allow-lists (enforcement in proxy, editing in UI).
4. OpenRouter provider.
5. Extras (stats, audit, health, export).
