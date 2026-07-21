#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
INSTALLER="$REPO_ROOT/install-release.sh"
OPENAI_MOCK="$REPO_ROOT/packaging/ci-macos/mock-openai.py"
SERVICE_NAME="alexandria"
SERVICE_UNIT="$HOME/.config/systemd/user/alexandria.service"
STATE_DIR="$HOME/.alexandria"
BASE_URL="http://127.0.0.1:4100"
SESSION_ID="ci-linux-installed-smoke"
MODEL="ci-smoke-model"

fail() {
  echo "installed Linux smoke: $*" >&2
  exit 1
}

for command in curl jq python3 systemctl tar; do
  command -v "$command" >/dev/null 2>&1 || fail "missing required command: $command"
done
[[ "$(uname -s)" == "Linux" ]] || fail "candidate gate requires Linux"
[[ "$(uname -m)" == "x86_64" ]] || fail "candidate gate requires x86_64"
# shellcheck disable=SC1091
. /etc/os-release
[[ "${ID:-}" == "ubuntu" ]] || fail "candidate gate requires Ubuntu, found ${ID:-unknown}"
systemctl --user show-environment >/dev/null \
  || fail "systemd user manager is unavailable on this runner"

SMOKE_ROOT="${ALEX_CI_LINUX_SMOKE_ROOT:?set ALEX_CI_LINUX_SMOKE_ROOT}"
[[ "$SMOKE_ROOT" = /*/alex-installed-linux ]] \
  || fail "ALEX_CI_LINUX_SMOKE_ROOT must be absolute and end in /alex-installed-linux"
RESULT_PATH="${ALEX_CI_LINUX_SMOKE_RESULT:?set ALEX_CI_LINUX_SMOKE_RESULT outside the smoke root}"
VERSION="${ALEX_CI_LINUX_VERSION:?set ALEX_CI_LINUX_VERSION}"
ASSET_DIR="${ALEX_CI_LINUX_ASSET_DIR:?set ALEX_CI_LINUX_ASSET_DIR}"
INSTALL_DIR="$SMOKE_ROOT/bin"
ALEX_BIN="$INSTALL_DIR/alex"
ASSET_NAME="alex-cli-$VERSION-linux-x86_64.tar.gz"
ASSET_PID=""
MOCK_PID=""
ROOT_OWNED=0
SERVICE_CLEANUP_ALLOWED=0

case "$RESULT_PATH" in
  "$SMOKE_ROOT"|"$SMOKE_ROOT"/*)
    fail "ALEX_CI_LINUX_SMOKE_RESULT must persist outside the smoke root"
    ;;
esac
[[ -x "$INSTALLER" ]] || fail "release installer is not executable"
[[ -f "$ASSET_DIR/$ASSET_NAME" ]] || fail "candidate archive is missing"
[[ -f "$ASSET_DIR/$ASSET_NAME.sha256" ]] || fail "candidate checksum is missing"
[[ -f "$OPENAI_MOCK" ]] || fail "shared deterministic OpenAI mock is missing"

cleanup() {
  status=$?
  trap - EXIT INT TERM
  set +e
  cleanup_ok=1
  if [[ "$SERVICE_CLEANUP_ALLOWED" -eq 1 ]]; then
    if [[ -x "$ALEX_BIN" ]]; then
      "$ALEX_BIN" service uninstall >/dev/null 2>&1
    fi
    systemctl --user disable --now "$SERVICE_NAME" >/dev/null 2>&1
    rm -f "$SERVICE_UNIT"
    systemctl --user daemon-reload >/dev/null 2>&1
    systemctl --user reset-failed "$SERVICE_NAME" >/dev/null 2>&1
    rm -rf "$STATE_DIR"
    if systemctl --user is-active --quiet "$SERVICE_NAME" \
      || [[ -e "$SERVICE_UNIT" ]] \
      || [[ -e "$STATE_DIR" ]]; then
      echo "installed Linux smoke: service cleanup was incomplete; preserving $SMOKE_ROOT" >&2
      cleanup_ok=0
    fi
  fi
  for pid in "$MOCK_PID" "$ASSET_PID"; do
    if [[ -n "$pid" ]]; then
      kill "$pid" >/dev/null 2>&1
      wait "$pid" >/dev/null 2>&1
    fi
  done
  if [[ "$ROOT_OWNED" -eq 1 && "$cleanup_ok" -eq 1 ]]; then
    rm -rf "$SMOKE_ROOT"
  fi
  if [[ "$status" -eq 0 && "$cleanup_ok" -ne 1 ]]; then
    status=1
  fi
  exit "$status"
}
trap cleanup EXIT INT TERM

if systemctl --user is-active --quiet "$SERVICE_NAME"; then
  fail "runner is not clean: $SERVICE_NAME user service is already active"
fi
[[ ! -e "$SERVICE_UNIT" ]] || fail "runner is not clean: $SERVICE_UNIT already exists"
[[ ! -e "$STATE_DIR" ]] || fail "runner is not clean: $STATE_DIR already exists"

rm -rf "$SMOKE_ROOT"
mkdir -p "$SMOKE_ROOT" "$(dirname "$RESULT_PATH")"
ROOT_OWNED=1
SERVICE_CLEANUP_ALLOWED=1
touch "$SMOKE_ROOT/service-cleanup-allowed"

ASSET_READY="$SMOKE_ROOT/assets.port"
ASSET_LOG="$SMOKE_ROOT/assets.ndjson"
python3 "$SCRIPT_DIR/serve-assets.py" \
  --directory "$ASSET_DIR" \
  --ready-file "$ASSET_READY" \
  --log-file "$ASSET_LOG" \
  >"$SMOKE_ROOT/assets.stdout.log" 2>"$SMOKE_ROOT/assets.stderr.log" &
ASSET_PID=$!
printf '%s\n' "$ASSET_PID" > "$SMOKE_ROOT/assets.pid"

MOCK_READY="$SMOKE_ROOT/mock.port"
MOCK_LOG="$SMOKE_ROOT/mock.ndjson"
python3 "$OPENAI_MOCK" \
  --ready-file "$MOCK_READY" \
  --log-file "$MOCK_LOG" \
  >"$SMOKE_ROOT/mock.stdout.log" 2>"$SMOKE_ROOT/mock.stderr.log" &
MOCK_PID=$!
printf '%s\n' "$MOCK_PID" > "$SMOKE_ROOT/mock.pid"

for ready in "$ASSET_READY:$ASSET_PID:$SMOKE_ROOT/assets.stderr.log" \
             "$MOCK_READY:$MOCK_PID:$SMOKE_ROOT/mock.stderr.log"; do
  IFS=: read -r ready_file server_pid error_file <<< "$ready"
  for ((attempt = 0; attempt < 100; attempt++)); do
    [[ -s "$ready_file" ]] && break
    kill -0 "$server_pid" >/dev/null 2>&1 || {
      cat "$error_file" >&2
      fail "loopback server exited before becoming ready"
    }
    sleep 0.1
  done
  [[ -s "$ready_file" ]] || fail "loopback server did not become ready"
done

ASSET_PORT="$(tr -d '[:space:]' < "$ASSET_READY")"
MOCK_PORT="$(tr -d '[:space:]' < "$MOCK_READY")"
[[ "$ASSET_PORT" =~ ^[0-9]+$ ]] || fail "asset server returned an invalid port"
[[ "$MOCK_PORT" =~ ^[0-9]+$ ]] || fail "OpenAI mock returned an invalid port"
ASSET_URL="http://127.0.0.1:$ASSET_PORT"
MOCK_URL="http://127.0.0.1:$MOCK_PORT"

ALEX_VERSION="$VERSION" \
ALEX_ASSET_BASE_URL="$ASSET_URL" \
ALEX_INSTALL_DIR="$INSTALL_DIR" \
  "$INSTALLER"

[[ -x "$ALEX_BIN" ]] || fail "release installer did not install alex"
[[ -x "$INSTALL_DIR/alexandria" ]] || fail "release installer did not install alexandria"
"$ALEX_BIN" --version | grep -F "$VERSION" >/dev/null \
  || fail "installed binary does not report candidate version $VERSION"
jq -s -e --arg archive "/$ASSET_NAME" --arg checksum "/$ASSET_NAME.sha256" '
  any(.[]; .method == "GET" and .path == $archive)
  and any(.[]; .method == "GET" and .path == $checksum)
' "$ASSET_LOG" >/dev/null \
  || fail "installer did not fetch both candidate archive and checksum from loopback"

grep -Fqx "ExecStart=$ALEX_BIN daemon" "$SERVICE_UNIT" \
  || fail "systemd unit is not pinned to the installed candidate binary"
systemctl --user is-active --quiet "$SERVICE_NAME" \
  || fail "installed systemd user service is not active"

wait_for_health() {
  for ((attempt = 0; attempt < 150; attempt++)); do
    curl -fsS --max-time 1 "$BASE_URL/health" >/dev/null 2>&1 && return 0
    sleep 0.1
  done
  return 1
}
wait_for_health || fail "installed systemd daemon did not become healthy"

PATH="$INSTALL_DIR:$PATH" "$ALEX_BIN" status --json > "$SMOKE_ROOT/status-before.json"
jq -e --arg binary "$INSTALL_DIR/alexandria" '
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

jq -nc --arg model "exo/$MODEL" '
  {model:$model,stream:false,messages:[{role:"user",content:"installed Linux smoke"}]}
' > "$SMOKE_ROOT/request.json"
curl -fsS --max-time 10 \
  -H "Authorization: Bearer $LOCAL_KEY" \
  -H 'content-type: application/json' \
  -H 'x-alexandria-harness: clean-machine-ci-linux' \
  -H "x-session-id: $SESSION_ID" \
  --data-binary @"$SMOKE_ROOT/request.json" \
  "$BASE_URL/v1/chat/completions" > "$SMOKE_ROOT/response.json"
jq -e '.choices[0].message.content == "installed route ok"' \
  "$SMOKE_ROOT/response.json" >/dev/null \
  || fail "routed response did not come from the deterministic loopback mock"
jq -s -e --arg model "$MODEL" '
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

curl -fsS --max-time 5 -H "x-api-key: $LOCAL_KEY" \
  "$BASE_URL/traces/$TRACE_ID" > "$SMOKE_ROOT/trace-before.json"
jq -e --arg id "$TRACE_ID" --arg session "$SESSION_ID" --arg model "$MODEL" '
  .trace.id == $id
  and .trace.session_id == $session
  and .trace.status == 200
  and .trace.upstream_provider == "exo"
  and .trace.requested_model == ("exo/" + $model)
  and .trace.routed_model == $model
  and .trace.harness == "clean-machine-ci-linux"
' "$SMOKE_ROOT/trace-before.json" >/dev/null \
  || fail "trace detail does not describe the installed Linux route"
jq -S '{id:.trace.id,session_id:.trace.session_id,status:.trace.status,
  provider:.trace.upstream_provider,requested_model:.trace.requested_model,
  routed_model:.trace.routed_model,harness:.trace.harness}' \
  "$SMOKE_ROOT/trace-before.json" > "$SMOKE_ROOT/trace-before-canonical.json"
curl -fsS --max-time 5 -H "x-api-key: $LOCAL_KEY" \
  "$BASE_URL/traces/$TRACE_ID/body/response" > "$SMOKE_ROOT/body-before.json"
jq -e '.choices[0].message.content == "installed route ok"' \
  "$SMOKE_ROOT/body-before.json" >/dev/null \
  || fail "trace response body is not readable before restart"

service_pid() {
  systemctl --user show --property MainPID --value "$SERVICE_NAME" 2>/dev/null
}
PID_BEFORE="$(service_pid)"
[[ "$PID_BEFORE" =~ ^[1-9][0-9]*$ ]] || fail "systemd did not report the daemon PID"
"$ALEX_BIN" service restart

PID_AFTER=""
for ((attempt = 0; attempt < 150; attempt++)); do
  candidate="$(service_pid)"
  if [[ "$candidate" =~ ^[1-9][0-9]*$ && "$candidate" != "$PID_BEFORE" ]] \
    && systemctl --user is-active --quiet "$SERVICE_NAME" \
    && curl -fsS --max-time 1 "$BASE_URL/health" >/dev/null 2>&1; then
    PID_AFTER="$candidate"
    break
  fi
  sleep 0.1
done
[[ -n "$PID_AFTER" ]] || fail "systemd restart did not replace daemon PID $PID_BEFORE"

curl -fsS --max-time 5 -H "x-api-key: $LOCAL_KEY" \
  "$BASE_URL/traces/$TRACE_ID" > "$SMOKE_ROOT/trace-after.json"
jq -S '{id:.trace.id,session_id:.trace.session_id,status:.trace.status,
  provider:.trace.upstream_provider,requested_model:.trace.requested_model,
  routed_model:.trace.routed_model,harness:.trace.harness}' \
  "$SMOKE_ROOT/trace-after.json" > "$SMOKE_ROOT/trace-after-canonical.json"
cmp "$SMOKE_ROOT/trace-before-canonical.json" "$SMOKE_ROOT/trace-after-canonical.json" \
  || fail "trace metadata changed or disappeared across the systemd restart"
curl -fsS --max-time 5 -H "x-api-key: $LOCAL_KEY" \
  "$BASE_URL/traces/$TRACE_ID/body/response" > "$SMOKE_ROOT/body-after.json"
cmp "$SMOKE_ROOT/body-before.json" "$SMOKE_ROOT/body-after.json" \
  || fail "trace body changed or disappeared across the systemd restart"

jq -n \
  --arg version "$VERSION" \
  --arg trace_id "$TRACE_ID" \
  --arg session_id "$SESSION_ID" \
  --arg model "exo/$MODEL" \
  --argjson pid_before "$PID_BEFORE" \
  --argjson pid_after "$PID_AFTER" \
  '{schema_version:1,passed:true,platform:{os:"ubuntu",arch:"x86_64"},
    package:{version:$version,archive_checksum_verified:true,
    installed_binary:true},service:{manager:"systemd-user",managed:true,
    pid_before:$pid_before,pid_after:$pid_after,replaced:true},
    route:{provider:"exo",model:$model,loopback_mock:true,
    response:"installed route ok"},trace:{id:$trace_id,session_id:$session_id,
    persisted_across_restart:true},external_provider_network:false}' > "$RESULT_PATH"

echo "installed Linux smoke passed: trace $TRACE_ID survived systemd PID $PID_BEFORE -> $PID_AFTER"
