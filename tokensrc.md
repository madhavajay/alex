# Subscription Login & Token / API Auth in `pi`

How subscription-based OAuth login, token storage, refresh, and per-request
authentication work across the `pi` monorepo. This covers the three built-in
subscription providers (Anthropic Claude Pro/Max, ChatGPT/OpenAI Codex, GitHub
Copilot), plus the generic API-key path and how custom providers plug in.

## Where the code lives

| Concern | File |
|---|---|
| OAuth provider interface & shared types | `packages/ai/src/utils/oauth/types.ts` |
| Provider registry + high-level `getOAuthApiKey` | `packages/ai/src/utils/oauth/index.ts` |
| Anthropic (Claude Pro/Max) flow | `packages/ai/src/utils/oauth/anthropic.ts` |
| OpenAI Codex (ChatGPT) flow | `packages/ai/src/utils/oauth/openai-codex.ts` |
| GitHub Copilot flow | `packages/ai/src/utils/oauth/github-copilot.ts` |
| PKCE helper | `packages/ai/src/utils/oauth/pkce.ts` |
| Device-code polling loop (RFC 8628) | `packages/ai/src/utils/oauth/device-code.ts` |
| Browser callback success/error HTML | `packages/ai/src/utils/oauth/oauth-page.ts` |
| Credential storage (`auth.json`, file locking) | `packages/coding-agent/src/core/auth-storage.ts` |
| Model registry auth resolution | `packages/coding-agent/src/core/model-registry.ts` |
| `/login` `/logout` UI wiring | `packages/coding-agent/src/modes/interactive/interactive-mode.ts` |
| Login dialog TUI component | `.../interactive/components/login-dialog.ts` |
| Provider selector TUI component | `.../interactive/components/oauth-selector.ts` |
| Anthropic request auth/headers | `packages/ai/src/providers/anthropic.ts` |
| Codex request auth/headers | `packages/ai/src/providers/openai-codex-responses.ts` |
| Config paths (`~/.pi/agent/auth.json`) | `packages/coding-agent/src/config.ts` |

## Core abstraction: `OAuthProviderInterface`

Every provider implements one interface (`utils/oauth/types.ts`):

```ts
interface OAuthProviderInterface {
  readonly id: string;
  readonly name: string;
  usesCallbackServer?: boolean;                       // supports local redirect server + manual paste
  login(callbacks: OAuthLoginCallbacks): Promise<OAuthCredentials>;
  refreshToken(credentials: OAuthCredentials): Promise<OAuthCredentials>;
  getApiKey(credentials: OAuthCredentials): string;   // credentials -> bearer string used in requests
  modifyModels?(models, credentials): Model[];        // e.g. rewrite baseUrl (Copilot)
}
```

Credentials are a loose bag persisted verbatim to disk:

```ts
type OAuthCredentials = {
  refresh: string;
  access: string;
  expires: number;        // epoch ms; when the access token is considered dead
  [key: string]: unknown; // providers stash extras here (accountId, enterpriseUrl, ...)
};
```

The UI talks to a provider through a callback bag (`OAuthLoginCallbacks`) so the
flow logic is decoupled from the TUI: `onAuth` (show URL), `onDeviceCode` (show
user code), `onPrompt` (ask for pasted input), `onManualCodeInput` (a promise
that resolves when the user pastes a redirect URL, raced against the callback
server), `onSelect` (choose a sub-method), `onProgress`, and an `AbortSignal`.

### Provider registry

`utils/oauth/index.ts` holds a `Map<id, provider>` seeded with the three
built-ins (`anthropicOAuthProvider`, `githubCopilotOAuthProvider`,
`openaiCodexOAuthProvider`). Custom providers register via
`registerOAuthProvider()`; `resetOAuthProviders()` restores built-ins.
`getOAuthProvider(id)` / `getOAuthProviders()` are the lookups used everywhere.

`getOAuthApiKey(providerId, credsById)` is the high-level helper: it returns the
current access token, transparently refreshing first if `Date.now() >= expires`.

## The three login flows

All three ultimately produce an `OAuthCredentials` object that gets stored as
`{ type: "oauth", ...credentials }` under the provider id in `auth.json`.

### 1. Anthropic — Claude Pro/Max (authorization code + PKCE + loopback server)

`anthropic.ts`. Constants:
- `CLIENT_ID` — base64-obfuscated in source (`atob(...)`), a fixed public client id.
- `AUTHORIZE_URL` = `https://claude.ai/oauth/authorize`
- `TOKEN_URL` = `https://platform.claude.com/v1/oauth/token`
- Loopback redirect: `http://localhost:53692/callback` (host overridable via
  `PI_OAUTH_CALLBACK_HOST`, default `127.0.0.1`).
- Scopes: `org:create_api_key user:profile user:inference
  user:sessions:claude_code user:mcp_servers user:file_upload`.

Flow (`loginAnthropic`):
1. `generatePKCE()` → `{ verifier, challenge }` (SHA-256, base64url; `pkce.ts`).
   Note: the **verifier is reused as the OAuth `state`**.
2. Start a Node `http.createServer` on port 53692 that waits for `/callback`,
   validates `state === verifier`, and resolves with `{ code, state }`. Renders
   success/error HTML from `oauth-page.ts`.
3. `onAuth` fires with the authorize URL (`response_type=code`,
   `code_challenge_method=S256`, `state=verifier`). The TUI opens the browser
   and simultaneously shows a paste box.
4. The browser callback **races** the manual-paste promise
   (`onManualCodeInput`) — whichever resolves first wins. If neither yields a
   code, it falls back to `onPrompt`. Pasted input can be a bare code,
   `code#state`, a `code=...` query string, or a full redirect URL
   (`parseAuthorizationInput`).
5. `exchangeAuthorizationCode` POSTs JSON to `TOKEN_URL` with
   `grant_type=authorization_code`, `client_id`, `code`, `state`, `redirect_uri`,
   `code_verifier`.
6. Returns `{ refresh, access, expires: now + expires_in*1000 - 5min }`.
   **A 5-minute safety margin is subtracted** so refresh happens early.

Refresh (`refreshAnthropicToken`): POST `grant_type=refresh_token` +
`client_id` + `refresh_token`; same 5-minute early-expiry margin. Returns a new
refresh token too (rotating).

### 2. OpenAI Codex — ChatGPT Plus/Pro (two sub-methods)

`openai-codex.ts`. `onSelect` offers **browser login** (default) or **device
code** (headless). Constants: `CLIENT_ID = app_EMoamEEZ73f0CkXaXp7hrann`,
`AUTH_BASE_URL = https://auth.openai.com`, loopback redirect
`http://localhost:1455/auth/callback`, scope `openid profile email
offline_access`.

**Browser sub-flow** (`loginOpenAICodex`): PKCE + a random 16-byte hex `state`
(distinct from the verifier here, unlike Anthropic). Authorize URL adds
`id_token_add_organizations=true`, `codex_cli_simplified_flow=true`, and an
`originator` (default `pi`). Local server on 1455 validates `state`, resolves the
code; same browser-vs-manual-paste race. Exchanges via
`grant_type=authorization_code` (`application/x-www-form-urlencoded`).

**Device-code sub-flow** (`loginOpenAICodexDeviceCode`): POST to
`/api/accounts/deviceauth/usercode` → `{ device_auth_id, user_code, interval }`.
Show `user_code` + `https://auth.openai.com/codex/device`. Poll
`/api/accounts/deviceauth/token` until it returns `{ authorization_code,
code_verifier }`, then exchange those against the device redirect URI. 15-minute
timeout.

**Account id:** the access token is a JWT; `getAccountId` decodes the
`https://api.openai.com/auth` claim to pull `chatgpt_account_id`, which is stored
on the credentials as `accountId` and **required** — login throws if absent.

Refresh (`refreshOpenAICodexToken`): `grant_type=refresh_token`; re-derives
`accountId` from the new token. Note: no 5-minute margin here — `expires` is the
raw `now + expires_in*1000`.

### 3. GitHub Copilot — device code, then token exchange

`github-copilot.ts`. Base64-obfuscated `CLIENT_ID`. Two-token model:

1. **Device flow** on the GitHub OAuth endpoint
   (`https://github.com/login/device/code`, scope `read:user`). Show `user_code`
   + `verification_uri`; poll `.../login/oauth/access_token` with
   `grant_type=urn:ietf:params:oauth:grant-type:device_code` until a GitHub
   OAuth access token comes back. `verification_uri` is validated to be http(s).
2. **Copilot token exchange** (`refreshGitHubCopilotToken`): call
   `https://api.github.com/copilot_internal/v2/token` with
   `Authorization: Bearer <github-oauth-token>` and Copilot editor headers
   (`User-Agent: GitHubCopilotChat/...`, `Editor-Version`, `Copilot-Integration-Id`).
   Response `{ token, expires_at }` becomes the short-lived Copilot API token.

Here the persisted `refresh` field is the **long-lived GitHub OAuth token**, and
`access` is the **short-lived Copilot token** (minus a 5-minute margin). So
"refresh" just re-runs the exchange with the stored GitHub token — that's why
`refreshToken` reuses `refresh` unchanged.

**Enterprise:** the first prompt asks for a GHE domain; if given, all URLs are
rederived per-domain and `enterpriseUrl` is stored on the credential.

**Base URL from token:** Copilot tokens embed `proxy-ep=...`;
`getGitHubCopilotBaseUrl` rewrites `proxy.*` → `api.*` to find the API host.
`modifyModels` uses this to set each Copilot model's `baseUrl` at registry-build
time.

**Model enablement:** after login, `enableAllGitHubCopilotModels` POSTs
`{ state: "enabled" }` to `/models/<id>/policy` for every Copilot model (some,
e.g. Claude/Grok, require policy acceptance before use).

### Device-code polling loop (`device-code.ts`)

Shared by Codex device-code and Copilot. Implements RFC 8628: honors the server
`interval` (min 1s, default 5s), handles `slow_down` by adding 5s to the
interval, respects `expires_in` as a hard deadline, and is abortable via
`AbortSignal`. Distinct timeout message for `slow_down`-induced timeouts (hints
at VM/WSL clock drift).

## Credential storage — `auth.json`

`auth-storage.ts`. Stored at `getAuthPath()` = `~/.pi/agent/auth.json`
(directory overridable via the agent-dir env var; see `config.ts`). File is
created `0o600`, parent dir `0o700`.

Shape — a map of provider id → credential, each tagged with a `type`:

```jsonc
{
  "anthropic":      { "type": "oauth", "refresh": "...", "access": "sk-ant-oat...", "expires": 1730000000000 },
  "openai-codex":   { "type": "oauth", "refresh": "...", "access": "...", "expires": ..., "accountId": "..." },
  "github-copilot": { "type": "oauth", "refresh": "<gh-oauth>", "access": "<copilot>", "expires": ..., "enterpriseUrl": "..." },
  "openai":         { "type": "api_key", "key": "sk-..." }
}
```

**Backends:** `FileAuthStorageBackend` (default) and `InMemoryAuthStorageBackend`
(tests). Both expose `withLock` / `withLockAsync`.

**File locking:** uses `proper-lockfile` so multiple concurrent `pi` processes
don't clobber each other during a token refresh. Sync path retries on `ELOCKED`
(10× / 20ms); async path uses exponential backoff with a 30s stale timeout and
an `onCompromised` guard that aborts the write if the lock was lost mid-op.

**`AuthStorage` API:** `get/set/remove/list/has`, `hasAuth` (any auth source,
no refresh), `getAuthStatus` (source without exposing stuff), `login`,
`logout`, and the important `getApiKey`.

### `getApiKey(providerId)` resolution order

1. **Runtime override** — CLI `--api-key` (`setRuntimeApiKey`, never persisted).
2. **`api_key` credential** from `auth.json` (`resolveConfigValue`, so it can be
   an env-ref or command).
3. **`oauth` credential** from `auth.json` — if `Date.now() >= expires`, refresh
   under a file lock (`refreshOAuthTokenWithLock`); else return the current
   access token via the provider's `getApiKey`.
4. **Environment variable** for the provider.
5. **Fallback resolver** (custom-provider keys from `models.json`).

**Locked refresh (`refreshOAuthTokenWithLock`):** re-reads `auth.json` inside the
lock (another process may have already refreshed), rechecks `expires`, calls
`getOAuthApiKey`, and writes the merged rotated credentials back atomically. If
refresh throws, it reloads and checks whether a sibling process refreshed
successfully; otherwise returns `undefined` (model discovery then skips that
provider; the user re-runs `/login`). Credentials are **not** deleted on a failed
refresh, so a retry is possible.

## `/login` and `/logout` (interactive TUI)

`interactive-mode.ts`. `/login` → `showLoginAuthTypeSelector` → provider list
(`OAuthSelectorComponent`), split into OAuth vs API-key. Selecting an OAuth
provider calls `showLoginDialog`, which:

1. Builds a `LoginDialogComponent` and swaps it into the editor container.
2. Calls `authStorage.login(providerId, callbacks)`, wiring each provider
   callback to a dialog method:
   - `onAuth` → `dialog.showAuth(url)` (renders clickable OSC-8 hyperlink, opens
     browser) and, for callback-server providers, `showManualInput` to race a
     pasted redirect URL against the loopback callback.
   - `onDeviceCode` → `dialog.showDeviceCode` + "Waiting for authentication…".
   - `onPrompt` → `dialog.showPrompt`; `onSelect` → a sub-selector (Codex
     browser vs device); `onProgress` → status line.
   - `signal` = the dialog's `AbortController` (Esc cancels the whole flow).
3. On success, `AuthStorage.login` persists `{ type: "oauth", ...creds }`, then
   `completeProviderAuthentication` refreshes the model registry, auto-selects
   the provider's default model if the current model is unknown, and shows where
   credentials were saved.

`/logout` (`showOAuthSelector("logout")`) lists only providers with stored
credentials and calls `authStorage.logout(id)` (removes the `auth.json` entry).
It explicitly does **not** touch env vars or `models.json` config.

## Applying tokens to actual API requests

The model registry resolves auth per request via
`getApiKeyAndHeaders(model)` (`model-registry.ts`), which pulls the key from
`AuthStorage.getApiKey` (auto-refreshing OAuth) and merges provider/model
headers. `isUsingOAuth(model)` is a fast check (`credential.type === "oauth"`)
used to decide OAuth-specific request shaping.

### Anthropic requests (`providers/anthropic.ts`)

The provider branches on how the key looks / where it came from:

- **OAuth token** — detected by `isOAuthToken(apiKey)` = key contains
  `sk-ant-oat`. Sent as **Bearer** (`authToken`, not `x-api-key`) with Claude
  Code identity headers:
  - `anthropic-beta: claude-code-20250219,oauth-2025-04-20,<other betas>`
  - `user-agent: claude-cli/<version>`, `x-app: cli`
  - The system prompt is **forced** to begin with `"You are Claude Code,
    Anthropic's official CLI for Claude."` and tool names are rewritten to Claude
    Code's canonical names (`toClaudeCodeName`) — the subscription endpoint
    expects the Claude Code client identity.
- **GitHub Copilot provider** — Bearer `authToken`, Copilot base URL, selective
  betas.
- **Cloudflare AI Gateway** — key goes in `cf-aig-authorization: Bearer`, with
  `x-api-key`/`Authorization` nulled.
- **Plain API key** — normal `x-api-key` auth, optional `x-session-affinity`.

### OpenAI Codex requests (`providers/openai-codex-responses.ts`)

`buildBaseCodexHeaders` sets `Authorization: Bearer <access>`,
`chatgpt-account-id: <accountId>` (extracted from the JWT), `originator: pi`, and
a `pi (<os>)` user-agent. SSE requests add `OpenAI-Beta: responses=experimental`
and echo the session id as `session-id` / `x-client-request-id`.

## End-to-end summary

1. `/login` → pick provider → provider-specific OAuth flow (loopback+PKCE,
   device code, or GitHub device→exchange) driven through TUI callbacks.
2. Resulting `OAuthCredentials` saved as `{ type: "oauth", ... }` in
   `~/.pi/agent/auth.json` (0600, file-locked).
3. Each request: model registry → `AuthStorage.getApiKey` → if `expires` passed,
   locked refresh via the provider's `refreshToken` (rotating tokens written
   back atomically) → provider builds request auth (Bearer + provider-specific
   identity headers).
4. Anthropic OAuth additionally spoofs the Claude Code client identity (headers,
   system-prompt prefix, tool-name casing); Codex attaches the `chatgpt-account-id`
   from the token JWT; Copilot derives its API base URL from the token and
   pre-enables gated models.
