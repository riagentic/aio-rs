# aio wire formats: aio-rs talking to aio

What an aio-rs app speaks to work with aio apps, pages and tools. Source:
aio's code (`src/…` paths below). Literal values on the wire (`v1` in a
probe, `aio-pair-v1`, `x-aio-peer: v1 …`) are aio's protocol names and are
spelled exactly so.

## What listens: aio and aio-rs

| Who talks | aio | aio-rs |
|---|---|---|
| the app's window | a Unix socket or named pipe (NDJSON envelopes), the page over `aio://` | IPC through the Electron runtime's preload; no socket, no port |
| this user's tools (`am`, `ax`), other apps on the machine | the same local socket, `x-aio-peer` on TCP | the local door: a Unix socket in a private dir (`$XDG_RUNTIME_DIR/aio-rt/…`) or a named pipe, length-prefixed frames carrying `/ws`'s JSON. A release build's door has a client's rights, a debug build's the operator's |
| clients over the network | WebSocket `/ws` (`--expose`, HTTPS) | WebSocket `/ws` (`[server] listen`, `--expose`), wire 2; aio's protocol 3 too with `compat` |
| a browser or another service | TCP only when a port is named | only in `client-server`/`server` mode |

So aio's pages, `am` and aio apps reach an aio-rs app over `/ws`, on a
served app built with `compat`; aio's local socket and its NDJSON framing
are not spoken by aio-rs's door. A desktop-only aio-rs app (`standalone`)
has no port at all, as an aio desktop app has none.

## aio-rs's feature `compat`

- With `features = ["compat"]`, `aio::compat` adds:
  - `wire`: protocol v3 on `/ws` for aio pages and tools (with `server`). A cell's `#[aio_cell]` fields are aio cells of their own there: `state`, `patches`, `subs` and `resync` per slice; `<slice>:<m>` calls the cell's `<slice>_<m>`, else `m`.
  - `crdt`: aio's offline sync (`op`, `sync-req`) on aio-rs's event log: an op is a synced call (its id made from the op's id and stamp, so a re-send is acked again, never run twice), `serverTs` its event's seq; every catch-up is a snapshot at the cell's seq (aio's answer after a compaction); no `push` frames (aio's client asks again when state frames differ). aio's own client proves it: `AIO_DENO_SRC=<copy of aio>/src cargo test -p aio-rs --all-features --test compat_wire` (needs deno).
  - `AioLink`: an aio-rs app as a client of an aio app (proto, state, patches, action/ack, server fns).
  - `local`: aio's lock record (`<lock dir>/<key>.lock`, aio's folder and key) and aio's local socket (NDJSON lines of aio's envelope, `ctl` → `ctlr`), so `am instances` lists a running aio-rs app and `am state`, `am dispatch` reach it through its doors, a debug build's only (aio's trojan is dev-only); `/__aio/health` on any build. No `server` needed. [ax](../clients/ax.md#what-am-sees-of-an-aio-rs-app)
  - `AioPeer`: an aio app on this machine called as this app, proven (aio's `peer()`): found by its lock record, reached on its local socket (a named pipe on Windows), aio's identity hello first; aio's socket sends no `boot`, so its first `state` ends the hello there. aio's own app proves it: `AIO_DENO_SRC=<copy of aio>/src cargo test -p aio-rs --all-features --test compat_peer` (needs deno).
  - `scatter`: aio's one-time move of a scattered layout into the home ([data](data.md) 7).
  - `#[on_aio]`: a cell's hook on what aio changed in its `state.db`.
- Without it: a `/ws` client that doesn't send `x-aio-wire: 2` (or `?wire=2`) gets HTTP 426 before the upgrade; `#[on_aio]` fails the build with the line to add. Every aio-rs client sends the marker. `ax clients` shows each link's wire: `aio-rs`, `aio` or `tool`.
- Not in it: reading and writing aio's `state.db` (`aio_kv`, `#[aio_cell]` slices, `#[table]`, the home claim, `[app] port_of`, `[home]`). That is every native build's (`aio::compat::data`).
- LAN discovery and pairing (below) are aio-rs's own, in `tls`: they are not behind `compat`.
- CI: `compat` alone, `compat,net`, and `net` without it (`sync::without_aio_only_wire_2_is_spoken`).

## LAN discovery: UDP 8099 (`server/discovery.ts`)

- Probe, then reply. Nothing is announced on a timer. Port 8099; `AIO_DISCOVERY_PORT` overrides it (1–65535).
- **Probe** (ASCII): `AIO_DISCOVER? v1` or `AIO_DISCOVER? v1 <nonce>`.
  - `nonce`: `^[A-Za-z0-9_-]{1,64}$`.
  - Sent to `255.255.255.255` and to each IPv4 interface's directed broadcast (skips /31 and /32).
  - The client collects replies for 1200 ms.
- **Responder:** `udp4`, `reuseAddr`. Answers only private or loopback sources, at most 5 replies per second per source.
- **Reply:** one datagram per exposed app: `AIO1 ` followed by JSON. Fields:
  - `name` (the appId), `port`, `title?`, `needsAuth`, `tls`
  - `nonce?`: echoed back when the probe had one
  - `addrs?`: the host's IPv4 addresses, at most 32
- **Peer key:** `host:port:name`. Replies from two addresses of one machine are merged when each address appears in the other's `addrs`.
- No signing.

## Pairing (`server/pairing.ts`, `server/cpace.ts`, `server/pair-pake-route.ts`)

- **Endpoint:** `POST /__aio/pair`. Body is JSON, at most 4096 bytes.
- **PIN:** 6 digits (8 = PAKE mode). Valid for 3 minutes and single-use.
  - Wrong guesses: 8 per address (/64 for IPv6); 20 in total burns the PIN.
- **Plain mode:** `{pin}` returns `{aio:1, name, title, port, tls, cert|null, key}`, the app's shared key, with `no-store`.
  - Errors: 400, 401 `{error, hint}`, 413, 429.
- **PAKE mode:** CPace draft-21, `CPACE-X25519-SHA512`.
  - `fp` = SHA-256 of the DER certificate (empty over http).
  - PRS = PIN; CI = `lv_cat("aio-pair-v1", fp)`; ADa = `""`; ADb = `fp`.
  - Message 1: C→S `{cpace, sid(16 bytes), Ya(32 bytes)}`; S→C `{cpace, id, Yb, ADb}`.
  - Message 2: C→S `{cpace, id, Ta(64 bytes)}`; S→C `{Tb, box}`.
  - mac_key = SHA-512(`"CPaceMac"` ‖ sid ‖ ISK). Ta / Tb = HMAC-SHA512(mac_key, lv_cat(Y, AD)).
  - `box` = iv(12 bytes) ‖ AES-256-GCM(HKDF-SHA256(ISK, salt = sid, info = `"aio-pair-v1 profile"`), profile JSON).
  - An exchange expires after 30 s.
  - Check the base64 alphabet in `cpace.ts`.
- **Re-pin proof:** `POST /__aio/pin-proof` `{nonce(32–128 hex), tag}`.
  - `tag` = HMAC-SHA256(key, `"aio-pin-proof-v1:c:"+nonce`).
  - Reply `{proof, cert}`, where `proof` = HMAC-SHA256(key, `"aio-pin-proof-v1:s:"+nonce+":"+cert` with whitespace removed).
- **Key afterwards:** `?token=`, `Authorization: Bearer`, or a cookie.

## Same machine: `x-aio-peer` (`server/peer-proof.ts`)

- **Header:** `v1 <appId> <pid> <mac>`, where `mac` is 64 lowercase hex characters.
- **MAC:** HMAC-SHA256(caller's `<data>/control.key`, `"aio-peer/v1\n"+callerApp+"\n"+callerPid+"\n"+targetApp+"\n"+targetPid`).
- **`control.key`:** 32 random bytes as hex plus a newline, mode 0600, minted each boot.
- Local, non-proxied addresses only, on `/ws` and `/__aio/blobs*`.
- **Over UDS:** the first frame is `{"v":2,"t":"type","d":{"kind":"peer","proof":"…","cid":"<uuid>"}}`, answered with `ack`.

## Envelope and protocol v3 (`protocol/envelope.ts`, `protocol/protocol-version.ts`)

- **Transport:** WebSocket `GET /ws` (one JSON text frame per message), or UDS with newline-delimited JSON.
- **Envelope:** `{"v":2,"t":"<kind>","d"?}`. Kind `x` is reserved: ignore it.
- **Handshake:** the server sends `proto {v:3, min:3, ver?, app?, rate?, maxMessageBytes?, bytesPerSec?, cfg?, appId?, user?}`.
  - The client replies `proto {v, min, …}`. The effective version is the smaller `v`, and it must be ≥ both `min`s.
  - Mismatch: `proto-err {reason}`, then `__proto-err:<reason>`, then close code 4505.
- **Connect sequence:** `proto`, then `state`, then `icon*`, then `boot {id}`, then `cfg?`.
- **Full state:** `{"v":2,"t":"state","d":{<cell>:…},"s":string[]|null}`.
- **Delta:** `patches` with `d` = `[{op:"add"|"replace"|"remove", path, value?}]`.
  - `{op:"append", path, value}` means `old + value`.
  - `path[0]` is the cell name. Refuse any `__proto__` segment.
- **Subscribe:** `subs {subs:[dot paths]|["*"]}`, at most 1024 paths, never empty. `resync` asks for the state again.
  - aio narrows to whole cells (`todos.items` subscribes `todos`), and so does aio-rs; aio-rs's `exact: true` narrows to the paths themselves (aio never sends it).
- **Backpressure:** `vitals-ping {t1, ms?, h?}`, `ms` the page's render staleness: past 100 ms the client's patches go 2x the window apart, past 300 ms 4x; three under 100 ms step it back.
- **Call:** `action {type:"<cell>:<method>", payload:{args:[…]}, cid?, stream?}`. The server strips `_source` and refuses `cell:__*`.
- **Reply:** `ack {cid, ok, value?, error?, code?, retryAfterMs?, unsaved?, short?}`. `chunk {cid, value}` frames come before a streamed `ack`.
- **CRDT:** `op`, `sync-req`, `sync-res`, `sync-ack`, `op-rejected`, `sync-err`.
  - HLC = `[ms, counter, nodeId]`.
  - `op {id, cell, action, payload:{args}, hlc}` → `sync-ack {cell, opId, serverHlc, serverTs, serverNow, unsaved?}` or `op-rejected {opId, cell, reason, serverNow?}`; peers get `op {…, serverTs}`.
  - `sync-req {clientId, reqId?, session?, sessionKey?, cells:{<cell>:{lastHlc, lastServerTs?}}, pendingOps, resync?, pushPatch?}` → acks for its pending ops, then `sync-res {mode: incremental|snapshot|push, reqId?, ops, snapshot?, lowWater, lastServerTs, reset?, serverNow}`.
  - Held (time travel paused, closing): `sync-err {reason}`; the client asks again. Refused as `stale-beyond-retention…`: the client makes the op again.

## Lock records (`server/single-instance-lock.ts`, `server/lock-dir.ts`)

- **Directory:** `$XDG_RUNTIME_DIR/aio`, else `/tmp/aio`, mode 0700.
- **File:** `<appId>.lock`, `<appId>@<profile>.lock` or `<appId>@<fnv1a8(home)>.lock`.
- **JSON:** `appId, pid, port, startedAt, status(starting|started|stopping), cwd`, plus optional `discovery {title?, tls, needsAuth}`, `socketPath`, `httpSocketPath`, and others.
- With feature `compat` an aio-rs app files its own (`compat::local`): at its first served cell (with `socketPath`: aio's local socket, for `am`), or at the first `AioPeer` link (without one: listed, reached by nothing). Its key is aio's `lockKey` (a profile names it only where the home is that profile's), with `home`, `dataDir`, `startToken`, `aioVersion`, and one per-run `<home>/data/control.key` (0600) for `am`'s `X-Aio-Control` and the `x-aio-peer` proofs both. A clean stop (`aio::shutdown`) removes the record and the key; a crash leaves a record aio drops as stale. It still claims the home (`.aio-instance.lock`, [data](data.md)), so aio and aio-rs never run on the same data.

## Blobs

- `GET /__aio/blobs/<sha256>`, with Range support.
- `POST /__aio/blobs?name=` with raw bytes returns `{id, size, name}`. Proven peers only.
