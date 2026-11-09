# Other view libraries: the JS client

aio-rs's views are `aio::ui` (`rsx!`). A page drawn by another library
(React, Preact, Vue, Svelte, Solid, plain DOM) reaches the same cells
through aio-rs's JS client (aio's `aio/state-core`): it holds a copy of
each cell it opens, follows the server's patches, calls `#[client]`
methods (shown at once with a prediction, answered by the server) and
keeps calls made offline until the server takes them. It speaks the
app's own wire (wire 2 on `/ws`, every cell on one socket), so the
server's rules hold for it as for any copy: the key or a session,
`#[client]` and its roles, `visible`, pacing.

It has no dependency, and every served app hands it out. In the desktop
window, whose page may connect nowhere, it rides the window's own wire to
the app instead ([below](#in-the-desktop-window)), with the same API.

| Where | What |
|---|---|
| `/__aio/client.js` | the client, served by every aio-rs server (a module: `import { connect } from "/__aio/client.js"`) |
| `crates/aio/client` | the same file as a package, `aio-rs-client` (not on npm yet): `aio-rs-client`, `aio-rs-client/react`, `aio-rs-client/solid`, `aio-rs-client/vue`, with their types |
| `ax tsgen <app>` | the app's cells and methods as TypeScript types |

## A page of plain DOM

A page the app serves (a `Route`, `[server] dirs`) loads the client from
its server:

```html
<p id="n"></p><button id="add">+1</button>
<script type="module">
  import { connect } from "/__aio/client.js";
  const tally = connect().cell({ id: "tally", v: 1 });
  tally.select((s) => s.n).subscribe((n) => (document.getElementById("n").textContent = n));
  document.getElementById("add").onclick = () =>
    tally.call("add", [1], { predict: (s) => ({ ...s, n: s.n + 1 }) });
</script>
```

- `connect()` links to the server that served the page (`ws(s)://<host>/ws?wire=2`),
  passing on the page's `?token=`; a page a `?token=` link opened has the
  key's cookie, which its socket shows. `connect({ url })` names another.
- `client.cell({ id, v })`: the cell's id and its version (`#[cell(version
  = n)]`, 1 unless set). A copy of another version is refused, as a native
  copy is; `ax tsgen` writes both.
- The page's policy must allow the script: an app's own page does
  (`script-src 'self'`); `aio_shell::web::tabs` and the window, whose
  pages run only their own scripts, don't (use an island there, below).

## The client

| Call | Does |
|---|---|
| `connect({ url, storage, app, WebSocket })` | a client; `storage` keeps each copy (default `localStorage`; `null`: none), `app` names its keys |
| `client.cell(ref)` | the copy of one cell (one per id, shared) |
| `cell.ready` | settles once a state is held |
| `cell.get()` | the state to show: the server's plus the waiting calls' predictions; `undefined` until `ready` |
| `cell.confirmed()` | the server's state as last heard |
| `cell.select(f, eq)` | `f(state)` as a store: it changes only when `f`'s answer does (`eq`, default `Object.is`) |
| `cell.subscribe(f)`, `cell.listen(f)` | `f` now and after each change; `f` after each change |
| `cell.call(method, args, { predict })` | the server's answer (a promise); `predict(state)` shows the call at once |
| `cell.status` | a store of `{ online, pending, conn, degraded }`; `conn` is `online`, `offline`, `syncing` or `blocked` |
| `cell.predict({ method: (state, ...args) => state })` | predictions by method name, for calls without their own |
| `cell.lastSync()` | when the server's state last came |
| `cell.onRefused(f)` | `f(method, why)` when the server refuses a call |
| `cell.close()`, `client.close()` | let go of the link |

- **Calls.** `call` sends `method` with `args` (the method's arguments in
  order) and resolves to what the method returned on the server; an async
  method resolves at its end. A refusal rejects with a `Rejected`: `code`
  is `null` for the method's own (`ensure!`), else `ACCESS_DENIED`,
  `ACTION_REFUSED`, … (as `Rejected::code()`).
- **Predictions.** The client can't run the method's Rust, so a
  prediction is a function of the state: `predict` shows the call at once;
  the server's change, named for the call, replaces it (shown once, never
  twice); a refusal takes it away. A prediction that throws shows nothing.
  `cell.predict({ add: (s, by) => ({ ...s, n: s.n + by }) })` gives
  predictions by method name: a call without its own shows by them, and
  so does a call restored after a reload or taken from a twin tab.
- **Patches.** Each patch applies to the copy without touching the parts
  it didn't change, so a selector of an untouched part keeps its value and
  the views reading it don't run. A patch that doesn't follow the last
  one, or doesn't fit, asks the server for its whole view (`resync`):
  never a guess.
- **Digest.** The client says `digest: "js"` in its hello, and the server
  adds `jsdigest` to each patch: SHA-256 of the view it makes as
  JavaScript holds it (canonical JSON, keys in code point order, each
  number as `String(x)` writes it: `1.0` as `1`, `1e21` as `1e+21`, `-0`
  as `0`), the first 16 bytes in hex. The client hashes the view the patch
  makes; if it differs, the view isn't shown and the server's whole view
  is asked for. A copy that doesn't ask (every native one) gets its
  patches as before, with `digest` alone. `jsDigest(v)` is exported.
- **Offline.** A call made offline shows (its prediction) and waits, up
  to `PENDING_MAX` (500; then `QUEUE_OVERFLOW`); kept in `storage`, it
  survives a reload and is sent once linked, and the server applies each
  call id once. A copy kept by another user is shown, never its calls.
  A reloaded call shows by its method's predictor (`cell.predict`); one
  the server's view already holds is not shown twice.
- **Twin tabs.** Two pages of the app in one browser share their waiting
  calls through `storage` (as the wasm copy does): a call made in a tab
  that is offline is shown and sent by a linked one, once; a call either
  saw answered is never put back.
- **Links.** Every cell of a client rides one socket (`aio-mux`; an older
  server: a socket per cell). The server pings each link every 2 s; one
  that is silent for 5 s (`DEAD`) is dropped. A dropped link is opened
  again after 250 ms, doubling up to 10 s, with jitter; a refusal (another
  cell, a wrong key) is not tried again, another version of the cell is.
- **Parts.** `client.cell({ id, v, paths: ["items"] })` holds only those
  parts of the cell, as a native copy's `subscribe`.

## React

```jsx
import { connect } from "aio-rs-client";
import { useCall, useCell, useStatus } from "aio-rs-client/react";
import { tally } from "./aio-types"; // ax tsgen

const cell = connect().cell(tally);

export function Counter() {
  const n = useCell(cell, (s) => s.n); // re-renders only when n changes
  const { conn } = useStatus(cell);
  const [add, { error }] = useCall(cell, "add", (s, by) => ({ ...s, n: s.n + by }));
  return <button onClick={() => add(2)} title={error?.message ?? conn}>{n}</button>;
}
```

- `useCell(cell, select, eq)`: the state, or the part `select` picks; the
  component renders again only when it changed (`useSyncExternalStore`).
- `useCall(cell, method, predict)`: `[call, { pending, error, data }]`;
  `call(...args)` resolves to the server's answer, or to `undefined` when
  refused (`error` says why: nothing to catch in a handler).
- `useStatus(cell)`: the link's status.

## Solid, Svelte, Vue

`cell.select(..)` and `cell.status` are stores of Svelte's contract
(`subscribe` calls back at once, then on each change), which Solid's
`from` takes too; Vue gets refs (`aio-rs-client/vue`):

```js
// Solid: aio-rs-client/solid
import { createCell, createStatus } from "aio-rs-client/solid";
const n = createCell(cell, (s) => s.n); // an accessor: an effect reading it runs when n changes
```

```svelte
<!-- Svelte: a store as it is -->
<script>
  const n = cell.select((s) => s.n);
</script>
<button on:click={() => cell.call("add", [1])}>{$n}</button>
```

```js
// Vue: aio-rs-client/vue, in a component's setup
import { useCell, useStatus } from "aio-rs-client/vue";
const n = useCell(cell, (s) => s.n); // a shallow ref: what reads it renders when n changes
```

## In an aio-rs page: islands

A React tree (or any library) inside an `rsx!` page is an island
([islands](../ui/islands.md#react-components)); with the client it reads
and calls the page's own cells, and the page's view and the island show
the same state:

```jsx
// island.jsx, bundled: esbuild island.jsx --bundle --format=iife --global-name=m --footer:js='return m.default'
import * as react from "react";
import * as reactDomClient from "react-dom/client";
import { connect } from "aio-rs-client";
import { useCell } from "aio-rs-client/react";

const cell = connect().cell({ id: "tally", v: 1 });
const Tally = () => <b>{useCell(cell, (s) => s.n)}</b>;
export default { react, reactDomClient, component: Tally };
```

In a page served to a browser (a web build, `aio_shell::web::tabs`) the
island's socket is the page's server's (`connect-src 'self'`).

## In the desktop window

The window's page may connect nowhere. `use_js_client` in the app's root
view puts a socket in the page that rides the window's own wire to the
app's process; `connect()` there takes it, so an island's code is the same
in a window and in a browser:

```text
use aio::kit::{JsCell, use_js_client};

#[component]
pub fn App() -> View {
    use_js_client([JsCell::from(HANDLE.read_untracked().clone())]); // client!'s HANDLE
    rsx! { WebIsland::<(), ()> { island: CHART, props: () } }
}
```

- Each link is served as a network client's is (its hello, `#[client]`
  and its rules, `visible`, patches, pings), on the window's copy of the
  cell: the app's own store, or in a `client` app its server's (carried
  there as it is, with the app's key).
- Call it once, in the root, before the islands mount; it lives as long
  as that view. A cell not given is refused by name.

## Types: `ax tsgen`

```text
ax tsgen <app> [--out file]
```

Reads the app's source (no running app needed) and writes, by default
`aio-types.ts` beside its `aio.toml`, a typed `CellRef` per
`#[cell(shared)]` cell: its state as serde writes it (`rename_all`,
`rename`, `skip`, `flatten`, tagged and untagged enums, `Option` as
`| null`, maps as `Record<string, V>`; `#[server]` fields left out, a
`Hidden` field as its value or `{ $hidden: true }`), and each `#[client]`
method's arguments (`cx: &Cx` left out) and answer (`Result<T>` answers
`T`). `client.cell(tally)` is then typed: a method that isn't
`#[client]`, a wrong argument or a field read by its Rust name fails the
page's type check.

```ts
export const tally: CellRef<{
  state: Tally;
  methods: {
    add: { args: [by: number]; ok: number };
  };
}> = { id: "tally", v: 1 };
```

A type it can't see (one from another crate, a generic parameter) is
`unknown`. Numbers are JavaScript's: a `u64` past 2^53 loses its last
digits, as in any JSON client.

**Integers past 2^53.** `JSON.parse` reads `9007199254740993` as
`9007199254740992`, the nearest double. The server's `jsdigest` hashes
that same double (`patch::js_digest`), so such a view checks clean and the
digest never asks again and again for a view the page can't hold; the lost
digits stay lost. Send an id or amount that needs them as a string.

## Tests

`crates/aio/client` has the client's unit tests (`node --test test/`,
a stand-in server) and its type check (`tsc -p test`, the generated types
of `test/types/fixture.rs` at work). aio-rs-shell's `js_client` test runs
them, then:

- serves a cell to a headless Chromium: React, Solid, Svelte and Vue
  islands and the page's own `rsx!` view on it, each one's calls reaching
  the others, a refusal rolled back, and a change of another field running
  none of them again; a page of plain DOM whose call made offline shows by
  its predictor, survives a reload, and is sent once linked, and whose
  twin tab sends the call an offline tab made;
- mounts the same islands in the desktop window's page (`TestUi`, the
  Electron runtime) over `use_js_client`, on the app's store and, in a
  `client` app, on its server's.

It needs `npm ci` in `crates/aio/client` (`./ci.sh` runs it).

## Compared with aio

| aio | aio-rs |
|---|---|
| `aio/state-core`: `getCellSignal`, `getStateSignal`, `send`, `setTransport`, `handleMessage`, `ready` | `connect`, `cell.select`, `cell.get`, `cell.call`, `cell.ready`; the wire is aio-rs's own |
| AIR's React-named hooks (`useAio`, `useCell`) | `aio-rs-client/react`: `useCell`, `useCall`, `useStatus` on real React |
| Svelte and Vue recipes on `getStateSignal()` | stores of Svelte's contract per cell and per part; `aio-rs-client/solid`, `aio-rs-client/vue`; each tested on the real library |
| the window's page over IPC | `use_js_client`: the window's own wire, the same API |
| an action sent, its reducer run in the page | a call sent; its prediction a function, the answer the server's |
| an offline queue in memory | an offline queue kept in `storage`, sent once per call id |
| types from the app's own TypeScript | `ax tsgen`, from the app's Rust |

## Limits

- Not on npm yet: take `crates/aio/client` by path, or `/__aio/client.js`.
