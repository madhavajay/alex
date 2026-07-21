#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
SERVICE_LABEL="com.madhavajay.alex.daemon"
SERVICE_TARGET="gui/$(id -u)/$SERVICE_LABEL"
SERVICE_PLIST="$HOME/Library/LaunchAgents/$SERVICE_LABEL.plist"
BASE_URL="http://127.0.0.1:4100"
SESSION_ID="ci-installed-smoke"
MODEL="ci-smoke-model"

fail() {
  echo "installed macOS smoke: $*" >&2
  exit 1
}

for command in codesign curl jq launchctl plutil python3; do
  command -v "$command" >/dev/null 2>&1 || fail "missing required command: $command"
done
[[ "$(uname -s)" == "Darwin" ]] || fail "this gate requires macOS"
[[ -d "$SCRIPT_DIR/Alex.app" ]] || fail "Alex.app is missing from the extracted bundle"
[[ -x "$SCRIPT_DIR/bin/alex" ]] || fail "bin/alex is missing from the extracted bundle"
[[ -x "$SCRIPT_DIR/mock-openai.py" ]] || fail "mock-openai.py is missing from the extracted bundle"

SMOKE_ROOT="${ALEX_CI_SMOKE_ROOT:?set ALEX_CI_SMOKE_ROOT to an isolated runner directory}"
[[ "$SMOKE_ROOT" = /*/alex-installed-smoke ]] \
  || fail "ALEX_CI_SMOKE_ROOT must be an absolute path ending in /alex-installed-smoke"
RESULT_PATH="${ALEX_CI_SMOKE_RESULT:-$SMOKE_ROOT/smoke-result.json}"
STATE_DIR="$SMOKE_ROOT/state"
APP_DEST="$SMOKE_ROOT/Applications/Alex.app"
BIN_DEST="$SMOKE_ROOT/bin"
ALEX_BIN="$BIN_DEST/alex"
MOCK_READY="$SMOKE_ROOT/mock.port"
MOCK_LOG="$SMOKE_ROOT/mock.ndjson"
MOCK_PID=""
ROOT_OWNED=0
SERVICE_CLEANUP_ALLOWED=0

cleanup() {
  status=$?
  trap - EXIT INT TERM
  set +e
  if [[ "$SERVICE_CLEANUP_ALLOWED" -eq 1 ]]; then
    if [[ -x "$ALEX_BIN" ]]; then
      ALEX_HOME="$STATE_DIR" "$ALEX_BIN" service uninstall >/dev/null 2>&1
    fi
    launchctl bootout "$SERVICE_TARGET" >/dev/null 2>&1
    rm -f "$SERVICE_PLIST"
  fi
  if [[ -n "$MOCK_PID" ]]; then
    kill "$MOCK_PID" >/dev/null 2>&1
    wait "$MOCK_PID" >/dev/null 2>&1
  fi
  if [[ "$ROOT_OWNED" -eq 1 ]]; then
    rm -rf "$SMOKE_ROOT"
  fi
  exit "$status"
}
trap cleanup EXIT INT TERM

launchctl print "$SERVICE_TARGET" >/dev/null 2>&1 \
  && fail "runner is not clean: $SERVICE_TARGET is already loaded"
[[ ! -e "$SERVICE_PLIST" ]] \
  || fail "runner is not clean: $SERVICE_PLIST already exists"

rm -rf "$SMOKE_ROOT"
mkdir -p "$SMOKE_ROOT" "$(dirname "$APP_DEST")" "$(dirname "$RESULT_PATH")"
ROOT_OWNED=1
SERVICE_CLEANUP_ALLOWED=1
touch "$SMOKE_ROOT/service-cleanup-allowed"

python3 "$SCRIPT_DIR/mock-openai.py" \
  --ready-file "$MOCK_READY" \
  --log-file "$MOCK_LOG" \
  >"$SMOKE_ROOT/mock.stdout.log" 2>"$SMOKE_ROOT/mock.stderr.log" &
MOCK_PID=$!
printf '%s\n' "$MOCK_PID" > "$SMOKE_ROOT/mock.pid"
for ((attempt = 0; attempt < 100; attempt++)); do
  [[ -s "$MOCK_READY" ]] && break
  kill -0 "$MOCK_PID" >/dev/null 2>&1 || {
    cat "$SMOKE_ROOT/mock.stderr.log" >&2
    fail "loopback mock exited before becoming ready"
  }
  sleep 0.1
done
[[ -s "$MOCK_READY" ]] || fail "loopback mock did not become ready"
MOCK_PORT="$(tr -d '[:space:]' < "$MOCK_READY")"
[[ "$MOCK_PORT" =~ ^[0-9]+$ ]] || fail "loopback mock returned an invalid port"
MOCK_URL="http://127.0.0.1:$MOCK_PORT"

export ALEX_HOME="$STATE_DIR"
export ALEX_APP_DEST="$APP_DEST"
export ALEX_BIN_DEST="$BIN_DEST"
"$SCRIPT_DIR/install.sh" --no-open

[[ -d "$APP_DEST" ]] || fail "installer did not copy Alex.app"
[[ -x "$ALEX_BIN" ]] || fail "installer did not copy the alex binary"
cmp "$SCRIPT_DIR/bin/alex" "$ALEX_BIN" \
  || fail "installed alex binary differs from the packaged binary"
codesign --verify --deep --strict "$APP_DEST"

PLIST_PROGRAM="$(plutil -extract ProgramArguments.0 raw -o - "$SERVICE_PLIST")"
PLIST_HOME="$(plutil -extract EnvironmentVariables.ALEX_HOME raw -o - "$SERVICE_PLIST")"
[[ "$PLIST_PROGRAM" == "$ALEX_BIN" ]] \
  || fail "launchd is not pinned to the packaged binary: $PLIST_PROGRAM"
[[ "$PLIST_HOME" == "$STATE_DIR" ]] \
  || fail "launchd did not retain the isolated Alex home"

wait_for_health() {
  for ((attempt = 0; attempt < 150; attempt++)); do
    curl -fsS --max-time 1 "$BASE_URL/health" >/dev/null 2>&1 && return 0
    sleep 0.1
  done
  return 1
}
wait_for_health || fail "installed launchd daemon did not become healthy"

PATH="$BIN_DEST:$PATH" "$ALEX_BIN" status --json > "$SMOKE_ROOT/status-before.json"
jq -e --arg binary "$BIN_DEST/alex" '
  .daemon.running == true
  and .daemon.service.managed == true
  and any(.daemon.binaries[]; .path == $binary)
' "$SMOKE_ROOT/status-before.json" >/dev/null \
  || fail "public status API does not report the installed managed binary"

"$ALEX_BIN" credentials --json > "$SMOKE_ROOT/credentials.json"
LOCAL_KEY="$(jq -er '.openai.env.OPENAI_API_KEY' "$SMOKE_ROOT/credentials.json")"
[[ "$LOCAL_KEY" == alx-* ]] || fail "credentials command did not return a local key"

jq -nc --arg url "$MOCK_URL" --arg model "$MODEL" \
  '{url:$url,enabled_models:[$model]}' > "$SMOKE_ROOT/exo-config.json"
curl -fsS --max-time 5 \
  -X PUT \
  -H "x-api-key: $LOCAL_KEY" \
  -H 'content-type: application/json' \
  --data-binary @"$SMOKE_ROOT/exo-config.json" \
  "$BASE_URL/admin/exo" > "$SMOKE_ROOT/exo-config-response.json"
jq -e --arg url "$MOCK_URL" --arg model "$MODEL" \
  '.url == $url and .enabled_models == [$model]' \
  "$SMOKE_ROOT/exo-config-response.json" >/dev/null \
  || fail "daemon did not persist the loopback Exo route"

curl -fsS --max-time 5 \
  -H "Authorization: Bearer $LOCAL_KEY" \
  "$BASE_URL/v1/models" > "$SMOKE_ROOT/models.json"
jq -e --arg model "alex/$MODEL" 'any(.data[]; .id == $model)' \
  "$SMOKE_ROOT/models.json" >/dev/null \
  || fail "enabled loopback model was not published"

jq -nc --arg model "exo/$MODEL" '
  {model:$model,stream:false,messages:[{role:"user",content:"installed smoke"}]}
' > "$SMOKE_ROOT/request.json"
curl -fsS --max-time 10 \
  -H "Authorization: Bearer $LOCAL_KEY" \
  -H 'content-type: application/json' \
  -H 'x-alex-harness: clean-machine-ci' \
  -H "x-session-id: $SESSION_ID" \
  --data-binary @"$SMOKE_ROOT/request.json" \
  "$BASE_URL/v1/chat/completions" > "$SMOKE_ROOT/response.json"
jq -e '
  .id == "chatcmpl-ci-installed-smoke"
  and .choices[0].message.content == "installed route ok"
' "$SMOKE_ROOT/response.json" >/dev/null \
  || fail "routed response did not come from the deterministic loopback mock"
jq -se --arg model "$MODEL" '
  any(.[]; .event == "chat" and .path == "/v1/chat/completions"
    and .model == $model and .authorized == true and .stream == false)
' "$MOCK_LOG" >/dev/null \
  || fail "loopback mock did not observe the normalized Exo request"

TRACE_ID=""
for ((attempt = 0; attempt < 100; attempt++)); do
  curl -fsS --max-time 2 \
    -H "x-api-key: $LOCAL_KEY" \
    "$BASE_URL/admin/traces?session=$SESSION_ID&limit=10" \
    > "$SMOKE_ROOT/traces.json"
  TRACE_ID="$(jq -r --arg session "$SESSION_ID" '
    [.traces[] | select(.session_id == $session and .status == 200
      and .upstream_provider == "exo")][0].id // empty
  ' "$SMOKE_ROOT/traces.json")"
  [[ -n "$TRACE_ID" ]] && break
  sleep 0.1
done
[[ -n "$TRACE_ID" ]] || fail "routed request was not written to the trace API"

curl -fsS --max-time 5 \
  -H "x-api-key: $LOCAL_KEY" \
  "$BASE_URL/traces/$TRACE_ID" > "$SMOKE_ROOT/trace-before.json"
jq -e --arg id "$TRACE_ID" --arg session "$SESSION_ID" --arg model "$MODEL" '
  .trace.id == $id
  and .trace.session_id == $session
  and .trace.status == 200
  and .trace.upstream_provider == "exo"
  and .trace.requested_model == ("exo/" + $model)
  and .trace.routed_model == $model
  and .trace.harness == "clean-machine-ci"
' "$SMOKE_ROOT/trace-before.json" >/dev/null \
  || fail "trace detail does not describe the routed installed-binary request"
jq -S '{id:.trace.id,session_id:.trace.session_id,status:.trace.status,
  provider:.trace.upstream_provider,requested_model:.trace.requested_model,
  routed_model:.trace.routed_model,harness:.trace.harness}' \
  "$SMOKE_ROOT/trace-before.json" > "$SMOKE_ROOT/trace-before-canonical.json"
curl -fsS --max-time 5 \
  -H "x-api-key: $LOCAL_KEY" \
  "$BASE_URL/traces/$TRACE_ID/body/response" > "$SMOKE_ROOT/body-before.json"
jq -e '.choices[0].message.content == "installed route ok"' \
  "$SMOKE_ROOT/body-before.json" >/dev/null \
  || fail "persisted trace response body is not readable before restart"

service_pid() {
  launchctl print "$SERVICE_TARGET" 2>/dev/null \
    | awk '$1 == "pid" && $2 == "=" { print $3; exit }'
}
PID_BEFORE="$(service_pid)"
[[ "$PID_BEFORE" =~ ^[0-9]+$ ]] || fail "launchd did not report the installed daemon PID"
"$ALEX_BIN" service restart --force

PID_AFTER=""
for ((attempt = 0; attempt < 150; attempt++)); do
  candidate="$(service_pid)"
  if [[ "$candidate" =~ ^[0-9]+$ && "$candidate" != "$PID_BEFORE" ]] \
    && curl -fsS --max-time 1 "$BASE_URL/health" >/dev/null 2>&1; then
    PID_AFTER="$candidate"
    break
  fi
  sleep 0.1
done
[[ -n "$PID_AFTER" ]] \
  || fail "launchd hard restart did not replace daemon PID $PID_BEFORE"

curl -fsS --max-time 5 \
  -H "x-api-key: $LOCAL_KEY" \
  "$BASE_URL/traces/$TRACE_ID" > "$SMOKE_ROOT/trace-after.json"
jq -S '{id:.trace.id,session_id:.trace.session_id,status:.trace.status,
  provider:.trace.upstream_provider,requested_model:.trace.requested_model,
  routed_model:.trace.routed_model,harness:.trace.harness}' \
  "$SMOKE_ROOT/trace-after.json" > "$SMOKE_ROOT/trace-after-canonical.json"
cmp "$SMOKE_ROOT/trace-before-canonical.json" "$SMOKE_ROOT/trace-after-canonical.json" \
  || fail "trace metadata changed or disappeared across the launchd restart"
curl -fsS --max-time 5 \
  -H "x-api-key: $LOCAL_KEY" \
  "$BASE_URL/traces/$TRACE_ID/body/response" > "$SMOKE_ROOT/body-after.json"
cmp "$SMOKE_ROOT/body-before.json" "$SMOKE_ROOT/body-after.json" \
  || fail "trace body changed or disappeared across the launchd restart"

jq -n \
  --arg trace_id "$TRACE_ID" \
  --arg session_id "$SESSION_ID" \
  --arg model "exo/$MODEL" \
  --arg provider "exo" \
  --arg response "installed route ok" \
  --argjson pid_before "$PID_BEFORE" \
  --argjson pid_after "$PID_AFTER" \
  '{schema_version:1,passed:true,install:{app:true,packaged_binary:true,
    launchd_managed:true},route:{provider:$provider,model:$model,
    loopback_mock:true,response:$response},trace:{id:$trace_id,
    session_id:$session_id,persisted_across_restart:true},
    launchd:{pid_before:$pid_before,pid_after:$pid_after,replaced:true},
    external_provider_network:false}' > "$RESULT_PATH"

echo "installed macOS smoke passed: trace $TRACE_ID survived launchd PID $PID_BEFORE -> $PID_AFTER"
