#!/usr/bin/env bash
# Alex test suite (TODO.md section 11): ./test.sh [unit|mock|wire|harness|cliproxyapi|dario|all] [flags]
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONFIG_FILE="${ALEX_CONFIG:-$HOME/.alex/config.toml}"
PROMPT="Reply with exactly: alex-test-ok"

TIERS=""
ONLY=""
PROVIDER_FILTER=""
HARNESS_FILTER=""
JOBS=""
JSON=0
TIMEOUT=""
HOST_OVERRIDE=""
PORT_OVERRIDE=""
BASE_OVERRIDE=""
KEY_OVERRIDE=""
ALLOW_UNIMPL=0
STRICT=0

usage() {
  cat <<'EOF'
Usage: ./test.sh [TIER ...] [flags]

Tiers (default: unit wire):
  unit      cargo test --workspace
  mock      credential-free CLI + daemon tier against alex-fakeprov (M1..M6)
  wire      curl-level matrix through the proxy (W1..W12), all cells parallel
  harness   Docker harness matrix (H1..H7), parallel
  cliproxyapi pinned real CLIProxyAPI v7 Docker matrix, both proxy directions
  dario     dario supervisor cells (SKIP cleanly when /admin/dario is absent)
  all       unit + mock + wire + harness + cliproxyapi + dario

Flags:
  --only M1,W1,H2,...     run only these cell ids (UNIT, M*, W*, H*, CLIPROXYAPI, DARIO; W11 matches W11a+W11b)
  --provider P            anthropic|openai|xai - only cells needing provider P
  --harness H             claude|codex|grok-build|kimi - only these harness cells
  --jobs N                max parallel cells (default: CPU count; harness capped at 4)
  --json                  machine-readable report on stdout
  --timeout N             per-cell seconds (default: 120 wire / 600 harness)
  --host H, --port N      daemon overrides (default: ~/.alex/config.toml)
  --base URL              point the whole suite at a (possibly remote) proxy,
                          e.g. http://192.168.1.150:4100 for the Mac's daemon;
                          uses the already-running daemon, does not start one
  --key KEY               admin/local key for that proxy (default: config local_key)
  --allow-unimplemented   proxy HTTP 501 becomes SKIP instead of FAIL
  --strict                run cells even when the provider preflight ping failed
  -h, --help              show this help

Preflight: before wire/harness cells run, one tiny live completion is sent per
needed provider (PING-anthropic, PING-openai, PING-xai rows). A failed ping
SKIPs all dependent cells unless --strict; a missing vault account always SKIPs.
EOF
}

need_val() {
  if [ "$#" -lt 2 ]; then
    echo "missing value for $1" >&2
    exit 2
  fi
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    unit|mock|wire|harness|cliproxyapi|dario|all) TIERS="$TIERS $1" ;;
    --only)        need_val "$@"; ONLY=$2; shift ;;
    --only=*)      ONLY=${1#*=} ;;
    --provider)    need_val "$@"; PROVIDER_FILTER=$2; shift ;;
    --provider=*)  PROVIDER_FILTER=${1#*=} ;;
    --harness)     need_val "$@"; HARNESS_FILTER=$2; shift ;;
    --harness=*)   HARNESS_FILTER=${1#*=} ;;
    --jobs)        need_val "$@"; JOBS=$2; shift ;;
    --jobs=*)      JOBS=${1#*=} ;;
    --timeout)     need_val "$@"; TIMEOUT=$2; shift ;;
    --timeout=*)   TIMEOUT=${1#*=} ;;
    --host)        need_val "$@"; HOST_OVERRIDE=$2; shift ;;
    --host=*)      HOST_OVERRIDE=${1#*=} ;;
    --port)        need_val "$@"; PORT_OVERRIDE=$2; shift ;;
    --port=*)      PORT_OVERRIDE=${1#*=} ;;
    --base)        need_val "$@"; BASE_OVERRIDE=$2; shift ;;
    --base=*)      BASE_OVERRIDE=${1#*=} ;;
    --key)         need_val "$@"; KEY_OVERRIDE=$2; shift ;;
    --key=*)       KEY_OVERRIDE=${1#*=} ;;
    --json)        JSON=1 ;;
    --allow-unimplemented) ALLOW_UNIMPL=1 ;;
    --strict)      STRICT=1 ;;
    -h|--help)     usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
  shift
done

if [ -z "$TIERS" ]; then
  TIERS="unit wire"
fi
case " $TIERS " in *" all "*) TIERS="unit mock wire harness cliproxyapi dario" ;; esac

has_tier() {
  case " $TIERS " in *" $1 "*) return 0 ;; esac
  return 1
}

if [ -n "$PROVIDER_FILTER" ]; then
  case "$PROVIDER_FILTER" in
    anthropic|openai|xai) ;;
    *) echo "--provider must be anthropic|openai|xai" >&2; exit 2 ;;
  esac
fi
if [ -n "$HARNESS_FILTER" ]; then
  case "$HARNESS_FILTER" in
    claude|codex|grok-build|kimi) ;;
    *) echo "--harness must be claude|codex|grok-build|kimi" >&2; exit 2 ;;
  esac
fi
for n in "$JOBS" "$TIMEOUT" "$PORT_OVERRIDE"; do
  case "$n" in ''|[0-9]*) ;; *) echo "numeric flag got non-number: $n" >&2; exit 2 ;; esac
  case "$n" in *[!0-9]*) echo "numeric flag got non-number: $n" >&2; exit 2 ;; esac
done

cfg_str() {
  if [ -f "$CONFIG_FILE" ]; then
    sed -n "s/^$1[[:space:]]*=[[:space:]]*\"\([^\"]*\)\".*/\1/p" "$CONFIG_FILE" | head -1
  fi
}
cfg_num() {
  if [ -f "$CONFIG_FILE" ]; then
    sed -n "s/^$1[[:space:]]*=[[:space:]]*\([0-9][0-9]*\).*/\1/p" "$CONFIG_FILE" | head -1
  fi
}

if [ -n "$BASE_OVERRIDE" ]; then
  # --base http[s]://host:port  -> point the whole suite at a (possibly remote)
  # proxy. Derive HOST/PORT from it for the health checks; the suite will use the
  # already-running daemon rather than starting a local one.
  BASE="${BASE_OVERRIDE%/}"
  rest="${BASE#*://}"
  HOST="${rest%%:*}"
  PORT="${rest##*:}"
  case "$PORT" in *[!0-9]*|"") PORT=$([ "${BASE%%:*}" = https ] && echo 443 || echo 80) ;; esac
else
  HOST=${HOST_OVERRIDE:-$(cfg_str host)}
  HOST=${HOST:-127.0.0.1}
  PORT=${PORT_OVERRIDE:-$(cfg_num port)}
  PORT=${PORT:-4100}
  BASE="http://$HOST:$PORT"
fi
KEY=${KEY_OVERRIDE:-$(cfg_str local_key)}
KEY=${KEY:-}
# Treat an explicit --base, or any non-loopback host, as a remote proxy we must
# not try to spawn locally.
REMOTE=0
if [ -n "$BASE_OVERRIDE" ]; then REMOTE=1; fi
case "$HOST" in 127.0.0.1|localhost|::1|"") ;; *) REMOTE=1 ;; esac

if [ -z "$JOBS" ]; then
  JOBS=$(sysctl -n hw.ncpu 2>/dev/null || getconf _NPROCESSORS_ONLN 2>/dev/null || echo 4)
fi
if [ "$JOBS" -lt 1 ]; then JOBS=1; fi
if [ "$JOBS" -lt 4 ]; then HJOBS=$JOBS; else HJOBS=4; fi

WIRE_TIMEOUT=${TIMEOUT:-120}
HARNESS_TIMEOUT=${TIMEOUT:-600}

TMP=$(mktemp -d "${TMPDIR:-/tmp}/alx-test.XXXXXX")
RESULTS="$TMP/results"
PRE="$TMP/pre"
mkdir -p "$RESULTS" "$PRE"
DAEMON_PID=""
MOCK_DAEMON_PID=""
FAKEPROV_PID=""

cleanup() {
  rc=$?
  if [ -n "$DAEMON_PID" ]; then
    pkill -P "$DAEMON_PID" 2>/dev/null || true
    kill "$DAEMON_PID" 2>/dev/null || true
  fi
  if [ -n "$MOCK_DAEMON_PID" ]; then
    pkill -P "$MOCK_DAEMON_PID" 2>/dev/null || true
    kill "$MOCK_DAEMON_PID" 2>/dev/null || true
  fi
  if [ -n "$FAKEPROV_PID" ]; then
    kill "$FAKEPROV_PID" 2>/dev/null || true
  fi
  rm -rf "$TMP"
  exit "$rc"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

log() { printf '%s\n' "$*" >&2; }
now_ms() { python3 -c 'import time; print(int(time.time()*1000))'; }
oneline() { printf '%s' "$1" | tr '\t\n\r' '   ' | cut -c1-300; }

write_result() {
  printf '%s\t%s\t%s\t%s\n' "$1" "$2" "$3" "$(oneline "$4")" > "$RESULTS/$1.res"
  log "  $1 $2 (${3}ms) $(oneline "$4")"
}

upper() { printf '%s' "$1" | tr '[:lower:]' '[:upper:]'; }

in_only() {
  if [ -z "$ONLY" ]; then return 0; fi
  local id tok IFS=','
  id=$(upper "$1")
  for tok in $ONLY; do
    tok=$(upper "$tok" | tr -d ' ')
    if [ "$tok" = "$id" ]; then return 0; fi
    case "$id" in "$tok"[A-Z]) return 0 ;; esac
  done
  return 1
}

health_ok() { curl -fsS --max-time 3 "$BASE/health" >/dev/null 2>&1; }

ensure_daemon() {
  if health_ok; then
    log "daemon: using running instance at $BASE"
    return 0
  fi
  if [ "$REMOTE" = "1" ]; then
    log "daemon: remote proxy at $BASE is not reachable (health check failed); not starting a local daemon"
    exit 2
  fi
  log "daemon: none at $BASE - starting ./alex daemon (cargo may compile; waiting up to 60s)"
  "$ROOT/alex" daemon --host "$HOST" --port "$PORT" >"$TMP/daemon.log" 2>&1 &
  DAEMON_PID=$!
  local i=0
  while [ "$i" -lt 60 ]; do
    if health_ok; then
      log "daemon: healthy (pid $DAEMON_PID, started by test.sh, will be stopped on exit)"
      return 0
    fi
    if ! kill -0 "$DAEMON_PID" 2>/dev/null; then break; fi
    sleep 1
    i=$((i + 1))
  done
  log "daemon: failed to become healthy; last log lines:"
  tail -n 20 "$TMP/daemon.log" >&2 || true
  exit 2
}

fetch_accounts() {
  curl -sS --max-time 10 -H "x-api-key: $KEY" -o "$TMP/accounts.json" \
    "$BASE/admin/accounts" 2>/dev/null || true
}

account_active() {
  if [ ! -s "$TMP/accounts.json" ]; then return 0; fi
  python3 - "$TMP/accounts.json" "$1" <<'PY'
import json, sys
try:
    accounts = json.load(open(sys.argv[1])).get("accounts", [])
except Exception:
    sys.exit(0)
ok = any(a.get("provider") == sys.argv[2] and a.get("status") == "active" for a in accounts)
sys.exit(0 if ok else 1)
PY
}

DARIO_STATE=""
dario_active() {
  if [ -z "$DARIO_STATE" ]; then
    local code
    code=$(curl -sS --max-time 5 -o "$TMP/dario.json" -w '%{http_code}' \
      -H "x-api-key: $KEY" "$BASE/admin/dario" 2>/dev/null || echo 000)
    DARIO_STATE=inactive
    if [ "$code" = "200" ]; then
      if python3 - "$TMP/dario.json" <<'PY'
import json, sys
try:
    d = json.load(open(sys.argv[1]))
except Exception:
    sys.exit(1)
sys.exit(0 if "active" in json.dumps(d).lower() else 1)
PY
      then DARIO_STATE=active; fi
    fi
  fi
  [ "$DARIO_STATE" = "active" ]
}

build_body() {
  local fmt=$1 model=$2 stream=$3 maxtok=${4:-64}
  case "$fmt" in
    anthropic)
      if [ "$stream" = "1" ]; then
        printf '{"model":"%s","max_tokens":%s,"stream":true,"messages":[{"role":"user","content":"%s"}]}' \
          "$model" "$maxtok" "$PROMPT"
      else
        printf '{"model":"%s","max_tokens":%s,"messages":[{"role":"user","content":"%s"}]}' \
          "$model" "$maxtok" "$PROMPT"
      fi
      ;;
    openai-chat)
      printf '{"model":"%s","messages":[{"role":"user","content":"%s"}]}' "$model" "$PROMPT"
      ;;
    openai-responses)
      local s=false
      if [ "$stream" = "1" ]; then s=true; fi
      printf '{"model":"%s","stream":%s,"store":false,"input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"%s"}]}]}' \
        "$model" "$s" "$PROMPT"
      ;;
    gemini)
      printf '{"contents":[{"role":"user","parts":[{"text":"%s"}]}],"generationConfig":{"maxOutputTokens":%s}}' \
        "$PROMPT" "$maxtok"
      ;;
  esac
}

# fields: id|client_format|endpoint|model|provider|format_prefix|bucket|needs|stream|cross|routed_model|dario
wire_cells() {
  cat <<'EOF'
W1|anthropic|/v1/messages|claude-haiku-4-5|anthropic|anthropic|subscription|anthropic|0|0|claude-haiku-4-5|0
W2|anthropic|/v1/messages|claude-haiku-4-5|anthropic|anthropic|subscription|anthropic|1|0|claude-haiku-4-5|0
W3|openai-responses|/v1/responses|gpt-5.6-luna|openai|openai-responses|subscription|openai|1|0|gpt-5.6-luna|0
W4|openai-responses|/v1/responses|gpt-5.6-luna|openai|openai-responses|subscription|openai|0|0|gpt-5.6-luna|0
W5|openai-chat|/v1/chat/completions|gpt-5.6-luna|openai|openai-|subscription|openai|0|0|gpt-5.6-luna|0
W6|anthropic|/v1/messages|gpt-5.6-luna|openai|openai-|subscription|openai|0|1|gpt-5.6-luna|0
W7|openai-chat|/v1/chat/completions|claude-haiku-4-5|anthropic|anthropic|subscription|anthropic|0|1|claude-haiku-4-5|0
W8|openai-responses|/v1/responses|claude-haiku-4-5|anthropic|anthropic|subscription|anthropic|0|1|claude-haiku-4-5|0
W9|openai-chat|/v1/chat/completions|grok-code-fast-1|xai|openai-|subscription|xai|0|0|grok-code-fast-1|0
W10|anthropic|/v1/messages|claude-haiku-4-5|anthropic|anthropic|subscription|anthropic|0|0|claude-haiku-4-5|1
W11a|openai-chat|/v1/chat/completions|alex/gpt-5.6-luna|openai|openai-|subscription|openai|0|0|gpt-5.6-luna|0
W11b|anthropic|/v1/messages|haiku-4.5|anthropic|anthropic|subscription|anthropic|0|0|claude-haiku-4-5|0
W12|gemini|/v1beta/models/gpt-5.6-luna:generateContent|gpt-5.6-luna|openai|openai-|subscription|openai|0|1|gpt-5.6-luna|0
W13|gemini|/v1beta/models/gpt-5.6-luna:streamGenerateContent?alt=sse|gpt-5.6-luna|openai|openai-|subscription|openai|1|1|gpt-5.6-luna|0
W14|openai-chat|/v1/chat/completions|openrouter/google/gemma-4-26b-a4b-it:free|openrouter|openai-|api|openrouter|0|0|google/gemma-4-26b-a4b-it:free|0
EOF
}

# fields: id|harness|model|needs
harness_cells() {
  cat <<'EOF'
H1|claude|claude-haiku-4-5|anthropic
H2|claude|gpt-5.6-luna|openai
H3|codex|gpt-5.6-luna|openai
H4|codex|claude-haiku-4-5|anthropic
H5|grok-build|gpt-5.6-luna|openai
H6|grok-build|grok-code-fast-1|xai
H7|kimi|claude-fable-5|anthropic
EOF
}

WAIT_PIDS=""
throttle() {
  local max=$1 n alive pid
  while :; do
    n=0
    alive=""
    for pid in $WAIT_PIDS; do
      if kill -0 "$pid" 2>/dev/null; then
        alive="$alive $pid"
        n=$((n + 1))
      fi
    done
    WAIT_PIDS=$alive
    if [ "$n" -lt "$max" ]; then return 0; fi
    sleep 0.2
  done
}
wait_cells() {
  local pid
  for pid in $WAIT_PIDS; do
    wait "$pid" 2>/dev/null || true
  done
  WAIT_PIDS=""
}

run_wire_cell() {
  local id=$1 fmt=$2 ep=$3 model=$4 provider=$5 fprefix=$6 bucket=$7
  local stream=$8 cross=$9 routed=${10} dflag=${11}
  local t0 t1 sess body out code snippet tf i found msg
  t0=$(now_ms)
  sess="tsh-$id-$$-$t0"
  body=$(build_body "$fmt" "$model" "$stream")
  out="$TMP/cell.$id.body"
  code=$(curl -sS --max-time "$WIRE_TIMEOUT" -o "$out" -w '%{http_code}' \
    -H "x-api-key: $KEY" -H "content-type: application/json" -H "x-session-id: $sess" \
    -d "$body" "$BASE$ep" 2>"$TMP/cell.$id.curlerr" || echo 000)
  if [ "$code" = "501" ] && [ "$ALLOW_UNIMPL" = "1" ]; then
    t1=$(now_ms)
    write_result "$id" SKIP "$((t1 - t0))" "unimplemented (http 501)"
    return 0
  fi
  if [ "$code" != "200" ]; then
    if [ "$code" = "000" ]; then
      snippet="curl error/timeout: $(cat "$TMP/cell.$id.curlerr" 2>/dev/null)"
    else
      snippet="http $code: $(head -c 200 "$out" 2>/dev/null)"
    fi
    t1=$(now_ms)
    write_result "$id" FAIL "$((t1 - t0))" "$snippet"
    return 0
  fi
  tf="$TMP/cell.$id.traces.json"
  found=0
  i=0
  while [ "$i" -lt 15 ]; do
    curl -sS --max-time 10 -H "x-api-key: $KEY" -o "$tf" \
      "$BASE/admin/traces?session=$sess&limit=5" 2>/dev/null || true
    if python3 - "$tf" <<'PY'
import json, sys
try:
    sys.exit(0 if json.load(open(sys.argv[1])).get("traces") else 1)
except Exception:
    sys.exit(1)
PY
    then
      found=1
      break
    fi
    sleep 1
    i=$((i + 1))
  done
  if [ "$found" != "1" ]; then
    t1=$(now_ms)
    write_result "$id" FAIL "$((t1 - t0))" "http 200 but no trace row for session $sess after 15s"
    return 0
  fi
  set -- --traces "$tf" --session "$sess" --provider "$provider" \
    --format-prefix "$fprefix" --bucket "$bucket" --routed "$routed" \
    --base "$BASE" --key "$KEY"
  if [ "$cross" = "1" ]; then set -- "$@" --cross; fi
  if [ "$dflag" = "1" ]; then set -- "$@" --expect-dario; fi
  if msg=$(python3 "$ROOT/scripts/test-assert.py" "$@" 2>&1); then
    t1=$(now_ms)
    write_result "$id" PASS "$((t1 - t0))" "$msg"
  else
    t1=$(now_ms)
    write_result "$id" FAIL "$((t1 - t0))" "$msg"
  fi
  return 0
}

preflight_provider() {
  local p=$1 ep body t0 t1 code
  if ! account_active "$p"; then
    printf 'SKIP|0|no active %s account in vault\n' "$p" > "$PRE/$p"
    return 0
  fi
  case "$p" in
    anthropic)  ep="/v1/messages";        body=$(build_body anthropic claude-haiku-4-5 0 16) ;;
    openai)     ep="/v1/responses";       body=$(build_body openai-responses gpt-5.6-luna 0) ;;
    xai)        ep="/v1/chat/completions"; body=$(build_body openai-chat grok-code-fast-1 0) ;;
    openrouter) ep="/v1/chat/completions"; body=$(build_body openai-chat "openrouter/google/gemma-4-26b-a4b-it:free" 0) ;;
    *)
      printf 'SKIP|0|no preflight ping defined for provider %s\n' "$p" > "$PRE/$p"
      return 0
      ;;
  esac
  t0=$(now_ms)
  code=$(curl -sS --max-time "$WIRE_TIMEOUT" -o "$TMP/ping.$p.body" -w '%{http_code}' \
    -H "x-api-key: $KEY" -H "content-type: application/json" \
    -H "x-session-id: tsh-ping-$p-$$-$t0" \
    -d "$body" "$BASE$ep" 2>"$TMP/ping.$p.err" || echo 000)
  t1=$(now_ms)
  case "$code" in
    2*)
      printf 'PASS|%s|http %s\n' "$((t1 - t0))" "$code" > "$PRE/$p"
      ;;
    000)
      printf 'FAIL|%s|curl error/timeout: %s\n' "$((t1 - t0))" \
        "$(oneline "$(cat "$TMP/ping.$p.err" 2>/dev/null)")" > "$PRE/$p"
      ;;
    *)
      printf 'FAIL|%s|http %s: %s\n' "$((t1 - t0))" "$code" \
        "$(oneline "$(head -c 160 "$TMP/ping.$p.body" 2>/dev/null)")" > "$PRE/$p"
      ;;
  esac
  return 0
}

prune_dario_cells() {
  grep -q '|1$' "$TMP/wire.cells" 2>/dev/null || return 0
  if dario_active; then return 0; fi
  local id
  for id in $(grep '|1$' "$TMP/wire.cells" | cut -d'|' -f1); do
    write_result "$id" SKIP 0 "dario unavailable (/admin/dario missing or no active generation)"
  done
  grep -v '|1$' "$TMP/wire.cells" > "$TMP/wire.cells.tmp" || true
  mv "$TMP/wire.cells.tmp" "$TMP/wire.cells"
}

run_preflight() {
  local provs p pids="" st ms msg
  provs=$( { cut -d'|' -f8 "$TMP/wire.cells"; cut -d'|' -f4 "$TMP/harness.cells"; } 2>/dev/null | sort -u | grep -v '^$' || true )
  if [ -z "$provs" ]; then return 0; fi
  # shellcheck disable=SC2086
  log "== preflight: pinging providers:$(printf ' %s' $provs) =="
  for p in $provs; do
    ( set +e; preflight_provider "$p" ) &
    pids="$pids $!"
  done
  for p in $pids; do
    wait "$p" 2>/dev/null || true
  done
  for p in $provs; do
    if [ ! -f "$PRE/$p" ]; then continue; fi
    IFS='|' read -r st ms msg < "$PRE/$p"
    write_result "PING-$p" "$st" "$ms" "$msg"
  done
}

gate_reason() {
  local p=$1 st ms msg
  if [ ! -f "$PRE/$p" ]; then
    echo ""
    return 0
  fi
  IFS='|' read -r st ms msg < "$PRE/$p"
  case "$st" in
    PASS) echo "" ;;
    SKIP) echo "$msg" ;;
    FAIL)
      if [ "$STRICT" = "1" ]; then echo ""; else echo "preflight failed: $msg"; fi
      ;;
    *) echo "" ;;
  esac
}

select_wire() {
  : > "$TMP/wire.cells"
  local id fmt ep model provider fprefix bucket needs stream cross routed dflag
  while IFS='|' read -r id fmt ep model provider fprefix bucket needs stream cross routed dflag; do
    if [ -z "$id" ]; then continue; fi
    in_only "$id" || continue
    if [ -n "$PROVIDER_FILTER" ] && [ "$needs" != "$PROVIDER_FILTER" ]; then continue; fi
    printf '%s|%s|%s|%s|%s|%s|%s|%s|%s|%s|%s|%s\n' \
      "$id" "$fmt" "$ep" "$model" "$provider" "$fprefix" "$bucket" "$needs" \
      "$stream" "$cross" "$routed" "$dflag" >> "$TMP/wire.cells"
  done <<EOF
$(wire_cells)
EOF
}

select_harness() {
  : > "$TMP/harness.cells"
  local docker_ok=1 id h model needs
  if ! docker info >/dev/null 2>&1; then docker_ok=0; fi
  while IFS='|' read -r id h model needs; do
    if [ -z "$id" ]; then continue; fi
    in_only "$id" || continue
    if [ -n "$PROVIDER_FILTER" ] && [ "$needs" != "$PROVIDER_FILTER" ]; then continue; fi
    if [ -n "$HARNESS_FILTER" ] && [ "$h" != "$HARNESS_FILTER" ]; then continue; fi
    if [ "$docker_ok" = "0" ]; then
      write_result "$id" SKIP 0 "docker unavailable (docker info failed)"
      continue
    fi
    printf '%s|%s|%s|%s\n' "$id" "$h" "$model" "$needs" >> "$TMP/harness.cells"
  done <<EOF
$(harness_cells)
EOF
}

run_wire_cells() {
  if [ ! -s "$TMP/wire.cells" ]; then return 0; fi
  log "== wire: $(wc -l < "$TMP/wire.cells" | tr -d ' ') cells, jobs=$JOBS, timeout=${WIRE_TIMEOUT}s =="
  local id fmt ep model provider fprefix bucket needs stream cross routed dflag reason
  while IFS='|' read -r id fmt ep model provider fprefix bucket needs stream cross routed dflag; do
    reason=$(gate_reason "$needs")
    if [ -n "$reason" ]; then
      write_result "$id" SKIP 0 "$reason"
      continue
    fi
    throttle "$JOBS"
    ( set +e; run_wire_cell "$id" "$fmt" "$ep" "$model" "$provider" "$fprefix" "$bucket" "$stream" "$cross" "$routed" "$dflag" ) &
    WAIT_PIDS="$WAIT_PIDS $!"
  done < "$TMP/wire.cells"
  wait_cells
}

run_h7_dario_assertion() {
  local summary=$1 status traces mode session msg code
  status="$TMP/cell.H7.dario.json"
  traces="$TMP/cell.H7.dario.traces.json"
  code=$(curl -sS --max-time 5 -H "x-api-key: $KEY" -o "$status" -w '%{http_code}' \
    "$BASE/admin/dario" 2>/dev/null || echo 000)
  mode=$(python3 - "$status" <<'PY'
import json, sys
try:
    print(json.load(open(sys.argv[1])).get("routing_mode") or "")
except Exception:
    print("")
PY
  )
  if [ "$code" != "200" ] || [ "$mode" != "dario" ]; then
    mode=${mode:-unavailable}
    write_result H7-DARIO SKIP 0 "anthropic_upstream=$mode (Dario generation assertion requires dario mode)"
    return 0
  fi
  session=$(python3 - "$summary" <<'PY'
import json, os, sys
try:
    d = json.load(open(sys.argv[1]))
except Exception:
    d = None
if d is None:
    for line in reversed(open(sys.argv[1]).read().strip().splitlines()):
        try:
            d = json.loads(line)
            break
        except Exception:
            pass
print(os.path.basename((d or {}).get("session_dir") or ""))
PY
  )
  if [ -z "$session" ]; then
    write_result H7-DARIO FAIL 0 "H7 JSON summary did not contain a session directory"
    return 0
  fi
  curl -sS --max-time 10 -H "x-api-key: $KEY" -o "$traces" \
    "$BASE/admin/traces?session=$session&limit=20" 2>/dev/null || true
  if msg=$(python3 - "$traces" "$session" <<'PY' 2>&1
import json, sys
try:
    rows = json.load(open(sys.argv[1])).get("traces", [])
except Exception as e:
    print(f"cannot read H7 traces: {e}")
    sys.exit(1)
rows = [r for r in rows if r.get("session_id") == sys.argv[2] and r.get("upstream_provider") == "anthropic"]
if not rows:
    print("no anthropic-bound trace found for the H7 session")
    sys.exit(1)
missing = [r.get("id") or "<unknown>" for r in rows if not r.get("dario_generation")]
if missing:
    print("anthropic trace(s) missing dario_generation: " + ", ".join(missing))
    sys.exit(1)
generations = sorted({r["dario_generation"] for r in rows})
print(f"anthropic traces={len(rows)} dario_generation={','.join(generations)}")
PY
  ); then
    write_result H7-DARIO PASS 0 "$msg"
  else
    write_result H7-DARIO FAIL 0 "$msg"
  fi
}

run_harness_cell() {
  local id=$1 h=$2 model=$3 t0 t1 rc out msg
  t0=$(now_ms)
  out="$TMP/cell.$id.harness.out"
  rc=0
  set -- "$ROOT/alex" harness run "$h" --model "$model" --json --timeout-secs "$HARNESS_TIMEOUT"
  if [ "$id" = "H7" ]; then
    set -- "$@" --prompt "List the files in the current directory using your file tools, then reply with just the file count."
  fi
  "$@" >"$out" 2>"$TMP/cell.$id.harness.err" || rc=$?
  t1=$(now_ms)
  if [ "$rc" -ne 0 ]; then
    msg="exit $rc: $(tail -c 200 "$TMP/cell.$id.harness.err" 2>/dev/null)"
    write_result "$id" FAIL "$((t1 - t0))" "$msg"
    return 0
  fi
  if msg=$(python3 - "$out" <<'PY' 2>&1
import json, sys
txt = open(sys.argv[1]).read()
d = None
try:
    d = json.loads(txt)
except Exception:
    for ln in reversed(txt.strip().splitlines()):
        ln = ln.strip()
        if ln.startswith("{"):
            try:
                d = json.loads(ln)
                break
            except Exception:
                pass
if d is None:
    print("exit 0 but no JSON summary on stdout")
    sys.exit(1)
cap = (d.get("capture") or {}).get("complete")
if cap is True:
    print("capture.complete=true")
    sys.exit(0)
print("capture.complete=%r" % (cap,))
sys.exit(1)
PY
  ); then
    write_result "$id" PASS "$((t1 - t0))" "$msg"
    if [ "$id" = "H7" ]; then
      run_h7_dario_assertion "$out"
    fi
  else
    write_result "$id" FAIL "$((t1 - t0))" "$msg"
  fi
  return 0
}

run_harness_cells() {
  if [ ! -s "$TMP/harness.cells" ]; then return 0; fi
  log "== harness: $(wc -l < "$TMP/harness.cells" | tr -d ' ') cells, jobs=$HJOBS, timeout=${HARNESS_TIMEOUT}s =="
  local id h model needs reason
  while IFS='|' read -r id h model needs; do
    reason=$(gate_reason "$needs")
    if [ -n "$reason" ]; then
      write_result "$id" SKIP 0 "$reason"
      continue
    fi
    throttle "$HJOBS"
    ( set +e; run_harness_cell "$id" "$h" "$model" ) &
    WAIT_PIDS="$WAIT_PIDS $!"
  done < "$TMP/harness.cells"
  wait_cells
}

dario_field() {
  curl -sS --max-time 5 -H "x-api-key: $KEY" -o "$TMP/dario-field.json" "$BASE/admin/dario" 2>/dev/null || true
  python3 - "$1" "$TMP/dario-field.json" <<'PY'
import json, sys
try:
    d = json.load(open(sys.argv[2]))
except Exception:
    print("")
    sys.exit(0)
active = d.get("active_generation_id") or ""
gen = next((g for g in d.get("generations", []) if g.get("id") == active), {})
key = sys.argv[1]
if key == "gen":
    print(active)
elif key == "pid":
    print(gen.get("pid") or "")
elif key == "phase":
    print(gen.get("phase") or gen.get("state") or "")
elif key == "probe_ok":
    lp = gen.get("last_probe") or {}
    print("1" if lp.get("ok") else "0" if lp else "-")
PY
}

run_dario_tier() {
  in_only DARIO || { run_dario_probe_cell; run_dario_recover_cell; run_dario_cc_direct_cell; run_dario_serve_noncc_cell; return 0; }
  log "== dario =="
  if dario_active; then
    write_result DARIO PASS 0 "active generation reported by /admin/dario"
    if [ ! -f "$RESULTS/W10.res" ] && in_only W10; then
      ( set +e; run_wire_cell W10 anthropic /v1/messages claude-haiku-4-5 anthropic anthropic subscription 0 0 claude-haiku-4-5 1 ) || true
    fi
  else
    write_result DARIO SKIP 0 "dario unavailable (/admin/dario 404 or no active generation)"
  fi
  run_dario_probe_cell
  run_dario_recover_cell
  run_dario_cc_direct_cell
  run_dario_serve_noncc_cell
}

run_dario_cc_direct_cell() {
  in_only DARIO-CC-DIRECT || return 0
  if ! command -v claude >/dev/null 2>&1; then write_result DARIO-CC-DIRECT SKIP 0 "claude is not on host PATH"; return 0; fi
  if ! dario_active; then write_result DARIO-CC-DIRECT SKIP 0 "dario unavailable"; return 0; fi
  local t0 t1 marker all selected sess msg
  t0=$(now_ms); marker="alex-dario-cc-$t0-$$"
  ANTHROPIC_BASE_URL="$BASE" ANTHROPIC_API_KEY="$KEY" claude --model claude-haiku-4-5 -p "$marker" >"$TMP/dario-cc.out" 2>"$TMP/dario-cc.err" || { t1=$(now_ms); write_result DARIO-CC-DIRECT FAIL "$((t1-t0))" "claude failed: $(tail -c 180 "$TMP/dario-cc.err")"; return 0; }
  all="$TMP/dario-cc.all.json"; selected="$TMP/dario-cc.traces.json"
  curl -sS --max-time 10 -H "x-api-key: $KEY" -o "$all" "$BASE/admin/traces?limit=200" 2>/dev/null || true
  sess=$(python3 -c 'import json,sys; rows=json.load(open(sys.argv[1])).get("traces",[]); rows=[r for r in rows if (r.get("harness") or "").startswith("claude-cli/") and (r.get("ts_request_ms") or 0)>=int(sys.argv[3])]; assert rows; r=max(rows,key=lambda x:x.get("ts_request_ms",0)); json.dump({"traces":[r]},open(sys.argv[2],"w")); print(r.get("session_id") or "")' "$all" "$selected" "$t0" 2>/dev/null) || { t1=$(now_ms); write_result DARIO-CC-DIRECT FAIL "$((t1-t0))" "no Claude Code trace after request"; return 0; }
  if [ -z "$sess" ]; then t1=$(now_ms); write_result DARIO-CC-DIRECT FAIL "$((t1-t0))" "Claude Code trace missing session"; return 0; fi
  if msg=$(python3 "$ROOT/scripts/test-assert.py" --traces "$selected" --session "$sess" --base "$BASE" --key "$KEY" --reject-via-dario 2>&1); then t1=$(now_ms); write_result DARIO-CC-DIRECT PASS "$((t1-t0))" "$msg"; else t1=$(now_ms); write_result DARIO-CC-DIRECT FAIL "$((t1-t0))" "$msg"; fi
}

run_dario_serve_noncc_cell() {
  in_only DARIO-SERVE-NONCC || return 0
  if ! dario_active; then write_result DARIO-SERVE-NONCC SKIP 0 "dario unavailable"; return 0; fi
  local model=claude-haiku-4-5 t0 t1 sess i msg code
  t0=$(now_ms)
  curl -sS --max-time "$WIRE_TIMEOUT" -o "$TMP/dario-warm.body" -w '%{http_code}' -H "x-api-key: $KEY" -H 'content-type: application/json' -H "x-session-id: tsh-dario-warm-$$-$t0" -d "$(build_body anthropic "$model" 0 1)" "$BASE/v1/messages" >/dev/null 2>&1 || true
  i=0
  while [ "$i" -lt 40 ]; do
    curl -sS --max-time 5 -H "x-api-key: $KEY" -o "$TMP/dario-caches.json" "$BASE/admin/dario" 2>/dev/null || true
    if python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); assert any(x.get("model")==sys.argv[2] for x in d.get("prompt_caches",[]))' "$TMP/dario-caches.json" "$model" 2>/dev/null; then break; fi
    sleep 1; i=$((i+1))
  done
  if [ "$i" -eq 40 ]; then t1=$(now_ms); write_result DARIO-SERVE-NONCC SKIP "$((t1-t0))" "dario prompt cache did not warm — claude binary reachable?"; return 0; fi
  sess="tsh-dario-serve-$$-$(now_ms)"
  code=$(curl -sS --max-time "$WIRE_TIMEOUT" -o "$TMP/dario-serve.body" -w '%{http_code}' -H "x-api-key: $KEY" -H 'content-type: application/json' -H "x-session-id: $sess" -d "$(build_body anthropic "$model" 0)" "$BASE/v1/messages" 2>/dev/null || echo 000)
  if [ "$code" != 200 ]; then t1=$(now_ms); write_result DARIO-SERVE-NONCC FAIL "$((t1-t0))" "http $code"; return 0; fi
  sleep 1
  curl -sS --max-time 10 -H "x-api-key: $KEY" -o "$TMP/cell.DARIO-SERVE-NONCC.traces.json" "$BASE/admin/traces?session=$sess&limit=5" 2>/dev/null || true
  if msg=$(python3 "$ROOT/scripts/test-assert.py" --traces "$TMP/cell.DARIO-SERVE-NONCC.traces.json" --session "$sess" --base "$BASE" --key "$KEY" --expect-via-dario 2>&1); then t1=$(now_ms); write_result DARIO-SERVE-NONCC PASS "$((t1-t0))" "$msg"; else t1=$(now_ms); write_result DARIO-SERVE-NONCC FAIL "$((t1-t0))" "$msg"; fi
}

run_dario_probe_cell() {
  in_only DARIO-PROBE || return 0
  if ! dario_active; then
    write_result DARIO-PROBE SKIP 0 "dario unavailable"
    return 0
  fi
  local phase probe
  phase=$(dario_field phase)
  probe=$(dario_field probe_ok)
  if [ "$phase" = "ready" ] || { [ "$phase" = "active" ] && [ "$probe" != "0" ]; }; then
    write_result DARIO-PROBE PASS 0 "phase=$phase last_probe_ok=$probe"
  else
    write_result DARIO-PROBE FAIL 0 "phase=$phase last_probe_ok=$probe"
  fi
}

run_dario_recover_cell() {
  in_only DARIO-RECOVER || return 0
  # The pid comes from the daemon's admin JSON; against a remote daemon it
  # belongs to the remote machine, and kill-ing it here would hit an arbitrary
  # LOCAL process.
  if [ "$REMOTE" = "1" ]; then
    write_result DARIO-RECOVER SKIP 0 "remote daemon: refusing to kill a remote-reported pid locally"
    return 0
  fi
  if ! dario_active; then
    write_result DARIO-RECOVER SKIP 0 "dario unavailable"
    return 0
  fi
  local gen0 pid t0 t1 i gen1 phase
  gen0=$(dario_field gen)
  pid=$(dario_field pid)
  if [ -z "$pid" ]; then
    write_result DARIO-RECOVER SKIP 0 "no pid reported for active generation"
    return 0
  fi
  log "  DARIO-RECOVER: kill -9 $pid (generation $gen0), waiting for self-heal"
  t0=$(now_ms)
  kill -9 "$pid" 2>/dev/null || {
    write_result DARIO-RECOVER FAIL 0 "could not kill pid $pid"
    return 0
  }
  i=0
  while [ "$i" -lt 60 ]; do
    sleep 2
    gen1=$(dario_field gen)
    phase=$(dario_field phase)
    if [ -n "$gen1" ] && [ "$gen1" != "$gen0" ] && { [ "$phase" = "ready" ] || [ "$phase" = "active" ]; }; then
      t1=$(now_ms)
      ( set +e; run_wire_cell DARIO-RECOVER anthropic /v1/messages claude-haiku-4-5 anthropic anthropic subscription 0 0 claude-haiku-4-5 1 ) || true
      if grep -q "PASS" "$RESULTS/DARIO-RECOVER.res" 2>/dev/null; then
        write_result DARIO-RECOVER PASS "$((t1 - t0))" "recovered: $gen0 -> $gen1, completion OK after kill -9"
      fi
      return 0
    fi
    i=$((i + 1))
  done
  write_result DARIO-RECOVER FAIL "$(( $(now_ms) - t0 ))" "no new ready generation within 120s after kill -9 (last: gen=$gen1 phase=$phase)"
}

run_unit_tier() {
  in_only UNIT || return 0
  log "== unit: cargo test --workspace =="
  local t0 t1 rc=0
  t0=$(now_ms)
  (cd "$ROOT" && cargo test --workspace) >&2 || rc=$?
  t1=$(now_ms)
  if [ "$rc" -eq 0 ]; then
    write_result UNIT PASS "$((t1 - t0))" "cargo test --workspace"
  else
    write_result UNIT FAIL "$((t1 - t0))" "cargo test --workspace exited $rc"
  fi
}

mock_env() {
  ALEX_HOME="$1" \
  ALEX_UPSTREAM_ANTHROPIC_URL="$2" \
  ALEX_UPSTREAM_OPENAI_URL="$2" \
  ALEX_UPSTREAM_CODEX_URL="$2" \
  ALEX_UPSTREAM_XAI_URL="$2" \
  ALEX_UPSTREAM_GEMINI_URL="$2" \
  ALEX_UPSTREAM_GEMINI_CODE_ASSIST_URL="$2" \
  ALEX_UPSTREAM_OPENROUTER_URL="$2" \
  ALEX_UPSTREAM_KIMI_URL="$2" \
  ALEX_UPSTREAM_AMP_URL="$2" \
  "${@:3}"
}

run_mock_m1() {
  in_only M1 || return 0
  local base=$1 key=$2 out="$TMP/mock-m1.json" t0 t1 code session="mock-m1-$$"
  t0=$(now_ms)
  code=$(curl -sS --max-time "$WIRE_TIMEOUT" -o "$out" -w '%{http_code}' \
    -H "x-api-key: $key" -H 'content-type: application/json' \
    -H "x-session-id: $session" -H 'x-alex-harness: mock-tier' \
    -d '{"model":"claude-sonnet-4-5","max_tokens":64,"messages":[{"role":"user","content":"mock anthropic"}]}' \
    "$base/v1/messages" 2>"$TMP/mock-m1.err" || echo 000)
  t1=$(now_ms)
  if [ "$code" = "200" ] && python3 - "$out" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
assert d["content"][0]["text"] == "Fake Anthropic response."
assert d["usage"]["input_tokens"] == 8 and d["usage"]["output_tokens"] == 4
PY
  then
    write_result M1 PASS "$((t1-t0))" "anthropic completion returned expected text and usage"
  else
    write_result M1 FAIL "$((t1-t0))" "http $code: $(head -c 180 "$out" 2>/dev/null)"
  fi
}

run_mock_m2() {
  in_only M2 || return 0
  local base=$1 key=$2 out="$TMP/mock-m2.json" t0 t1 code session="mock-m2-$$"
  t0=$(now_ms)
  code=$(curl -sS --max-time "$WIRE_TIMEOUT" -o "$out" -w '%{http_code}' \
    -H "x-api-key: $key" -H 'content-type: application/json' \
    -H "x-session-id: $session" -H 'x-alex-harness: mock-tier' \
    -d '{"model":"gpt-4.1","messages":[{"role":"user","content":"mock openai"}]}' \
    "$base/v1/chat/completions" 2>"$TMP/mock-m2.err" || echo 000)
  t1=$(now_ms)
  if [ "$code" = "200" ] && python3 - "$out" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
assert d["choices"][0]["message"]["content"] == "Fake OpenAI chat response."
assert d["usage"]["prompt_tokens"] == 8 and d["usage"]["completion_tokens"] == 4
PY
  then
    write_result M2 PASS "$((t1-t0))" "openai chat completion returned expected text and usage"
  else
    write_result M2 FAIL "$((t1-t0))" "http $code: $(head -c 180 "$out" 2>/dev/null)"
  fi
}

run_mock_m3() {
  in_only M3 || return 0
  local base=$1 key=$2 out="$TMP/mock-m3.json" t0 t1 code
  t0=$(now_ms)
  code=$(curl -sS --max-time "$WIRE_TIMEOUT" -o "$out" -w '%{http_code}' \
    -H "x-api-key: $key" -H 'content-type: application/json' -H 'x-mock-fail: 429' \
    -H "x-session-id: mock-m3-$$" -H 'x-alex-harness: mock-tier' \
    -d '{"model":"claude-sonnet-4-5","max_tokens":64,"messages":[{"role":"user","content":"mock failure"}]}' \
    "$base/v1/messages" 2>"$TMP/mock-m3.err" || echo 000)
  t1=$(now_ms)
  if [ "$code" = "429" ] && python3 - "$out" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
assert d["type"] == "error"
assert d["error"]["type"] == "rate_limit_error"
assert d["error"]["message"] == "rate limit exceeded"
PY
  then
    write_result M3 PASS "$((t1-t0))" "x-mock-fail surfaced native anthropic 429 error"
  else
    write_result M3 FAIL "$((t1-t0))" "http $code: $(head -c 180 "$out" 2>/dev/null)"
  fi
}

run_mock_m4() {
  in_only M4 || return 0
  local base=$1 key=$2 out="$TMP/mock-m4.json" t0 t1 code msg
  t0=$(now_ms)
  code=$(curl -sS --max-time 15 -o "$out" -w '%{http_code}' \
    -H "x-api-key: $key" "$base/v1/models" 2>"$TMP/mock-m4.err" || echo 000)
  t1=$(now_ms)
  if [ "$code" = "200" ] && msg=$(python3 - "$out" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
ids = [row["id"] for row in d["data"]]
assert any(model.startswith("claude-") for model in ids)
assert any(model.startswith("gpt-") for model in ids)
print(f"{len(ids)} models include anthropic and openai")
PY
  ); then
    write_result M4 PASS "$((t1-t0))" "$msg"
  else
    write_result M4 FAIL "$((t1-t0))" "http $code: $(head -c 180 "$out" 2>/dev/null)"
  fi
}

run_mock_m5() {
  in_only M5 || return 0
  local home=$1 fake_base=$2 control_key=$3 out="$TMP/mock-m5.json" requests="$TMP/mock-m5-requests.json" t0 t1 msg rc=0
  t0=$(now_ms)
  mock_env "$home" "$fake_base" "$ROOT/target/debug/alex" ping openai --json \
    >"$out" 2>"$TMP/mock-m5.err" || rc=$?
  curl -sS --max-time 10 -H "x-control-key: $control_key" \
    -o "$requests" "$fake_base/_control/requests" 2>/dev/null || true
  t1=$(now_ms)
  if [ "$rc" -eq 0 ] && msg=$(python3 - "$out" "$requests" <<'PY'
import json, sys
result = json.load(open(sys.argv[1]))
assert result["ok"] is True
rows = result["results"]
assert len(rows) == 1 and rows[0]["provider"] == "openai" and rows[0]["status"] == 200
requests = json.load(open(sys.argv[2]))
assert any(row["path"] == "/v1/responses" and row["headers"].get("authorization") == "Bearer mock-openai-key" for row in requests)
print(result["summary"] + "; fakeprov received /v1/responses")
PY
  ); then
    write_result M5 PASS "$((t1-t0))" "$msg"
  else
    write_result M5 FAIL "$((t1-t0))" "exit $rc: $(tail -c 180 "$TMP/mock-m5.err" 2>/dev/null)"
  fi
}

run_mock_m6() {
  in_only M6 || return 0
  local home=$1 out="$TMP/mock-m6.json" t0 t1 msg rc=0
  t0=$(now_ms)
  ALEX_HOME="$home" "$ROOT/target/debug/alex" traces search --since 10m --json \
    >"$out" 2>"$TMP/mock-m6.err" || rc=$?
  t1=$(now_ms)
  if [ "$rc" -eq 0 ] && msg=$(python3 - "$out" "mock-m1-$$" "mock-m2-$$" <<'PY'
import json, sys
rows = json.load(open(sys.argv[1]))
by_session = {row.get("session_id"): row for row in rows}
for session, provider, model in [(sys.argv[2], "anthropic", "claude-sonnet-4-5"), (sys.argv[3], "openai", "gpt-4.1")]:
    row = by_session[session]
    assert row["status"] == 200 and row["upstream_provider"] == provider and row["routed_model"] == model
    assert row.get("req_body_path") and row.get("resp_body_path")
print("M1/M2 traces persisted with request and response bodies")
PY
  ); then
    write_result M6 PASS "$((t1-t0))" "$msg"
  else
    write_result M6 FAIL "$((t1-t0))" "exit $rc: $(tail -c 180 "$TMP/mock-m6.err" 2>/dev/null)"
  fi
}

run_mock_tier() {
  local build_start build_end rc=0 home fake_line fake_base control_key port base key i
  log "== mock: building alex + alex-fakeprov =="
  build_start=$(now_ms)
  (cd "$ROOT" && cargo build -p alex --bin alex -p alex-fakeprov --bin alex-fakeprov) >&2 || rc=$?
  build_end=$(now_ms)
  if [ "$rc" -ne 0 ]; then
    write_result M1 FAIL "$((build_end-build_start))" "cargo build exited $rc"
    return 0
  fi

  home="$TMP/mock-home"
  mkdir -p "$home/accounts"
  port=$(python3 - <<'PY'
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
  )
  key="alx-mock-tier-key"
  python3 - "$home" "$port" "$key" <<'PY'
import json, os, sys
home, port, key = sys.argv[1], int(sys.argv[2]), sys.argv[3]
quoted = json.dumps(home)
with open(os.path.join(home, "config.toml"), "w") as f:
    f.write(f'host = "127.0.0.1"\nport = {port}\ndata_dir = {quoted}\nlocal_key = "{key}"\nheartbeat_minutes = 0\nreauth_check_minutes = 0\nupdate_check_hours = 0\nanthropic_upstream = "direct"\ndario_mode_migrated = true\ndario_update_check_minutes = 0\n')
accounts = [
    {"id": "mock-anthropic", "provider": "anthropic", "kind": "api_key", "name": "mock", "api_key": "mock-anthropic-key", "status": "active"},
    {"id": "mock-openai", "provider": "openai", "kind": "api_key", "name": "mock", "api_key": "mock-openai-key", "status": "active"},
]
for account in accounts:
    with open(os.path.join(home, "accounts", account["id"] + ".json"), "w") as f:
        json.dump(account, f, indent=2)
PY
  chmod 600 "$home/config.toml" "$home/accounts/"*.json

  : > "$TMP/fakeprov.handshake"
  "$ROOT/target/debug/alex-fakeprov" --port 0 >"$TMP/fakeprov.handshake" 2>"$TMP/fakeprov.log" &
  FAKEPROV_PID=$!
  i=0
  while [ "$i" -lt 100 ] && [ ! -s "$TMP/fakeprov.handshake" ]; do
    if ! kill -0 "$FAKEPROV_PID" 2>/dev/null; then break; fi
    sleep 0.1
    i=$((i + 1))
  done
  fake_line=$(head -1 "$TMP/fakeprov.handshake" 2>/dev/null || true)
  fake_base=$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["base_url"])' "$fake_line" 2>/dev/null || true)
  control_key=$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["control_key"])' "$fake_line" 2>/dev/null || true)
  if [ -z "$fake_base" ] || [ -z "$control_key" ]; then
    write_result M1 FAIL 0 "fakeprov did not emit a valid JSON handshake"
    return 0
  fi

  base="http://127.0.0.1:$port"
  mock_env "$home" "$fake_base" "$ROOT/target/debug/alex" daemon --host 127.0.0.1 --port "$port" \
    >"$TMP/mock-daemon.log" 2>&1 &
  MOCK_DAEMON_PID=$!
  i=0
  while [ "$i" -lt 240 ]; do
    if curl -fsS --max-time 1 "$base/health" >/dev/null 2>&1; then break; fi
    if ! kill -0 "$MOCK_DAEMON_PID" 2>/dev/null; then break; fi
    sleep 0.25
    i=$((i + 1))
  done
  if ! curl -fsS --max-time 2 "$base/health" >/dev/null 2>&1; then
    write_result M1 FAIL 0 "mock daemon failed to become healthy: $(tail -c 180 "$TMP/mock-daemon.log" 2>/dev/null)"
    return 0
  fi

  run_mock_m1 "$base" "$key"
  run_mock_m2 "$base" "$key"
  run_mock_m3 "$base" "$key"
  run_mock_m4 "$base" "$key"
  run_mock_m5 "$home" "$fake_base" "$control_key"
  run_mock_m6 "$home"

  kill "$MOCK_DAEMON_PID" 2>/dev/null || true
  wait "$MOCK_DAEMON_PID" 2>/dev/null || true
  MOCK_DAEMON_PID=""
  kill "$FAKEPROV_PID" 2>/dev/null || true
  wait "$FAKEPROV_PID" 2>/dev/null || true
  FAKEPROV_PID=""
}

run_cliproxyapi_tier() {
  in_only CLIPROXYAPI || return 0
  if ! command -v docker >/dev/null 2>&1 || ! docker info >/dev/null 2>&1; then
    write_result CLIPROXYAPI SKIP 0 "docker unavailable"
    return 0
  fi
  log "== cliproxyapi: pinned v7 Docker compatibility matrix =="
  local t0 t1 rc=0
  t0=$(now_ms)
  (cd "$ROOT" && ./scripts/cliproxyapi-v1-integration.sh) >&2 || rc=$?
  t1=$(now_ms)
  if [ "$rc" -eq 0 ]; then
    write_result CLIPROXYAPI PASS "$((t1 - t0))" "CLIProxyAPI v7.2.92 both directions"
  else
    write_result CLIPROXYAPI FAIL "$((t1 - t0))" "Docker fixture exited $rc"
  fi
}

main() {
  if has_tier unit; then run_unit_tier; fi
  if has_tier mock; then run_mock_tier; fi
  if has_tier cliproxyapi; then run_cliproxyapi_tier; fi
  : > "$TMP/wire.cells"
  : > "$TMP/harness.cells"
  if has_tier wire; then select_wire; fi
  if has_tier harness; then select_harness; fi
  local need_daemon=0
  if [ -s "$TMP/wire.cells" ] || [ -s "$TMP/harness.cells" ]; then need_daemon=1; fi
  if has_tier dario && in_only DARIO; then need_daemon=1; fi
  if [ "$need_daemon" = "1" ]; then
    if [ -z "$KEY" ]; then
      log "warning: no local_key in $CONFIG_FILE - proxy auth will likely fail"
    fi
    ensure_daemon
    fetch_accounts
    prune_dario_cells
    run_preflight
    run_wire_cells
    run_harness_cells
    if has_tier dario; then run_dario_tier; fi
  fi
  if [ "$JSON" = "1" ]; then
    python3 "$ROOT/scripts/test-report.py" "$RESULTS" --json
  else
    python3 "$ROOT/scripts/test-report.py" "$RESULTS"
  fi
}

main
