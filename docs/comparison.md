# aio-rs next to Tauri 2 and Dioxus 0.7

How building an app feels in aio-rs, Tauri and Dioxus. The same tiny
app in each: a counter, a text list, one stylesheet, and one Rust function the window shows (`label(n)`, for the "Rust logic" edit).
Measured once each on one Linux x86_64 machine (other work running beside
it), 2026-10-31; one run, not a benchmark: read the shape, not the decimals.

- aio-rs: `ax create` (app template), the cell holds count and items
  (saved, with history). aio-rs 0.1.0, Dioxus 0.7.10, Electron 44.
- Tauri 2.12 (CLI 2.12.4): create-tauri-app's vanilla shape: `src/` HTML,
  CSS and JS, state in a `Mutex` behind three `#[tauri::command]`s.
- Dioxus 0.7.10 desktop (wry, WebKitGTK), signals, `asset!()` stylesheet.

Each in its own folder and target dir, cargo's defaults (incremental debug
builds); live edits through each one's own dev command (`ax dev`, `tauri
dev`, `dx serve --hot-patch`), timed from the save to the last change of
the screen (Xvfb, polled ~10/s).

## Numbers

| | aio-rs | Tauri 2 | Dioxus 0.7 |
|---|---|---|---|
| Crates (`cargo tree -e normal,build`, host, unique) | **157** | 262 | 354 |
| Clean debug build (`cargo build`) | **15.9 s** (82 CPU-s) | 34.3 s (194 CPU-s) | 30.3 s (225 CPU-s) |
| One-line rebuild (debug) | **0.6 s** | 3.3–5.6 s | 0.8–0.9 s |
| Live CSS edit, save → visible | 0.09 s | none: frontend embedded, shows after a Rust rebuild | **0.03 s** |
| Live markup edit (rsx / HTML) | **0.03 s** | none (as CSS) | 0.05 s |
| Live Rust logic edit | **0.5 s**, hot-patched, state kept | 3.3 s, rebuild + restart, state lost | 2.5 s, hot-patched, state kept |
| Typo in Rust while running (`c.cuont`) | rustc's error in the terminal and on the window (0.46 s), app keeps running; the fix takes it off | rustc's error in the terminal; the window closes until it builds | rustc's message lines in the terminal; a toast "the build failed" with no detail |
| Debug target dir | **894 MB** (no dependency debug info) | 3.1 GB | 2.3 GB, + 1.4 GB for `dx serve`'s profile |
| Release build (from empty) | 65 s, bundles and smoke test included | 73 s compile; `tauri build` 298 s (fetches linuxdeploy) | 51 s compile; `dx bundle` +48 s (fetches linuxdeploy) |
| Release binary | 5.3 MB | 4.0 MB | **3.8 MB** |
| Package | `.tar.gz` 2.8 MB, AppImage **3.5 MB** | `.deb` 1.6 MB, AppImage 83 MB | `.deb` **1.4 MB**, AppImage 81 MB |
| Idle window memory, private / PSS | ~81 / ~167 MB (Electron, 7 processes) | 104 / 147 MB (WebKitGTK, 3) | 101 / 151 MB (WebKitGTK, 3) |

- Packages: aio-rs's leave out the window runtime (Electron, 284 MB unpacked),
  fetched and checked once per machine by `ax setup` and shared by every
  aio-rs app. A `.deb` of Tauri or Dioxus leans on the system's WebKitGTK; their
  AppImages carry it (~80 MB each).
- Memory: Electron's 6 processes measured 74 / 160 MB; the app's own
  process can't be read from outside (aio-rs makes it non-dumpable), aio-rs's
  smoke puts it at 7 MB. aio's PSS shrinks while other aio-rs apps share the
  runtime; WebKitGTK's PSS is low because the desktop already maps GTK.
- Builds: aio-rs's template builds dependencies without debug info
  (`debug = "line-tables-only"` for the app only); the other two use
  cargo's default (full). Tauri's rebuild relinks a `staticlib` + `cdylib`
  + binary (its template's crate types).
- `ax dev` builds into one folder per machine (`~/.local/share/aio-rt/target`):
  dependencies built for one aio-rs app serve the next; `dx serve` and `tauri
  dev` build into the project.

## First run: from nothing to a window

| | aio-rs | Tauri 2 | Dioxus 0.7 |
|---|---|---|---|
| System packages (Linux) | none beyond a C compiler: Electron is fetched (desktop distros have its libraries) | `sudo apt install libwebkit2gtk-4.1-dev libxdo-dev libayatana-appindicator3-dev librsvg2-dev build-essential …` | `sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev libxdo-dev …` |
| Steps | install ax (prebuilt, checksummed, or from source) → `ax setup` → `ax create x` → `ax dev x` | Node or `cargo install create-tauri-app` → `npm create tauri-app` (prompts) → `npm install` → `npm run tauri dev` | install dx (`cargo binstall` or its installer) → `dx new x` (prompts) → `dx serve` |
| What the new app has | state saved with history, a UI test, `AGENTS.md`/`CLAUDE.md`, `aio.toml` with every default | window + one command | window + a starter template |
| Docs | `ax agent` (brief for agents), `cargo doc -p aio-rs`, `docs/basics/setup.md`; no website | large site, guides per OS, API docs | site with guides (0.7), API docs |

- On this machine (runtime GTK/WebKit libraries, no `-dev` packages, no
  sudo) aio-rs built and ran as is; Tauri and Dioxus stopped at `pkg-config`
  (`gdk-x11-3.0`, `dbus-1`, `webkit2gtk-4.1`… not found) until their
  development packages were stood in for.
- Dioxus by hand (no `dx new`): `dx bundle` refused twice, one missing
  `Dioxus.toml` key at a time (`identifier`, then `publisher`).
- Tauri: an icon set is required before the first build (`tauri icon`); a
  JS call with a wrong argument name only rejects in the page's console.
- aio-rs: `act!()` returns `Option` (a refusal is logged), so `.is_ok()` on it
  is a plain rustc type error; the error says so.

## Where the others are ahead

- CSS goes through a hot-patch in `ax dev` (0.09 s) where Dioxus swaps the
  stylesheet (0.03 s): both under a frame or a few.
