#!/usr/bin/env bash
# air's measurements (docs/design/air.md), 3 runs where timing: macro
# expansion and metadata, an incremental rebuild after a one-line view edit,
# a clean build, a held ↓ over a 1000-row list in the Electron runtime.
# (The spike's 0.1 renderer columns in docs/design/air.md are its record.)
#
#   crates/air-bench/bench.sh <work dir> <target dir>
#
# Needs: the aio-rs checkout it lives in, `AIORT_RUNTIME` (an Electron
# runtime folder) for the held key, network-free (`--offline`).
set -euo pipefail
work=${1:?work dir}
target=${2:?target dir}
repo=$(cd "$(dirname "$0")/../.." && pwd)
runs=3
mkdir -p "$work"
cargo run -q --release -p air-bench --bin gen --offline -- "$work" "$repo"
cargo build -q --release -p air-bench --bin hold --offline

secs() {
  local a b
  a=$(date +%s.%N)
  "$@" >/dev/null 2>&1
  b=$(date +%s.%N)
  awk "BEGIN { printf \"%.1f\", $b - $a }"
}
build() { (cd "$work/$1" && CARGO_TARGET_DIR="$target/$1" cargo build -q --offline); }
check() { (cd "$work/$1" && CARGO_TARGET_DIR="$target/$1" cargo check -q --offline --lib); }

echo "## (a) expansion, metadata, LLVM IR (bytes / lines)"
echo "crate | expanded B | rmeta B | llvm-ir lines | bin B"
for c in air-1 air-50; do
  build "$c"
  exp=$(cd "$work/$c" && CARGO_TARGET_DIR="$target/$c" RUSTC_BOOTSTRAP=1 cargo rustc -q --lib --offline -- -Zunpretty=expanded | wc -c)
  rmeta=$(stat -c %s "$target/$c"/debug/deps/libviews-*.rmeta | sort -n | tail -1)
  (cd "$work/$c" && CARGO_TARGET_DIR="$target/$c" cargo rustc -q --lib --offline -- --emit=llvm-ir)
  ir=$(cat "$target/$c"/debug/deps/views-*.ll | wc -l)
  rm -f "$target/$c"/debug/deps/views-*.ll
  bin=$(stat -c %s "$target/$c/debug/bench-${c%-*}-${c#*-}")
  echo "$c | $exp | $rmeta | $ir | $bin"
done

echo "## (b) one-line view edit, then build (s; the lib and the binary)"
for c in air-50; do
  f="$work/$c/src/views/v25.rs"
  build "$c"
  t=()
  for i in $(seq $runs); do
    sed -i "s/\"Label 7\"/\"Label 7 $i\"/; s/\"Label 7 [a-z0-9]*\"/\"Label 7 $i\"/" "$f"
    t+=("$(secs build "$c")")
  done
  ct=()
  for i in $(seq $runs); do
    sed -i "s/\"Label 7 [a-z0-9]*\"/\"Label 7 c$i\"/" "$f"
    ct+=("$(secs check "$c")")
  done
  echo "$c | build ${t[*]} | check ${ct[*]}"
done

echo "## (c) clean build (s), crates compiled, crates in the graph"
for c in air-50; do
  t=()
  for i in $(seq $runs); do
    rm -rf "${target:?}/$c"
    t+=("$(secs build "$c")")
  done
  rm -rf "${target:?}/$c"
  n=$(cd "$work/$c" && CARGO_TARGET_DIR="$target/$c" cargo build --offline 2>&1 | grep -c Compiling || true)
  g=$(cd "$work/$c" && cargo tree --offline --prefix none -e normal,build 2>/dev/null | sed 's/ (\*)//' | sort -u | wc -l)
  echo "$c | ${t[*]} | compiled $n | graph $g"
done

echo "## (d) held ↓, keydown -> DOM applied, release, Electron headless"
for rows in 1000 10000; do
  for i in $(seq $runs); do
    "${CARGO_TARGET_DIR:-$repo/target}/release/hold" air 200 40 "$rows" 2>/dev/null | tail -1
  done
done
