# A secret-holding relay

Some partner APIs (a swap service, a payment provider, a maps key with a
budget) want a key that must not ship inside the app: anything in an
APK, a desktop binary or a web page is read by whoever installs it. The
app's installs call a small server instead, and that server calls the
partner with the key. `ax create <name> --template relay` makes one:

```sh
ax create relay --template relay
RELAY_KEY=<the partner's key> ax dev relay
ax test relay
ax build relay      # one binary (a .tar.gz) and its systemd, launchd or Task Scheduler file
```

It is a template, not a recipe to copy: what makes a relay safe (the
allow-list, the encoding of what installs fill in, no redirects, no
logging, bounded limits) is written and tested once, and its tests run
in the new app as they run in aio-rs's own (`src/lib.rs`).

## What it does

- **Only the listed calls pass.** `CALLS` names each one: its method,
  its path (`:name` for a part the install fills in) and the query keys
  it passes on. Every other path or method needs the app's own key
  (`401`), which installs don't hold; it reaches no one.

```rust,ignore
// ax create's template (src/lib.rs, --template relay), compiled by ax's own tests:
pub const CALLS: &[Call] = &[
    Call { method: "GET", path: "/v1/quote", query: &["from", "to", "amount"] },
    Call { method: "POST", path: "/v1/swap", query: &[] },
    Call { method: "GET", path: "/v1/swap/:id", query: &[] },
];
```

- **The key is added here.** `RELAY_KEY` from the environment (the
  binary refuses to start without it), in the partner's header
  (`KEY_HEADER`). Nothing an install sends goes on but the listed query
  keys, the body and its `Content-Type`: not its headers, cookies or a
  key of its own.
- **Limits per install.** An install sends a random id it keeps
  (`x-install-id`, 8 to 64 letters, digits, `-` or `_`): it has
  `per_install` calls a minute (30), its address `per_address` (300, all
  its installs together); without an id, the address is the install.
  Past them: `429` with `Retry-After`.
- **It keeps nothing.** `[data] persist = false`: no store, no state.
  The counts live in memory for the current minute, at most 100 000
  keys (a new one waits past that).
- **The partner's answer comes back** as it was (status, type, at most
  1 MB), `Cache-Control: no-store`; the partner unreachable or slow
  (15 s) is `502`.

## Deploying it

One binary, built for the server's OS (`ax build relay`, kind `cli`):
copy it, set `RELAY_KEY` where the service starts (a systemd
`EnvironmentFile=` readable by its user alone, a secret store), install
the supervisor file beside it ([targets](../build/targets.md#service)).

- **Behind a TLS proxy** (nginx, Caddy, a load balancer): keep `listen`
  on loopback, set `[server] trust_proxy` so the limits count each
  client's address, not the proxy's, and `hosts` to the public name
  ([behind a reverse proxy](../auth/README.md#behind-a-reverse-proxy)).
- **On its own**: `[server] cert` and `key` (a public CA's) with
  `tls = true` ([TLS](../auth/README.md#tls-and-the-network)).
- `[capabilities] network` lists the partner's host: the relay reaches
  that host and no other, whatever an install asks.
- A new key: change `RELAY_KEY` and restart. Nothing else holds it.

## Security notes

- **An open relay is the risk.** A relay that passes any path (`/*`), any
  method, any query key or any header turns the partner's key into
  everyone's: they reach endpoints the app never uses (an account's
  settings, a withdrawal, a higher tier). List each call; list query keys
  one by one; never forward a caller's headers.
- **A part the install fills in is one part.** It is percent-encoded
  whole (`a/b` goes as `a%2Fb`) and `.` or `..` are refused, so no value
  climbs out of its place in the path to another endpoint.
- **No redirect is followed**: the key goes to the partner's host alone.
- **Logging.** The relay writes nothing of a call: no path parts, query,
  body, answer or key. A failure is said by the call's route alone
  (`GET /v1/swap/:id: the partner API did not answer`). Keep it so when
  you change it: a body or a query holds what people do (addresses,
  amounts), and a log is kept, copied and sent in reports. A proxy in
  front logs too: keep its access log to the path, or off.
- **Install ids are not proof.** Anyone can make one; the per-address
  limit is what bounds a caller who changes ids every call. The relay
  proves nothing about who calls: a call it lists is open to anyone who
  finds it, within the limits. When abuse would cost (a paid tier), add
  accounts ([auth](../auth/README.md)) or the platform's app attestation
  in front of the calls that matter, and keep the partner's own limits
  and alerts on.
- **A browser page** of another origin can't call it (no CORS headers):
  serve the page from the relay's origin, or call it from the app.

## Calling it from the app

From an install: the listed path on the relay's address, with the
install's id. The desktop and Android apps use aio's HTTPS client
(`aio::capabilities::agent`, the relay's host in their own
`[capabilities] network`); the id is made once and kept with the app's
data (a field of a saved cell, or a file in `data/`).

```text
GET https://relay.example.org/v1/quote?from=SOL&to=USDC&amount=1
x-install-id: 6f1c2e0a9b7d4c3e
```

## Tests

`src/lib.rs` tests the relay against a partner played on loopback:

- only the listed calls reach the partner, with the relay's key (never
  the caller's) and the listed query keys alone; an unlisted path or
  method and a `..` part reach no one;
- an install has its calls a minute, an address those of all its
  installs, and a new minute starts over;
- nothing of a call is written: the key, the body, the answer and what
  the install filled in appear in no log line, a failure included.
