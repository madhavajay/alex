#!/usr/bin/env bash
set -euo pipefail

# lib: install-common.sh (curl-to-sh entry; shared helpers stay inline.)

usage() {
  cat <<'EOF'
Usage: ./install.sh [--service] [--upgrade] [--prefix DIR]

  (none)     build --release and install the alex binary (+ alexandria symlink) system-wide
  --service  also install + load the launchd agent (macOS) so it runs at login
  --upgrade  zero-downtime deploy: build + install, start a NEW daemon on the
             same port (SO_REUSEPORT), wait until it is healthy, then SIGTERM
             the old one so it drains in-flight requests (incl. SSE) and exits
  --prefix   install dir (default: /usr/local/bin, falls back to ~/.local/bin)
One daemon per machine. During --upgrade two daemons briefly share the port;
new connections land on either until the old one stops accepting, then all
traffic flows to the new binary. The old daemon's dario generation drains and
dies with it; the new daemon spawns its own.

Little Snitch note: approving the installed path once
(e.g. /usr/local/bin/alex) survives upgrades that reuse that path.
EOF
}

SERVICE=0
UPGRADE=0
PREFIX=""
while [ $# -gt 0 ]; do
  case "$1" in
    --service) SERVICE=1 ;;
    --upgrade) UPGRADE=1 ;;
    --prefix) PREFIX="$2"; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown flag: $1" >&2; usage; exit 2 ;;
  esac
  shift
done

ROOT="$(cd "$(dirname "$0")" && pwd)"
cd "$ROOT"


if [ -z "$PREFIX" ]; then
  if [ -w /usr/local/bin ] 2>/dev/null; then
    PREFIX=/usr/local/bin
  elif command -v sudo >/dev/null && sudo -n true 2>/dev/null; then
    PREFIX=/usr/local/bin
  else
    PREFIX="$HOME/.local/bin"
  fi
fi
BIN="$PREFIX/alex"
ALIAS_BIN="$PREFIX/alexandria"

say() {
  echo "$1"
}

is_alex_daemon_pid() {
  ps -p "$1" -o command= 2>/dev/null | awk '
    NR == 1 {
      binary = $1
      sub(/^.*\//, "", binary)
      exit !((binary == "alex" || binary == "alexandria") && $2 == "daemon")
    }
    END { if (NR == 0) exit 1 }
  '
}

say "◆ building release binary…"
BUILD_LOG=$(mktemp -t alx-build)
if ! cargo build --release > "$BUILD_LOG" 2>&1; then
  echo "build failed:" >&2
  tail -30 "$BUILD_LOG" >&2
  exit 1
fi
say "◆ build complete — installing to $PREFIX"
mkdir -p "$PREFIX" 2>/dev/null || true
if [ -w "$PREFIX" ]; then
  install -m 0755 target/release/alex "$BIN"
  ln -sf "$BIN" "$ALIAS_BIN"
else
  sudo install -m 0755 target/release/alex "$BIN"
  sudo ln -sf "$BIN" "$ALIAS_BIN"
fi
say "◆ installed $BIN"
case ":$PATH:" in
  *":$PREFIX:"*) ;;
  *) echo "note: $PREFIX is not on your PATH" ;;
esac

say "◆ preparing Dario runtime…"
if ! "$BIN" dario bootstrap; then
  echo "warning: Dario could not be prepared; install Node.js 18+ and npm, pnpm, or Bun, then run: $BIN dario bootstrap" >&2
fi

CONFIG="$HOME/.alexandria/config.toml"
PORT=4100
HOST=127.0.0.1
if [ -f "$CONFIG" ]; then
  PORT=$(sed -n 's/^port *= *\([0-9]*\)/\1/p' "$CONFIG" | head -1)
  HOST=$(sed -n 's/^host *= *"\(.*\)"/\1/p' "$CONFIG" | head -1)
  PORT=${PORT:-4100}
  HOST=${HOST:-127.0.0.1}
fi
CHECK_HOST=$HOST
[ "$CHECK_HOST" = "0.0.0.0" ] && CHECK_HOST=127.0.0.1

PLIST_LABEL=com.alexandria.daemon
PLIST_DST="$HOME/Library/LaunchAgents/$PLIST_LABEL.plist"

SYSTEMD_UNIT="$HOME/.config/systemd/user/alexandria.service"

if [ "$SERVICE" = "1" ]; then
  "$BIN" service install
fi

if [ "$UPGRADE" = "1" ]; then
  say "◆ zero-downtime upgrade on $CHECK_HOST:$PORT"
  if ! command -v lsof >/dev/null; then
    echo "lsof not found — cannot discover the old daemon; restart it manually" >&2
    exit 1
  fi
  LISTENER_PIDS=$(lsof -ti "tcp:$PORT" -sTCP:LISTEN 2>/dev/null || true)
  OLD_PIDS=""
  for pid in $LISTENER_PIDS; do
    if ! is_alex_daemon_pid "$pid"; then
      echo "refusing to replace non-Alexandria listener pid $pid on port $PORT" >&2
      exit 1
    fi
    OLD_PIDS="${OLD_PIDS}${OLD_PIDS:+ }$pid"
  done
  if [ -z "$OLD_PIDS" ]; then
    say "no running daemon found; starting fresh"
    nohup "$BIN" daemon >> "$HOME/.alexandria/daemon.log" 2>&1 &
    NEW_PID=$!
  elif [ "$(uname)" = "Darwin" ] && launchctl print "gui/$(id -u)/$PLIST_LABEL" >/dev/null 2>&1; then
    echo "daemon is launchd-managed: using kickstart (drain happens before restart;"
    echo "expect a short accept gap while the old instance drains)"
    launchctl kickstart -k "gui/$(id -u)/$PLIST_LABEL"
    NEW_PID=""
  elif [ "$(uname)" = "Linux" ] && command -v systemctl >/dev/null \
      && systemctl --user is-active alexandria >/dev/null 2>&1; then
    echo "daemon is systemd-managed: using systemctl restart (drain happens on stop;"
    echo "expect a short accept gap while the old instance drains)"
    systemctl --user restart alexandria
    NEW_PID=""
  else
    say "old daemon pid(s): $OLD_PIDS"
    nohup "$BIN" daemon >> "$HOME/.alexandria/daemon.log" 2>&1 &
    NEW_PID=$!
    say "started new daemon pid $NEW_PID; waiting for it to listen"
    i=0
    until lsof -a -p "$NEW_PID" -i "tcp:$PORT" -sTCP:LISTEN >/dev/null 2>&1; do
      i=$((i + 1))
      if [ "$i" -gt 120 ]; then
        echo "new daemon never bound port $PORT — leaving old daemon untouched" >&2
        kill "$NEW_PID" 2>/dev/null || true
        exit 1
      fi
      if ! kill -0 "$NEW_PID" 2>/dev/null; then
        echo "new daemon exited during startup — leaving old daemon untouched; see ~/.alexandria/daemon.log" >&2
        if tail -5 "$HOME/.alexandria/daemon.log" 2>/dev/null | grep -q "Address already in use"; then
          echo "the running daemon predates SO_REUSEPORT support, so the port cannot be shared." >&2
          echo "one-time migration: stop it (kill \$(lsof -ti tcp:$PORT -sTCP:LISTEN)), start the new" >&2
          echo "binary once ($BIN daemon), and every future --upgrade will be zero-downtime." >&2
        fi
        exit 1
      fi
      sleep 0.5
    done
    curl -fsS --max-time 5 "http://$CHECK_HOST:$PORT/health" >/dev/null
    say "new daemon healthy; draining old daemon(s): $OLD_PIDS"
    for pid in $OLD_PIDS; do
      [ "$pid" = "$NEW_PID" ] && continue
      if ! is_alex_daemon_pid "$pid"; then
        echo "not TERMing pid $pid: it is no longer an Alexandria daemon" >&2
        continue
      fi
      kill -TERM "$pid" 2>/dev/null || true
    done
  fi
  say "◆ upgrade complete — old instance drains in-flight requests then exits"

  sleep 1
  curl -fsS --max-time 5 "http://$CHECK_HOST:$PORT/health" && echo
fi

"$BIN" --version
