#!/bin/sh
# What CI checks. Run it before pushing.
set -eu
cd "$(dirname "$0")"
# Headless, whoever runs it (a person's desktop, an agent, CI): no step may
# reach a screen or the session bus (no notification, portal or dialog).
# A step that needs a display starts its own Xvfb (aio-rs-cli's display tests,
# `ax build --smoke`); the UI tests draw in Electron's headless ozone.
unset DISPLAY WAYLAND_DISPLAY
export DBUS_SESSION_BUS_ADDRESS=disabled:
# Its three parts, each alone with `./ci.sh lint|test|dx` (GitHub CI runs
# them side by side); no part: all three, in that order.
part="${1:-all}"
case "$part" in
  all | lint | test | dx) ;;
  *) echo "ci: no part $part (lint, test or dx)" >&2; exit 2 ;;
esac
on() { [ "$part" = all ] || [ "$part" = "$1" ]; }
web_skip=""
# Android's NDK, when it is here.
ndk=$(ls -d "${ANDROID_HOME:-$HOME/Android/Sdk}"/ndk/* 2>/dev/null | sort -V | tail -1)
if on lint; then
cargo fmt --all --check
# The MSRV the crates declare is the toolchain CI builds with (rust-toolchain.toml).
msrv=$(sed -n 's/^channel = "\([0-9]*\.[0-9]*\).*/\1/p' rust-toolchain.toml)
grep -q "^rust-version = \"$msrv\"" Cargo.toml || { echo "ci: Cargo.toml's rust-version is not rust-toolchain.toml's $msrv" >&2; exit 1; }
# Every crate listed with its reason, no unlisted duplicate, no set above its ceiling (deps.toml).
scripts/deps-check
scripts/slim-metrics --check
# The published crates' public API as checked in (scripts/api, TS21): a change
# shows in review, and none names aio-rs 0.1's renderer.
scripts/public-api --check
scripts/dx-metrics --self-test
# A packaged app ships no other renderer: each of air-ship's checks red on a planted one.
scripts/air-ship --self-test
# The guide (docs/): links resolve, each page with Rust is a doctest (run by
# the workspace tests below), the verb list is whole, the wording public.
scripts/docs-check
# The mutation ledger holds together: each break's line found once, its test there
# (scripts/mutants runs the breaks; GitHub CI runs the fast ones).
scripts/mutants --check
# air is aio-rs's one renderer since 0.2.0: no crate of the workspace's
# graph (every target, feature and kind) is Dioxus's, and no file names it
# but the record (CHANGELOG, the migration guide, the design notes), the
# tools that refuse or find it (ax lint, air-ship, public-api, dx-metrics' planted
# dependency, the 0.1 migration script), license attributions and the
# comparisons with other frameworks.
if cargo tree --locked --workspace --all-features --target all -e all --prefix none | grep -i dioxus; then
  echo "ci: the crates above are Dioxus's" >&2
  exit 1
fi
if git grep -n -I -i dioxus -- . ':!CHANGELOG.md' ':!docs/migration' ':!docs/design' ':!docs/comparison.md' \
  ':!crates/aio-cli/src/lint' ':!scripts/air-ship' ':!scripts/dx-metrics' ':!scripts/aio-ui-migrate' ':!scripts/public-api' ':!ci.sh' \
  | grep -v -e 'Ported from' -e '[Cc]ompar'; then
  echo "ci: the lines above name Dioxus: aio::ui is air's (the migration guide has the names)" >&2
  exit 1
fi
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
# The API docs agents read (`cargo doc -p aio-rs`): every link resolves.
RUSTDOCFLAGS='-D warnings' cargo doc --no-deps -p aio-rs -p aio-rs-shell -p aio-rs-config -p aio-rs-macros -p aio-rs-config-macro -p aio-rs-air \
  --all-features --locked
# Each feature alone stays clean (none: the core a standalone app builds).
if cargo hack --version >/dev/null 2>&1; then
  cargo hack clippy -p aio-rs --lib --each-feature --locked -- -D warnings
else
  for f in "" outbound server tls oidc br net kit air imbl updates web dev testing video compat; do
    cargo clippy -p aio-rs --lib --no-default-features --features "$f" --locked -- -D warnings
  done
fi
# The shell: each feature alone and with `dev`.
for f in "" web dev web,dev updates; do
  cargo clippy -p aio-rs-shell --lib --no-default-features --features "$f" --locked -- -D warnings
done
# Working with aio over the network: aio pages and tools on `/ws`, `AioLink`.
cargo clippy -p aio-rs --lib --no-default-features --features compat,net --locked -- -D warnings
# air's code-size rule: LLVM IR lines per view of the bench's 50 views.
air_ir=$(mktemp -d)
scripts/air-ir "$air_ir"
rm -rf "$air_ir"
# The browser build (`ax build --web`): aio's core in the page, no SQLite, no ring.
cargo clippy -p aio-rs --lib --no-default-features --features web --locked --target wasm32-unknown-unknown -- -D warnings
# With the kit: its views drawn by air's interpreter, its controls and the sign-in form in the page.
cargo clippy -p aio-rs --lib --no-default-features --features web,kit --locked --target wasm32-unknown-unknown -- -D warnings
# Windows: the same lint (its tests: below, and GitHub CI's Windows job).
cargo clippy --workspace --all-targets --all-features --locked --target x86_64-pc-windows-gnu -- -D warnings
# Android: the host and runtime lint clean for phones (with the NDK).
if [ -n "$ndk" ]; then
  bin="$ndk/toolchains/llvm/prebuilt/linux-x86_64/bin"
  CC_aarch64_linux_android="$bin/aarch64-linux-android29-clang" AR_aarch64_linux_android="$bin/llvm-ar" \
    cargo clippy -p aio-rs -p aio-rs-shell --locked --target aarch64-linux-android -- -D warnings
fi
fi
if on test; then
# Without `compat` a socket that doesn't say wire 2 is refused (the rest runs with every feature, below).
cargo test -p aio-rs --no-default-features --features net --test sync --test http --test compat_data --locked
# air's gates (no leaks, misuse failing, planted faults failing a test) in a
# release build too.
cargo test -p aio-rs-air --release --all-features --locked
# The runtime's benchmark gate (aio's D12): a start, a store's open, a call,
# a saved call, each under twice what it measured, in the release build
# (the debug build's ceilings: in the workspace's run below).
cargo test -p aio-rs --release --test rt_gate --locked
# The web build's browser test binds with the lock's wasm-bindgen, on PATH or
# aio's own (`ax setup` fetches it); without it the test skips, said here and
# last. With AIO_CI_WEB set (GitHub CI) a browser test that would skip fails.
wb=$(sed -n '/^name = "wasm-bindgen"$/{n;s/^version = "\(.*\)"$/\1/p;q}' Cargo.lock)
wb_own="${XDG_DATA_HOME:-$HOME/.local/share}/aio-rt/tools/wasm-bindgen-$wb/wasm-bindgen"
if [ ! -x "$wb_own" ] && [ "$(wasm-bindgen --version 2>/dev/null)" != "wasm-bindgen $wb" ]; then
  web_skip="ci: the web build's browser test SKIPPED: no wasm-bindgen $wb (ax setup fetches it into $wb_own)"
  echo "$web_skip" >&2
fi
# The JS client's dev packages (React, Solid, esbuild, TypeScript) for its
# tests (aio-rs-shell's js_client); without npm they are skipped, said here
# and last (with AIO_CI_WEB set they fail).
js_skip=""
if command -v npm >/dev/null 2>&1; then
  npm ci --prefix crates/aio/client --no-audit --no-fund
else
  js_skip="ci: the JS client's tests SKIPPED: no npm (npm ci in crates/aio/client)"
  echo "$js_skip" >&2
fi
cargo test --workspace --all-features --locked
# ax dev's builds run the app as release does: the template's tests and a
# fixture (logic at its edges, overflow, the window through TestUi) say the
# same under ax dev's build, ax test and --release; shown failing first on
# a plain debug build, which doesn't.
scripts/parity --self-test
# The tests on a real Windows when one is named
# (`AIO_CI_WINDOWS="user@host [ssh options]"`; Wine misruns threaded tests).
if [ -n "${AIO_CI_WINDOWS:-}" ]; then
  # shellcheck disable=SC2086
  ./ci-windows.sh $AIO_CI_WINDOWS
fi
fi
# The DX trend: a template app built, edited live under `ax dev` and
# smoke-run, its row added to scripts/dx-trend.csv (commit it); a number past
# its limit fails, naming both (scripts/dx-metrics --trend). Only this
# machine's rows count; where it has none (GitHub CI), AIO_DX_BASE names the
# commit whose sizes are measured here too, AIO_DX_ROWS a folder keeping rows.
# Its packages (release binary, .tar.gz, AppImage, web build) hold no other
# renderer (scripts/air-ship, shown failing on a planted dependency first).
apk_skip=""
if on dx; then
scripts/dx-metrics --check ${AIO_DX_BASE:+--base "$AIO_DX_BASE"} ${AIO_DX_ROWS:+--rows "$AIO_DX_ROWS"}
# Its APKs where ax builds them here (the NDK, Rust's three Android targets);
# GitHub CI's air-apk job runs this alone.
if [ -n "$ndk" ] && [ "$(rustup target list --installed | grep -c -x -e aarch64-linux-android -e armv7-linux-androideabi -e x86_64-linux-android)" = 3 ]; then
  scripts/dx-metrics --check --apk ${AIO_DX_BASE:+--base "$AIO_DX_BASE"} ${AIO_DX_ROWS:+--rows "$AIO_DX_ROWS"}
else
  apk_skip="ci: the template's APKs SKIPPED: no NDK or no Rust Android targets (rustup target add armv7-linux-androideabi x86_64-linux-android)"
  echo "$apk_skip" >&2
fi
fi
for skip in "$web_skip" "$apk_skip" "${js_skip:-}"; do
  if [ -n "$skip" ]; then
    echo "$skip" >&2
  fi
done
