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
MODEL="ci-upgrade-model"

fail() {
  echo "Linux upgrade/rollback smoke: $*" >&2
  exit 1
}

for command in awk cmp curl jq python3 readlink sha256sum stat systemctl tar; do
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
CANDIDATE_VERSION="${ALEX_CI_LINUX_VERSION:?set ALEX_CI_LINUX_VERSION}"
BASE_VERSION="${ALEX_CI_LINUX_BASE_VERSION:?set ALEX_CI_LINUX_BASE_VERSION}"
BASE_SHA="${ALEX_CI_LINUX_BASE_SHA:?set ALEX_CI_LINUX_BASE_SHA}"
CANDIDATE_SHA="${ALEX_CI_LINUX_CANDIDATE_SHA:?set ALEX_CI_LINUX_CANDIDATE_SHA}"
ASSET_DIR="${ALEX_CI_LINUX_ASSET_DIR:?set ALEX_CI_LINUX_ASSET_DIR}"
[[ "$BASE_VERSION" != "$CANDIDATE_VERSION" ]] || fail "base and candidate versions must differ"

INSTALL_DIR="$SMOKE_ROOT/bin"
ALEX_BIN="$INSTALL_DIR/alex"
BASE_ASSET="alex-cli-$BASE_VERSION-linux-x86_64.tar.gz"
CANDIDATE_ASSET="alex-cli-$CANDIDATE_VERSION-linux-x86_64.tar.gz"
ASSET_PID=""
MOCK_PID=""
ROOT_OWNED=0
SERVICE_CLEANUP_ALLOWED=0
CURRENT_PID=""
LAST_TRACE_ID=""

case "$RESULT_PATH" in
  "$SMOKE_ROOT"|"$SMOKE_ROOT"/*)
    fail "ALEX_CI_LINUX_SMOKE_RESULT must persist outside the smoke root"
    ;;
esac
[[ -x "$INSTALLER" ]] || fail "release installer is not executable"
[[ -x "$OPENAI_MOCK" ]] || fail "shared deterministic OpenAI mock is missing"
for asset in "$BASE_ASSET" "$CANDIDATE_ASSET"; do
  [[ -f "$ASSET_DIR/$asset" ]] || fail "local asset is missing: $asset"
  [[ -f "$ASSET_DIR/$asset.sha256" ]] || fail "local checksum is missing: $asset.sha256"
done

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
      echo "Linux upgrade/rollback smoke: service cleanup was incomplete; preserving $SMOKE_ROOT" >&2
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

wait_for_health() {
  for ((attempt = 0; attempt < 150; attempt++)); do
    curl -fsS --max-time 1 "$BASE_URL/health" >/dev/null 2>&1 && return 0
    sleep 0.1
  done
  return 1
}

service_pid() {
  systemctl --user show --property MainPID --value "$SERVICE_NAME" 2>/dev/null
}

install_version() {
  local phase="$1"
  local version="$2"
  local version_output
  if ! ALEX_VERSION="$version" \
    ALEX_ASSET_BASE_URL="$ASSET_URL" \
    ALEX_INSTALL_DIR="$INSTALL_DIR" \
      "$INSTALLER" >"$SMOKE_ROOT/install-$phase.log" 2>&1; then
    cat "$SMOKE_ROOT/install-$phase.log" >&2
    fail "$phase installer failed for Alex $version"
  fi
  version_output="$($ALEX_BIN --version)"
  if [[ "$phase" == "candidate" ]]; then
    [[ "$version_output" == "alex $version" ]] \
      || fail "$phase did not install Alex $version (reported: $version_output)"
  else
    # The PR-base binary predates the user-visible Alexandria-to-Alex rename.
    # A pinned rollback must verify its exact version without rejecting that
    # known legacy executable label.
    [[ "$version_output" == "alex $version" \
      || "$version_output" == "alexandria $version" ]] \
      || fail "$phase did not install version $version (reported: $version_output)"
  fi
}

assert_service_version() {
  local phase="$1"
  local previous_pid="${2:-}"
  local process_inode
  local installed_inode
  wait_for_health || fail "$phase service did not become healthy"
  systemctl --user is-active --quiet "$SERVICE_NAME" \
    || fail "$phase service is not active"
  grep -Fqx "ExecStart=$ALEX_BIN daemon" "$SERVICE_UNIT" \
    || fail "$phase service unit is not pinned to the installed binary"
  CURRENT_PID="$(service_pid)"
  [[ "$CURRENT_PID" =~ ^[1-9][0-9]*$ ]] || fail "$phase service PID is invalid"
  if [[ -n "$previous_pid" && "$CURRENT_PID" == "$previous_pid" ]]; then
    fail "$phase did not replace service PID $previous_pid"
  fi
  process_inode="$(stat -Lc '%d:%i' "/proc/$CURRENT_PID/exe")"
  installed_inode="$(stat -Lc '%d:%i' "$ALEX_BIN")"
  [[ "$process_inode" == "$installed_inode" ]] \
    || fail "$phase service is still running an older executable inode"
  [[ "$(readlink "/proc/$CURRENT_PID/exe")" == "$ALEX_BIN" ]] \
    || fail "$phase service executable path is stale"
}

credentials_key() {
  "$ALEX_BIN" credentials --json | jq -er '.openai.env.OPENAI_API_KEY'
}

assert_route_config() {
  local phase="$1"
  curl -fsS --max-time 5 -H "x-api-key: $LOCAL_KEY" \
    "$BASE_URL/admin/exo" > "$SMOKE_ROOT/exo-$phase.json"
  jq -e --arg url "$MOCK_URL" --arg model "$MODEL" \
    '.url == $url and .enabled_models == [$model]' \
    "$SMOKE_ROOT/exo-$phase.json" >/dev/null \
    || fail "$phase did not preserve the Exo route"
}

capture_trace() {
  local trace_id="$1"
  local prefix="$2"
  curl -fsS --max-time 5 -H "x-api-key: $LOCAL_KEY" \
    "$BASE_URL/traces/$trace_id" > "$SMOKE_ROOT/$prefix-detail.json"
  jq -S '{id:.trace.id,session_id:.trace.session_id,status:.trace.status,
    provider:.trace.upstream_provider,requested_model:.trace.requested_model,
    routed_model:.trace.routed_model,harness:.trace.harness}' \
    "$SMOKE_ROOT/$prefix-detail.json" > "$SMOKE_ROOT/$prefix-canonical.json"
  curl -fsS --max-time 5 -H "x-api-key: $LOCAL_KEY" \
    "$BASE_URL/traces/$trace_id/body/response" > "$SMOKE_ROOT/$prefix-body.json"
}

assert_trace_unchanged() {
  local trace_id="$1"
  local prefix="$2"
  local phase="$3"
  capture_trace "$trace_id" "$prefix-$phase"
  cmp "$SMOKE_ROOT/$prefix-canonical.json" \
    "$SMOKE_ROOT/$prefix-$phase-canonical.json" \
    || fail "$prefix trace metadata changed after $phase"
  cmp "$SMOKE_ROOT/$prefix-body.json" "$SMOKE_ROOT/$prefix-$phase-body.json" \
    || fail "$prefix trace body changed after $phase"
}

send_request() {
  local session="$1"
  local prefix="$2"
  jq -nc --arg model "exo/$MODEL" --arg content "$prefix request" \
    '{model:$model,stream:false,messages:[{role:"user",content:$content}]}' \
    > "$SMOKE_ROOT/$prefix-request.json"
  curl -fsS --max-time 10 \
    -H "Authorization: Bearer $LOCAL_KEY" \
    -H 'content-type: application/json' \
    -H 'x-alexandria-harness: upgrade-rollback-ci-linux' \
    -H "x-session-id: $session" \
    --data-binary @"$SMOKE_ROOT/$prefix-request.json" \
    "$BASE_URL/v1/chat/completions" > "$SMOKE_ROOT/$prefix-response.json"
  jq -e '.choices[0].message.content == "installed route ok"' \
    "$SMOKE_ROOT/$prefix-response.json" >/dev/null \
    || fail "$prefix request did not route through the loopback mock"

  LAST_TRACE_ID=""
  for ((attempt = 0; attempt < 100; attempt++)); do
    curl -fsS --max-time 2 -H "x-api-key: $LOCAL_KEY" \
      "$BASE_URL/admin/traces?session=$session&limit=10" \
      > "$SMOKE_ROOT/$prefix-traces.json"
    LAST_TRACE_ID="$(jq -r --arg session "$session" '
      [.traces[] | select(.session_id == $session and .status == 200
        and .upstream_provider == "exo")][0].id // empty
    ' "$SMOKE_ROOT/$prefix-traces.json")"
    [[ -n "$LAST_TRACE_ID" ]] && break
    sleep 0.1
  done
  [[ -n "$LAST_TRACE_ID" ]] || fail "$prefix request did not produce a trace"
  capture_trace "$LAST_TRACE_ID" "$prefix"
  jq -e --arg id "$LAST_TRACE_ID" --arg session "$session" --arg model "$MODEL" '
    .trace.id == $id
    and .trace.session_id == $session
    and .trace.status == 200
    and .trace.upstream_provider == "exo"
    and .trace.requested_model == ("exo/" + $model)
    and .trace.routed_model == $model
    and .trace.harness == "upgrade-rollback-ci-linux"
  ' "$SMOKE_ROOT/$prefix-detail.json" >/dev/null \
    || fail "$prefix trace does not describe the expected routed request"
}

# The PR-base service unit still declares `%h/.npm` as a mandatory writable
# path. Seed that legacy compatibility prerequisite so this exact-delta test
# reaches the upgrade transition; the separate installed-package smoke retains
# responsibility for proving candidate B works with a truly fresh user home.
mkdir -p "$HOME/.npm"

# A: install from the PR base source with a lower synthetic version.
install_version base "$BASE_VERSION"
assert_service_version base
PID_BASE="$CURRENT_PID"
LOCAL_KEY="$(credentials_key)"
[[ "$LOCAL_KEY" == alx-* ]] || fail "base install did not create a local key"

jq -nc --arg url "$MOCK_URL" --arg model "$MODEL" \
  '{url:$url,enabled_models:[$model]}' > "$SMOKE_ROOT/exo-config.json"
curl -fsS --max-time 5 -X PUT \
  -H "x-api-key: $LOCAL_KEY" \
  -H 'content-type: application/json' \
  --data-binary @"$SMOKE_ROOT/exo-config.json" \
  "$BASE_URL/admin/exo" > "$SMOKE_ROOT/exo-config-response.json"
assert_route_config base
send_request ci-upgrade-base base
TRACE_BASE="$LAST_TRACE_ID"

# B: install the candidate over A and require the managed process to change.
install_version candidate "$CANDIDATE_VERSION"
assert_service_version candidate "$PID_BASE"
PID_CANDIDATE="$CURRENT_PID"
[[ "$(credentials_key)" == "$LOCAL_KEY" ]] \
  || fail "candidate upgrade changed the local key"
assert_route_config candidate
assert_trace_unchanged "$TRACE_BASE" base candidate
send_request ci-upgrade-candidate candidate
TRACE_CANDIDATE="$LAST_TRACE_ID"

# A again: explicit pinned reinstall is the rollback path. It must run A, not
# merely replace the file under a still-running B process.
install_version rollback "$BASE_VERSION"
assert_service_version rollback "$PID_CANDIDATE"
PID_ROLLBACK="$CURRENT_PID"
[[ "$(credentials_key)" == "$LOCAL_KEY" ]] \
  || fail "rollback changed the local key"
assert_route_config rollback
assert_trace_unchanged "$TRACE_BASE" base rollback
assert_trace_unchanged "$TRACE_CANDIDATE" candidate rollback
send_request ci-upgrade-rollback rollback
TRACE_ROLLBACK="$LAST_TRACE_ID"

# Prove all three installs fetched their archive and checksum from loopback.
jq -s -e \
  --arg base "/$BASE_ASSET" \
  --arg base_sum "/$BASE_ASSET.sha256" \
  --arg candidate "/$CANDIDATE_ASSET" \
  --arg candidate_sum "/$CANDIDATE_ASSET.sha256" '
  ([.[] | select(.method == "GET" and .path == $base)] | length) == 2
  and ([.[] | select(.method == "GET" and .path == $base_sum)] | length) == 2
  and ([.[] | select(.method == "GET" and .path == $candidate)] | length) == 1
  and ([.[] | select(.method == "GET" and .path == $candidate_sum)] | length) == 1
' "$ASSET_LOG" >/dev/null || fail "installer asset evidence is incomplete"

BASE_DIGEST="$(sha256sum "$ASSET_DIR/$BASE_ASSET" | awk '{print $1}')"
CANDIDATE_DIGEST="$(sha256sum "$ASSET_DIR/$CANDIDATE_ASSET" | awk '{print $1}')"

jq -n \
  --arg base_version "$BASE_VERSION" \
  --arg candidate_version "$CANDIDATE_VERSION" \
  --arg base_commit "$BASE_SHA" \
  --arg candidate_commit "$CANDIDATE_SHA" \
  --arg base_sha256 "$BASE_DIGEST" \
  --arg candidate_sha256 "$CANDIDATE_DIGEST" \
  --arg trace_base "$TRACE_BASE" \
  --arg trace_candidate "$TRACE_CANDIDATE" \
  --arg trace_rollback "$TRACE_ROLLBACK" \
  --argjson pid_base "$PID_BASE" \
  --argjson pid_candidate "$PID_CANDIDATE" \
  --argjson pid_rollback "$PID_ROLLBACK" \
  '{schema_version:1,passed:true,platform:{os:"ubuntu",arch:"x86_64"},
    source:{base_commit:$base_commit,candidate_commit:$candidate_commit,
    base_version_kind:"synthetic-lower-semver-from-pr-base",
    base_compatibility_prerequisites:["home-npm-directory"]},
    packages:{assets_local:true,checksums_verified:true,
    base_version:$base_version,candidate_version:$candidate_version,
    rollback_version:$base_version,base_sha256:$base_sha256,
    candidate_sha256:$candidate_sha256},
    transitions:{upgrade:{from:$base_version,to:$candidate_version,
    pid_before:$pid_base,pid_after:$pid_candidate,replaced:true},
    rollback:{from:$candidate_version,to:$base_version,
    pid_before:$pid_candidate,pid_after:$pid_rollback,replaced:true}},
    state:{local_key_preserved:true,route_preserved:true},
    traces:{base:{id:$trace_base,preserved_after_upgrade:true,
    preserved_after_rollback:true},candidate:{id:$trace_candidate,
    preserved_after_rollback:true},rollback:{id:$trace_rollback,readable:true}},
    service:{manager:"systemd-user",installed_inode_executed:true},
    external_asset_network:false,external_provider_network:false}' > "$RESULT_PATH"

echo "Linux upgrade/rollback smoke passed: $BASE_VERSION ($PID_BASE) -> $CANDIDATE_VERSION ($PID_CANDIDATE) -> $BASE_VERSION ($PID_ROLLBACK)"
