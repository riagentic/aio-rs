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
`ax`. A new app takes aio-rs from git at that `ax`'s release tag, whose
templates may be older than `main`'s: on `main`, make it on your checkout
(`ax create <name> --aio <dir>`; the app names it by a relative path,
so make the app beside the checkout).

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
- `scripts/public-api --check`: the published crates' public API is the one
  checked in (`scripts/api/<crate>.txt`, from rustdoc's JSON), and none of
  it names aio-rs 0.1's renderer. A change to the API: run
  `scripts/public-api` and commit what it wrote.
- `scripts/docs-check`: the guide holds together (below).
- `scripts/mutants --check`: the mutation ledger (`scripts/mutants.toml`)
  holds together. `scripts/mutants` plants each break, in the source or in
  the template app's built `.deb`, and needs its named test red; run it
  after changing a line the ledger names, and commit
  `scripts/mutants-ledger.csv` ([Proving the tests](docs/testing/proving.md)).
- clippy with `-D warnings`: the workspace with every feature, each
  feature of `aio-rs` alone, the browser build (wasm32), Windows, and
  Android when an NDK is found.
- `cargo doc` with `-D warnings`: every API doc link resolves.
- `cargo test --workspace --all-features`, plus `aio-rs` without `compat`.
  The web build's browser test needs the lock's `wasm-bindgen` (`ax setup`
  fetches it); without it the test is skipped and `./ci.sh` says so.
  The JS client's tests (`crates/aio/client`: its unit tests, type check,
  and React, Solid, Svelte, Vue and plain pages in a browser and in the
  window) need its dev packages, which `./ci.sh` installs with `npm ci`;
  without npm they are skipped.
  GitHub CI sets `AIO_CI_WEB`: there a browser test that would skip fails.
- `scripts/dx-metrics --check`: a template app built, edited live under
  `ax dev` and smoke-run; its row (build times, sizes, memory) is appended
  to `scripts/dx-trend.csv` (commit it). A number past its limit fails,
  naming both. `scripts/dx-metrics --trend` shows the last rows and the
  limits. Only the same machine's rows count. GitHub CI has none: its
  sizes and crate counts are held to the PR's base (main's head) measured
  on the same runner in the same job (or kept from main's run on a runner
  of the same machine id); its times are said, not gated. Growth meant: a
  committed row of that machine (the failing run prints it).

The Windows tests run on a real Windows when one is named:
`AIO_CI_WINDOWS="user@host [ssh options]" ./ci.sh` (or
`./ci-windows.sh user@host`; the machine needs only OpenSSH).

GitHub (`.github/workflows/ci.yml`) runs `./ci.sh` on Ubuntu after
`ax setup` (with `ffmpeg` for the test videos), its parts side by side:
`./ci.sh lint` and `./ci.sh test` (each part runs alone so; no part: all
of them), its dx part as `scripts/dx-metrics --air` and `--apk`, each
against the base measured on its own runner; the ledger's fast breaks (`scripts/mutants --fast`), the workspace clippy and tests on
Windows and macOS with the Electron runtime (`ax runtime --fetch`), and
`cargo audit` (accepted advisories in `.cargo/audit.toml`). Rust caches
are saved by main's runs only; a PR's newer push cancels its older run,
main's runs always finish.

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

Set the version (below), then, on the commit to release:

1. **Gates**, each green on that commit:
   - `./ci.sh`, with the Windows lane (`AIO_CI_WINDOWS`) and the web
     build's browser test; the Mac's `cargo test --workspace`;
   - GitHub CI on the pull request or `main`: the tests on Linux, Windows
     and macOS, and `update-proof` on each (a sample app installed,
     updated through a signed patch, a broken update rolled back with its
     data: `scripts/update-proof`);
   - **the Android gate**, required: `scripts/android-gate` (a throwaway
     emulator on port 5600; a sample APK started, upgraded in place, its
     data kept; [ci](docs/build/ci.md#the-android-gate));
   - a dry run of the release workflow (Actions → release → Run
     workflow): every archive built, smoked and installed by the one-line
     installer on its OS, the files as the artifact `aio-rs-release`.
2. **Template app** (the app pipeline proven on each OS): in a throwaway
   folder, `ax create template`, `git init` and one commit (the build
   number), then on Linux `ax build template --deb --appimage --smoke`,
   `--windows` and `--android`; on the Mac `ax build template --smoke`.
3. **Tag**: `git tag -a v<version> -m "aio-rs <version>"`,
   `git push origin v<version>`. The release workflow
   (`.github/workflows/release.yml`) builds `ax` on macOS (arm64 and
   x86_64, each on its own runner), then on Linux the Linux and Windows
   archives, pins every archive in `install.sh` / `install.ps1`, writes
   `SHA256SUMS` and signs it with the repository secret `AIO_RELEASE_KEY`
   (the update key file; without it, or with no key pinned in
   `crates/aio-cli/release.pub`, the release stops), installs it on each OS and makes a
   **draft** release with every file. A tag that isn't the version
   fails it.
4. **Check and publish**: download the draft's files,
   `sha256sum -c SHA256SUMS`, then publish the draft. The installers
   download from there
   (`https://github.com/riagentic/aio-rs/releases/download/v<version>/`,
   the default of `--url`).

By hand instead (OS signing needs it: the workflow holds no Windows
certificate or Apple identity): on an arm64 Mac
`ax release --aio --target x86_64-apple-darwin --dir <d>` (both Mac archives),
copy the two archives into the Linux folder, then on Linux
`ax release --aio --windows --dir dist/v<version>`, check, and
`gh release create v<version> --repo riagentic/aio-rs dist/v<version>/*`.

`dist/` is git-ignored. The version is `[workspace.package] version` in
`Cargo.toml` (every crate takes it) and the eight `[workspace.dependencies]`
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

aio-rs's update key is the one signature a release can't go without:
`ax upgrade` checks `SHA256SUMS.sig` against the public key ax pins in
`crates/aio-cli/release.pub` before it runs the install script, and
refuses a release with no signature or another key's. Set it up once:

1. `ax keygen --aio`: the key in the keys dir, its public key printed.
2. Put the public key (`ed25519:<base64>`, one line) in
   `crates/aio-cli/release.pub` and commit it.
3. `gh secret set AIO_RELEASE_KEY < ~/.config/aio-rt/keys/aio.release.key`.

`ax release --aio` refuses a key that isn't the one pinned. Rotating the
key: a new `release.pub` in one release signed by the old key, so every
ax can reach it; the next release is signed by the new one.

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
