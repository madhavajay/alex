#!/usr/bin/env bash
# Offline suite runner: all mock-backed tiers in parallel lanes.
# Lanes: rust (workspace), mock (daemon+fakeprov e2e), swift (macOS only), webui (Playwright).
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$ROOT"

LOGDIR="$(mktemp -d /tmp/alex-test-all.XXXXXX)"
declare -a LANES=()
declare -a PIDS=()
declare -a STARTS=()

note() { printf '%s\n' "$*"; }

note "logs: $LOGDIR"
note "prebuild: shared cargo artifacts (avoids target-dir lock contention between lanes)"
if ! cargo build -q -p alex -p alex-fakeprov 2>"$LOGDIR/prebuild.log"; then
  cat "$LOGDIR/prebuild.log"
  exit 1
fi
cargo test -q --workspace --no-run 2>>"$LOGDIR/prebuild.log" || true

launch() {
  local name="$1"; shift
  LANES+=("$name")
  STARTS+=("$(date +%s)")
  ( "$@" ) >"$LOGDIR/$name.log" 2>&1 &
  PIDS+=($!)
  note "lane started: $name"
}

webui_lane() {
  cd webui-tests
  if ! command -v pnpm >/dev/null 2>&1; then
    corepack enable pnpm >/dev/null 2>&1 || npm install -g pnpm
  fi
  pnpm install --frozen-lockfile
  pnpm exec playwright install chromium
  pnpm test
}

launch rust  cargo test --workspace
launch mock  ./test.sh mock
launch webui webui_lane
if [ "$(uname -s)" = "Darwin" ]; then
  launch swift bash -c 'cd macos && swift test'
else
  note "lane skipped: swift (not macOS)"
fi

FAIL=0
ROWS=""
for i in "${!LANES[@]}"; do
  name="${LANES[$i]}"
  if wait "${PIDS[$i]}"; then
    status="PASS"
  elif [ "$name" = "swift" ]; then
    # Wall-clock perf assertions in the Swift suite can miss their budget
    # under full-CPU lane contention; a serial rerun is authoritative.
    note "swift lane failed under contention; rerunning serially"
    if (cd macos && swift test) >>"$LOGDIR/swift.log" 2>&1; then
      status="PASS*"
    else
      status="FAIL"; FAIL=1
    fi
  else
    status="FAIL"; FAIL=1
  fi
  secs=$(( $(date +%s) - STARTS[i] ))
  ROWS+="$(printf '%-6s %-5s %4ss  %s' "$name" "$status" "$secs" "$LOGDIR/$name.log")"$'\n'
done

echo
printf '%-6s %-5s %5s  %s\n' LANE STATUS TIME LOG
printf '%s' "$ROWS"
[ "$(uname -s)" != "Darwin" ] && printf '%-6s %-5s %5s\n' swift SKIP -

if [ "$FAIL" -ne 0 ]; then
  echo
  for i in "${!LANES[@]}"; do
    name="${LANES[$i]}"
    grep -lq . "$LOGDIR/$name.log" 2>/dev/null || continue
  done
  echo "failure detail: tail the FAIL lane logs above"
  exit 1
fi
echo
echo "all lanes passed"
