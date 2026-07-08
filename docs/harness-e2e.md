# Harness E2E Smoke Runs

Alexandria's harness E2E support is intentionally small. It does not evaluate task quality, score outputs, or mutate prompts. It only answers:

1. Can a frozen harness package run in a clean container?
2. Can it reach the Alexandria proxy with the expected API shape?
3. Did Alexandria capture the request, response, headers, model routing, usage, and body artifacts?

## Alexandria Pattern

Alexandria keeps the harness smoke layer deliberately separate from benchmarks:

- It treats harness CLIs as artifacts. Registry packages are packed into `~/.alexandria/harness-packages/*.tgz`; local source builds should be packed with `npm pack` or the harness's native build step before running.
- It runs harnesses in containers with only the required proxy env/config. Claude gets `ANTHROPIC_BASE_URL` and model aliases. Codex gets an isolated `CODEX_HOME` with a `model_providers.*` entry and `wire_api = "responses"`.
- It verifies transport health separately from benchmark success. The smoke run checks that a request reaches Alexandria and that the trace contains headers, routing metadata, usage, and body artifacts.

There is no task scoring, output grading, or benchmark orchestration in this layer.

## Commands

List configured harness smoke definitions:

```bash
cargo run -p alexandria-daemon -- harness list
```

Prepare a frozen npm tarball in Alexandria's cache:

```bash
cargo run -p alexandria-daemon -- harness pack claude
```

Run Claude Code from Alexandria's frozen package cache:

```bash
cargo run -p alexandria-daemon -- harness run claude
```

Run a different frozen package:

```bash
cargo run -p alexandria-daemon -- harness pack @openai/codex
cargo run -p alexandria-daemon -- harness run codex \
  --model codex:gpt-5.5
```

The daemon should be running before the harness command. From Docker, the default container URL is `http://host.docker.internal:<port>`.

## Dario Notes

Dario is a Claude subscription proxy that is useful reference material when an OpenAI-format harness needs to hit Claude. The classic bridge path is:

- OpenAI-format harness -> OpenAI-compatible bridge proxy
- Bridge proxy -> Dario Anthropic proxy
- Dario -> Anthropic Claude subscription endpoint

For Alexandria, Dario is useful as reference material, not as another mandatory process. The important bits to retain are:

- Dario exposes Anthropic-compatible `/v1/messages` with a local API key.
- Its proxy mode rewrites requests so SDK-style Anthropic traffic can bill against Claude subscription OAuth instead of a raw API key.
- Its shim mode uses `NODE_OPTIONS=--require <runtime.cjs>` to patch Node/Bun `fetch` inside a child process. That is useful as a fallback pattern for Claude-family CLIs when base URL env vars are insufficient, but Alexandria should prefer the explicit proxy path first.
- Dario keeps detailed request logs and billing bucket classification; Alexandria's equivalent is the SQLite trace plus gzipped request/response bodies under the Alexandria data dir.

The current Alexandria runner does not start Dario. If we add Dario parity later, it should be another harness route mode, not embedded in scoring or benchmark orchestration.
