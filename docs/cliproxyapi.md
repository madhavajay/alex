# CLIProxyAPI integration

Alex supports both arrangements without making either proxy a hidden default:

```text
Harness → Alex → CLIProxyAPI → provider
Harness → CLIProxyAPI → Alex → provider
```

The first direction is configured with `alex connect cliproxyapi --url ...
--key ...`. Alex probes CLIProxyAPI's authenticated `/v1/models`, stores the
credential in its vault, publishes safe models as `cliproxyapi/<model>`, and
routes OpenAI Chat, OpenAI Responses, and Anthropic Messages requests through
the OpenAI-compatible upstream.

## Generate the reverse provider

CLIProxyAPI v7 can treat Alex as an `openai-compatibility` provider. Generate a
private fragment instead of copying Alex's local/admin key into a hand-written
file:

```bash
alex daemon --background
alex cliproxyapi capabilities
alex cliproxyapi export \
  --output ./alex-provider.yaml \
  --cliproxyapi-version v7.4.1
```

The command:

- refuses non-loopback HTTP URLs, URL credentials, queries, and fragments;
- probes `GET /v1/alex/capabilities` and checks the integration schema;
- requires CLIProxyAPI major v7 or newer when a version is supplied;
- mints a non-expiring `harness` key labelled `cliproxyapi` (not an Alex
  local/admin key);
- exports only safe `alex/*` catalog entries and filters CLIProxyAPI routes that
  could recurse;
- creates a new file with mode `0600` on Unix and never overwrites an existing
  file; and
- prints only the path, model count, schema, and key ID—not the credential.

The output is a config fragment. Merge its `passthrough-headers: true` and
`openai-compatibility` entry into CLIProxyAPI's existing `config.yaml`, then
reload or restart CLIProxyAPI. The file contains the scoped key and remains
secret material. `--model gpt-5 --model alex/claude-opus-4-8` restricts the
exported catalog.

For a remote Alex, pass an existing scoped key through a private file:

```bash
alex cliproxyapi export \
  --url https://alex.example.invalid/v1 \
  --key-file ./alex-harness.key \
  --output ./alex-provider.yaml
```

`ALEXANDRIA_HARNESS_KEY` is the non-file alternative. Alex deliberately does
not ask for or accept a remote daemon's local/admin key in this workflow.

## Version and capability negotiation

The reverse schema is `alex.cliproxyapi.reverse/v1`. The generated fragment
targets CLIProxyAPI major v7+ and uses these v7 config capabilities:

- `openai-compatibility` with `prefix`, `headers`, `api-key-entries`, and
  explicit `models`;
- global `passthrough-headers` so `x-alexandria-trace-id` reaches the original
  client on successful responses; and
- OpenAI-compatible translation for Chat, Responses, Anthropic Messages,
  streaming, and tool calls.

Alex advertises the live contract at `GET /v1/alex/capabilities`. Export stops
if the returned schema differs or the supplied CLIProxyAPI major is too old.
`alex cliproxyapi capabilities --json` shows the local binary's expected
contract without contacting a daemon. At runtime, generated configurations
declare their schema and capabilities on every request. Alex returns HTTP 426
before routing when an explicit schema is incompatible or the request's
protocol capability is absent. Older configurations that send no negotiation
headers remain accepted for compatibility.

## Correlation and loop prevention

The generated provider adds only non-secret metadata headers:

- `x-alexandria-harness: cliproxyapi` and the CLIProxyAPI version for trace
  provenance;
- `x-alexandria-integration-schema` and `x-alexandria-capabilities` for the
  negotiated contract; and
- `x-alexandria-route-chain: cliproxyapi` for loop detection.

Alex records the route chain in trace tags and returns
`x-alexandria-trace-id`. When Alex calls CLIProxyAPI in the other direction it
adds its parent trace ID and an `alex` route-chain hop. A request marked as
originating from CLIProxyAPI that tries to select a `cliproxyapi/*` model is
rejected with HTTP 508 before any upstream connection is made.

Provider 401, 429, and 5xx statuses and structured JSON bodies are preserved
through the reverse path. Current CLIProxyAPI v7 OpenAI-compatible execution
does not attach upstream response headers to non-2xx errors, so `Retry-After`
and `x-alexandria-trace-id` do not survive that second hop on error responses.
This is a CLIProxyAPI boundary limitation; Alex still emits both headers.
WebSocket Responses, Gemini ingress, and image endpoints are not part of the
reverse v1 contract.
