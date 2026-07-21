#!/usr/bin/env bash
# Real CLIProxyAPI v7 Docker compatibility fixture for both supported V1
# arrangements. All listeners and published ports are loopback-only, all keys
# are generated into a private temporary directory, and request logging is off.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE_DEFAULT="eceasy/cli-proxy-api:v7.2.92@sha256:af18f6fb364bfb7b482a1ca6c6c85fd7df2c0d6a3a497ebb82c337ac2216dc41"
IMAGE="${ALEX_CPA_FIXTURE_IMAGE:-$IMAGE_DEFAULT}"
PORT_BASE="${ALEX_CPA_FIXTURE_PORT_BASE:-$((44000 + ($$ % 10000)))}"
ALEX_PORT="$PORT_BASE"
PROVIDER_PORT="$((PORT_BASE + 1))"
CPA_PORT="$((PORT_BASE + 2))"
TMP="$(mktemp -d "${TMPDIR:-/tmp}/alex-cpa-v1.XXXXXX")"
RESPONSES="$TMP/responses"
CONTAINER="alex-cpa-v1-$$"
FIXTURE_PID=""
KEEP="${ALEX_CPA_FIXTURE_KEEP:-0}"
mkdir -p "$RESPONSES" "$TMP/auth" "$TMP/data"
chmod 700 "$TMP" "$RESPONSES" "$TMP/auth" "$TMP/data"

log() { printf '%s\n' "$*" >&2; }
fail() { log "FAIL: $*"; exit 1; }

cleanup() {
  local rc=$?
  docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
  if [ -n "$FIXTURE_PID" ]; then
    kill "$FIXTURE_PID" >/dev/null 2>&1 || true
    wait "$FIXTURE_PID" >/dev/null 2>&1 || true
  fi
  if [ "$KEEP" = 1 ]; then
    log "fixture artifacts retained at $TMP"
  else
    rm -rf "$TMP"
  fi
  exit "$rc"
}
trap cleanup EXIT INT TERM

command -v docker >/dev/null 2>&1 || fail "docker is required"
command -v curl >/dev/null 2>&1 || fail "curl is required"
command -v python3 >/dev/null 2>&1 || fail "python3 is required"
command -v openssl >/dev/null 2>&1 || fail "openssl is required"
docker info >/dev/null 2>&1 || fail "docker daemon is unavailable"

# Values are random, fixture-scoped, and only written below into mode-0600
# files. Curl reads Authorization headers from files so keys never appear in
# process arguments or fixture output.
LOCAL_KEY="fixture-local-$(openssl rand -hex 24)"
HARNESS_KEY="alxk-$(openssl rand -hex 32)"
CPA_KEY="fixture-cpa-$(openssl rand -hex 24)"
PROVIDER_KEY="fixture-provider-$(openssl rand -hex 24)"
BAD_KEY="fixture-invalid-$(openssl rand -hex 24)"
printf '%s\n%s\n%s\n%s\n%s\n' \
  "$LOCAL_KEY" "$HARNESS_KEY" "$CPA_KEY" "$PROVIDER_KEY" "$BAD_KEY" > "$TMP/secrets"
printf 'Authorization: Bearer %s\n' "$HARNESS_KEY" > "$TMP/alex.headers"
printf 'Authorization: Bearer %s\n' "$CPA_KEY" > "$TMP/cpa.headers"
printf 'Authorization: Bearer %s\n' "$BAD_KEY" > "$TMP/bad.headers"
chmod 600 "$TMP/secrets" "$TMP/alex.headers" "$TMP/cpa.headers" "$TMP/bad.headers"

case "$(uname -s)" in
  Linux)
    CPA_BIND_HOST="127.0.0.1"
    CPA_CONTAINER_PORT="$CPA_PORT"
    HOST_FROM_CONTAINER="127.0.0.1"
    DOCKER_NETWORK_ARGS=(--network host)
    ;;
  Darwin)
    CPA_BIND_HOST="0.0.0.0"
    CPA_CONTAINER_PORT="8317"
    HOST_FROM_CONTAINER="host.docker.internal"
    DOCKER_NETWORK_ARGS=(
      --add-host host.docker.internal:host-gateway
      -p "127.0.0.1:$CPA_PORT:8317"
    )
    ;;
  *) fail "unsupported Docker host OS $(uname -s); use Linux or macOS" ;;
esac

cat > "$TMP/config.yaml" <<EOF
host: "$CPA_BIND_HOST"
port: $CPA_CONTAINER_PORT
auth-dir: "/root/.cli-proxy-api"
api-keys:
  - "$CPA_KEY"
debug: false
request-log: false
commercial-mode: true
logging-to-file: false
usage-statistics-enabled: false
force-model-prefix: true
passthrough-headers: true
request-retry: 0
max-retry-credentials: 1
max-retry-interval: 0
disable-cooling: true
remote-management:
  allow-remote: false
  secret-key: ""
  disable-control-panel: true
openai-compatibility:
  - name: "fixture-provider"
    prefix: "cpa"
    base-url: "http://$HOST_FROM_CONTAINER:$PROVIDER_PORT/v1"
    disable-cooling: true
    api-key-entries:
      - api-key: "$PROVIDER_KEY"
    models:
      - {name: "fixture-echo", alias: "echo", input-modalities: [text], output-modalities: [text]}
      - {name: "fixture-tool", alias: "tool", input-modalities: [text], output-modalities: [text]}
      - {name: "fixture-auth", alias: "auth", input-modalities: [text], output-modalities: [text]}
      - {name: "fixture-rate", alias: "rate", input-modalities: [text], output-modalities: [text]}
      - {name: "fixture-server", alias: "server", input-modalities: [text], output-modalities: [text]}
  - name: "alex"
    prefix: "alex"
    base-url: "http://$HOST_FROM_CONTAINER:$ALEX_PORT/v1"
    disable-cooling: true
    headers:
      X-Alex-Harness: "cliproxyapi"
      X-Alex-Harness-Version: "v7.2.92"
      X-Alex-Integration-Schema: "alex.cliproxyapi.reverse/v1"
      X-Alex-Capabilities: "openai-chat,openai-responses,anthropic-messages,streaming,tool-calls,structured-errors,trace-correlation"
      X-Alex-Route-Chain: "cliproxyapi"
    api-key-entries:
      - api-key: "$HARNESS_KEY"
    models:
      - {name: "alex/echo", alias: "echo", input-modalities: [text], output-modalities: [text]}
      - {name: "alex/tool", alias: "tool", input-modalities: [text], output-modalities: [text]}
      - {name: "alex/auth", alias: "auth", input-modalities: [text], output-modalities: [text]}
      - {name: "alex/rate", alias: "rate", input-modalities: [text], output-modalities: [text]}
      - {name: "alex/server", alias: "server", input-modalities: [text], output-modalities: [text]}
EOF
chmod 600 "$TMP/config.yaml"

log "building loopback Alex fixture server"
cargo build -q -p alex-proxy --example cliproxyapi_v1_fixture
ALEX_CPA_FIXTURE_ALEX_PORT="$ALEX_PORT" \
ALEX_CPA_FIXTURE_PROVIDER_PORT="$PROVIDER_PORT" \
ALEX_CPA_FIXTURE_DATA_DIR="$TMP/data" \
ALEX_CPA_FIXTURE_LOCAL_KEY="$LOCAL_KEY" \
ALEX_CPA_FIXTURE_HARNESS_KEY="$HARNESS_KEY" \
ALEX_CPA_FIXTURE_CPA_URL="http://127.0.0.1:$CPA_PORT" \
ALEX_CPA_FIXTURE_CPA_KEY="$CPA_KEY" \
ALEX_CPA_FIXTURE_PROVIDER_KEY="$PROVIDER_KEY" \
  "$ROOT/target/debug/examples/cliproxyapi_v1_fixture" > "$TMP/alex-fixture.log" 2>&1 &
FIXTURE_PID=$!

for _ in $(seq 1 100); do
  curl -fsS --max-time 1 "http://127.0.0.1:$ALEX_PORT/health" >/dev/null 2>&1 && break
  kill -0 "$FIXTURE_PID" >/dev/null 2>&1 || fail "Alex fixture server exited during startup"
  sleep 0.1
done
curl -fsS --max-time 2 "http://127.0.0.1:$ALEX_PORT/health" >/dev/null \
  || fail "Alex fixture server did not become ready"

log "starting pinned CLIProxyAPI v7.2.92 Docker image"
docker run -d --rm --name "$CONTAINER" --log-driver none \
  "${DOCKER_NETWORK_ARGS[@]}" \
  -v "$TMP/config.yaml:/CLIProxyAPI/config.yaml:ro" \
  -v "$TMP/auth:/root/.cli-proxy-api" \
  "$IMAGE" >/dev/null

for _ in $(seq 1 200); do
  curl -fsS --max-time 1 -H @"$TMP/cpa.headers" \
    "http://127.0.0.1:$CPA_PORT/v1/models" > "$RESPONSES/models.body" 2>/dev/null && break
  docker inspect "$CONTAINER" >/dev/null 2>&1 || fail "CLIProxyAPI container exited during startup"
  sleep 0.1
done
curl -fsS --max-time 2 -H @"$TMP/cpa.headers" \
  "http://127.0.0.1:$CPA_PORT/v1/models" > "$RESPONSES/models.body" \
  || fail "CLIProxyAPI did not become ready"

request() {
  local id=$1 base=$2 headers=$3 path=$4 body=$5 expected=$6 code
  shift 6
  code=$(curl -sS --max-time 15 -D "$RESPONSES/$id.headers" \
    -o "$RESPONSES/$id.body" -w '%{http_code}' \
    -H @"$headers" -H 'content-type: application/json' \
    "$@" --data-binary "$body" "$base$path")
  [ "$code" = "$expected" ] || fail "$id returned HTTP $code, expected $expected"
}

assert_trace_header() {
  local id=$1
  grep -Eiq '^x-alex-trace-id:' "$RESPONSES/$id.headers" \
    || fail "$id is missing x-alex-trace-id"
}

assert_no_header() {
  local id=$1 name=$2
  ! grep -Eiq "^${name}:" "$RESPONSES/$id.headers" \
    || fail "$id unexpectedly returned $name"
}

run_success_matrix() {
  local direction=$1 base=$2 headers=$3 model=$4
  request "$direction-chat" "$base" "$headers" /v1/chat/completions \
    "{\"model\":\"$model/echo\",\"messages\":[{\"role\":\"user\",\"content\":\"test\"}]}" 200
  python3 "$ROOT/scripts/cliproxyapi-v1-assert.py" success "$RESPONSES/$direction-chat.body" chat
  assert_trace_header "$direction-chat"

  request "$direction-responses" "$base" "$headers" /v1/responses \
    "{\"model\":\"$model/echo\",\"input\":\"test\"}" 200
  python3 "$ROOT/scripts/cliproxyapi-v1-assert.py" success "$RESPONSES/$direction-responses.body" responses
  assert_trace_header "$direction-responses"

  request "$direction-anthropic" "$base" "$headers" /v1/messages \
    "{\"model\":\"$model/echo\",\"max_tokens\":32,\"messages\":[{\"role\":\"user\",\"content\":\"test\"}]}" 200
  python3 "$ROOT/scripts/cliproxyapi-v1-assert.py" success "$RESPONSES/$direction-anthropic.body" anthropic
  assert_trace_header "$direction-anthropic"

  request "$direction-tool-stream" "$base" "$headers" /v1/messages \
    "{\"model\":\"$model/tool\",\"stream\":true,\"max_tokens\":32,\"messages\":[{\"role\":\"user\",\"content\":\"run pwd\"}],\"tools\":[{\"name\":\"shell\",\"description\":\"run command\",\"input_schema\":{\"type\":\"object\",\"properties\":{\"command\":{\"type\":\"string\"}}}}]}" 200
  python3 "$ROOT/scripts/cliproxyapi-v1-assert.py" stream "$RESPONSES/$direction-tool-stream.body"
  assert_trace_header "$direction-tool-stream"
}

run_error_matrix() {
  local direction=$1 base=$2 headers=$3 model=$4
  request "$direction-rate" "$base" "$headers" /v1/chat/completions \
    "{\"model\":\"$model/rate\",\"messages\":[{\"role\":\"user\",\"content\":\"test\"}]}" 429
  python3 "$ROOT/scripts/cliproxyapi-v1-assert.py" error "$RESPONSES/$direction-rate.body" fixture_rate
  assert_no_header "$direction-rate" retry-after

  request "$direction-server" "$base" "$headers" /v1/chat/completions \
    "{\"model\":\"$model/server\",\"messages\":[{\"role\":\"user\",\"content\":\"test\"}]}" 503
  python3 "$ROOT/scripts/cliproxyapi-v1-assert.py" error "$RESPONSES/$direction-server.body" fixture_server

  request "$direction-auth" "$base" "$headers" /v1/chat/completions \
    "{\"model\":\"$model/auth\",\"messages\":[{\"role\":\"user\",\"content\":\"test\"}]}" 401
  python3 "$ROOT/scripts/cliproxyapi-v1-assert.py" error "$RESPONSES/$direction-auth.body" fixture_auth
}

log "testing Harness -> Alex -> CLIProxyAPI -> provider"
run_success_matrix a1 "http://127.0.0.1:$ALEX_PORT" "$TMP/alex.headers" "cliproxyapi/cpa"
run_error_matrix a1 "http://127.0.0.1:$ALEX_PORT" "$TMP/alex.headers" "cliproxyapi/cpa"
assert_trace_header a1-rate

log "testing Harness -> CLIProxyAPI -> Alex -> provider"
run_success_matrix a2 "http://127.0.0.1:$CPA_PORT" "$TMP/cpa.headers" "alex"
run_error_matrix a2 "http://127.0.0.1:$CPA_PORT" "$TMP/cpa.headers" "alex"
assert_no_header a2-rate retry-after
assert_no_header a2-rate x-alex-trace-id

request cpa-bad-key "http://127.0.0.1:$CPA_PORT" "$TMP/bad.headers" /v1/chat/completions \
  '{"model":"alex/echo","messages":[{"role":"user","content":"test"}]}' 401
request alex-bad-key "http://127.0.0.1:$ALEX_PORT" "$TMP/bad.headers" /v1/chat/completions \
  '{"model":"cliproxyapi/cpa/echo","messages":[{"role":"user","content":"test"}]}' 401

curl -fsS --max-time 2 "http://127.0.0.1:$PROVIDER_PORT/fixture/stats" > "$RESPONSES/stats-before-loop.body"
request loop-guard "http://127.0.0.1:$ALEX_PORT" "$TMP/alex.headers" /v1/chat/completions \
  '{"model":"cliproxyapi/cpa/echo","messages":[{"role":"user","content":"test"}]}' 508 \
  -H 'x-alex-harness: cliproxyapi' \
  -H 'x-alex-route-chain: cliproxyapi' \
  -H 'x-alex-integration-schema: alex.cliproxyapi.reverse/v1' \
  -H 'x-alex-capabilities: openai-chat'
curl -fsS --max-time 2 "http://127.0.0.1:$PROVIDER_PORT/fixture/stats" > "$RESPONSES/stats-after-loop.body"
python3 "$ROOT/scripts/cliproxyapi-v1-assert.py" stats "$RESPONSES/stats-before-loop.body" 14
python3 "$ROOT/scripts/cliproxyapi-v1-assert.py" stats "$RESPONSES/stats-after-loop.body" 14

python3 "$ROOT/scripts/cliproxyapi-v1-assert.py" no-secrets "$RESPONSES" "$TMP/secrets"
log "PASS: CLIProxyAPI V1 pinned Docker matrix (v7.2.92, both arrangements, 14 exact provider calls, loop guard clean)"
