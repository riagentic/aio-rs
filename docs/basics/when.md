# When to use aio-rs, and when not

aio-rs is for an **app with a window and state**: the state is the
point, kept, replayed and synced, and the window (or service, or tool)
shows and changes it. Read this page before you start; it is short on
purpose.

## Use it for

- **A desktop app that keeps data**: notes, todo, contacts, a ledger, an
  admin tool. One process per app, its state in SQLite on the machine,
  every change saved and replayable ([concepts](concepts.md)).
- **The same app in several places**: a window on Linux, Windows and
  macOS, an Android app, a page in a browser, from one codebase
  ([targets](../build/targets.md)).
- **Local first, synced when there is a network**: copies of a cell on
  several machines and a server, offline edits replayed
  ([sync](../state/sync.md)).
- **A small service or a command-line tool with saved state**, run by a
  supervisor and updated by signed releases ([release](../build/release.md)).
- **Types checked from the cell to the view**: Rust end to end, one native
  binary per app.

## Don't use it for

| You need | Why not | Use instead |
|---|---|---|
| SEO-first content sites (a blog, a shop front, marketing) | an app's pages are rendered per request and hydrated ([views](../ui/views.md#server-rendering-and-hydration)), and streamed, with code splitting, but there is no static generation | a site framework (Next.js, Astro, Leptos SSR) |
| A CDN or the edge (serverless functions, Workers) | an app is one long-running process with its data on a disk | an edge runtime and a hosted database |
| One state across many servers, or millions of stateless requests per second | each app is one process owning its cells ([FAQ](faq.md)) | a stateless API behind a load balancer and a shared database |
| State redrawn every frame (a game, a 3D scene) | views redraw on a change of state, not per frame | a game engine, wgpu |
| An app running on iOS itself | an iPhone or iPad gets a thin client of a server (`ax build --ios`, or the server's page on the Home Screen), as with aio ([targets](../build/targets.md#ios)) | a native iOS stack |
| A frozen API today | 0.x: a minor version may break ([versions](versioning.md)) | aio 1.x, or pin aio-rs (below) |
| A small download with the system webview | desktop windows draw in one shared Electron runtime ([why](why.md#when-not-to-use-aio-rs)) | Tauri, or another system-webview stack ([comparison](../comparison.md)) |

## Pin the version

Never build on aio-rs's `main`: build on a release tag, and move on
purpose.

```sh
ax create notes --aio-version 0.4.2   # the app depends on the tag v0.4.2
ax pin notes                          # says which aio-rs the app uses
ax pin notes 0.4.2                    # moves it, read the CHANGELOG first
```

`Cargo.lock` pins the commit; commit it. Read the
[CHANGELOG](../../CHANGELOG.md) before each move: until 1.0.0 a minor
version may break things, and each break says how to move.

## How proven each part is

**Proven**: run by every pull request's CI on GitHub's Linux, Windows and
macOS runners, and by the release gates ([CONTRIBUTING](../../CONTRIBUTING.md#make-a-release)).
**Experimental**: built and tested, but not yet proven on real devices
per release; it may change more in a minor version.

| Part | Status |
|---|---|
| Cells, methods, the log, persistence, `ax test` | proven |
| Desktop windows (Linux, Windows, macOS) | proven |
| Services and command-line tools | proven |
| Signed updates, delta patches, the way back | proven (Linux, Windows and macOS in CI, `scripts/update-proof`) |
| Sync between a server and its clients | proven |
| Android | **experimental**: gated by an emulator run before each release (`scripts/android-gate`), no Play Store flow |
| The web build (wasm, PWA) | **experimental**: tested in one browser |
| Peers, pairing, discovery on the LAN | **experimental** |
| aio's wire and data (`compat`) | **experimental**: an app may move between aio and aio-rs, checked against aio's fixtures |
| Islands (another view library in the window) | **experimental** |
| The JS client (React, Solid, Svelte, Vue or plain DOM on the cells) | **experimental**: tested in one browser, not on npm yet |
| iOS | not there |

Coming from aio: [aio and aio-rs compared](../aio-vs-aio-rs.md).
