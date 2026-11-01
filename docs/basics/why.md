# Why aio-rs, not plain Rust

Any aio-rs app could be written with plain Rust: a UI crate, SQLite, a
sync layer and a packager, glued by hand. This page says what aio-rs
gives over that, and when plain Rust (or something else) is the better
choice. Every line is a feature of aio-rs 0.1.0 with the page that shows it.

## What you'd write yourself, what aio-rs gives

| You'd write | aio-rs |
|---|---|
| A state struct behind a `Mutex`, a command enum, an events table, replay and undo code | `#[aio::cell(persist)]` + `#[aio::methods]`: every call is an event in the store's log; `ax events`, `ax replay`, `ax timetravel` step through it, in a release build too, after a restart ([cells](../state/cells.md), [time travel](../debugging/time-travel.md)) |
| A rusqlite schema, serialization, versioned migrations, a debounce for frequent writes, an integrity check | `#[cell(persist, version = N, migrate = f)]`; `#[coalesce]` holds a held key's writes to one per 100 ms window; every boot runs `quick_check` and recovers; `ax backup`, `ax restore` ([persistence](../persistence/README.md)) |
| A WebSocket server, diffs, reconnects, an offline queue, conflict rules, access checks | `#[cell(shared)]`, `[server] listen`: copies predict each call and the server confirms it; `#[client]` rules, `visible` per viewer, an offline queue that can survive a restart (`connect_saved`) ([sync](../state/sync.md), [offline](../persistence/offline.md)) |
| Certificates, a pairing flow, LAN discovery, accounts | TLS on every exposed launch (a machine root for the LAN, your certificate for a public server), PAKE pairing (`ax pair`), `ax discover`, `[accounts]` with TOTP and OIDC ([sync](../state/sync.md), [accounts](../auth/README.md)) |
| One shell per target | one app as a desktop window (Linux, Windows, macOS), a web build (wasm, PWA), an Android APK, a headless service or a command-line tool: `[app] mode` and `ax build` ([targets](../build/targets.md)) |
| Glue from your state to a UI crate, a list that stays fast | `aio::ui` views (`rsx!`) reading fields with `part`, calling with `act!`; the kit: controls, keys, windowed lists, forms, dialogs, toasts ([views](../ui/views.md), [kit](../ui/kit.md)) |
| Mocks of the store, a clock you can move, a headless browser harness | `TestCells` (virtual time, seeded fuzzing), `TestUi` (the real views in the runtime, headless), `TestServer`, cassettes ([testing](../testing/README.md)) |
| `cargo watch` and a restart that loses the screen's state | `ax dev`: Rust edits hot-patched with the state kept, CSS live, a compile error on the window while the app keeps running ([dev](../build/dev.md)) |
| Packaging per OS, signing keys, an updater, a way back from a bad release | `ax build` (`.tar.gz`, `.deb`, AppImage, Windows installer, macOS `.app`, APK, web), `ax keygen`, `ax release`: signed and delta updates, gated on data compatibility, rolled back when a new version fails to start ([release](../build/release.md)) |
| Log files, a health endpoint, a way to read live state | `ax state`, `ax logs`, `ax health`, `ax doctor`, `ax sql`, `ax heap`, problem reports ([debugging](../debugging/README.md)) |
| A README for your coding agent | `ax agent`, and `AGENTS.md`/`CLAUDE.md` in every new app ([agents](../agents.md)) |

In code, the whole saved, replayable, synced state of a counter is:

```text
#[aio::cell(persist, shared, id = "counter")]
pub struct Counter { pub count: i64 }

#[aio::methods]
impl Counter {
    #[client]
    pub fn add(&mut self, by: i64) { self.count += by; }
}
```

The same type is checked from the cell to the view: a misspelt field or a
wrong argument type is a build error, and `aio.toml` is parsed as the
app builds, so a typo in it is one too.

## When not to use aio-rs

- **It is alpha.** 0.1.0 is the first release; until 1.0.0 a minor
  version may break things ([versions](versioning.md)).
- **Not on crates.io yet.** Apps build on an aio-rs source checkout; a
  prebuilt `ax` needs `ax create --aio <checkout>` ([setup](setup.md)).
- **Electron draws desktop windows.** One pinned runtime (284 MB unpacked)
  is fetched once per machine and shared by every aio-rs app, or bundled
  into a release (`[build] runtime = "bundled"`). An idle window costs
  ~81 MB private, ~167 MB PSS ([comparison](../comparison.md)). If a
  system webview and a small download matter most, Tauri or Dioxus
  desktop fit better.
- **Rust compile times.** A Rust edit recompiles its whole crate:
  0.6 s in the template, ~10 s in a crate of about 74 000 lines; a first build
  takes 15 s to ~60 s. Splitting a big app into crates brings a page edit
  to ~1.9 s ([what an edit costs](../build/README.md#what-an-edit-costs)).
  No hot-patching in the web build or on Android.
- **No iOS.** And no prebuilt `ax` for macOS (build it from source).
- **Not OS-signed yet.** Windows SmartScreen and macOS Gatekeeper warn on
  first start of the prebuilt archives and app bundles.
- **`aio::ui` views only** in the app's window; another view library comes
  in as an island or as a separate client over the wire.
- **The state isn't the point.** Server-rendered, SEO-first pages; state
  redrawn every frame (a game, a 3D scene); one state across many servers;
  millions of stateless requests per second: each app is one process
  ([FAQ](faq.md)).

Coming from aio, the Deno-based framework: [aio and aio-rs compared](../aio-vs-aio-rs.md).
