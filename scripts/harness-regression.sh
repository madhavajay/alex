#!/usr/bin/env bash
# Real harness -> Alexandria Docker regression lane.  It never gives a
# container the local/admin key: each cell mints and later revokes one scoped
# harness key whose run_id is used for all host-side assertions.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONFIG_FILE="${ALEXANDRIA_CONFIG:-$HOME/.alexandria/config.toml}"
ONLY="${ALEX_INTEGRATION_ONLY:-}"
TIMEOUT="${ALEX_INTEGRATION_TIMEOUT:-600}"
HOST="${ALEX_INTEGRATION_HOST:-}"
PORT="${ALEX_INTEGRATION_PORT:-}"
TMP="$(mktemp -d "${TMPDIR:-/tmp}/alex-harness-integration.XXXXXX")"
RESULTS="$TMP/results"
mkdir -p "$RESULTS"
DAEMON_PID=""

cfg_str() { [ -f "$CONFIG_FILE" ] && sed -n "s/^$1[[:space:]]*=[[:space:]]*\"\([^\"]*\)\".*/\1/p" "$CONFIG_FILE" | head -1; }
cfg_num() { [ -f "$CONFIG_FILE" ] && sed -n "s/^$1[[:space:]]*=[[:space:]]*\([0-9][0-9]*\).*/\1/p" "$CONFIG_FILE" | head -1; }
HOST="${HOST:-$(cfg_str host)}"; HOST="${HOST:-127.0.0.1}"
PORT="${PORT:-$(cfg_num port)}"; PORT="${PORT:-4100}"
KEY="$(cfg_str local_key)"; BASE="http://$HOST:$PORT"

cleanup() {
  local rc=$?
  [ -z "$DAEMON_PID" ] || { kill "$DAEMON_PID" 2>/dev/null || true; }
  rm -rf "$TMP"
  exit "$rc"
}
trap cleanup EXIT INT TERM
now_ms() { python3 -c 'import time; print(int(time.time()*1000))'; }
log() { printf '%s\n' "$*" >&2; }
result() { printf '%s\t%s\t%s\n' "$1" "$2" "$3" > "$RESULTS/$1"; log "  $1 $2 $3"; }
selected() { [ -z "$ONLY" ] || [[ ",$ONLY," == *",$1,"* ]]; }
health() { curl -fsS --max-time 3 "$BASE/health" >/dev/null 2>&1; }

ensure_daemon() {
  health && return 0
  "$ROOT/alex" daemon --host "$HOST" --port "$PORT" >"$TMP/daemon.log" 2>&1 & DAEMON_PID=$!
  for _ in $(seq 1 60); do
    health && return 0
    kill -0 "$DAEMON_PID" 2>/dev/null || break
    sleep 1
  done
  log "daemon did not become healthy"; tail -20 "$TMP/daemon.log" >&2 || true; return 1
}

accounts() { curl -fsS --max-time 5 -H "x-api-key: $KEY" "$BASE/admin/accounts" > "$TMP/accounts.json"; }
active_account() { python3 - "$TMP/accounts.json" "$1" <<'PY'
import json,sys
try: rows=json.load(open(sys.argv[1])).get('accounts', [])
except Exception: rows=[]
raise SystemExit(0 if any(r.get('provider') == sys.argv[2] and r.get('status') == 'active' for r in rows) else 1)
PY
}

preflight() {
  local provider=$1 model=$2 endpoint body code
  [ -f "$TMP/pre.$provider" ] && [ "$(cat "$TMP/pre.$provider")" = ok ] && return 0
  [ -f "$TMP/pre.$provider" ] && return 1
  if ! active_account "$provider"; then echo "no active $provider account" > "$TMP/pre.$provider"; return 1; fi
  case "$provider" in
    openai) endpoint=/v1/responses; body="{\"model\":\"$model\",\"max_output_tokens\":16,\"input\":\"Reply with only OK\"}" ;;
    anthropic) endpoint=/v1/messages; body="{\"model\":\"$model\",\"max_tokens\":16,\"messages\":[{\"role\":\"user\",\"content\":\"Reply with only OK\"}]}" ;;
    *) echo "no cheap preflight for $provider" > "$TMP/pre.$provider"; return 1 ;;
  esac
  code=$(curl -sS --max-time 90 -o "$TMP/pre.$provider.body" -w '%{http_code}' -H "x-api-key: $KEY" -H 'content-type: application/json' -d "$body" "$BASE$endpoint" || true)
  if [[ "$code" == 2* ]]; then echo ok > "$TMP/pre.$provider"; return 0; fi
  printf 'preflight http %s: %s' "$code" "$(head -c 160 "$TMP/pre.$provider.body" 2>/dev/null)" > "$TMP/pre.$provider"; return 1
}

mint_key() {
  local harness=$1 run_id=$2 out status
  printf '{"kind":"harness","label":"%s","run_id":"%s","tags":{"suite":"harness-regression","harness":"%s"}}' "$harness" "$run_id" "$harness" > "$TMP/mint.json"
  status=$(curl -sS --max-time 10 -o "$TMP/minted.json" -w '%{http_code}' -H "x-api-key: $KEY" -H 'content-type: application/json' --data-binary @"$TMP/mint.json" "$BASE/admin/run-keys")
  [ "$status" = 201 ] || return 1
  python3 - "$TMP/minted.json" "$TMP/run.key" <<'PY'
import json,sys
d=json.load(open(sys.argv[1])); assert d['kind']=='harness' and d.get('key') and d.get('run_id')
open(sys.argv[2],'w').write(d['key']+'\n')
print(d['id'])
PY
}
revoke_key() { curl -fsS --max-time 5 -X DELETE -H "x-api-key: $KEY" "$BASE/admin/run-keys/$1" >/dev/null 2>&1 || true; }
wait_traces() {
  local run_id=$1 out=$2
  for _ in $(seq 1 20); do
    curl -fsS --max-time 5 -H "x-api-key: $KEY" "$BASE/admin/traces?run_id=$run_id&limit=100" > "$out" || true
    python3 - "$out" <<'PY' && return 0
import json,sys
try: raise SystemExit(0 if json.load(open(sys.argv[1])).get('traces') else 1)
except Exception: raise SystemExit(1)
PY
    sleep 1
  done
  return 1
}
quota_error() { rg -qi 'insufficient|quota|credit|rate.limit|429|not subscribed|billing' "$1" 2>/dev/null; }
assert_trace() {
  local id=$1 run_id=$2 harness=$3 provider=$4 model=$5 expect_dario=${6:-0} traces="$TMP/$id.traces.json" details trace_id
  details=$(python3 "$ROOT/scripts/harness-regression-assert.py" trace --traces "$traces" --run-id "$run_id" --harness "$harness" --provider "$provider" --model "$model") || return 1
  trace_id=$(python3 -c 'import json,sys; print(json.load(sys.stdin)["trace_id"])' <<<"$details")
  for kind in request response; do
    curl -fsS --max-time 5 -H "x-api-key: $KEY" "$BASE/traces/$trace_id/body/$kind" > "$TMP/$id.$kind.body" || return 1
    [ -s "$TMP/$id.$kind.body" ] || return 1
  done
  if [ "$expect_dario" = 1 ]; then
    for kind in dario-upstream-request dario-upstream-response; do
      curl -fsS --max-time 5 -H "x-api-key: $KEY" "$BASE/traces/$trace_id/body/$kind" > "$TMP/$id.$kind.body" || return 1
      [ -s "$TMP/$id.$kind.body" ] || return 1
    done
  fi
  printf '%s' "$details"
}

run_builtin() {
  local id=$1 harness=$2 provider=$3 model=$4 expect_dario=${5:-0} run_id key_id t0 rc=0 msg
  selected "$id" || return 0
  t0=$(now_ms)
  if ! docker info >/dev/null 2>&1; then result "$id" SKIP 'docker unavailable'; return 0; fi
  if ! preflight "$provider" "$model" >/dev/null; then result "$id" SKIP "$(cat "$TMP/pre.$provider")"; return 0; fi
  run_id="hreg-$id-$t0-$RANDOM"
  key_id=$(mint_key "$harness" "$run_id") || { result "$id" SKIP 'daemon cannot mint scoped harness key'; return 0; }
  chmod 600 "$TMP/run.key"
  "$ROOT/alex" harness run "$harness" --model "$model" --prompt 'Reply with exactly: harness-regression-ok' --timeout-secs "$TIMEOUT" --run-key-file "$TMP/run.key" --run-id "$run_id" --json >"$TMP/$id.run.json" 2>"$TMP/$id.run.err" || rc=$?
  if [ "$rc" -ne 0 ]; then
    if quota_error "$TMP/$id.run.err"; then result "$id" SKIP "provider unavailable: $(tail -c 180 "$TMP/$id.run.err")"; else result "$id" FAIL "harness exit $rc: $(tail -c 180 "$TMP/$id.run.err")"; fi
    revoke_key "$key_id"; return 0
  fi
  if ! wait_traces "$run_id" "$TMP/$id.traces.json"; then result "$id" FAIL 'harness exited but no trace for scoped run_id'; revoke_key "$key_id"; return 0; fi
  if msg=$(assert_trace "$id" "$run_id" "$harness" "$provider" "$model" "$expect_dario" 2>&1); then result "$id" PASS "run_id=$run_id $msg"; else result "$id" FAIL "$msg"; fi
  revoke_key "$key_id"
}

optional_cell() {
  local id=$1 requirement=$2
  selected "$id" || return 0
  result "$id" SKIP "$requirement"
}

# Native fixture images are deliberately supplied by the environment rather
# than embedded here: their CLIs and commercial credentials are optional.
# Each image must contain an Alexandria-generated config/plugin and the command
# must run a single deterministic task using the injected proxy variables.
run_fixture() {
  local id=$1 harness=$2 provider=$3 model=$4 image_var=$5 command_var=$6 check=$7
  local image="${!image_var:-}" command="${!command_var:-}" run_id key_id rc=0 details session tool_id
  selected "$id" || return 0
  if [ -z "$image" ] || [ -z "$command" ]; then
    result "$id" SKIP "fixture absent (set $image_var and $command_var)"
    return 0
  fi
  if ! docker image inspect "$image" >/dev/null 2>&1; then result "$id" SKIP "fixture image missing: $image"; return 0; fi
  if ! preflight "$provider" "$model" >/dev/null; then result "$id" SKIP "$(cat "$TMP/pre.$provider")"; return 0; fi
  run_id="hreg-$id-$(now_ms)-$RANDOM"
  key_id=$(mint_key "$harness" "$run_id") || { result "$id" SKIP 'daemon cannot mint scoped harness key'; return 0; }
  docker run --rm --add-host host.docker.internal:host-gateway \
    -e "ALEXANDRIA_BASE_URL=http://host.docker.internal:$PORT" -e "ALEXANDRIA_RUN_ID=$run_id" -e "ALEXANDRIA_HARNESS=$harness" \
    -e "OPENAI_BASE_URL=http://host.docker.internal:$PORT/v1" -e "OPENAI_API_BASE=http://host.docker.internal:$PORT/v1" -e "OPENAI_API_KEY=$(cat "$TMP/run.key")" \
    -e "ANTHROPIC_BASE_URL=http://host.docker.internal:$PORT" -e "ANTHROPIC_API_KEY=$(cat "$TMP/run.key")" \
    "$image" sh -lc "$command" >"$TMP/$id.fixture.out" 2>"$TMP/$id.fixture.err" || rc=$?
  if [ "$rc" -ne 0 ]; then
    if quota_error "$TMP/$id.fixture.err"; then result "$id" SKIP "provider unavailable: $(tail -c 180 "$TMP/$id.fixture.err")"; else result "$id" FAIL "fixture exit $rc: $(tail -c 180 "$TMP/$id.fixture.err")"; fi
    revoke_key "$key_id"; return 0
  fi
  if ! wait_traces "$run_id" "$TMP/$id.traces.json"; then result "$id" FAIL 'fixture exited but no trace for scoped run_id'; revoke_key "$key_id"; return 0; fi
  if ! details=$(assert_trace "$id" "$run_id" "$harness" "$provider" "$model" 2>&1); then result "$id" FAIL "$details"; revoke_key "$key_id"; return 0; fi
  session=$(python3 -c 'import json,sys; print(json.load(sys.stdin)["session_id"])' <<<"$details")
  case "$check" in
    tools)
      curl -fsS --max-time 5 -H "x-api-key: $KEY" "$BASE/traces/sessions/$session/transcript" > "$TMP/$id.transcript.json" || { result "$id" FAIL 'cannot read tool transcript'; revoke_key "$key_id"; return 0; }
      tool_id=$(python3 "$ROOT/scripts/harness-regression-assert.py" tools --transcript "$TMP/$id.transcript.json" | python3 -c 'import json,sys; print(json.load(sys.stdin)["tool_id"])') || { result "$id" FAIL 'tool capture assertion failed'; revoke_key "$key_id"; return 0; }
      for kind in args result; do curl -fsS --max-time 5 -H "x-api-key: $KEY" "$BASE/tools/$tool_id/body/$kind" > "$TMP/$id.tool-$kind.body" || { result "$id" FAIL "tool $kind body missing"; revoke_key "$key_id"; return 0; }; done
      if [ -n "${ALEX_INTEGRATION_TEST_SECRET:-}" ] && rg -q --fixed-strings "$ALEX_INTEGRATION_TEST_SECRET" "$TMP/$id.tool-args.body" "$TMP/$id.tool-result.body"; then result "$id" FAIL 'tool body leaked ALEX_INTEGRATION_TEST_SECRET'; revoke_key "$key_id"; return 0; fi
      ;;
    lineage)
      curl -fsS --max-time 5 -H "x-api-key: $KEY" "$BASE/traces/sessions?limit=1000" > "$TMP/$id.sessions.json" || { result "$id" FAIL 'cannot read sessions'; revoke_key "$key_id"; return 0; }
      local lineage_args=(lineage --traces "$TMP/$id.traces.json" --sessions "$TMP/$id.sessions.json" --run-id "$run_id")
      [ "$id" != I9 ] || lineage_args+=(--agent-type pi)
      python3 "$ROOT/scripts/harness-regression-assert.py" "${lineage_args[@]}" > "$TMP/$id.lineage.json" || { result "$id" FAIL 'lineage assertion failed'; revoke_key "$key_id"; return 0; }
      ;;
  esac
  result "$id" PASS "run_id=$run_id $details"
  revoke_key "$key_id"
}

dario_active() {
  curl -fsS --max-time 5 -H "x-api-key: $KEY" "$BASE/admin/dario" > "$TMP/dario.json" 2>/dev/null || return 1
  python3 - "$TMP/dario.json" <<'PY'
import json,sys
try: d=json.load(open(sys.argv[1]))
except Exception: raise SystemExit(1)
raise SystemExit(0 if 'active' in json.dumps(d).lower() else 1)
PY
}

main() {
  if [ -z "$KEY" ]; then log "no local_key in $CONFIG_FILE"; exit 2; fi
  ensure_daemon || exit 2
  accounts || { log 'cannot read /admin/accounts with local key'; exit 2; }
  log '== harness integration: scoped Docker keys + host API assertions =='
  run_builtin I1 codex openai gpt-5.6-luna
  run_builtin I2 claude anthropic claude-haiku-4-5
  run_fixture I3 pi openai gpt-5.6-luna ALEX_INTEGRATION_PI_IMAGE ALEX_INTEGRATION_PI_COMMAND tools
  run_fixture I4 codex openai gpt-5.6-luna ALEX_INTEGRATION_CODEX_SUBAGENT_IMAGE ALEX_INTEGRATION_CODEX_SUBAGENT_COMMAND lineage
  # The command must invoke `pi --print --no-session '…'` from the parent Pi
  # process. The child inherits ALEXANDRIA_SESSION_ID from its parent.
  run_fixture I9 pi openai gpt-5.6-luna ALEX_INTEGRATION_PI_IMAGE ALEX_INTEGRATION_PI_SUBAGENT_COMMAND lineage
  optional_cell I5A 'Amp wrap fixture unavailable (requires logged-in Amp CLI)'
  optional_cell I5B 'Cursor Agent wrap fixture unavailable (requires logged-in agent CLI)'
  if dario_active; then run_builtin I6 codex anthropic claude-fable-5 1; else optional_cell I6 'Dario fable fixture unavailable (requires active dario generation + Anthropic account)'; fi
  run_fixture I7 grok openai gpt-5.6-luna ALEX_INTEGRATION_GROK_IMAGE ALEX_INTEGRATION_GROK_COMMAND trace
  run_fixture I8 gemini openai gpt-5.6-luna ALEX_INTEGRATION_GEMINI_IMAGE ALEX_INTEGRATION_GEMINI_COMMAND trace
  local failed=0 f
  for f in "$RESULTS"/*; do [ "$(cut -f2 "$f")" = FAIL ] && failed=$((failed+1)); done
  printf '\n'; cat "$RESULTS"/* | sort | while IFS=$'\t' read -r id status detail; do printf '%-4s %-4s %s\n' "$id" "$status" "$detail"; done
  [ "$failed" -eq 0 ]
}
main "$@"
