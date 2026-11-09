#!/bin/sh
# Installs aio-rs @VERSION@ (the `ax` command), prebuilt: made by
# `ax release --aio`, which pinned each build's SHA-256 here. Nothing
# runs before its checksum matches.
#   sh install.sh                    (beside the release's files: offline)
#   curl -fsSL <url>/install.sh | sh (AIO_DOWNLOAD=<url> for another url,
#                                     or a folder holding the release's files)
# ax goes where cargo puts what it installs ($CARGO_HOME/bin), then
# `ax setup` checks and fetches what making apps needs.
# All of it is in main(), called on the last line: a download cut short
# is a syntax error, never the half of the script that came.
set -eu

main() {
  version=@VERSION@
  here=$(cd "$(dirname "$0")" && pwd)
  base=${AIO_DOWNLOAD:-@URL@}
  if [ -z "$base" ]; then
    base=$here
  fi
  source_build="from source instead (minutes): cargo install --locked --git https://github.com/riagentic/aio-rs --tag v$version aio-rs-cli"
  case "$(uname -s)-$(uname -m)" in
    Linux-x86_64) triple=x86_64-unknown-linux-gnu ;;
    Linux-aarch64 | Linux-arm64) triple=aarch64-unknown-linux-gnu ;;
    Darwin-x86_64) triple=x86_64-apple-darwin ;;
    Darwin-arm64) triple=aarch64-apple-darwin ;;
    *) triple=unknown ;;
  esac
  case "$triple" in
@PINS@
    *)
      echo "ax: no prebuilt aio-rs $version for $(uname -s) $(uname -m); $source_build" >&2
      exit 1
      ;;
  esac
  bin="${CARGO_HOME:-$HOME/.cargo}/bin"
  if [ ! -d "$bin" ]; then
    echo "ax: no $bin: install Rust first (ax builds apps with cargo): curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh" >&2
    exit 1
  fi
  file="aio-rs-$version-$triple.tar.gz"
  # Run beside the release's files, it takes them (the pinned url is for curl | sh).
  if [ -z "${AIO_DOWNLOAD:-}" ] && [ -f "$here/$file" ]; then
    base=$here
  fi
  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' EXIT
  case "$base" in
    http://* | https://* | file://*) curl -fsSL -o "$tmp/$file" "$base/$file" ;;
    *) cp "$base/$file" "$tmp/$file" ;;
  esac || {
    echo "ax: could not get $base/$file; $source_build" >&2
    exit 1
  }
  if command -v sha256sum >/dev/null 2>&1; then
    got=$(sha256sum "$tmp/$file" | cut -d' ' -f1)
  else
    got=$(shasum -a 256 "$tmp/$file" | cut -d' ' -f1)
  fi
  if [ "$got" != "$want" ]; then
    echo "ax: $file has SHA-256 $got, expected $want: not installed" >&2
    exit 1
  fi
  tar -xzf "$tmp/$file" -C "$tmp"
  mv -f "$tmp/ax" "$bin/ax"
  echo "aio-rs $version installed: $bin/ax"
  "$bin/ax" setup
}

main "$@"
