#!/usr/bin/env bash
# Build GNU and musl alex binaries for the two supported Linux architectures.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

targets=(
  x86_64-unknown-linux-gnu
  x86_64-unknown-linux-musl
  aarch64-unknown-linux-gnu
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
for target in "${targets[@]}"; do
  echo "Building alex for $target"
  "${builder[@]}" build -p alex --bin alex --release --target "$target"
done

echo "Built binaries:"
for target in "${targets[@]}"; do
  echo "  $repo_root/target/$target/release/alex"
done
