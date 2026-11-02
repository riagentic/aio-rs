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
# Every crate listed with its reason, no unlisted duplicate, no set above its ceiling (deps.toml).
scripts/deps-check
scripts/slim-metrics --check
scripts/dx-metrics --self-test
# An app on air ships no Dioxus: each of air-ship's checks red on a planted one.
scripts/air-ship --self-test
# The guide (docs/): links resolve, each page with Rust is a doctest (run by
# the workspace tests below), the verb list is whole, the wording public.
scripts/docs-check
# The mutation ledger holds together: each break's line found once, its test there
# (scripts/mutants runs the breaks; GitHub CI runs the fast ones).
scripts/mutants --check
# Apps are written against aio::ui, never the renderer behind it (air
# replaces it: docs/design/air.md): the template, the kit, `ax ui`, the views
# ax writes, the tests and the guide name no `dioxus` (ax lint holds apps to it).
if grep -rn dioxus README.md docs crates/aio/src/kit crates/aio/tests crates/aio-shell/tests \
  crates/aio-cli/src/create crates/aio-cli/src/ui crates/aio-cli/src/web crates/aio-cli/src/surface.rs \
  crates/aio-cli/src/preview.rs crates/aio-cli/src/preview_fixture.rs crates/aio-cli/src/headless.rs \
  crates/aio-cli/src/headless_fixture.rs crates/aio-cli/tests | grep -v '^docs/design/\|^docs/migration/'; then
  echo "ci: the lines above name dioxus: use aio::ui (the migration guide has the names)" >&2
  exit 1
fi
# A release build has no live-reload tools: only `ax dev` adds them (`aio-shell/dev`).
if cargo tree --locked -p aio-rs-shell -e normal --prefix none | grep -q '^dioxus-devtools '; then
  echo "ci: aio-rs-shell without \`dev\` pulls dioxus-devtools" >&2
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
  for f in "" outbound server tls oidc br net kit air imbl web liveview dev testing video compat; do
    cargo clippy -p aio-rs --lib --no-default-features --features "$f" --locked -- -D warnings
  done
fi
# The shell needs a renderer: each feature with each one (Dioxus's, air).
for f in dioxus air web dioxus,dev air,dev; do
  cargo clippy -p aio-rs-shell --lib --no-default-features --features "$f" --locked -- -D warnings
done
# Working with aio over the network: aio pages and tools on `/ws`, `AioLink`.
cargo clippy -p aio-rs --lib --no-default-features --features compat,net --locked -- -D warnings
# air, aio-rs's own renderer (docs/design/air.md): the facade and TestUi on
# it with no Dioxus in the build, the shell on air (the window's views,
# Android's), with no Dioxus in either graph.
cargo clippy -p aio-rs --lib --test air_ui --test facade --test ui --test kit --test signin --test route --test route_tree --no-default-features --features air,net --locked -- -D warnings
if cargo tree --locked -p aio-rs --no-default-features --features air -e normal --prefix none | grep -q '^dioxus'; then
  echo "ci: aio-rs with \`air\` alone pulls dioxus" >&2
  exit 1
fi
cargo clippy -p aio-rs-shell --all-targets --no-default-features --features air,dev --locked -- -D warnings
if cargo tree --locked -p aio-rs-shell --no-default-features --features air,dev -e normal --prefix none | grep -q '^dioxus'; then
  echo "ci: aio-rs-shell with \`air\` pulls dioxus" >&2
  exit 1
fi
# air's code-size rule: LLVM IR lines per view of the bench's 50 views.
air_ir=$(mktemp -d)
scripts/air-ir "$air_ir"
rm -rf "$air_ir"
# The browser build (`ax build --web`): aio's core in the page, no SQLite, no ring.
cargo clippy -p aio-rs --lib --no-default-features --features web --locked --target wasm32-unknown-unknown -- -D warnings
# With the kit: its controls and the sign-in form in the page.
cargo clippy -p aio-rs --lib --no-default-features --features web,kit --locked --target wasm32-unknown-unknown -- -D warnings
# On air: its views drawn in the page by air's interpreter, no Dioxus.
cargo clippy -p aio-rs --lib --no-default-features --features web,air --locked --target wasm32-unknown-unknown -- -D warnings
# Windows: the same lint (its tests: below, and GitHub CI's Windows job).
cargo clippy --workspace --all-targets --all-features --locked --target x86_64-pc-windows-gnu -- -D warnings
# Android: the host and runtime lint clean for phones (with the NDK).
if [ -n "$ndk" ]; then
  bin="$ndk/toolchains/llvm/prebuilt/linux-x86_64/bin"
  CC_aarch64_linux_android="$bin/aarch64-linux-android29-clang" AR_aarch64_linux_android="$bin/llvm-ar" \
    cargo clippy -p aio-rs -p aio-rs-shell --locked --target aarch64-linux-android -- -D warnings
  CC_aarch64_linux_android="$bin/aarch64-linux-android29-clang" AR_aarch64_linux_android="$bin/llvm-ar" \
    cargo clippy -p aio-rs-shell --no-default-features --features air --locked --target aarch64-linux-android -- -D warnings
fi
fi
if on test; then
# Without `compat` a socket that doesn't say wire 2 is refused (the rest runs with every feature, below).
cargo test -p aio-rs --no-default-features --features net --test sync --test http --test compat_data --locked
# air: the facade and TestUi on it with no Dioxus in the build, and its
# gates (no leaks, misuse failing, planted faults failing a test) in a
# release build too.
cargo test -p aio-rs --no-default-features --features air,net --test air_ui --test facade --test ui --test kit --test signin --test route --test route_tree --locked
# The shell on air: the window's views, Android's.
cargo test -p aio-rs-shell --no-default-features --features air,dev --locked
cargo test -p aio-rs-air --release --all-features --locked
# The web build's browser test binds with the lock's wasm-bindgen, on PATH or
# aio's own (`ax setup` fetches it); without it the test skips, said here and last.
wb=$(sed -n '/^name = "wasm-bindgen"$/{n;s/^version = "\(.*\)"$/\1/p;q}' Cargo.lock)
wb_own="${XDG_DATA_HOME:-$HOME/.local/share}/aio-rt/tools/wasm-bindgen-$wb/wasm-bindgen"
if [ ! -x "$wb_own" ] && [ "$(wasm-bindgen --version 2>/dev/null)" != "wasm-bindgen $wb" ]; then
  web_skip="ci: the web build's browser test SKIPPED: no wasm-bindgen $wb (ax setup fetches it into $wb_own)"
  echo "$web_skip" >&2
fi
cargo test --workspace --all-features --locked
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
# The same template on air too: its packages (release binary, .tar.gz,
# AppImage, web build) hold no Dioxus (scripts/air-ship, shown failing on a
# planted dependency first), their sizes and its edit times gated alike.
apk_skip=""
if on dx; then
scripts/dx-metrics --check ${AIO_DX_BASE:+--base "$AIO_DX_BASE"} ${AIO_DX_ROWS:+--rows "$AIO_DX_ROWS"}
# Its APKs where ax builds them here (the NDK, Rust's three Android targets);
# GitHub CI's air-apk job runs this alone.
if [ -n "$ndk" ] && [ "$(rustup target list --installed | grep -c -x -e aarch64-linux-android -e armv7-linux-androideabi -e x86_64-linux-android)" = 3 ]; then
  scripts/dx-metrics --check --apk ${AIO_DX_BASE:+--base "$AIO_DX_BASE"} ${AIO_DX_ROWS:+--rows "$AIO_DX_ROWS"}
else
  apk_skip="ci: the air template's APKs SKIPPED: no NDK or no Rust Android targets (rustup target add armv7-linux-androideabi x86_64-linux-android)"
  echo "$apk_skip" >&2
fi
fi
for skip in "$web_skip" "$apk_skip"; do
  if [ -n "$skip" ]; then
    echo "$skip" >&2
  fi
done
