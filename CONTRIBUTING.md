# Contributing to aio-rs

Changes are welcome as issues and pull requests on
[github.com/riagentic/aio-rs](https://github.com/riagentic/aio-rs). This
page is for working on aio-rs itself; building apps with it is in
[docs/](docs/README.md).

## The workspace

| Crate (package) | Path | What it is |
|---|---|---|
| `aio-rs` (`use aio::…`) | `crates/aio` | the core: cells, the store, sync, the server, the kit, testing |
| `aio-rs-macros` | `crates/aio-macros` | `#[aio::cell]`, `#[aio::methods]`, `#[aio::data]` |
| `aio-rs-config` | `crates/aio-config` | `aio.toml`'s types and checks |
| `aio-rs-config-macro` | `crates/aio-config-macro` | the config baked into a build as a literal |
| `aio-rs-shell` | `crates/aio-shell` | the desktop host: the shared Electron runtime, windows, updates |
| `aio-rs-cli` | `crates/aio-cli` | `ax`, and the templates of `ax create` |

From a checkout, `cargo install --locked --path crates/aio-cli` installs
`ax`; an `ax` built from a checkout makes its new apps build on that
checkout (`ax create --aio <dir>` names another).

## Before a pull request: `./ci.sh`

`./ci.sh` is what CI runs, headless (it unsets `DISPLAY` and the session
bus; the UI tests draw in Electron's headless mode). Run it before pushing.
It needs the toolchain of `rust-toolchain.toml` (rustup installs it, with
the Windows and Android targets), the `wasm32-unknown-unknown` target, a C
toolchain, `Xvfb` for the display tests and `ax build --smoke`, and
`gcc-mingw-w64-x86-64` for the Windows lint. `cargo hack`, when installed,
lints each feature alone.

The gates, in order:

- `cargo fmt --all --check` (`rustfmt.toml`).
- `scripts/deps-check`: every crate in the graph is listed in `deps.toml`
  with its reason; no unlisted duplicate. A new dependency needs an entry
  and a reason.
- `scripts/slim-metrics --check`: each feature set's crate count stays
  under its ceiling (`deps.toml [ceilings]`).
- `scripts/docs-check`: the guide holds together (below).
- clippy with `-D warnings`: the workspace with every feature, each
  feature of `aio-rs` alone, the browser build (wasm32), Windows, and
  Android when an NDK is found.
- `cargo doc` with `-D warnings`: every API doc link resolves.
- `cargo test --workspace --all-features`, plus `aio-rs` without `compat`.
  The web build's browser test needs the lock's `wasm-bindgen` (`ax setup`
  fetches it); without it the test is skipped and `./ci.sh` says so.
- `scripts/dx-metrics --check`: a template app built, edited live under
  `ax dev` and smoke-run; its row (build times, sizes, memory) is appended
  to `scripts/dx-trend.csv` (commit it). A number past its limit fails,
  naming both. `scripts/dx-metrics --trend` shows the last rows and the
  limits.

The Windows tests run on a real Windows when one is named:
`AIO_CI_WINDOWS="user@host [ssh options]" ./ci.sh` (or
`./ci-windows.sh user@host`; the machine needs only OpenSSH).

## The guide (`docs/`)

- Every Rust block in a guide page is a doctest: the page is included by
  `crates/aio/src/guide.rs` (under the feature it needs), so its examples
  compile and run with `cargo test -p aio-rs --all-features --doc`. Only
  an excerpt of the app template may be marked `ignore`, and its first
  line says so (`// ax create's …`).
- Every whole `aio.toml` in a page (a TOML block with `[app]`) parses
  (`crates/aio/tests/docs.rs`).
- `scripts/docs-check`: every relative link and `#heading` resolves; every
  page with Rust is in `guide.rs`; `docs/clients/ax.md` names every verb of
  `crates/aio-cli/src/verbs.rs`; no personal path, address or private name
  (one per line in the file `AIO_PRIVATE_TERMS` names, kept outside the
  repository).
- The Deno-based framework is "aio" (command `am`), this one "aio-rs"
  (command `ax`).

## Commits

Plain, factual messages: what changed and why, in the imperative. One
topic per commit; `./ci.sh` green on each.

## Releasing aio-rs

How aio-rs itself is released and what signs it. An app's own releases
(`ax keygen <app>`, `ax release <app>`) are in the
[release guide](docs/build/release.md).

### Make a release

Set the version (below), then, in a clean checkout of the commit to
release:

1. **Gates**: `./ci.sh` green, with the Windows lane (`AIO_CI_WINDOWS`)
   and the web build's browser test; the Mac's `cargo test --workspace`;
   the template app's APK started on an emulator.
2. **Mac** (macOS builds only on a Mac): `ax release --aio --dir <d>` makes
   `aio-rs-<version>-aarch64-apple-darwin.tar.gz` (`ax` inside), signed and
   notarized when the macOS keys are set. Copy it into the Linux release
   folder.
3. **Template app** (the app pipeline proven on each OS): in a throwaway
   folder, `ax create template`, `git init` and one commit (the build
   number), then on Linux `ax build template --deb --appimage --smoke`,
   `--windows` and `--android`; on the Mac `ax build template --smoke`.
   Keep the packages out of the release folder (`SHA256SUMS` covers
   every file in it).
4. **Linux, last**: `ax release --aio --windows --dir dist/v<version>`
   builds the Linux and Windows archives, pins every aio-rs archive in the
   folder (the Mac's too) in `install.sh` / `install.ps1`, writes
   `SHA256SUMS` over every file in the folder and signs it.
5. **Check**: `cd dist/v<version> && sha256sum -c SHA256SUMS`.
6. **Tag and publish**: `git tag -a v<version> -m "aio-rs <version>"`,
   `git push origin v<version>`, then attach every file of the folder to
   the tag's GitHub release:
   `gh release create v<version> --repo riagentic/aio-rs dist/v<version>/*`.
   The installers download from there
   (`https://github.com/riagentic/aio-rs/releases/download/v<version>/`,
   the default of `--url`).

`dist/` is git-ignored. The version is `[workspace.package] version` in
`Cargo.toml` (every crate takes it) and the five `[workspace.dependencies]`
lines beside it. Add its entry to `CHANGELOG.md`.

### Signing

Each signature turns on by itself when its keys are set. Until then the
release says, for each one, `<what>: unsigned: no keys configured (<the
names that turn it on>)`, and builds the rest. A key that is named but
can't be used (a missing file, a password not set) stops the release.

| Signature | Set | Tool, where | Signs |
|---|---|---|---|
| Windows Authenticode | `AIO_WINDOWS_CERT` (a `.pfx`), `AIO_WINDOWS_CERT_PASSWORD`; `AIO_WINDOWS_TIMESTAMP_URL` (optional, default DigiCert's RFC 3161 server) | `osslsigncode`, in the Linux run | `ax.exe`, before it is zipped |
| macOS codesign + notarize | `AIO_MACOS_SIGN_IDENTITY` (`Developer ID Application: …`, in the login keychain); `AIO_MACOS_NOTARY_PROFILE` (a profile saved by `xcrun notarytool store-credentials`) | `codesign` (hardened runtime, timestamp), `notarytool submit --wait`, in the Mac run | `ax`, before it is packed; without the profile it is signed, not notarized, and says so |
| aio-rs's update key | `AIO_RELEASE_KEY` (a key file), else `aio.release.key` in the keys dir (`ax keygen --aio` makes it) | ax, in the Linux run | `SHA256SUMS`, as `SHA256SUMS.sig` (Ed25519, base64) |

The keys dir is `$XDG_CONFIG_HOME/aio-rt/keys` (else `~/.config/aio-rt/keys`).
Never commit a key; back each up: a lost update key ends the updates it
signs.

`SHA256SUMS.sig` checks with nothing but OpenSSL, given aio-rs's public key
(`ed25519:<base64>`, printed by `ax keygen --aio` and by the release):

```sh
{ printf '302a300506032b6570032100' | xxd -r -p; printf '%s' '<base64>' | base64 -d; } > aio-pub.der
base64 -d SHA256SUMS.sig > SHA256SUMS.sig.bin
openssl pkeyutl -verify -pubin -keyform DER -inkey aio-pub.der -rawin -in SHA256SUMS -sigfile SHA256SUMS.sig.bin
```
