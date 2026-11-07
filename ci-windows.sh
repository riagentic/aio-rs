#!/bin/sh
# The workspace tests on a real Windows: built here as Windows programs, run
# there over SSH, then removed. Usage: ./ci-windows.sh user@host [ssh options]
# (the machine needs only OpenSSH; PowerShell is its default shell). The UI
# tests draw in the pinned Electron runtime: fetched here once (SHA-256
# checked), copied into the run's folder there, gone with it.
set -eu
cd "$(dirname "$0")"
host=$1
shift
# Its own folder there: two runs at once (other checkouts) never meet.
dir=aio-rs-ci-$$
out="${CARGO_TARGET_DIR:-target}"
mkdir -p "$out/electron"
ver=$(sed -n '/macro_rules! electron_version/,/}/p' crates/aio/src/paths.rs | grep -o '"[0-9.]*"' | tr -d '"')
zip="electron-v$ver-win32-x64.zip"
base="${AIORT_ELECTRON_MIRROR:-https://github.com/electron/electron/releases/download/}"
base="${base%/}/v$ver"
checked() { (cd "$out/electron" && grep -F " *$zip" SHASUMS256.txt 2>/dev/null | sha256sum -c --status); }
if ! checked; then
  curl -fsSL -o "$out/electron/SHASUMS256.txt" "$base/SHASUMS256.txt"
  curl -fsSL -o "$out/electron/$zip" "$base/$zip"
  checked || {
    echo "ci-windows: $zip does not match its SHA-256 in SHASUMS256.txt" >&2
    rm -f "$out/electron/$zip"
    exit 1
  }
fi
# Not the hot-patch test: it builds a fixture with cargo from this checkout,
# which isn't there; GitHub CI's Windows job runs it natively (MSVC and GNU).
cargo test --workspace --all-features --locked --target x86_64-pc-windows-gnu --no-run --message-format=json \
  | grep '"profile":{[^}]*"test":true' | grep -o '"executable":"[^"]*"' | cut -d'"' -f4 \
  | grep -v '/hotpatch-[0-9a-f]*\.exe$' | sort -u >"$out/win-tests"
ssh "$@" "$host" "Remove-Item -Recurse -Force -ErrorAction Ignore $dir; New-Item -ItemType Directory $dir | Out-Null"
# shellcheck disable=SC2046
scp -q "$@" $(cat "$out/win-tests") "$out/electron/$zip" "$host:$dir/"
# Every program runs, even after a failure; the count of failed ones is the
# exit code. Their temp files stay in the run's folder; it goes once the last
# browser (quitting as its test ended) is gone.
ssh "$@" "$host" "cd $dir; \$here = (Resolve-Path .).Path; New-Item -ItemType Directory electron, tmp | Out-Null; tar -xf $zip -C electron; Remove-Item $zip; \$env:AIORT_RUNTIME = \"\$here\\electron\"; \$env:TEMP = \$env:TMP = \"\$here\\tmp\"; \$failed = 0; foreach (\$t in Get-ChildItem *.exe) { & \$t.FullName -q; if (\$LASTEXITCODE) { \$failed++; Write-Output \"FAILED: \$(\$t.Name)\" } }; cd ..; Get-Process | Where-Object { \$_.Path -like \"\$here\\*\" } | Wait-Process -Timeout 30 -ErrorAction Ignore; Remove-Item -Recurse -Force $dir; exit \$failed"
