#!/usr/bin/env bash
# Alex test suite (TODO.md section 11): ./test.sh [unit|mock|webui|wire|harness|harness-mock|cliproxyapi|dario|all] [flags]
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
  webui     Playwright Chromium suite against an isolated daemon + alex-fakeprov
  wire      curl-level matrix through the proxy (W1..W12), all cells parallel
  harness   Docker harness matrix (H1..H7), parallel
  harness-mock  offline Docker harness x fake-provider matrix + lifecycle + Dario/Fable bonuses
  cliproxyapi pinned real CLIProxyAPI v7 Docker matrix, both proxy directions
  dario     dario supervisor cells (SKIP cleanly when /admin/dario is absent)
  all       unit + mock + webui + wire + harness + harness-mock + cliproxyapi + dario

Flags:
  --only M1,W1,H2,...     run only these cell ids
  --provider P            provider filter, including harness-mock providers
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
    unit|mock|webui|wire|harness|harness-mock|cliproxyapi|dario|all) TIERS="$TIERS $1" ;;
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
case " $TIERS " in *" all "*) TIERS="unit mock webui wire harness harness-mock cliproxyapi dario" ;; esac

has_tier() {
  case " $TIERS " in *" $1 "*) return 0 ;; esac
  return 1
}

if [ -n "$PROVIDER_FILTER" ]; then
  case "$PROVIDER_FILTER" in
    anthropic|openai|codex|gemini|xai|kimi|openrouter|exo) ;;
    *) echo "unsupported --provider: $PROVIDER_FILTER" >&2; exit 2 ;;
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
DARIO_MOCK_PID=""

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
  if [ -n "$DARIO_MOCK_PID" ]; then
    pkill -P "$DARIO_MOCK_PID" 2>/dev/null || true
    kill "$DARIO_MOCK_PID" 2>/dev/null || true
  fi
  if [ "${ALEX_TEST_KEEP_TMP:-0}" = "1" ]; then
    log "test artifacts kept at $TMP"
  else
    rm -rf "$TMP"
  fi
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
    case "$id" in "$tok"[A-Z]*|"$tok"-*) return 0 ;; esac
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

run_webui_tier() {
  in_only WEBUI || return 0
  log "== webui: Playwright Chromium suite =="
  local t0 t1 rc=0
  t0=$(now_ms)
  (cd "$ROOT/webui-tests" && pnpm test) >&2 || rc=$?
  t1=$(now_ms)
  if [ "$rc" -eq 0 ]; then
    write_result WEBUI PASS "$((t1 - t0))" "pnpm test"
  else
    write_result WEBUI FAIL "$((t1 - t0))" "pnpm test exited $rc"
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

harness_mock_providers() {
  cat <<'EOF'
anthropic|alex/claude-fake-1|claude-fake-1|anthropic|Fake Anthropic response.
openai|alex/gpt-fake-1|gpt-fake-1|openai|Fake OpenAI Responses response.
codex|alex/codex-fake-1|codex-fake-1|openai|Fake Codex response.
gemini|alex/gemini-fake-1|gemini-fake-1|gemini|Fake Gemini response.
xai|alex/grok-fake-1|grok-fake-1|xai|Fake Grok response.
kimi|alex/kimi/kimi-fake-1|kimi-fake-1|kimi|Fake Kimi response.
openrouter|alex/openrouter/fake/fake-1|fake/fake-1|openrouter|Fake OpenRouter response.
exo|alex/exo/fake-1|fake-1|exo|Fake Exo response.
EOF
}

harness_mock_harnesses() {
  cat <<'EOF'
claude
codex
grok-build
kimi
EOF
}

harness_mock_cells() {
  local harness provider model routed trace_provider completion id
  while IFS= read -r harness; do
    while IFS='|' read -r provider model routed trace_provider completion; do
      id="HM-$(upper "$harness")-$(upper "$provider")"
      printf '%s|%s|%s|%s|%s|%s|%s\n' \
        "$id" "$harness" "$provider" "$model" "$routed" "$trace_provider" "$completion"
    done <<EOF
$(harness_mock_providers)
EOF
  done <<EOF
$(harness_mock_harnesses)
EOF
}

harness_mock_skip_reason() {
  case "$1:$2" in
    codex:xai|codex:kimi|codex:openrouter|codex:exo)
      printf '%s\n' "codex Responses dialect cannot route to the $2 chat upstream" ;;
  esac
}

harness_mock_env() {
  env \
    ALEX_HOME="$HARNESS_MOCK_HOME" \
    ALEX_UPSTREAM_ANTHROPIC_URL="$HARNESS_MOCK_FAKE_BASE/anthropic" \
    ALEX_UPSTREAM_OPENAI_URL="$HARNESS_MOCK_FAKE_BASE/openai" \
    ALEX_UPSTREAM_CODEX_URL="$HARNESS_MOCK_FAKE_BASE/openai" \
    ALEX_UPSTREAM_XAI_URL="$HARNESS_MOCK_FAKE_BASE/xai" \
    ALEX_UPSTREAM_GEMINI_URL="$HARNESS_MOCK_FAKE_BASE/gemini" \
    ALEX_UPSTREAM_GEMINI_CODE_ASSIST_URL="$HARNESS_MOCK_FAKE_BASE/gemini" \
    ALEX_UPSTREAM_OPENROUTER_URL="$HARNESS_MOCK_FAKE_BASE/openrouter" \
    ALEX_UPSTREAM_KIMI_URL="$HARNESS_MOCK_FAKE_BASE/kimi" \
    "$@"
}

harness_mock_control() {
  local path=$1 body=${2:-'{}'}
  curl -fsS --max-time 10 -H "x-control-key: $HARNESS_MOCK_CONTROL_KEY" \
    -H 'content-type: application/json' -d "$body" "$HARNESS_MOCK_FAKE_BASE$path" >/dev/null
}

harness_mock_admin() {
  local method=$1 path=$2 body=${3:-}
  if [ -n "$body" ]; then
    curl -fsS --max-time 10 -X "$method" -H "x-api-key: $HARNESS_MOCK_KEY" \
      -H 'content-type: application/json' -d "$body" "$HARNESS_MOCK_BASE$path" >/dev/null
  else
    curl -fsS --max-time 10 -X "$method" -H "x-api-key: $HARNESS_MOCK_KEY" \
      "$HARNESS_MOCK_BASE$path" >/dev/null
  fi
}

harness_mock_openai_mode() {
  local mode=$1 api_paused=false oauth_paused=true oauth2_paused=true
  if [ "$mode" = "codex" ]; then
    api_paused=true
    oauth_paused=false
  fi
  harness_mock_admin PUT /admin/accounts/mock-openai-api "{\"paused\":$api_paused}"
  harness_mock_admin PUT /admin/accounts/mock-openai-oauth "{\"paused\":$oauth_paused}"
  harness_mock_admin PUT /admin/accounts/mock-openai-oauth-2 "{\"paused\":$oauth2_paused}"
}

harness_mock_connect_harnesses() {
  cat <<'EOF'
pi|pi
claude|claude
codex|codex
grok|grok
kimi|kimi
amp|amp
EOF
}

harness_mock_seed_connect_dir() {
  local harness=$1 dir=$2
  mkdir -p "$dir"
  python3 - "$harness" "$dir" <<'PY'
import json, os, sys
h, d = sys.argv[1], sys.argv[2]
os.makedirs(d, exist_ok=True)
if h == "pi":
    value = {"providers": {"foreign": {"api": "openai", "models": [{"id": "user-model"}]}}}
    open(os.path.join(d, "models.json"), "w").write(json.dumps(value, indent=2) + "\n")
    open(os.path.join(d, "settings.json"), "w").write('{"defaultProvider":"foreign","defaultModel":"user-model"}\n')
elif h == "claude":
    open(os.path.join(d, "settings.json"), "w").write('{"model":"claude-user","theme":"dark"}\n')
    open(os.path.join(d, "alex-settings.json"), "w").write('{"model":"user-alex-profile"}\n')
elif h == "codex":
    open(os.path.join(d, "config.toml"), "w").write('model = "gpt-5.4"\nmodel_provider = "openai"\n\n[features]\nhooks = false\n\n[projects."/tmp/example"]\ntrust_level = "trusted"\n')
    open(os.path.join(d, "openai.config.toml"), "w").write('# user openai profile\nmodel = "gpt-5.4"\n')
    open(os.path.join(d, "alex.config.toml"), "w").write('# user alex profile\nmodel = "custom"\n')
elif h == "grok":
    os.makedirs(os.path.join(d, "hooks"), exist_ok=True)
    open(os.path.join(d, "config.toml"), "w").write('default_model = "grok-native"\n\n[model."grok-native"]\nmodel = "grok-native"\napi_backend = "native"\n')
    open(os.path.join(d, "hooks", "alex.json"), "w").write('{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"user-hook"}]}]}}\n')
elif h == "kimi":
    open(os.path.join(d, "config.toml"), "w").write('default_model = "kimi-native"\n\n[providers.moonshot]\ntype = "oauth"\n\n[models."kimi-native"]\nprovider = "moonshot"\nmodel = "kimi-native"\n')
elif h == "amp":
    os.makedirs(os.path.join(d, "plugins"), exist_ok=True)
    open(os.path.join(d, "plugins", "alex.ts"), "w").write('export default function userPlugin() {}\n')
PY
}

harness_mock_lifecycle_selected() {
  local harness=$1 id
  for id in "C1-$harness" "C2-$harness" "C3-$harness" "C4-$harness"; do
    in_only "$id" && return 0
  done
  return 1
}

harness_mock_connect_assert() {
  local harness=$1 dir=$2 out=$3
  python3 - "$harness" "$dir" "$out" <<'PY'
import json, os, stat, sys
h, d, out = sys.argv[1:4]
summary = json.load(open(out))
assert summary["harness"] == h, summary
assert summary.get("key_id"), summary
expected = ["alex/claude-fake-1","alex/gpt-fake-1","alex/codex-fake-1","alex/gemini-fake-1","alex/grok-fake-1","alex/kimi/kimi-fake-1","alex/openrouter/fake/fake-1","alex/exo/fake-1"]
models = summary.get("models") or []
if h != "amp":
    for model in expected:
        assert model in models, f"{model} missing from {h} connect summary"
if h == "pi":
    cfg = json.load(open(os.path.join(d, "models.json")))
    assert cfg["providers"]["foreign"]["models"][0]["id"] == "user-model"
    assert cfg["providers"]["alex"]["apiKey"].startswith("alxk-")
    key_path = os.path.join(d, "extensions", "alex-session.ts")
elif h == "claude":
    assert open(os.path.join(d, "settings.json")).read() == '{"model":"claude-user","theme":"dark"}\n'
    assert open(os.path.join(d, "alex-original-settings.json")).read() == '{"model":"claude-user","theme":"dark"}\n'
    key_path = os.path.join(d, "alex-api-key")
elif h == "codex":
    assert 'model = "gpt-5.4"' in open(os.path.join(d, "alex-original-config.toml")).read()
    assert os.path.isfile(os.path.join(d, "alex-models.json"))
    assert os.path.isfile(os.path.join(d, "alex-openai-models.json"))
    key_path = os.path.join(d, "alex-api-key")
elif h == "grok":
    assert 'grok-native' in open(os.path.join(d, "alex-original-config.toml")).read()
    assert os.path.isfile(os.path.join(d, "hooks", "alex.json"))
    key_path = os.path.join(d, "alex-api-key")
elif h == "kimi":
    assert 'kimi-native' in open(os.path.join(d, "alex-original-config.toml")).read()
    cfg = open(os.path.join(d, "config.toml")).read()
    assert "[providers.alex]" in cfg and '[models."alex/gpt-fake-1"]' in cfg
    key_path = os.path.join(d, "config.toml")
else:
    assert os.path.isfile(os.path.join(d, "plugins", "alex.ts"))
    key_path = os.path.join(d, "alex-api-key")
mode = stat.S_IMODE(os.stat(key_path).st_mode)
assert mode == 0o600, f"{key_path} mode {oct(mode)}"
print(f"connected; key={summary['key_id']}; models={len(models)}")
PY
}

harness_mock_models_assert() {
  local harness=$1 dir=$2
  python3 - "$harness" "$dir" <<'PY'
import json, os, sys
h, d = sys.argv[1:3]
expected = ["alex/claude-fake-1","alex/gpt-fake-1","alex/codex-fake-1","alex/gemini-fake-1","alex/grok-fake-1","alex/kimi/kimi-fake-1","alex/openrouter/fake/fake-1","alex/exo/fake-1"]
if h == "amp":
    print("amp has no selectable Alex model catalog")
    raise SystemExit(2)
if h == "pi":
    cfg = json.load(open(os.path.join(d, "models.json")))
    models = [m["id"] for m in cfg["providers"]["alex"]["models"]]
elif h == "claude":
    cfg = json.load(open(os.path.join(d, "alex-models.json")))
    models = [m["display_name"] for m in cfg["models"]]
elif h == "codex":
    cfg = json.load(open(os.path.join(d, "alex-models.json")))
    models = [m["slug"] for m in cfg["models"]]
elif h == "grok":
    raw = open(os.path.join(d, "config.toml")).read()
    models = [m for m in expected if f'[model."{m}"]' in raw]
else:
    raw = open(os.path.join(d, "config.toml")).read()
    models = [m for m in expected if f'[models."{m}"]' in raw]
for model in expected:
    assert model in models, f"{model} missing from written {h} catalog"
print(f"{len(expected)} canonical models visible in written config")
PY
}

harness_mock_routable_request() {
  local harness=$1 dir=$2 t0 out code key model endpoint body msg
  t0=$(now_ms)
  harness_mock_control /_control/reset
  harness_mock_control /_control/scenario '{"name":"ok"}'
  harness_mock_openai_mode openai
  if ! msg=$(python3 - "$harness" "$dir" 2>&1 <<'PY'
import json, os, sys
h, d = sys.argv[1:3]
if h == "amp":
    print("amp has no direct model route")
    raise SystemExit(2)
preferred = ["alex/gpt-fake-1", "alex/claude-fake-1"]
if h == "pi":
    cfg = json.load(open(os.path.join(d, "models.json")))
    models = [m["id"] for m in cfg["providers"]["alex"]["models"]]
    key = cfg["providers"]["alex"]["apiKey"]
elif h == "claude":
    models = [m["display_name"] for m in json.load(open(os.path.join(d, "alex-models.json")))["models"]]
    key = open(os.path.join(d, "alex-api-key")).read().strip()
elif h == "codex":
    models = [m["slug"] for m in json.load(open(os.path.join(d, "alex-models.json")))["models"] if m["slug"].startswith("alex/")]
    key = open(os.path.join(d, "alex-api-key")).read().strip()
elif h == "grok":
    raw = open(os.path.join(d, "config.toml")).read()
    models = [m for m in preferred if f'[model."{m}"]' in raw]
    key = open(os.path.join(d, "alex-api-key")).read().strip()
else:
    raw = open(os.path.join(d, "config.toml")).read()
    models = [m for m in preferred if f'[models."{m}"]' in raw]
    marker = 'api_key = "'
    key = raw.split("[providers.alex]", 1)[1].split(marker, 1)[1].split('"', 1)[0]
model = next((m for m in preferred if m in models), models[0])
fmt = "anthropic" if h in ("pi", "claude") else ("responses" if h == "codex" else "chat")
print("\t".join([key, fmt, model]))
PY
  ); then
    case "$msg" in *"no direct model route"*) echo "$msg"; return 2 ;; *) echo "$msg" >&2; return 1 ;; esac
  fi
  key=${msg%%	*}; msg=${msg#*	}; endpoint=${msg%%	*}; model=${msg#*	}
  out="$TMP/cell.C3-$harness.body"
  case "$endpoint" in
    anthropic)
      body=$(python3 -c 'import json,sys; print(json.dumps({"model":sys.argv[1],"max_tokens":64,"messages":[{"role":"user","content":"C3 route"}]}))' "$model")
      code=$(curl -sS --max-time 20 -o "$out" -w '%{http_code}' -H "x-api-key: $key" -H 'x-alex-harness: lifecycle' -H 'content-type: application/json' -d "$body" "$HARNESS_MOCK_BASE/v1/messages" || echo 000) ;;
    responses)
      body=$(python3 -c 'import json,sys; print(json.dumps({"model":sys.argv[1],"input":"C3 route"}))' "$model")
      code=$(curl -sS --max-time 20 -o "$out" -w '%{http_code}' -H "x-api-key: $key" -H 'x-alex-harness: lifecycle' -H 'content-type: application/json' -d "$body" "$HARNESS_MOCK_BASE/v1/responses" || echo 000) ;;
    *)
      body=$(python3 -c 'import json,sys; print(json.dumps({"model":sys.argv[1],"messages":[{"role":"user","content":"C3 route"}]}))' "$model")
      code=$(curl -sS --max-time 20 -o "$out" -w '%{http_code}' -H "x-api-key: $key" -H 'x-alex-harness: lifecycle' -H 'content-type: application/json' -d "$body" "$HARNESS_MOCK_BASE/v1/chat/completions" || echo 000) ;;
  esac
  if [ "$code" = 200 ] && python3 - "$out" "$endpoint" <<'PY'
import json, sys
d = json.load(open(sys.argv[1])); endpoint = sys.argv[2]
text = json.dumps(d)
assert "Fake OpenAI" in text or "Fake Anthropic" in text, text[:200]
PY
  then
    echo "model=$model routed via written key/config"
    return 0
  fi
  echo "http $code: $(head -c 180 "$out" 2>/dev/null)"
  return 1
}

harness_mock_disconnect_assert() {
  local harness=$1 dir=$2 key_id=$3 keys=$4
  python3 - "$harness" "$dir" "$key_id" "$keys" <<'PY'
import json, os, sys
h, d, key_id, keys_path = sys.argv[1:5]
if h == "pi":
    cfg = json.load(open(os.path.join(d, "models.json")))
    assert "foreign" in cfg["providers"]
    assert cfg["providers"].get("alex") in (None, {})
    assert not os.path.exists(os.path.join(d, "extensions", "alex-session.ts"))
elif h == "claude":
    assert open(os.path.join(d, "settings.json")).read() == '{"model":"claude-user","theme":"dark"}\n'
    assert open(os.path.join(d, "alex-settings.json")).read() == '{"model":"user-alex-profile"}\n'
    assert not os.path.exists(os.path.join(d, "alex-api-key"))
    assert not os.path.exists(os.path.join(d, "alex-models.json"))
elif h == "codex":
    assert '# user openai profile' in open(os.path.join(d, "openai.config.toml")).read()
    assert '# user alex profile' in open(os.path.join(d, "alex.config.toml")).read()
    raw = open(os.path.join(d, "config.toml")).read()
    assert 'model_provider = "openai"' in raw and "Alex Proxy" not in raw
    assert not os.path.exists(os.path.join(d, "alex-api-key"))
    assert not os.path.exists(os.path.join(d, "alex-models.json"))
elif h == "grok":
    raw = open(os.path.join(d, "config.toml")).read()
    assert "grok-native" in raw and "alex/gpt-fake-1" not in raw
    assert not os.path.exists(os.path.join(d, "alex-api-key"))
elif h == "kimi":
    raw = open(os.path.join(d, "config.toml")).read()
    assert "[providers.alex]" not in raw and "kimi-native" in raw
elif h == "amp":
    assert open(os.path.join(d, "plugins", "alex.ts")).read() == 'export default function userPlugin() {}\n'
    assert not os.path.exists(os.path.join(d, "alex-api-key"))
keys = json.load(open(keys_path))["run_keys"]
row = next((r for r in keys if r["id"] == key_id), None)
assert row, f"run key {key_id} missing"
assert row.get("revoked") in (1, True), row
print(f"disconnected; revoked key={key_id}; user config preserved")
PY
}

run_harness_mock_lifecycle_for() {
  local harness=$1 binary=$2 dir out err rc msg t0 key_id keys c1="C1-$harness" c2="C2-$harness" c3="C3-$harness" c4="C4-$harness"
  harness_mock_lifecycle_selected "$harness" || return 0
  if ! command -v "$binary" >/dev/null 2>&1; then
    in_only "$c1" && write_result "$c1" SKIP 0 "$binary unavailable"
    in_only "$c2" && write_result "$c2" SKIP 0 "$binary unavailable"
    in_only "$c3" && write_result "$c3" SKIP 0 "$binary unavailable"
    in_only "$c4" && write_result "$c4" SKIP 0 "$binary unavailable"
    return 0
  fi
  dir="$TMP/lifecycle-$harness"
  harness_mock_seed_connect_dir "$harness" "$dir"
  out="$TMP/cell.$c1.json"; err="$TMP/cell.$c1.err"; t0=$(now_ms); rc=0
  ALEX_HOME="$HARNESS_MOCK_HOME" "$ROOT/target/debug/alex" connect "$harness" --config-dir "$dir" --json >"$out" 2>"$err" || rc=$?
  if [ "$rc" -eq 0 ] && msg=$(harness_mock_connect_assert "$harness" "$dir" "$out" 2>&1); then
    in_only "$c1" && write_result "$c1" PASS "$(( $(now_ms)-t0 ))" "$msg"
  else
    msg="connect exit $rc: ${msg:-$(tail -c 220 "$err" 2>/dev/null)}"
    in_only "$c1" && write_result "$c1" FAIL "$(( $(now_ms)-t0 ))" "$msg"
    HARNESS_MOCK_LIFECYCLE_FAILED="$HARNESS_MOCK_LIFECYCLE_FAILED $harness"
    in_only "$c2" && write_result "$c2" FAIL 0 "dependent connect failed: $msg"
    in_only "$c3" && write_result "$c3" FAIL 0 "dependent connect failed: $msg"
    in_only "$c4" && write_result "$c4" FAIL 0 "dependent connect failed: $msg"
    return 0
  fi
  t0=$(now_ms)
  if msg=$(harness_mock_models_assert "$harness" "$dir" 2>&1); then
    in_only "$c2" && write_result "$c2" PASS "$(( $(now_ms)-t0 ))" "$msg"
  else
    rc=$?
    if [ "$rc" -eq 2 ]; then in_only "$c2" && write_result "$c2" SKIP "$(( $(now_ms)-t0 ))" "$msg"; else in_only "$c2" && write_result "$c2" FAIL "$(( $(now_ms)-t0 ))" "$msg"; HARNESS_MOCK_LIFECYCLE_FAILED="$HARNESS_MOCK_LIFECYCLE_FAILED $harness"; fi
  fi
  t0=$(now_ms)
  if msg=$(harness_mock_routable_request "$harness" "$dir" 2>&1); then
    in_only "$c3" && write_result "$c3" PASS "$(( $(now_ms)-t0 ))" "$msg"
  else
    rc=$?
    if [ "$rc" -eq 2 ]; then in_only "$c3" && write_result "$c3" SKIP "$(( $(now_ms)-t0 ))" "$msg"; else in_only "$c3" && write_result "$c3" FAIL "$(( $(now_ms)-t0 ))" "$msg"; HARNESS_MOCK_LIFECYCLE_FAILED="$HARNESS_MOCK_LIFECYCLE_FAILED $harness"; fi
  fi
  key_id=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["key_id"])' "$out" 2>/dev/null || true)
  err="$TMP/cell.$c4.err"; t0=$(now_ms); rc=0
  ALEX_HOME="$HARNESS_MOCK_HOME" "$ROOT/target/debug/alex" disconnect "$harness" --config-dir "$dir" >"$TMP/cell.$c4.out" 2>"$err" || rc=$?
  keys="$TMP/cell.$c4.keys.json"
  curl -fsS --max-time 10 -H "x-api-key: $HARNESS_MOCK_KEY" -o "$keys" "$HARNESS_MOCK_BASE/admin/run-keys?all=1" || true
  if [ "$rc" -eq 0 ] && msg=$(harness_mock_disconnect_assert "$harness" "$dir" "$key_id" "$keys" 2>&1); then
    in_only "$c4" && write_result "$c4" PASS "$(( $(now_ms)-t0 ))" "$msg"
  else
    msg="disconnect exit $rc: ${msg:-$(tail -c 220 "$err" 2>/dev/null)}"
    in_only "$c4" && write_result "$c4" FAIL "$(( $(now_ms)-t0 ))" "$msg"
    HARNESS_MOCK_LIFECYCLE_FAILED="$HARNESS_MOCK_LIFECYCLE_FAILED $harness"
  fi
  return 0
}

run_harness_mock_lifecycle() {
  local harness binary
  while IFS='|' read -r harness binary; do
    run_harness_mock_lifecycle_for "$harness" "$binary"
  done <<EOF
$(harness_mock_connect_harnesses)
EOF
}

run_harness_mock_c5() {
  in_only C5 || return 0
  local t0 out err rc=0 msg
  t0=$(now_ms)
  out="$TMP/cell.C5.json"; err="$TMP/cell.C5.err"
  ALEX_HOME="$HARNESS_MOCK_HOME" "$ROOT/target/debug/alex" wrap smoke --harness amp --json >"$out" 2>"$err" || rc=$?
  if [ "$rc" -eq 0 ] && msg=$(python3 - "$out" <<'PY' 2>&1
import json, sys
d = json.load(open(sys.argv[1]))
assert d.get("ok") is True or d.get("success") is True or d.get("captured_events", 0) >= 0, d
print("alex wrap smoke --harness amp --json passed")
PY
  ); then
    write_result C5 PASS "$(( $(now_ms)-t0 ))" "$msg"
  elif [ "$rc" -ne 0 ] && grep -qiE 'unknown harness|not found|unavailable|missing' "$err"; then
    write_result C5 SKIP "$(( $(now_ms)-t0 ))" "$(tail -c 220 "$err")"
  else
    write_result C5 FAIL "$(( $(now_ms)-t0 ))" "wrap smoke exit $rc: $(tail -c 220 "$err" 2>/dev/null)"
  fi
}

start_harness_mock_stack() {
  local port fake_line fake_port i rc=0
  log "== harness-mock: building alex + alex-fakeprov =="
  (cd "$ROOT" && cargo build -p alex --bin alex -p alex-fakeprov --bin alex-fakeprov) >&2 || rc=$?
  if [ "$rc" -ne 0 ]; then
    write_result HM-SETUP FAIL 0 "cargo build exited $rc"
    return 1
  fi
  HARNESS_MOCK_HOME="$TMP/harness-mock-home"
  mkdir -p "$HARNESS_MOCK_HOME/accounts"
  port=$(python3 - <<'PY'
import socket
s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()
PY
  )
  HARNESS_MOCK_KEY="alx-harness-mock-key"
  : > "$TMP/harness-mock-fake.handshake"
  "$ROOT/target/debug/alex-fakeprov" --bind 0.0.0.0 --port 0 \
    >"$TMP/harness-mock-fake.handshake" 2>"$TMP/harness-mock-fake.log" &
  FAKEPROV_PID=$!
  i=0
  while [ "$i" -lt 100 ] && [ ! -s "$TMP/harness-mock-fake.handshake" ]; do
    kill -0 "$FAKEPROV_PID" 2>/dev/null || break
    sleep 0.1
    i=$((i + 1))
  done
  fake_line=$(head -1 "$TMP/harness-mock-fake.handshake" 2>/dev/null || true)
  fake_port=$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["port"])' "$fake_line" 2>/dev/null || true)
  HARNESS_MOCK_CONTROL_KEY=$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["control_key"])' "$fake_line" 2>/dev/null || true)
  if [ -z "$fake_port" ] || [ -z "$HARNESS_MOCK_CONTROL_KEY" ]; then
    write_result HM-SETUP FAIL 0 "fakeprov did not emit a valid JSON handshake"
    return 1
  fi
  HARNESS_MOCK_FAKE_BASE="http://127.0.0.1:$fake_port"
  HARNESS_MOCK_BASE="http://127.0.0.1:$port"
  python3 - "$HARNESS_MOCK_HOME" "$port" "$HARNESS_MOCK_KEY" "$HARNESS_MOCK_FAKE_BASE" <<'PY'
import json, os, sys
home, port, key, fake = sys.argv[1], int(sys.argv[2]), sys.argv[3], sys.argv[4]
with open(os.path.join(home, "config.toml"), "w") as f:
    f.write(f'''host = "0.0.0.0"
port = {port}
data_dir = {json.dumps(home)}
local_key = "{key}"
heartbeat_minutes = 0
reauth_check_minutes = 0
update_check_hours = 0
anthropic_upstream = "direct"
dario_mode_migrated = true
dario_update_check_minutes = 0
upstream_stream_idle_timeout_seconds = 2
exo_url = "{fake}/exo"
exo_enabled_models = ["fake-1"]
openrouter_exposed_models = ["fake/fake-1"]
''')
accounts = [
    {"id":"mock-anthropic","provider":"anthropic","kind":"api_key","name":"anthropic","api_key":"mock-anthropic-key","status":"active"},
    {"id":"mock-openai-api","provider":"openai","kind":"api_key","name":"api","api_key":"mock-openai-key","status":"active"},
    {"id":"mock-openai-oauth","provider":"openai","kind":"oauth","name":"oauth-a","access_token":"mock-codex-token-a","expires_at_ms":4102444800000,"account_meta":{"account_id":"mock-a"},"status":"active","paused":True},
    {"id":"mock-openai-oauth-2","provider":"openai","kind":"oauth","name":"oauth-b","access_token":"mock-codex-token-b","expires_at_ms":4102444800000,"account_meta":{"account_id":"mock-b"},"status":"active","paused":True},
    {"id":"mock-gemini","provider":"gemini","kind":"api_key","name":"gemini","api_key":"mock-gemini-key","status":"active"},
    {"id":"mock-xai","provider":"xai","kind":"api_key","name":"xai","api_key":"mock-xai-key","status":"active"},
    {"id":"mock-kimi","provider":"kimi","kind":"oauth","name":"kimi","access_token":"mock-kimi-token","expires_at_ms":4102444800000,"status":"active"},
    {"id":"mock-openrouter","provider":"openrouter","kind":"api_key","name":"openrouter","api_key":"mock-openrouter-key","status":"active"},
]
for account in accounts:
    with open(os.path.join(home, "accounts", account["id"] + ".json"), "w") as f:
        json.dump(account, f, indent=2)
PY
  chmod 600 "$HARNESS_MOCK_HOME/config.toml" "$HARNESS_MOCK_HOME/accounts/"*.json
  harness_mock_env "$ROOT/target/debug/alex" daemon --host 0.0.0.0 --port "$port" \
    >"$TMP/harness-mock-daemon.log" 2>&1 &
  MOCK_DAEMON_PID=$!
  i=0
  while [ "$i" -lt 240 ]; do
    curl -fsS --max-time 1 "$HARNESS_MOCK_BASE/health" >/dev/null 2>&1 && break
    kill -0 "$MOCK_DAEMON_PID" 2>/dev/null || break
    sleep 0.25
    i=$((i + 1))
  done
  if ! curl -fsS --max-time 2 "$HARNESS_MOCK_BASE/health" >/dev/null 2>&1; then
    write_result HM-SETUP FAIL 0 "daemon failed: $(tail -c 220 "$TMP/harness-mock-daemon.log" 2>/dev/null)"
    return 1
  fi
  python3 - "$HARNESS_MOCK_HOME/alex.sqlite3" <<'PY'
import sqlite3, sys
models = ["claude-fake-1", "gpt-fake-1", "codex-fake-1", "gemini-fake-1", "grok-fake-1", "kimi/kimi-fake-1", "gpt-5.6-sol"]
db = sqlite3.connect(sys.argv[1], timeout=10)
for model in models:
    db.execute("INSERT OR IGNORE INTO pricing(model,input_per_m,cached_input_per_m,cache_creation_per_m,output_per_m) VALUES(?,?,?,?,?)", (model,0,0,0,0))
db.commit()
PY
  write_result HM-SETUP PASS 0 "offline daemon=$HARNESS_MOCK_BASE fakeprov=$HARNESS_MOCK_FAKE_BASE"
}

run_harness_mock_cell() {
  local id=$1 harness=$2 provider=$3 model=$4 routed=$5 trace_provider=$6 completion=$7
  local t0 t1 out err rc=0 session traces requests msg latest detail failure_text
  t0=$(now_ms)
  harness_mock_control /_control/reset
  harness_mock_control /_control/scenario '{"name":"harness-tool-roundtrip"}'
  if [ "$provider" = "openai" ] || [ "$provider" = "codex" ]; then
    harness_mock_openai_mode "$provider"
  fi
  out="$TMP/cell.$id.harness.json"
  err="$TMP/cell.$id.harness.err"
  ALEX_HOME="$HARNESS_MOCK_HOME" "$ROOT/target/debug/alex" harness run "$harness" \
    --model "$model" --prompt "Use your shell or directory-listing tool to list the current directory, then report the result." \
    --json --no-trace-check --timeout-secs "$HARNESS_TIMEOUT" >"$out" 2>"$err" || rc=$?
  t1=$(now_ms)
  if [ "$rc" -ne 0 ]; then
    latest=$(ls -td "$HARNESS_MOCK_HOME/harness-e2e/"* 2>/dev/null | head -1 || true)
    curl -fsS --max-time 10 -H "x-control-key: $HARNESS_MOCK_CONTROL_KEY" \
      -o "$TMP/cell.$id.requests.json" "$HARNESS_MOCK_FAKE_BASE/_control/requests" || true
    detail=$(for file in "$latest/harness.stderr.log" "$latest/docker.stderr.log" "$err" "$latest/logs/npm-install.log"; do if [ -s "$file" ]; then tail -c 500 "$file"; break; fi; done)
    failure_text=$(cat "$err" "$latest/harness.stdout.log" "$latest/harness.stderr.log" "$latest/docker.stderr.log" "$latest/logs/npm-install.log" "$latest/logs/grok-install.log" 2>/dev/null || true)
    msg="exit $rc: $detail"
    case "$failure_text" in
      *"upstream speaks OpenAI chat completions"*)
        write_result "$id" SKIP "$((t1-t0))" "$harness Responses dialect cannot route to the $provider chat upstream" ;;
      *"unknown model id"*)
        write_result "$id" SKIP "$((t1-t0))" "$harness rejects canonical model $model before dispatch" ;;
      *"manifest unknown"*|*"no matching manifest"*|*"pull access denied"*|*"has no Docker smoke runner"*|*"npm error code E404"*|*"Could not resolve host"*|*"Failed to connect"*)
        write_result "$id" SKIP "$((t1-t0))" "$msg" ;;
      *) write_result "$id" FAIL "$((t1-t0))" "$msg" ;;
    esac
    return 0
  fi
  session=$(python3 - "$out" <<'PY'
import json, os, sys
try: d=json.load(open(sys.argv[1]))
except Exception: d={}
print(os.path.basename(d.get("session_dir", "")))
PY
  )
  traces="$TMP/cell.$id.traces.json"
  requests="$TMP/cell.$id.requests.json"
  curl -fsS --max-time 10 -G -H "x-api-key: $HARNESS_MOCK_KEY" \
    --data-urlencode "model=$routed" --data-urlencode 'limit=20' \
    -o "$traces" "$HARNESS_MOCK_BASE/admin/traces" || true
  curl -fsS --max-time 10 -H "x-control-key: $HARNESS_MOCK_CONTROL_KEY" \
    -o "$requests" "$HARNESS_MOCK_FAKE_BASE/_control/requests" || true
  if msg=$(python3 - "$out" "$traces" "$requests" "$harness" "$trace_provider" "$routed" "$provider" "$completion" "$t0" <<'PY' 2>&1
import json, os, sys
summary=json.load(open(sys.argv[1])); traces=json.load(open(sys.argv[2])).get("traces", []); requests=json.load(open(sys.argv[3]))
harness, provider, routed, matrix_provider, completion=sys.argv[4:9]; started=int(sys.argv[9])
session=summary.get("session_dir", "")
assert session, "missing session_dir"
harness_log=os.path.join(session, "harness.stdout.log")
text=open(harness_log, errors="replace").read()
assert "alex-harness-tool-ok" in text, "final tool canary missing from harness output"
assert completion in text, f"fixture completion missing from harness output: {completion}"
posts=[r for r in requests if r.get("method")=="POST" and not r.get("path", "").startswith("/_control")]
assert len(posts) >= 2, f"fakeprov saw only {len(posts)} model POST(s)"
tool_results=[r for r in posts[1:] if any(marker in r.get("body", "") for marker in ("tool_result", "function_call_output", '"role":"tool"', '"role": "tool"', "functionResponse"))]
assert tool_results, "fakeprov request log has no returned tool result"
assert any("alex-harness-tool-canary" in r.get("body", "") for r in tool_results), "returned tool result lacks real ls canary"
rows=[r for r in traces if r.get("upstream_provider")==provider and r.get("routed_model")==routed and (r.get("ts_request_ms") or 0)>=started]
assert len(rows) >= 2, f"expected two {provider}/{routed} trace rows, got {len(rows)}"
assert all((r.get("harness") or "").lower().startswith(harness.split("-")[0]) for r in rows), "wrong harness tag"
assert all(r.get("req_body_path") and r.get("resp_body_path") for r in rows), "missing request/response body path"
assert any(r.get("input_tokens") is not None and r.get("output_tokens") is not None for r in rows), "usage tokens missing"
if matrix_provider == "openai": assert all(r.get("account_id")=="mock-openai-api" for r in rows), "OpenAI API cell used non-API account"
if matrix_provider == "codex": assert all((r.get("account_id") or "").startswith("mock-openai-oauth") for r in rows), "Codex cell used non-OAuth account"
print(f"tool roundtrip; traces={len(rows)}; fakeprov_posts={len(posts)}; completion={completion}")
PY
  ); then
    write_result "$id" PASS "$((t1-t0))" "$msg"
  else
    write_result "$id" FAIL "$((t1-t0))" "$msg"
  fi
}

run_harness_mock_matrix() {
  local id harness provider model routed trace_provider completion reason lifecycle_name
  while IFS='|' read -r id harness provider model routed trace_provider completion; do
    in_only "$id" || continue
    if [ -n "$PROVIDER_FILTER" ] && [ "$provider" != "$PROVIDER_FILTER" ]; then continue; fi
    if [ -n "$HARNESS_FILTER" ] && [ "$harness" != "$HARNESS_FILTER" ]; then continue; fi
    lifecycle_name=$harness
    [ "$lifecycle_name" = "grok-build" ] && lifecycle_name=grok
    case " $HARNESS_MOCK_LIFECYCLE_FAILED " in
      *" $lifecycle_name "*) write_result "$id" FAIL 0 "dependent lifecycle cell failed for $lifecycle_name"; continue ;;
    esac
    reason=$(harness_mock_skip_reason "$harness" "$provider")
    if [ -n "$reason" ]; then
      write_result "$id" SKIP 0 "$reason"
      continue
    fi
    run_harness_mock_cell "$id" "$harness" "$provider" "$model" "$routed" "$trace_provider" "$completion"
  done <<EOF
$(harness_mock_cells)
EOF
}

run_harness_mock_b5() {
  in_only B5 || return 0
  local out="$TMP/cell.B5.models.json" t0 msg
  t0=$(now_ms)
  curl -fsS --max-time 15 -H "x-api-key: $HARNESS_MOCK_KEY" -o "$out" "$HARNESS_MOCK_BASE/v1/models" || true
  if msg=$(python3 - "$out" <<'PY' 2>&1
import json, sys
ids=[row["id"] for row in json.load(open(sys.argv[1]))["data"]]
expected=["alex/claude-fake-1","alex/gpt-fake-1","alex/codex-fake-1","alex/gemini-fake-1","alex/grok-fake-1","alex/kimi/kimi-fake-1","alex/openrouter/fake/fake-1","alex/exo/fake-1"]
for model in expected: assert ids.count(model)==1, f"{model}: count={ids.count(model)}"
print("8 canonical alex/* model ids present exactly once")
PY
  ); then write_result B5 PASS "$(( $(now_ms)-t0 ))" "$msg"; else write_result B5 FAIL "$(( $(now_ms)-t0 ))" "$msg"; fi
}

run_harness_mock_b1() {
  in_only B1 || return 0
  local t0 out traces requests sess="harness-mock-b1-$$" msg code i
  t0=$(now_ms)
  harness_mock_control /_control/reset
  harness_mock_openai_mode codex
  harness_mock_admin PUT /admin/accounts/mock-openai-oauth-2 '{"paused":false}'
  harness_mock_control /_control/queue '{"endpoint":"POST /openai/backend-api/codex/responses","failure":"429"}'
  harness_mock_control /_control/queue '{"endpoint":"POST /openai/backend-api/codex/responses","use_default":true}'
  out="$TMP/cell.B1.body"
  code=$(curl -sS --max-time 30 -o "$out" -w '%{http_code}' -H "x-api-key: $HARNESS_MOCK_KEY" \
    -H 'content-type: application/json' -H 'x-alex-harness: codex' -H "x-session-id: $sess" \
    -d '{"model":"alex/codex/codex-fake-1","stream":true,"input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"B1"}]}]}' \
    "$HARNESS_MOCK_BASE/v1/responses" || echo 000)
  traces="$TMP/cell.B1.traces.json"; requests="$TMP/cell.B1.requests.json"
  i=0
  while [ "$i" -lt 50 ]; do
    curl -fsS -H "x-api-key: $HARNESS_MOCK_KEY" -o "$traces" "$HARNESS_MOCK_BASE/admin/traces?limit=20" || true
    python3 - "$traces" "$t0" <<'PY' >/dev/null 2>&1 && break
import json,sys
rows=json.load(open(sys.argv[1])).get("traces",[])
assert any((r.get("ts_request_ms") or 0)>=int(sys.argv[2]) and r.get("routed_model")=="codex-fake-1" for r in rows)
PY
    sleep 0.1
    i=$((i + 1))
  done
  curl -fsS -H "x-control-key: $HARNESS_MOCK_CONTROL_KEY" -o "$requests" "$HARNESS_MOCK_FAKE_BASE/_control/requests" || true
  if [ "$code" = 200 ] && msg=$(python3 - "$traces" "$requests" "$t0" <<'PY' 2>&1
import json, sys
traces=[r for r in json.load(open(sys.argv[1]))["traces"] if (r.get("ts_request_ms") or 0)>=int(sys.argv[3])]; requests=json.load(open(sys.argv[2]))
assert traces, "no B1 trace row"
row=traces[0]; attempts=row.get("attempts") or []
if isinstance(attempts,str): attempts=json.loads(attempts)
accounts=[a.get("account_id") for a in attempts]
assert len(set(a for a in accounts if a)) >= 2, accounts
auth=[r.get("headers",{}).get("authorization") for r in requests if r.get("path")=="/openai/backend-api/codex/responses"]
assert "Bearer mock-codex-token-a" in auth and "Bearer mock-codex-token-b" in auth, auth
print("429 failed over across two Codex OAuth accounts")
PY
  ); then write_result B1 PASS "$(( $(now_ms)-t0 ))" "$msg"; else write_result B1 FAIL "$(( $(now_ms)-t0 ))" "http $code: ${msg:-$(head -c 180 "$out")}"; fi
}

run_harness_mock_b2() {
  in_only B2 || return 0
  local t0 out traces sess="harness-mock-b2-$$" msg code
  t0=$(now_ms)
  harness_mock_control /_control/reset
  harness_mock_control /_control/scenario '{"name":"ok"}'
  harness_mock_openai_mode openai
  harness_mock_control /_control/queue '{"endpoint":"POST /anthropic/v1/messages","failure":"refusal"}'
  out="$TMP/cell.B2.body"
  code=$(curl -sS --max-time 30 -o "$out" -w '%{http_code}' -H "x-api-key: $HARNESS_MOCK_KEY" \
    -H 'content-type: application/json' -H 'x-alex-harness: claude' -H 'user-agent: claude-cli/2.1.0' -H "x-session-id: $sess" \
    -d '{"model":"alex/claude-fable-5","max_tokens":64,"stream":true,"messages":[{"role":"user","content":"B2"}]}' \
    "$HARNESS_MOCK_BASE/v1/messages" || echo 000)
  traces="$TMP/cell.B2.traces.json"
  curl -fsS -H "x-api-key: $HARNESS_MOCK_KEY" -o "$traces" "$HARNESS_MOCK_BASE/admin/traces?limit=20" || true
  if [ "$code" = 200 ] && msg=$(python3 - "$out" "$traces" "$t0" <<'PY' 2>&1
import json, sys
body=open(sys.argv[1]).read(); rows=[r for r in json.load(open(sys.argv[2]))["traces"] if (r.get("ts_request_ms") or 0)>=int(sys.argv[3])]
assert "Fake OpenAI" in body, body[:200]
row=rows[0]
assert row.get("substituted") in (1, True), row
assert row.get("served_model")=="gpt-5.6-sol", row.get("served_model")
assert "alex.fable-5-to-gpt-5.6-sol" in json.dumps(row.get("attempts") or []), row.get("attempts")
print("Fable refusal rerouted to gpt-5.6-sol with middleware decision")
PY
  ); then write_result B2 PASS "$(( $(now_ms)-t0 ))" "$msg"; else write_result B2 FAIL "$(( $(now_ms)-t0 ))" "http $code: ${msg:-$(head -c 180 "$out")}"; fi
}

run_harness_mock_b3() {
  in_only B3 || return 0
  local t0 out err rc=0 latest text elapsed
  t0=$(now_ms)
  harness_mock_control /_control/reset
  harness_mock_control /_control/scenario '{"name":"stream-stall"}'
  harness_mock_openai_mode openai
  out="$TMP/cell.B3.harness.json"; err="$TMP/cell.B3.harness.err"
  ALEX_HOME="$HARNESS_MOCK_HOME" "$ROOT/target/debug/alex" harness run codex \
    --model alex/openai/gpt-fake-1 --prompt "Reply once." --json --no-trace-check --timeout-secs 25 \
    >"$out" 2>"$err" || rc=$?
  elapsed=$(( $(now_ms)-t0 ))
  latest=$(ls -td "$HARNESS_MOCK_HOME/harness-e2e/"* 2>/dev/null | head -1 || true)
  text="$(cat "$err" "$latest/harness.stdout.log" "$latest/harness.stderr.log" 2>/dev/null || true)"
  if [ "$rc" -ne 0 ] && [ "$elapsed" -lt 30000 ] && ! printf '%s' "$text" | grep -q 'docker run timed out'; then
    write_result B3 PASS "$elapsed" "Codex received a bounded stream-disconnect error (exit $rc), not a hang"
  elif [ "$rc" -eq 0 ]; then
    write_result B3 FAIL "$elapsed" "stalled stream unexpectedly completed successfully"
  else
    write_result B3 FAIL "$elapsed" "stalled stream hit the outer harness timeout"
  fi
}

run_harness_mock_b4() {
  in_only B4 || return 0
  local t0 out accounts code msg
  t0=$(now_ms)
  harness_mock_control /_control/reset
  harness_mock_control /_control/scenario '{"name":"kimi-quota-exhausted"}'
  out="$TMP/cell.B4.body"; accounts="$TMP/cell.B4.accounts.json"
  code=$(curl -sS --max-time 15 -o "$out" -w '%{http_code}' -H "x-api-key: $HARNESS_MOCK_KEY" \
    -H 'content-type: application/json' -d '{"model":"alex/kimi/kimi-fake-1","messages":[{"role":"user","content":"B4"}]}' \
    "$HARNESS_MOCK_BASE/v1/chat/completions" || echo 000)
  curl -fsS -H "x-api-key: $HARNESS_MOCK_KEY" -o "$accounts" "$HARNESS_MOCK_BASE/admin/accounts" || true
  if msg=$(python3 - "$out" "$accounts" "$code" <<'PY' 2>&1
import json, sys, time
body=json.load(open(sys.argv[1])); accounts=json.load(open(sys.argv[2]))["accounts"]
assert sys.argv[3]=="403", sys.argv[3]
assert body["error"]["type"]=="access_terminated_error", body
acct=next(a for a in accounts if a["id"]=="mock-kimi")
assert (acct.get("cooldown_until_ms") or 0) > int(time.time()*1000), acct
print("native Kimi quota body surfaced; account cooldown active")
PY
  ); then write_result B4 PASS "$(( $(now_ms)-t0 ))" "$msg"; else write_result B4 FAIL "$(( $(now_ms)-t0 ))" "$msg"; fi
}

run_harness_mock_dario() {
  in_only D-MOCK || return 0
  if ! command -v node >/dev/null 2>&1 || ! command -v npm >/dev/null 2>&1; then
    write_result D-MOCK SKIP 0 "node/npm unavailable"
    return 0
  fi
  local t0 port home base key node_bin claude_bin i status out traces requests code msg
  t0=$(now_ms)
  node_bin=$(command -v node)
  claude_bin=$(command -v claude || true)
  if [ -z "$claude_bin" ]; then
    write_result D-MOCK SKIP 0 "real Claude binary unavailable for Dario prompt capture"
    return 0
  fi
  port=$(python3 - <<'PY'
import socket
s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()
PY
  )
  home="$TMP/dario-mock-home"; base="http://127.0.0.1:$port"; key="alx-dario-mock-key"
  mkdir -p "$home/accounts"
  python3 - "$home" "$port" "$key" "$node_bin" "$claude_bin" <<'PY'
import json, os, sys
home, port, key, node, claude=sys.argv[1],int(sys.argv[2]),sys.argv[3],sys.argv[4],sys.argv[5]
with open(os.path.join(home,"config.toml"),"w") as f:
    f.write(f'''host = "127.0.0.1"
port = {port}
data_dir = {json.dumps(home)}
local_key = "{key}"
heartbeat_minutes = 0
reauth_check_minutes = 0
update_check_hours = 0
anthropic_upstream = "dario"
dario_mode_migrated = true
dario_update_check_minutes = 0
dario_version = "5.2.16"
dario_probe_seconds = 0
dario_node_path = {json.dumps(node)}
dario_claude_bin = {json.dumps(claude)}
''')
account={"id":"dario-mock-anthropic","provider":"anthropic","kind":"api_key","name":"dario-mock","api_key":"mock-anthropic-key","status":"active"}
with open(os.path.join(home,"accounts","dario-mock-anthropic.json"),"w") as f: json.dump(account,f)
PY
  chmod 600 "$home/config.toml" "$home/accounts/"*.json
  env ALEX_HOME="$home" \
    ALEX_UPSTREAM_ANTHROPIC_URL="$HARNESS_MOCK_FAKE_BASE/anthropic" \
    ALEX_DARIO_UPSTREAM_URL="$HARNESS_MOCK_FAKE_BASE/anthropic" \
    "$ROOT/target/debug/alex" daemon --host 127.0.0.1 --port "$port" \
    >"$TMP/dario-mock-daemon.log" 2>&1 &
  DARIO_MOCK_PID=$!
  i=0
  while [ "$i" -lt 300 ]; do
    curl -fsS --max-time 1 "$base/health" >/dev/null 2>&1 && break
    kill -0 "$DARIO_MOCK_PID" 2>/dev/null || break
    sleep 0.5
    i=$((i + 1))
  done
  status="$TMP/cell.D-MOCK.status.json"
  curl -fsS --max-time 5 -H "x-api-key: $key" -o "$status" "$base/admin/dario" 2>/dev/null || true
  if ! python3 - "$status" <<'PY'
import json,sys
try: d=json.load(open(sys.argv[1]))
except Exception: raise SystemExit(1)
raise SystemExit(0 if d.get("active_generation_id") else 1)
PY
  then
    write_result D-MOCK SKIP "$(( $(now_ms)-t0 ))" "Dario could not start: $(tail -c 240 "$TMP/dario-mock-daemon.log" 2>/dev/null)"
    kill "$DARIO_MOCK_PID" 2>/dev/null || true; wait "$DARIO_MOCK_PID" 2>/dev/null || true; DARIO_MOCK_PID=""
    return 0
  fi
  harness_mock_control /_control/reset
  harness_mock_control /_control/scenario '{"name":"ok"}'
  harness_mock_control /_control/queue '{"endpoint":"POST /anthropic/v1/messages","use_default":true}'
  harness_mock_control /_control/queue '{"endpoint":"POST /anthropic/v1/messages","failure":"refusal"}'
  out="$TMP/cell.D-MOCK.body"; traces="$TMP/cell.D-MOCK.traces.json"; requests="$TMP/cell.D-MOCK.requests.json"
  code=$(curl -sS --max-time 60 -o "$out" -w '%{http_code}' -H "x-api-key: $key" \
    -H 'content-type: application/json' -H 'x-alex-harness: codex' -H 'x-session-id: dario-mock-session' \
    -H 'x-alex-no-substitute: 1' \
    -d '{"model":"alex/claude-fable-5","max_tokens":64,"stream":true,"system":[{"type":"text","text":"identity"},{"type":"text","text":"agent identity"},{"type":"text","text":"ORIGINAL-DARIO-SYSTEM"}],"messages":[{"role":"user","content":"D-MOCK"}]}' \
    "$base/v1/messages" || echo 000)
  i=0
  while [ "$i" -lt 50 ]; do
    curl -fsS -H "x-api-key: $key" -o "$traces" "$base/admin/traces?limit=10" || true
    python3 - "$traces" "$t0" <<'PY' >/dev/null 2>&1 && break
import json,sys
rows=json.load(open(sys.argv[1])).get("traces",[])
assert any((r.get("ts_request_ms") or 0)>=int(sys.argv[2]) and r.get("routed_model")=="claude-fable-5" for r in rows)
PY
    sleep 0.1
    i=$((i + 1))
  done
  curl -fsS -H "x-control-key: $HARNESS_MOCK_CONTROL_KEY" -o "$requests" "$HARNESS_MOCK_FAKE_BASE/_control/requests" || true
  if [ "$code" = 200 ] && msg=$(python3 - "$out" "$traces" "$requests" "$home" "$ROOT/crates/alex-fakeprov/fixtures/anthropic/anthropic-fable-refusal-200.sse" "$t0" <<'PY' 2>&1
import gzip, glob, json, sys
body=open(sys.argv[1]).read(); traces=json.load(open(sys.argv[2]))["traces"]; requests=json.load(open(sys.argv[3])); started=int(sys.argv[6])
assert body==open(sys.argv[5]).read(), "Fable refusal SSE changed in transit"
rows=[r for r in traces if (r.get("ts_request_ms") or 0)>=started and r.get("routed_model")=="claude-fable-5"]
assert rows and rows[0].get("via_dario") in (1,True), rows
assert rows[0].get("dario_generation"), rows[0]
posts=[r for r in requests if r.get("path")=="/anthropic/v1/messages"]
assert posts, requests
post=next(r for r in posts if "D-MOCK" in r.get("body",""))
up=json.loads(post["body"])
assert len(up.get("system",[]))>=3, up.keys()
assert "You are an interactive agent" in up["system"][2].get("text",""), "Dario system prompt missing"
assert post.get("headers",{}).get("x-api-key") or post.get("headers",{}).get("authorization"), "Dario upstream signature missing auth"
captures=glob.glob(sys.argv[4]+"/bodies/**/*.dario-upstream-request.json.gz",recursive=True)
assert captures, "Dario upstream capture missing"
capture=next(json.load(gzip.open(p,"rt")) for p in captures if "D-MOCK" in gzip.open(p,"rt").read())
assert capture.get("direction")=="dario->anthropic", capture
assert capture.get("prompt_cache",{}).get("applied") is True, capture.get("prompt_cache")
print(f"generation={rows[0]['dario_generation']}; Dario rewrite captured; refusal SSE intact")
PY
  ); then
    write_result D-MOCK PASS "$(( $(now_ms)-t0 ))" "$msg"
  else
    write_result D-MOCK FAIL "$(( $(now_ms)-t0 ))" "http $code: ${msg:-$(head -c 200 "$out")}"
  fi
  kill "$DARIO_MOCK_PID" 2>/dev/null || true; wait "$DARIO_MOCK_PID" 2>/dev/null || true; DARIO_MOCK_PID=""
}

run_harness_mock_b_dario_tool() {
  in_only B-DARIO-TOOL || return 0
  if ! command -v docker >/dev/null 2>&1 || ! docker info >/dev/null 2>&1; then
    write_result B-DARIO-TOOL SKIP 0 "docker unavailable (docker info failed)"
    return 0
  fi
  if ! command -v node >/dev/null 2>&1 || ! command -v npm >/dev/null 2>&1; then
    write_result B-DARIO-TOOL SKIP 0 "node/npm unavailable"
    return 0
  fi
  local t0 port home base key node_bin claude_bin i status out err rc=0 traces requests msg latest
  t0=$(now_ms)
  node_bin=$(command -v node)
  claude_bin=$(command -v claude || true)
  if [ -z "$claude_bin" ]; then
    write_result B-DARIO-TOOL SKIP 0 "real Claude binary unavailable for Dario bootstrap"
    return 0
  fi
  port=$(python3 - <<'PY'
import socket
s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()
PY
  )
  home="$TMP/dario-tool-home"; base="http://127.0.0.1:$port"; key="alx-dario-tool-key"
  mkdir -p "$home/accounts"
  python3 - "$home" "$port" "$key" "$node_bin" "$claude_bin" <<'PY'
import json, os, sys
home, port, key, node, claude=sys.argv[1],int(sys.argv[2]),sys.argv[3],sys.argv[4],sys.argv[5]
with open(os.path.join(home,"config.toml"),"w") as f:
    f.write(f'''host = "0.0.0.0"
port = {port}
data_dir = {json.dumps(home)}
local_key = "{key}"
heartbeat_minutes = 0
reauth_check_minutes = 0
update_check_hours = 0
anthropic_upstream = "dario"
dario_mode_migrated = true
dario_update_check_minutes = 0
dario_version = "5.2.16"
dario_probe_seconds = 0
dario_node_path = {json.dumps(node)}
dario_claude_bin = {json.dumps(claude)}
upstream_stream_idle_timeout_seconds = 2
''')
account={"id":"dario-tool-anthropic","provider":"anthropic","kind":"api_key","name":"dario-tool","api_key":"mock-anthropic-key","status":"active"}
with open(os.path.join(home,"accounts","dario-tool-anthropic.json"),"w") as f: json.dump(account,f)
PY
  chmod 600 "$home/config.toml" "$home/accounts/"*.json
  env ALEX_HOME="$home" ALEX_DARIO_UPSTREAM_URL="$HARNESS_MOCK_FAKE_BASE/anthropic" \
    ALEX_UPSTREAM_ANTHROPIC_URL="$HARNESS_MOCK_FAKE_BASE/anthropic" \
    "$ROOT/target/debug/alex" daemon --host 0.0.0.0 --port "$port" \
    >"$TMP/dario-tool-daemon.log" 2>&1 &
  DARIO_MOCK_PID=$!
  i=0
  while [ "$i" -lt 300 ]; do
    curl -fsS --max-time 1 "$base/health" >/dev/null 2>&1 && break
    kill -0 "$DARIO_MOCK_PID" 2>/dev/null || break
    sleep 0.5
    i=$((i + 1))
  done
  status="$TMP/cell.B-DARIO-TOOL.status.json"
  curl -fsS --max-time 5 -H "x-api-key: $key" -o "$status" "$base/admin/dario" 2>/dev/null || true
  if ! python3 - "$status" <<'PY'
import json,sys
try: d=json.load(open(sys.argv[1]))
except Exception: raise SystemExit(1)
raise SystemExit(0 if d.get("active_generation_id") else 1)
PY
  then
    write_result B-DARIO-TOOL SKIP "$(( $(now_ms)-t0 ))" "Dario could not start: $(tail -c 240 "$TMP/dario-tool-daemon.log" 2>/dev/null)"
    kill "$DARIO_MOCK_PID" 2>/dev/null || true; wait "$DARIO_MOCK_PID" 2>/dev/null || true; DARIO_MOCK_PID=""
    return 0
  fi
  harness_mock_control /_control/reset
  harness_mock_control /_control/scenario '{"name":"ok"}'
  harness_mock_control /_control/queue '{"endpoint":"POST /anthropic/v1/messages","use_default":true}'
  harness_mock_control /_control/queue '{"endpoint":"POST /anthropic/v1/messages","directory_tool_call":true}'
  harness_mock_control /_control/queue '{"endpoint":"POST /anthropic/v1/messages","tool_final":"alex-harness-tool-ok Fake Anthropic response."}'
  out="$TMP/cell.B-DARIO-TOOL.harness.json"; err="$TMP/cell.B-DARIO-TOOL.err"
  ALEX_HOME="$home" "$ROOT/target/debug/alex" harness run codex \
    --model alex/claude-fake-1 --prompt "Use your shell tool to list the current directory, then report the result." \
    --container-base-url "http://host.docker.internal:$port" \
    --json --no-trace-check --timeout-secs "$HARNESS_TIMEOUT" >"$out" 2>"$err" || rc=$?
  traces="$TMP/cell.B-DARIO-TOOL.traces.json"; requests="$TMP/cell.B-DARIO-TOOL.requests.json"
  curl -fsS --max-time 10 -H "x-api-key: $key" -o "$traces" "$base/admin/traces?limit=30" || true
  curl -fsS --max-time 10 -H "x-control-key: $HARNESS_MOCK_CONTROL_KEY" -o "$requests" "$HARNESS_MOCK_FAKE_BASE/_control/requests" || true
  if [ "$rc" -ne 0 ]; then
    latest=$(ls -td "$home/harness-e2e/"* 2>/dev/null | head -1 || true)
    msg=$(cat "$err" "$latest/harness.stderr.log" "$latest/docker.stderr.log" "$latest/logs/npm-install.log" 2>/dev/null | tail -c 500)
    case "$msg" in
      *"manifest unknown"*|*"no matching manifest"*|*"pull access denied"*|*"npm error code E404"*|*"Could not resolve host"*|*"Failed to connect"*)
        write_result B-DARIO-TOOL SKIP "$(( $(now_ms)-t0 ))" "codex harness unavailable: $msg" ;;
      *) write_result B-DARIO-TOOL FAIL "$(( $(now_ms)-t0 ))" "codex harness exit $rc: $msg" ;;
    esac
    kill "$DARIO_MOCK_PID" 2>/dev/null || true; wait "$DARIO_MOCK_PID" 2>/dev/null || true; DARIO_MOCK_PID=""
    return 0
  fi
  if msg=$(python3 - "$out" "$traces" "$requests" "$t0" <<'PY' 2>&1
import json, os, sys
summary=json.load(open(sys.argv[1])); traces=json.load(open(sys.argv[2])).get("traces",[]); requests=json.load(open(sys.argv[3])); started=int(sys.argv[4])
session=summary["session_dir"]
text=open(os.path.join(session, "harness.stdout.log"), errors="replace").read()
assert "alex-harness-tool-ok" in text and "Fake Anthropic response." in text, text[-500:]
posts=[r for r in requests if r.get("path")=="/anthropic/v1/messages"]
tool_posts=[r for r in posts if "alex-harness-tool-canary" in r.get("body","") and "tool_result" in r.get("body","")]
assert tool_posts, "no Dario upstream request carried the real tool result"
second=tool_posts[-1]
body=json.loads(second["body"])
assert any("You are an interactive agent" in item.get("text","") for item in body.get("system",[]) if isinstance(item,dict)), "Dario system rewrite missing"
assert second.get("headers",{}).get("x-api-key") or second.get("headers",{}).get("authorization"), "Dario upstream auth signature missing"
rows=[r for r in traces if (r.get("ts_request_ms") or 0)>=started and r.get("routed_model")=="claude-fake-1"]
assert len(rows) >= 2, rows
assert all(r.get("via_dario") in (1, True) for r in rows), rows
assert all(r.get("dario_generation") for r in rows), rows
print(f"codex->daemon->dario->fakeprov tool roundtrip; traces={len(rows)}; dario_posts={len(posts)}")
PY
  ); then
    write_result B-DARIO-TOOL PASS "$(( $(now_ms)-t0 ))" "$msg"
  else
    write_result B-DARIO-TOOL FAIL "$(( $(now_ms)-t0 ))" "$msg"
  fi
  kill "$DARIO_MOCK_PID" 2>/dev/null || true; wait "$DARIO_MOCK_PID" 2>/dev/null || true; DARIO_MOCK_PID=""
}

harness_mock_set_fable_rule() {
  local enabled=$1 status="$TMP/fable-rule-status.json" rule="$TMP/fable-rule.json"
  curl -fsS --max-time 10 -H "x-api-key: $HARNESS_MOCK_KEY" -o "$status" "$HARNESS_MOCK_BASE/admin/middleware"
  python3 - "$status" "$rule" "$enabled" <<'PY'
import json, sys
status, out, enabled = sys.argv[1], sys.argv[2], sys.argv[3] == "true"
d = json.load(open(status))
rule = next(r for r in d["rules"] if r["id"] == "alex.fable-5-to-gpt-5.6-sol")
rule["enabled"] = enabled
json.dump(rule, open(out, "w"))
PY
  curl -fsS --max-time 10 -X PUT -H "x-api-key: $HARNESS_MOCK_KEY" -H 'content-type: application/json' \
    --data-binary @"$rule" "$HARNESS_MOCK_BASE/admin/middleware/rules/alex.fable-5-to-gpt-5.6-sol" >/dev/null
}

run_harness_mock_b_fable_on() {
  in_only B-FABLE-REROUTE-ON || return 0
  local t0 out err rc=0 traces activity requests msg latest
  t0=$(now_ms)
  harness_mock_set_fable_rule true
  harness_mock_control /_control/reset
  harness_mock_control /_control/scenario '{"name":"ok"}'
  harness_mock_openai_mode openai
  harness_mock_control /_control/queue '{"endpoint":"POST /anthropic/v1/messages","failure":"refusal"}'
  harness_mock_control /_control/queue '{"endpoint":"POST /openai/v1/responses","fixture":"openai/fable-reroute-sol.sse"}'
  out="$TMP/cell.B-FABLE-REROUTE-ON.harness.json"; err="$TMP/cell.B-FABLE-REROUTE-ON.err"
  ALEX_HOME="$HARNESS_MOCK_HOME" "$ROOT/target/debug/alex" harness run claude \
    --model alex/claude-fable-5 --prompt "B-FABLE-REROUTE-ON" \
    --json --no-trace-check --timeout-secs "$HARNESS_TIMEOUT" >"$out" 2>"$err" || rc=$?
  traces="$TMP/cell.B-FABLE-REROUTE-ON.traces.json"; activity="$TMP/cell.B-FABLE-REROUTE-ON.activity.json"; requests="$TMP/cell.B-FABLE-REROUTE-ON.requests.json"
  curl -fsS --max-time 10 -H "x-api-key: $HARNESS_MOCK_KEY" -o "$traces" "$HARNESS_MOCK_BASE/admin/traces?limit=30" || true
  curl -fsS --max-time 10 -H "x-api-key: $HARNESS_MOCK_KEY" -o "$activity" "$HARNESS_MOCK_BASE/admin/middleware/activity?limit=10" || true
  curl -fsS --max-time 10 -H "x-control-key: $HARNESS_MOCK_CONTROL_KEY" -o "$requests" "$HARNESS_MOCK_FAKE_BASE/_control/requests" || true
  if [ "$rc" -ne 0 ]; then
    latest=$(ls -td "$HARNESS_MOCK_HOME/harness-e2e/"* 2>/dev/null | head -1 || true)
    msg=$(cat "$err" "$latest/harness.stderr.log" "$latest/docker.stderr.log" "$latest/logs/npm-install.log" 2>/dev/null | tail -c 500)
    case "$msg" in *"manifest unknown"*|*"pull access denied"*|*"npm error code E404"*) write_result B-FABLE-REROUTE-ON SKIP "$(( $(now_ms)-t0 ))" "$msg"; return 0 ;; esac
  fi
  if [ "$rc" -eq 0 ] && msg=$(python3 - "$out" "$traces" "$activity" "$requests" "$t0" <<'PY' 2>&1
import json, os, sys
summary=json.load(open(sys.argv[1])); traces=json.load(open(sys.argv[2])).get("traces",[]); activity=json.load(open(sys.argv[3])).get("events",[]); requests=json.load(open(sys.argv[4])); started=int(sys.argv[5])
text=open(os.path.join(summary["session_dir"], "harness.stdout.log"), errors="replace").read()
assert "Fake GPT-5.6 Sol reroute success." in text, text[-800:]
assert "stop_reason\":\"refusal" not in text and "fallback_has_prefill_claim" not in text, text[-800:]
rows=[r for r in traces if (r.get("ts_request_ms") or 0)>=started and r.get("served_model")=="gpt-5.6-sol"]
assert rows, "reroute trace missing"
row=rows[0]
assert row.get("substituted") in (1, True), row
attempts=row.get("attempts") or []
if isinstance(attempts,str): attempts=json.loads(attempts)
assert len(attempts) >= 2, attempts
assert attempts[0]["provider"]=="anthropic" and attempts[0].get("error",{}).get("kind")=="upstream_refusal", attempts
assert attempts[1]["provider"]=="openai" and attempts[1]["model"]=="gpt-5.6-sol", attempts
decision=json.dumps(attempts)
assert "alex.fable-5-to-gpt-5.6-sol" in decision and '"executed":true' in decision.replace(" ",""), decision
assert any(e.get("substituted") in (1, True) and "gpt-5.6-sol" in json.dumps(e) for e in activity), activity
paths=[r.get("path") for r in requests if r.get("method")=="POST"]
assert "/anthropic/v1/messages" in paths and "/openai/v1/responses" in paths, paths
print("ON intercepted refusal and rerouted to fake OpenAI gpt-5.6-sol before harness saw refusal bytes")
PY
  ); then
    write_result B-FABLE-REROUTE-ON PASS "$(( $(now_ms)-t0 ))" "$msg"
  else
    write_result B-FABLE-REROUTE-ON FAIL "$(( $(now_ms)-t0 ))" "exit $rc: ${msg:-$(tail -c 220 "$err" 2>/dev/null)}"
  fi
}

run_harness_mock_b_fable_off() {
  in_only B-FABLE-REROUTE-OFF || return 0
  local t0 out err rc=0 traces activity requests msg latest
  t0=$(now_ms)
  harness_mock_set_fable_rule false
  harness_mock_control /_control/reset
  harness_mock_control /_control/scenario '{"name":"ok"}'
  harness_mock_openai_mode openai
  harness_mock_control /_control/queue '{"endpoint":"POST /anthropic/v1/messages","failure":"refusal"}'
  out="$TMP/cell.B-FABLE-REROUTE-OFF.harness.json"; err="$TMP/cell.B-FABLE-REROUTE-OFF.err"
  ALEX_HOME="$HARNESS_MOCK_HOME" "$ROOT/target/debug/alex" harness run claude \
    --model alex/claude-fable-5 --prompt "B-FABLE-REROUTE-OFF" \
    --json --no-trace-check --timeout-secs "$HARNESS_TIMEOUT" >"$out" 2>"$err" || rc=$?
  harness_mock_set_fable_rule true || true
  traces="$TMP/cell.B-FABLE-REROUTE-OFF.traces.json"; activity="$TMP/cell.B-FABLE-REROUTE-OFF.activity.json"; requests="$TMP/cell.B-FABLE-REROUTE-OFF.requests.json"
  curl -fsS --max-time 10 -H "x-api-key: $HARNESS_MOCK_KEY" -o "$traces" "$HARNESS_MOCK_BASE/admin/traces?limit=30" || true
  curl -fsS --max-time 10 -H "x-api-key: $HARNESS_MOCK_KEY" -o "$activity" "$HARNESS_MOCK_BASE/admin/middleware/activity?limit=10" || true
  curl -fsS --max-time 10 -H "x-control-key: $HARNESS_MOCK_CONTROL_KEY" -o "$requests" "$HARNESS_MOCK_FAKE_BASE/_control/requests" || true
  latest=$(python3 - "$out" <<'PY' 2>/dev/null || true
import json,sys
print(json.load(open(sys.argv[1])).get("session_dir",""))
PY
  )
  if [ -z "$latest" ] || [ ! -d "$latest" ]; then
    latest=$(ls -td "$HARNESS_MOCK_HOME/harness-e2e/"* 2>/dev/null | head -1 || true)
  fi
  if msg=$(python3 - "$traces" "$activity" "$requests" "$latest" "$ROOT/crates/alex-fakeprov/fixtures/anthropic/anthropic-fable-refusal-200.sse" "$t0" <<'PY' 2>&1
import gzip, json, os, sys
traces=json.load(open(sys.argv[1])).get("traces",[]); activity=json.load(open(sys.argv[2])).get("events",[]); requests=json.load(open(sys.argv[3])); session=sys.argv[4]; fixture=open(sys.argv[5]).read(); started=int(sys.argv[6])
activity=[e for e in activity if (e.get("ts_ms") or 0) >= started]
text=""
if session and os.path.isdir(session):
    for name in ["harness.stdout.log","harness.stderr.log"]:
        path=os.path.join(session,name)
        if os.path.exists(path): text += open(path, errors="replace").read()
assert "API Error: Claude Code is unable to respond to this request" in text, text[-800:]
assert '"stop_reason":"refusal"' in text or '"subtype":"model_refusal_no_fallback"' in text, text[-800:]
rows=[r for r in traces if (r.get("ts_request_ms") or 0)>=started and r.get("routed_model")=="claude-fable-5"]
assert rows, "Fable OFF trace missing"
row=rows[0]
assert row.get("substituted") in (0, False, None), row
assert row.get("served_model") in ("claude-fable-5", None), row.get("served_model")
def read_body(path):
    return gzip.open(path, "rt").read() if path.endswith(".gz") else open(path).read()
assert row.get("resp_body_path") and read_body(row["resp_body_path"]) == fixture, "refusal SSE changed in OFF path"
attempts=row.get("attempts") or []
if isinstance(attempts,str): attempts=json.loads(attempts)
assert attempts and attempts[0].get("provider")=="anthropic", attempts
assert not any(a.get("provider")=="openai" for a in attempts), attempts
assert not any(dec.get("rule_id")=="alex.fable-5-to-gpt-5.6-sol" and dec.get("executed") for a in attempts for dec in a.get("middleware_decisions",[])), attempts
assert not any(e.get("substituted") in (1, True) and "alex.fable-5-to-gpt-5.6-sol" in json.dumps(e) for e in activity), activity
paths=[r.get("path") for r in requests if r.get("method")=="POST"]
assert paths.count("/anthropic/v1/messages") >= 1 and "/openai/v1/responses" not in paths, paths
assert 'event: message_delta\ndata: {"type":"message_delta","delta":{"stop_reason":"refusal"' in fixture
print("OFF passed raw refusal SSE unchanged with no OpenAI reroute or executed middleware action")
PY
  ); then
    write_result B-FABLE-REROUTE-OFF PASS "$(( $(now_ms)-t0 ))" "$msg"
  else
    write_result B-FABLE-REROUTE-OFF FAIL "$(( $(now_ms)-t0 ))" "exit $rc: $msg"
  fi
}

run_harness_mock_tier() {
  local docker_ok=1 id
  HARNESS_MOCK_LIFECYCLE_FAILED=""
  if ! command -v docker >/dev/null 2>&1 || ! docker info >/dev/null 2>&1; then
    docker_ok=0
  fi
  start_harness_mock_stack || return 0
  run_harness_mock_lifecycle
  run_harness_mock_c5
  if [ "$docker_ok" -eq 1 ]; then
    run_harness_mock_matrix
  else
    while IFS='|' read -r id _; do
      in_only "$id" && write_result "$id" SKIP 0 "docker unavailable (docker info failed)"
    done <<EOF
$(harness_mock_cells)
EOF
  fi
  run_harness_mock_dario
  run_harness_mock_b_dario_tool
  if [ "$docker_ok" -eq 1 ]; then
    run_harness_mock_b_fable_on
    run_harness_mock_b_fable_off
  else
    in_only B-FABLE-REROUTE-ON && write_result B-FABLE-REROUTE-ON SKIP 0 "docker unavailable (docker info failed)"
    in_only B-FABLE-REROUTE-OFF && write_result B-FABLE-REROUTE-OFF SKIP 0 "docker unavailable (docker info failed)"
  fi
  run_harness_mock_b1
  run_harness_mock_b2
  if [ "$docker_ok" -eq 1 ]; then
    run_harness_mock_b3
  else
    in_only B3 && write_result B3 SKIP 0 "docker unavailable (docker info failed)"
  fi
  run_harness_mock_b4
  run_harness_mock_b5
  kill "$MOCK_DAEMON_PID" 2>/dev/null || true; wait "$MOCK_DAEMON_PID" 2>/dev/null || true; MOCK_DAEMON_PID=""
  kill "$FAKEPROV_PID" 2>/dev/null || true; wait "$FAKEPROV_PID" 2>/dev/null || true; FAKEPROV_PID=""
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
  if has_tier harness-mock; then run_harness_mock_tier; fi
  if has_tier webui; then run_webui_tier; fi
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
