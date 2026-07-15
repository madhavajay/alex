#!/usr/bin/env bash
# Build container-friendly alex binaries for the two common Linux architectures.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

targets=(
  x86_64-unknown-linux-musl
  aarch64-unknown-linux-musl
)

if command -v cargo-zigbuild >/dev/null 2>&1 && command -v zig >/dev/null 2>&1; then
  builder=(cargo zigbuild)
  choice="cargo zigbuild (Zig cross-linker)"
elif command -v cross >/dev/null 2>&1 && command -v docker >/dev/null 2>&1; then
  builder=(cross)
  choice="cross (Docker)"
else
  builder=(cargo)
  choice="plain cargo (installed Rust target and musl linker required)"
fi

echo "Linux builder: $choice"
built_targets=()
for target in "${targets[@]}"; do
  echo "Building alex for $target"
  if "${builder[@]}" build -p alex --bin alex --release --target "$target"; then
    built_targets+=("$target")
    continue
  fi

  gnu_target="${target%musl}gnu"
  echo "musl build for $target failed; falling back to $gnu_target"
  "${builder[@]}" build -p alex --bin alex --release --target "$gnu_target"
  built_targets+=("$gnu_target")
done

echo "Built binaries:"
for target in "${built_targets[@]}"; do
  echo "  $repo_root/target/$target/release/alex"
done
