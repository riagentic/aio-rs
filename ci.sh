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
cargo fmt --all --check
# Every crate listed with its reason, no unlisted duplicate, no set above its ceiling (deps.toml).
scripts/deps-check
scripts/slim-metrics --check
scripts/dx-metrics --self-test
# The guide (docs/): links resolve, each page with Rust is a doctest (run by
# the workspace tests below), the verb list is whole, the wording public.
scripts/docs-check
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
  cargo hack clippy -p aio-rs -p aio-rs-shell --lib --each-feature --locked -- -D warnings
else
  for f in "" outbound server tls oidc br net kit air imbl web dev testing video compat; do
    cargo clippy -p aio-rs --lib --no-default-features --features "$f" --locked -- -D warnings
  done
  for f in "" web dev; do
    cargo clippy -p aio-rs-shell --lib --no-default-features --features "$f" --locked -- -D warnings
  done
fi
# Working with aio over the network: aio pages and tools on `/ws`, `AioLink`.
cargo clippy -p aio-rs --lib --no-default-features --features compat,net --locked -- -D warnings
# Without `compat` a socket that doesn't say wire 2 is refused (the rest runs with every feature, below).
cargo test -p aio-rs --no-default-features --features net --test sync --test http --test compat_data --locked
# air, aio-rs's own renderer (docs/design/air.md): the facade and TestUi on
# it with no Dioxus in the build, and its gates (no leaks, misuse failing,
# planted faults failing a test) in a release build too.
cargo clippy -p aio-rs --lib --test air_ui --test facade --test ui --test kit --test signin --no-default-features --features air,net --locked -- -D warnings
if cargo tree --locked -p aio-rs --no-default-features --features air -e normal --prefix none | grep -q '^dioxus'; then
  echo "ci: aio-rs with \`air\` alone pulls dioxus" >&2
  exit 1
fi
cargo test -p aio-rs --no-default-features --features air,net --test air_ui --test facade --test ui --test kit --test signin --locked
cargo test -p aio-rs-air --release --all-features --locked
# The browser build (`ax build --web`): aio's core in the page, no SQLite, no ring.
cargo clippy -p aio-rs --lib --no-default-features --features web --locked --target wasm32-unknown-unknown -- -D warnings
# With the kit: its controls and the sign-in form in the page.
cargo clippy -p aio-rs --lib --no-default-features --features web,kit --locked --target wasm32-unknown-unknown -- -D warnings
# The web build's browser test binds with the lock's wasm-bindgen, on PATH or
# aio's own (`ax setup` fetches it); without it the test skips, said here and last.
wb=$(sed -n '/^name = "wasm-bindgen"$/{n;s/^version = "\(.*\)"$/\1/p;q}' Cargo.lock)
wb_own="${XDG_DATA_HOME:-$HOME/.local/share}/aio-rt/tools/wasm-bindgen-$wb/wasm-bindgen"
web_skip=""
if [ ! -x "$wb_own" ] && [ "$(wasm-bindgen --version 2>/dev/null)" != "wasm-bindgen $wb" ]; then
  web_skip="ci: the web build's browser test SKIPPED: no wasm-bindgen $wb (ax setup fetches it into $wb_own)"
  echo "$web_skip" >&2
fi
cargo test --workspace --all-features --locked
# Windows: the same lint, and the tests on a real Windows when one is named
# (`AIO_CI_WINDOWS="user@host [ssh options]"`; Wine misruns threaded tests).
cargo clippy --workspace --all-targets --all-features --locked --target x86_64-pc-windows-gnu -- -D warnings
if [ -n "${AIO_CI_WINDOWS:-}" ]; then
  # shellcheck disable=SC2086
  ./ci-windows.sh $AIO_CI_WINDOWS
fi
# Android: the host and runtime lint clean for phones (when the NDK is here).
ndk=$(ls -d "${ANDROID_HOME:-$HOME/Android/Sdk}"/ndk/* 2>/dev/null | sort -V | tail -1)
if [ -n "$ndk" ]; then
  bin="$ndk/toolchains/llvm/prebuilt/linux-x86_64/bin"
  CC_aarch64_linux_android="$bin/aarch64-linux-android29-clang" AR_aarch64_linux_android="$bin/llvm-ar" \
    cargo clippy -p aio-rs -p aio-rs-shell --locked --target aarch64-linux-android -- -D warnings
fi
# The DX trend: a template app built, edited live under `ax dev` and
# smoke-run, its row added to scripts/dx-trend.csv (commit it); a number past
# its limit fails, naming both (scripts/dx-metrics --trend).
scripts/dx-metrics --check
if [ -n "$web_skip" ]; then
  echo "$web_skip" >&2
fi
