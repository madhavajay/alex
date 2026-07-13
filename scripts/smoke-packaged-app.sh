#!/usr/bin/env bash
set -euo pipefail

APP="${1:-macos/dist/AlexandriaBar.app}"
BIN="$APP/Contents/MacOS/AlexandriaBar"
SECONDS_TO_WATCH="${SECONDS_TO_WATCH:-10}"

if [[ ! -x "$BIN" ]]; then
  echo "packaged app executable not found: $BIN" >&2
  exit 1
fi

LOG="$(mktemp "${TMPDIR:-/tmp}/alexandria-packaged-smoke.XXXXXX.log")"
PID=""

cleanup() {
  if [[ -n "$PID" ]] && kill -0 "$PID" 2>/dev/null; then
    kill "$PID" 2>/dev/null || true
    wait "$PID" 2>/dev/null || true
  fi
  rm -f "$LOG"
}
trap cleanup EXIT

"$BIN" >"$LOG" 2>&1 &
PID=$!

for ((second = 1; second <= SECONDS_TO_WATCH; second++)); do
  if ! kill -0 "$PID" 2>/dev/null; then
    wait "$PID" || status=$?
    echo "packaged app exited during smoke test (status ${status:-unknown})" >&2
    tail -n 100 "$LOG" >&2 || true
    exit 1
  fi
  sleep 1
done

echo "packaged app remained alive for ${SECONDS_TO_WATCH}s"
