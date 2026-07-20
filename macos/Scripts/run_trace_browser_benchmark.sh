#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "Trace Browser packaged benchmark requires macOS (AppKit/WindowServer)." >&2
  exit 2
fi

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
RESULT_PATH="${TMPDIR:-/tmp}/alex-trace-browser-benchmark.json"
ARTIFACTS_DIR=""
KEEP_WORK=0
SKIP_BUILD=0

usage() {
  cat <<'EOF'
Usage: macos/Scripts/run_trace_browser_benchmark.sh [options]

Options:
  --result PATH          Machine-readable result (default: temporary directory)
  --artifacts-dir PATH   Copy daemon/app/proxy/import logs here
  --keep-work            Keep the generated isolated ALEXANDRIA_HOME
  --skip-build           Reuse target/release/alex and macos/dist/Alex.app
  -h, --help             Show this help
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --result) RESULT_PATH="$2"; shift 2 ;;
    --artifacts-dir) ARTIFACTS_DIR="$2"; shift 2 ;;
    --keep-work) KEEP_WORK=1; shift ;;
    --skip-build) SKIP_BUILD=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
done

mkdir -p "$(dirname "$RESULT_PATH")"
RESULT_PATH="$(cd "$(dirname "$RESULT_PATH")" && pwd)/$(basename "$RESULT_PATH")"
if [[ -z "$ARTIFACTS_DIR" ]]; then
  ARTIFACTS_DIR="${RESULT_PATH%.json}-artifacts"
fi
mkdir -p "$ARTIFACTS_DIR"
ARTIFACTS_DIR="$(cd "$ARTIFACTS_DIR" && pwd)"

WORK_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/alex-trace-browser-benchmark.XXXXXX")"
BENCH_ROOT="$WORK_ROOT/alexandria-home"
LOG_DIR="$WORK_ROOT/logs"
mkdir -p "$LOG_DIR"
DAEMON_PID=""
PROXY_PID=""
APP_PID=""

copy_artifacts() {
  cp "$LOG_DIR"/*.log "$ARTIFACTS_DIR/" 2>/dev/null || true
  cp "$BENCH_ROOT/import-report.json" "$ARTIFACTS_DIR/" 2>/dev/null || true
  cp "$BENCH_ROOT/lar-verification.json" "$ARTIFACTS_DIR/" 2>/dev/null || true
  cp "$BENCH_ROOT/corpus-manifest.json" "$ARTIFACTS_DIR/" 2>/dev/null || true
}

cleanup() {
  local rc=$?
  [[ -z "$APP_PID" ]] || kill "$APP_PID" 2>/dev/null || true
  [[ -z "$PROXY_PID" ]] || kill "$PROXY_PID" 2>/dev/null || true
  [[ -z "$DAEMON_PID" ]] || kill "$DAEMON_PID" 2>/dev/null || true
  copy_artifacts
  if [[ "$KEEP_WORK" == 1 ]]; then
    echo "isolated benchmark workspace kept at: $WORK_ROOT" >&2
  else
    rm -rf "$WORK_ROOT"
  fi
  exit "$rc"
}
trap cleanup EXIT INT TERM

free_port() {
  python3 - <<'PY'
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
}

wait_for_health() {
  local url=$1
  for _ in $(seq 1 120); do
    if curl -fsS --max-time 2 "$url/health" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.25
  done
  return 1
}

cd "$REPO_ROOT"
if [[ "$SKIP_BUILD" == 0 ]]; then
  cargo build --release --locked -p alex --bin alex
  CONFIGURATION=release IDENTITY=- "$REPO_ROOT/macos/Scripts/package_app.sh"
fi

ALEX_BIN="$REPO_ROOT/target/release/alex"
APP_BIN="$REPO_ROOT/macos/dist/Alex.app/Contents/MacOS/AlexandriaBar"
[[ -x "$ALEX_BIN" ]] || { echo "missing $ALEX_BIN" >&2; exit 1; }
[[ -x "$APP_BIN" ]] || { echo "missing packaged app executable $APP_BIN" >&2; exit 1; }

LONG_SESSION_ID="019f6872-a3ee-7431-b4bb-2bafbabb7235-synthetic"
SHORT_SESSION_ID="synthetic-short-session-navigation"
BASE_MS="$(python3 -c 'import time; print(int(time.time() * 1000) - 16 * 60 * 60 * 1000)')"

python3 "$REPO_ROOT/scripts/generate-lar-corpus.py" "$BENCH_ROOT" \
  --turns 1277 \
  --duration-ms 54000000 \
  --base-ms "$BASE_MS" \
  --tool-lines 8 \
  --response-bytes 2048 \
  --session-id "$LONG_SESSION_ID" \
  --short-session-id "$SHORT_SESSION_ID" \
  --short-session-turns 3 \
  >"$LOG_DIR/generator.log"

ALEXANDRIA_HOME="$BENCH_ROOT" "$ALEX_BIN" lar import-legacy --json \
  >"$BENCH_ROOT/import-report.json" 2>"$LOG_DIR/import.log"
ALEXANDRIA_HOME="$BENCH_ROOT" "$ALEX_BIN" lar verify --json \
  >"$BENCH_ROOT/lar-verification.json" 2>"$LOG_DIR/lar-verification.log"
python3 - "$BENCH_ROOT/import-report.json" "$BENCH_ROOT/lar-verification.json" <<'PY'
import json, sys
import_report = json.load(open(sys.argv[1], encoding="utf-8"))
verification = json.load(open(sys.argv[2], encoding="utf-8"))
if import_report.get("job_state") != "complete":
    raise SystemExit(f"LAR import incomplete: {import_report.get('job_state')}")
if import_report.get("failed") != 0 or import_report.get("remaining_items") != 0:
    raise SystemExit(f"LAR import has failures or remaining work: {import_report}")
if import_report.get("migrated", 0) <= 0:
    raise SystemExit("LAR import migrated no generated artifacts")
if not verification.get("valid"):
    raise SystemExit(f"LAR verification failed: {verification}")
if verification.get("artifacts_checked", 0) <= 0:
    raise SystemExit("LAR verification checked no artifact pointers")
PY

# The generated gzip files are removed only from this mktemp fixture. Every
# browser body read must now succeed through the imported LAR archive.
[[ "$BENCH_ROOT" == "$WORK_ROOT/alexandria-home" && -d "$BENCH_ROOT/bodies" ]] || {
  echo "refusing to remove unresolved benchmark body directory" >&2
  exit 1
}
rm -r -- "$BENCH_ROOT/bodies"

DAEMON_PORT="$(free_port)"
PROXY_READY="$BENCH_ROOT/proxy-ready.json"
python3 "$REPO_ROOT/scripts/trace-browser-benchmark-proxy.py" \
  --upstream-port "$DAEMON_PORT" \
  --listen-port 0 \
  --long-session-id "$LONG_SESSION_ID" \
  --ready-file "$PROXY_READY" \
  >"$LOG_DIR/proxy.stdout.log" 2>"$LOG_DIR/proxy.log" &
PROXY_PID=$!

for _ in $(seq 1 100); do
  [[ -s "$PROXY_READY" ]] && break
  kill -0 "$PROXY_PID" 2>/dev/null || { echo "delay proxy exited early" >&2; exit 1; }
  sleep 0.05
done
[[ -s "$PROXY_READY" ]] || { echo "delay proxy did not publish its port" >&2; exit 1; }
PROXY_PORT="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["port"])' "$PROXY_READY")"

# The daemon receives a one-shot direct port override; the packaged app reads
# the persisted proxy port from this isolated config.toml.
sed -i '' -E "s/^port = [0-9]+/port = $PROXY_PORT/" "$BENCH_ROOT/config.toml"
grep -Fx "port = $PROXY_PORT" "$BENCH_ROOT/config.toml" >/dev/null || {
  echo "isolated config did not adopt proxy port $PROXY_PORT" >&2
  exit 1
}

ALEXANDRIA_HOME="$BENCH_ROOT" \
ALEXANDRIA_LAR_BODY_STORE=lar-with-fallback \
  "$ALEX_BIN" daemon --host 127.0.0.1 --port "$DAEMON_PORT" \
  >"$LOG_DIR/daemon.log" 2>&1 &
DAEMON_PID=$!
if ! wait_for_health "http://127.0.0.1:$DAEMON_PORT"; then
  echo "isolated LAR-backed daemon did not become healthy" >&2
  tail -100 "$LOG_DIR/daemon.log" >&2 || true
  exit 1
fi
if ! wait_for_health "http://127.0.0.1:$PROXY_PORT"; then
  echo "benchmark delay proxy did not reach the daemon" >&2
  exit 1
fi

rm -f "$RESULT_PATH"
ALEXANDRIA_HOME="$BENCH_ROOT" \
ALEX_TRACE_BROWSER_BENCHMARK=1 \
ALEX_TRACE_BROWSER_BENCHMARK_RESULT="$RESULT_PATH" \
ALEX_TRACE_BROWSER_BENCHMARK_LONG_SESSION="$LONG_SESSION_ID" \
ALEX_TRACE_BROWSER_BENCHMARK_SHORT_SESSION="$SHORT_SESSION_ID" \
  "$APP_BIN" >"$LOG_DIR/app.log" 2>&1 &
APP_PID=$!

for _ in $(seq 1 180); do
  [[ -s "$RESULT_PATH" ]] && break
  if ! kill -0 "$APP_PID" 2>/dev/null; then
    wait "$APP_PID" || true
    echo "packaged app exited without a benchmark result" >&2
    tail -100 "$LOG_DIR/app.log" >&2 || true
    exit 1
  fi
  sleep 1
done
[[ -s "$RESULT_PATH" ]] || { echo "benchmark timed out after 180 seconds" >&2; exit 1; }

wait "$APP_PID" || true
APP_PID=""
python3 - "$RESULT_PATH" <<'PY'
import json, sys
path = sys.argv[1]
result = json.load(open(path, encoding="utf-8"))
if result.get("schema") != "alex-trace-browser-packaged-benchmark-v1":
    raise SystemExit(f"unexpected result schema in {path}")
if not result.get("passed"):
    print(json.dumps(result, indent=2, sort_keys=True), file=sys.stderr)
    raise SystemExit("Trace Browser packaged benchmark failed")
print(json.dumps(result, indent=2, sort_keys=True))
PY

echo "benchmark result: $RESULT_PATH" >&2
echo "benchmark artifacts: $ARTIFACTS_DIR" >&2
