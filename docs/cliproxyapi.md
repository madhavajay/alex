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

`ALEX_HARNESS_KEY` is the non-file alternative. Alex deliberately does
not ask for or accept a remote daemon's local/admin key in this workflow.

## Version and capability negotiation

The reverse schema is `alex.cliproxyapi.reverse/v1`. The generated fragment
targets CLIProxyAPI major v7+ and uses these v7 config capabilities:

- `openai-compatibility` with `prefix`, `headers`, `api-key-entries`, and
  explicit `models`;
- global `passthrough-headers` so `x-alex-trace-id` reaches the original
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

- `x-alex-harness: cliproxyapi` and the CLIProxyAPI version for trace
  provenance;
- `x-alex-integration-schema` and `x-alex-capabilities` for the
  negotiated contract; and
- `x-alex-route-chain: cliproxyapi` for loop detection.

Alex records the route chain in trace tags and returns
`x-alex-trace-id`. When Alex calls CLIProxyAPI in the other direction it
adds its parent trace ID and an `alex` route-chain hop. A request marked as
originating from CLIProxyAPI that tries to select a `cliproxyapi/*` model is
rejected with HTTP 508 before any upstream connection is made.

Provider 401, 429, and 5xx statuses and structured JSON bodies are preserved
through the reverse path. Current CLIProxyAPI v7 OpenAI-compatible execution
does not attach upstream response headers to non-2xx errors, so `Retry-After`
and `x-alex-trace-id` do not survive that second hop on error responses.
This is a CLIProxyAPI boundary limitation; Alex still emits both headers.
WebSocket Responses, Gemini ingress, and image endpoints are not part of the
reverse v1 contract.

## Pinned Docker compatibility matrix

The reproducible local gate uses the multi-architecture CLIProxyAPI image
`eceasy/cli-proxy-api:v7.2.92` pinned to manifest digest
`sha256:af18f6fb364bfb7b482a1ca6c6c85fd7df2c0d6a3a497ebb82c337ac2216dc41`.
It was verified on 2026-07-21. Run it directly or through the test driver:

```bash
./scripts/cliproxyapi-v1-integration.sh
./test.sh cliproxyapi
```

The fixture starts the real pinned CLIProxyAPI container, a real Alex router,
and a deterministic OpenAI-compatible provider. It never reads the user's Alex
config or vault. Host and published ports are loopback-only; one-time
credentials live in a mode-`0700` temporary directory; curl reads credentials
from mode-`0600` header files; CLIProxyAPI request logging and the Docker log
driver are disabled. Set `ALEX_CPA_FIXTURE_KEEP=1` only when local debugging
requires retaining those private artifacts.

| CLIProxyAPI version | Pinned image digest | Published platforms | Local gate |
|---|---|---|---|
| v7.2.92 | `sha256:af18f6fb364bfb7b482a1ca6c6c85fd7df2c0d6a3a497ebb82c337ac2216dc41` | `linux/amd64`, `linux/arm64` | Pass in both arrangements |

| Arrangement | Chat | Responses | Anthropic Messages | Streaming tool call | 401 / 429 / 503 | Correlation | Loop result |
|---|---|---|---|---|---|---|---|
| Harness → Alex → CLIProxyAPI → provider | Pass | Pass | Pass | Pass (`shell({"command":"pwd"})`) | Status and JSON body pass | Alex trace header reaches harness | Explicit reverse loop is HTTP 508 |
| Harness → CLIProxyAPI → Alex → provider | Pass | Pass | Pass | Pass (`shell({"command":"pwd"})`) | Status and JSON body pass | Alex trace header reaches harness on success | Explicit reverse loop is HTTP 508 |

The gate also verifies bad harness/API keys return 401, the deliberate loop
does not reach the provider, and the two complete matrices produce exactly 14
provider calls—one per intended request, with no hidden retry or translation
cycle. `request-retry: 0` makes that count deterministic.

The pinned real binary confirms the same error-header boundary described
above: in either arrangement the provider's `Retry-After` is lost when the
response crosses CLIProxyAPI's OpenAI-compatible executor. In the reverse
arrangement CLIProxyAPI also drops Alex's trace header on non-2xx responses.
The fixture asserts both absences, rather than weakening the contract around
status and structured JSON-body preservation.

This matrix is evidence for v7.2.92, not a claim about every v7 release. The
exporter negotiates major v7+ because the required config schema is public, but
new CLIProxyAPI versions should be added to the matrix only after running the
same digest-pinned fixture. WebSocket Responses, Gemini ingress, and image
endpoints remain outside V1 and are not exercised here.
