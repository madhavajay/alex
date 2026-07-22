# alex-fakeprov provider prefixes

Run `alex-fakeprov --port 0`; its startup JSON reports `base_url`. If that value is `http://127.0.0.1:4100`, use these Phase 2 upstream overrides:

| Provider | Override or configuration | Value |
| --- | --- | --- |
| Anthropic | `ALEX_UPSTREAM_ANTHROPIC_URL` | `http://127.0.0.1:4100/anthropic` |
| OpenAI and Codex | `ALEX_UPSTREAM_OPENAI_URL` | `http://127.0.0.1:4100/openai` |
| Gemini API and Code Assist | `ALEX_UPSTREAM_GEMINI_URL` | `http://127.0.0.1:4100/gemini` |
| Grok/xAI | `ALEX_UPSTREAM_XAI_URL` | `http://127.0.0.1:4100/xai` |
| Kimi | `ALEX_UPSTREAM_KIMI_URL` | `http://127.0.0.1:4100/kimi` |
| OpenRouter | `ALEX_UPSTREAM_OPENROUTER_URL` | `http://127.0.0.1:4100/openrouter` |
| Exo | Exo base URL | `http://127.0.0.1:4100/exo` |
| CLIProxyAPI | CLIProxyAPI API base | `http://127.0.0.1:4100/cliproxyapi/v1` |
| Amp | `ALEX_UPSTREAM_AMP_URL` | `http://127.0.0.1:4100/amp` |

The OpenAI prefix exposes API-key paths below `/openai/v1`, the original Codex path below `/openai/backend-api/codex`, and `/openai/responses` for an override that replaces the existing Codex base directly. Unprefixed Anthropic and OpenAI routes remain compatibility aliases.

The non-provider bases are `http://127.0.0.1:4100/github`, `http://127.0.0.1:4100/npm`, and `http://127.0.0.1:4100/telegram`. In particular, set `ALEX_UPDATE_MANIFEST_URL` to `http://127.0.0.1:4100/github/manifest.json`, `ALEX_UPDATE_RELEASES_URL` to `http://127.0.0.1:4100/github/releases`, and the Telegram API base to `http://127.0.0.1:4100/telegram`.
