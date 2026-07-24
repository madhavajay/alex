#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
DOCKERFILE="$SCRIPT_DIR/Dockerfile.systemd"
CONTAINER_USER="alex-smoke"
CONTAINER_UID="1001"
CONTAINER_HOME="/home/$CONTAINER_USER"
CONTAINER_ROOT="$CONTAINER_HOME/alex-installed-linux"
CONTAINER_PAYLOAD="/opt/alex-ci"
CONTAINER_RESULT="$CONTAINER_PAYLOAD/output/installed-smoke.json"
SERVICE_NAME="alex"

fail() {
  echo "Linux systemd container: $*" >&2
  exit 1
}

for command in docker mktemp; do
  command -v "$command" >/dev/null 2>&1 || fail "missing required command: $command"
done

RESULT_PATH="${ALEX_CI_LINUX_SMOKE_RESULT:?set ALEX_CI_LINUX_SMOKE_RESULT}"
RESULT_TMP="$RESULT_PATH.tmp"
VERSION="${ALEX_CI_LINUX_VERSION:?set ALEX_CI_LINUX_VERSION}"
ASSET_DIR="${ALEX_CI_LINUX_ASSET_DIR:?set ALEX_CI_LINUX_ASSET_DIR}"
CONTAINER_NAME="${ALEX_CI_LINUX_CONTAINER_NAME:-alex-linux-installed-smoke}"
ARCH="${ALEX_CI_LINUX_ARCH:-x86_64}"
LIBC="${ALEX_CI_LINUX_LIBC:-gnu}"
case "$ARCH" in
  x86_64)
    DOCKER_PLATFORM="linux/amd64"
    ;;
  aarch64)
    DOCKER_PLATFORM="linux/arm64"
    ;;
  *)
    fail "ALEX_CI_LINUX_ARCH must be x86_64 or aarch64"
    ;;
esac
case "$LIBC" in
  gnu) ASSET_NAME="alex-cli-$VERSION-linux-$ARCH.tar.gz" ;;
  musl) ASSET_NAME="alex-cli-$VERSION-linux-$ARCH-musl.tar.gz" ;;
  *) fail "ALEX_CI_LINUX_LIBC must be gnu or musl" ;;
esac
IMAGE_TAG="alex-linux-systemd-smoke-$ARCH-$LIBC:ubuntu-24.04"

[[ "$RESULT_PATH" = /* ]] || fail "ALEX_CI_LINUX_SMOKE_RESULT must be absolute"
[[ "$ASSET_DIR" = /* ]] || fail "ALEX_CI_LINUX_ASSET_DIR must be absolute"
[[ "$CONTAINER_NAME" =~ ^[a-zA-Z0-9][a-zA-Z0-9_.-]*$ ]] \
  || fail "ALEX_CI_LINUX_CONTAINER_NAME contains unsupported characters"
[[ -f "$ASSET_DIR/$ASSET_NAME" ]] || fail "candidate archive is missing"
[[ -f "$ASSET_DIR/$ASSET_NAME.sha256" ]] || fail "candidate checksum is missing"
[[ -x "$REPO_ROOT/install-release.sh" ]] || fail "release installer is not executable"
[[ -x "$SCRIPT_DIR/smoke-installed.sh" ]] || fail "installed smoke is not executable"
[[ -x "$SCRIPT_DIR/serve-assets.py" ]] || fail "asset server is not executable"
[[ -x "$REPO_ROOT/packaging/ci-macos/mock-openai.py" ]] \
  || fail "shared deterministic OpenAI mock is not executable"
rm -f "$RESULT_PATH" "$RESULT_TMP"

PAYLOAD_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/alex-linux-systemd.XXXXXX")"
CONTAINER_STARTED=0

user_exec() {
  docker exec \
    --user "$CONTAINER_UID:$CONTAINER_UID" \
    --env "HOME=$CONTAINER_HOME" \
    --env "USER=$CONTAINER_USER" \
    --env "LOGNAME=$CONTAINER_USER" \
    --env "XDG_RUNTIME_DIR=/run/user/$CONTAINER_UID" \
    --env "DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/$CONTAINER_UID/bus" \
    "$CONTAINER_NAME" \
    "$@"
}

cleanup() {
  status=$?
  trap - EXIT INT TERM
  set +e

  if [[ "$CONTAINER_STARTED" -eq 1 ]] \
    && docker container inspect "$CONTAINER_NAME" >/dev/null 2>&1; then
    if [[ "$status" -ne 0 ]]; then
      echo "::group::Linux installed-service diagnostics"
      user_exec systemctl --user status "$SERVICE_NAME" --no-pager --full || true
      user_exec journalctl --user-unit="$SERVICE_NAME" --no-pager -n 200 || true
      docker exec "$CONTAINER_NAME" cat \
        "$CONTAINER_HOME/.config/systemd/user/$SERVICE_NAME.service" || true
      echo "::endgroup::"
    fi
    user_exec systemctl --user disable --now "$SERVICE_NAME" >/dev/null 2>&1
    docker exec "$CONTAINER_NAME" systemctl stop "user@$CONTAINER_UID.service" \
      >/dev/null 2>&1
    docker rm --force "$CONTAINER_NAME" >/dev/null 2>&1
  fi
  rm -f "$RESULT_TMP"
  rm -rf "$PAYLOAD_ROOT"

  if docker container inspect "$CONTAINER_NAME" >/dev/null 2>&1; then
    echo "Linux systemd container: cleanup failed for $CONTAINER_NAME" >&2
    [[ "$status" -ne 0 ]] || status=1
  fi
  exit "$status"
}
trap cleanup EXIT INT TERM

if docker container inspect "$CONTAINER_NAME" >/dev/null 2>&1; then
  fail "container already exists: $CONTAINER_NAME"
fi

mkdir -p \
  "$PAYLOAD_ROOT/repo/packaging/ci-linux" \
  "$PAYLOAD_ROOT/repo/packaging/ci-macos" \
  "$PAYLOAD_ROOT/assets" \
  "$PAYLOAD_ROOT/output"
cp "$REPO_ROOT/install-release.sh" "$PAYLOAD_ROOT/repo/install-release.sh"
cp "$SCRIPT_DIR/smoke-installed.sh" "$PAYLOAD_ROOT/repo/packaging/ci-linux/"
cp "$SCRIPT_DIR/serve-assets.py" "$PAYLOAD_ROOT/repo/packaging/ci-linux/"
cp "$REPO_ROOT/packaging/ci-macos/mock-openai.py" \
  "$PAYLOAD_ROOT/repo/packaging/ci-macos/"
cp "$ASSET_DIR/$ASSET_NAME" "$ASSET_DIR/$ASSET_NAME.sha256" "$PAYLOAD_ROOT/assets/"

docker build \
  --platform "$DOCKER_PLATFORM" \
  --file "$DOCKERFILE" \
  --tag "$IMAGE_TAG" \
  "$SCRIPT_DIR"

docker run \
  --detach \
  --name "$CONTAINER_NAME" \
  --platform "$DOCKER_PLATFORM" \
  --privileged \
  --cgroupns host \
  --tmpfs /run \
  --tmpfs /run/lock \
  --volume /sys/fs/cgroup:/sys/fs/cgroup:rw \
  "$IMAGE_TAG" >/dev/null
CONTAINER_STARTED=1

SYSTEM_READY=0
for ((attempt = 0; attempt < 100; attempt++)); do
  state="$(docker exec "$CONTAINER_NAME" systemctl is-system-running 2>/dev/null || true)"
  if [[ "$state" == "running" || "$state" == "degraded" ]]; then
    SYSTEM_READY=1
    break
  fi
  docker container inspect --format '{{.State.Running}}' "$CONTAINER_NAME" \
    | grep -Fqx true || fail "container exited before systemd became ready"
  sleep 0.2
done
[[ "$SYSTEM_READY" -eq 1 ]] || fail "container systemd did not become ready"

docker exec "$CONTAINER_NAME" mkdir -p "$CONTAINER_PAYLOAD"
docker cp "$PAYLOAD_ROOT/." "$CONTAINER_NAME:$CONTAINER_PAYLOAD/"
docker exec "$CONTAINER_NAME" chown -R "$CONTAINER_UID:$CONTAINER_UID" \
  "$CONTAINER_PAYLOAD" "$CONTAINER_HOME"

# Linger gives the non-root account a durable user manager without fabricating
# a shell login. The smoke then talks to that manager over its real user bus.
docker exec "$CONTAINER_NAME" loginctl enable-linger "$CONTAINER_USER"
[[ "$(docker exec "$CONTAINER_NAME" loginctl show-user "$CONTAINER_USER" \
  --property Linger --value)" == "yes" ]] || fail "linger was not enabled"
docker exec "$CONTAINER_NAME" systemctl start "user@$CONTAINER_UID.service"
USER_MANAGER_READY=0
for ((attempt = 0; attempt < 100; attempt++)); do
  if user_exec systemctl --user show-environment >/dev/null 2>&1; then
    USER_MANAGER_READY=1
    break
  fi
  sleep 0.1
done
[[ "$USER_MANAGER_READY" -eq 1 ]] || fail "non-root systemd user manager is unavailable"

user_exec env \
  "ALEX_CI_LINUX_SMOKE_ROOT=$CONTAINER_ROOT" \
  "ALEX_CI_LINUX_SMOKE_RESULT=$CONTAINER_RESULT" \
  "ALEX_CI_LINUX_VERSION=$VERSION" \
  "ALEX_CI_LINUX_ARCH=$ARCH" \
  "ALEX_CI_LINUX_LIBC=$LIBC" \
  "ALEX_CI_LINUX_ASSET_DIR=$CONTAINER_PAYLOAD/assets" \
  "$CONTAINER_PAYLOAD/repo/packaging/ci-linux/smoke-installed.sh"

mkdir -p "$(dirname "$RESULT_PATH")"
docker cp "$CONTAINER_NAME:$CONTAINER_RESULT" "$RESULT_TMP"
mv "$RESULT_TMP" "$RESULT_PATH"

[[ -s "$RESULT_PATH" ]] || fail "smoke result was not copied out of the container"
echo "Linux systemd container passed; evidence: $RESULT_PATH"
