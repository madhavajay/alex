# Tool-call transcript fixtures

These fixtures are reduced, secret-free bodies from the local cove evaluation
captures. Only fields needed to preserve each wire dialect and its conversation
were retained; request headers were excluded entirely.

- `anthropic_*`: Claude Code capture, reconstructed from Anthropic Messages SSE.
- `openai_responses_*`: Codex capture, reconstructed from Responses API events.
- `openai_chat_*`: native OpenAI Chat Completions capture. The available Kimi
  Code run routed upstream through OpenAI Responses, so it had no matching
  native Chat response; this pair comes from a native Grok/OpenAI Chat run.
  The JSON response was reconstructed from the real stream; the SSE fixture
  retains real chunks and adds an OpenRouter-style comment so comment-prefixed
  streams stay covered.
- `gemini_request.json`: real Gemini CLI `generateContent` request, reduced to
  the relevant user content and `update_topic` declaration. No native Gemini
  upstream response existed in the available cove exports (the run routed to
  OpenAI Responses), so `gemini_response.json` faithfully translates that real
  function call and adds the assistant text required by this regression.

No credentials, authorization headers, cookies, or API keys are present. Any
such value must be replaced with `<REDACTED>` before updating these fixtures.
