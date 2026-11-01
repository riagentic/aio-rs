# Integrations

How an aio-rs app talks to the world beyond its own clients: it calls
outside HTTP APIs, receives webhooks, takes file uploads, lets people
sign in with an identity provider, and is watched by a monitoring
system. The example is a small shop hub that does all five.

| Need | Use | Section |
|---|---|---|
| call an outside API, retry with backoff | an async method, `aio::capabilities::agent`, `cx.backoff` | [1](#1-calling-an-http-api) |
| receive a provider's webhook | a public `Route` with a signature check | [2](#2-webhooks) |
| take files from clients | a streamed `Route` into `aio::blobs::Blobs` | [3](#3-uploads) |
| sign in with an identity provider | `Http::oidc` | [4](#4-sign-in-with-an-identity-provider) |
| scrape metrics, probe health | `/__aio/metrics`, `/__aio/health` | [5](#5-monitoring) |

Every Rust block on this page compiles and runs as a test of aio-rs. The
app needs aio-rs's features `net` (the server, routes, sign-in) and
`outbound` (the HTTP client; `net` includes it), and `ureq = { version =
"3", default-features = false, features = ["rustls"] }` for the agent's
type. Prerequisites: [sync](../state/sync.md#server-functions-and-routes)
and [accounts](../auth/README.md).

## 1. Calling an HTTP API

An outside call is I/O, so it never runs in a plain method. It runs as
the await of an async method: the part before the await is a call, the
request runs as an effect, and the part after it is a second call with
the answer, which the log keeps. A replay of the log rebuilds the rates
without asking the API again.

```rust
use aio::prelude::*;
use std::collections::BTreeMap;
use std::time::Duration;

#[data]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Payment {
    pub id: String,
    pub cents: u64,
}

#[data]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Upload {
    pub blob: String,
    pub name: String,
    pub size: u64,
}

#[cell(persist, shared, id = "hub")]
pub struct Hub {
    pub api: String,
    pub rates: BTreeMap<String, f64>,
    pub rates_at: Option<i64>,
    pub rates_error: Option<String>,
    pub tries: u32,
    pub payments: Vec<Payment>,
    pub uploads: Vec<Upload>,
}

type Rates = BTreeMap<String, f64>;

/// `GET url` as JSON, through aio's agent: every host it reaches is held
/// to `[capabilities] network` before a byte is sent. Blocking: run off
/// the async threads.
fn get_rates(url: &str) -> std::result::Result<Rates, String> {
    let cfg = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(10))).build();
    let agent = aio::capabilities::agent("rates", cfg);
    let mut res = agent.get(url).call().map_err(|e| format!("{url}: {e}"))?;
    let text = res.body_mut().read_to_string().map_err(|e| e.to_string())?;
    aio::serde_json::from_str(&text).map_err(|e| format!("{url}: not rates ({e})"))
}

async fn fetch_rates(url: String) -> std::result::Result<Rates, String> {
    aio::rt().spawn_blocking(move || get_rates(&url)).await.unwrap_or_else(|e| Err(e.to_string()))
}

#[methods]
impl Hub {
    /// Where the rates come from; the first refresh in a second.
    pub fn use_api(&mut self, cx: &Cx, url: String) -> Result {
        ensure!(url.starts_with("https://") || url.starts_with("http://127.0.0.1"), "an https:// URL, please");
        self.api = url;
        cx.after("refresh", 1_000, "refresh", ())
    }

    /// Fetch the rates. Then again in an hour, or sooner after a failure:
    /// 5 s, 10 s, 20 s… at most 5 minutes.
    pub async fn refresh(&mut self, cx: &Cx) -> Result {
        ensure!(!self.api.is_empty(), "no rates API set (use_api)");
        let got: std::result::Result<Rates, String> = fetch_rates(self.api.clone()).await;
        match got {
            Ok(rates) => {
                (self.rates, self.rates_at, self.rates_error, self.tries) = (rates, Some(cx.now()), None, 0);
                cx.after("refresh", 3_600_000, "refresh", ())
            }
            Err(why) => {
                (self.rates_error, self.tries) = (Some(why), self.tries + 1);
                let wait = aio::Backoff { base: 5_000, factor: 2.0, max: 300_000 };
                cx.backoff("refresh", self.tries - 1, wait, "refresh", ())
            }
        }
    }
#     pub fn paid(&mut self, id: String, cents: u64) {}
#     pub fn uploaded(&mut self, blob: String, name: String, size: u64) {}
}

# #[cell(shared)]
# pub struct FakeApi {}
# #[methods]
# impl FakeApi {}
# fn main() {
use aio::http::{Http, Reply, Route};
use aio::testing::TestServer;
let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
rt.block_on(async {
    // The outside API, played on loopback.
    let rates = Route::get("/v1/rates", |_| async { Reply::text(r#"{"EUR":0.92,"GBP":0.79}"#) }).public();
    let api = TestServer::start(Handle::<FakeApi>::memory(), Http::default().route(rates)).await.unwrap();

    let h = Handle::<Hub>::memory();
    h.use_api(format!("http://{}/v1/rates", api.addr)).unwrap();
    h.refresh().unwrap();
    h.done("refresh").await; // its effect, then its step
    assert_eq!(h.state().rates["EUR"], 0.92);
    assert_eq!(h.state().tries, 0);

    // A failure is kept, counted and retried later.
    h.use_api("http://127.0.0.1:9/v1/rates".into()).unwrap(); // nothing listens on port 9
    h.refresh().unwrap();
    h.done("refresh").await;
    assert!(h.state().rates_error.is_some());
    assert_eq!((h.state().tries, h.state().rates["GBP"]), (1, 0.79), "the last good rates stay");
});
assert_eq!(aio::Backoff { base: 5_000, factor: 2.0, max: 300_000 }.ms(3), 40_000);

// A host the app didn't list is refused before anything is sent, and says how to allow it.
let listed = vec!["api.example.org".to_string()];
let no = aio::capabilities::held(Some(&listed), "rates", "https://rates.example.net/v1").unwrap_err();
assert!(no.to_string().contains(r#"add "rates.example.net" to network in aio.toml"#));
# }
```

- **The network list**: `[capabilities] network = ["api.example.org"]`
  in `aio.toml`. An app reaches this machine and the hosts listed, no
  other: the agent's resolver checks each host, redirects too. An entry
  may be a host, `host:port`, `*.example.org`, `lan` or `*`.
- **Off the async threads**: `ureq` blocks, so the request runs in
  `aio::rt().spawn_blocking(..)`. An async HTTP client works too, as long
  as its future is the awaited one.
- **Retries**: `cx.after` and `cx.backoff` with the same key replace the
  one set before, so there is only ever one refresh waiting. Schedules
  asked for by a call are saved: the chain survives a restart.
  `aio::effect::retry(n, timeout, || io())` retries inside one await
  instead, and the log keeps only the last answer.
- **What the log keeps**: the answer's type (`Result<Rates, String>`) is
  how it is saved; it must be `Serialize`, so errors travel as text.
- **Timeouts**: the agent's own (10 s here), and each await of an async
  method at most `aio::effect::CALL_CEILING` (30 s; `#[long]` lifts it).
- **Secrets**: an API key comes from the environment or a file in
  `data/` and is added in `get_rates`; never a method argument (the log
  keeps those) unless the method is `#[redact]`.

## 2. Webhooks

A provider calls the app back: a payment went through. The route is
`public()` (the provider holds no key of the app's), so it proves the
caller itself: an HMAC-SHA256 of the body with a secret both sides know,
compared in constant time. Only then does it call the cell.

```rust
# use aio::prelude::*;
# use std::collections::BTreeMap;
# #[data]
# #[derive(Clone, Debug, Default, PartialEq)]
# pub struct Payment { pub id: String, pub cents: u64 }
# #[data]
# #[derive(Clone, Debug, Default, PartialEq)]
# pub struct Upload { pub blob: String, pub name: String, pub size: u64 }
# #[cell(persist, shared, id = "hub")]
# pub struct Hub {
#     pub payments: Vec<Payment>,
#     pub uploads: Vec<Upload>,
# }
use aio::blobs::Blobs;
use aio::http::{Http, Reply, Route};
use aio::sha256::{Sha256, hex};
use aio::Source;
use std::sync::OnceLock;

/// The hub, set once at boot (and by tests): routes reach it here.
pub static HUB: OnceLock<Source<Hub>> = OnceLock::new();

fn hub() -> &'static Source<Hub> {
    HUB.get().expect("HUB is set at boot")
}

#[methods]
impl Hub {
    /// A paid order, as the provider said. Not `#[client]`: only the
    /// webhook (the app's own code) calls it. Providers send an event more
    /// than once: the second is a no-op.
    pub fn paid(&mut self, id: String, cents: u64) -> Result {
        ensure!(!id.is_empty(), "a payment needs an id");
        if !self.payments.iter().any(|p| p.id == id) {
            self.payments.push(Payment { id, cents });
        }
        Ok(())
    }

    /// An upload's metadata; its bytes are a blob.
    pub fn uploaded(&mut self, blob: String, name: String, size: u64) {
        self.uploads.push(Upload { blob, name, size });
    }
}

pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut k = [0u8; 64];
    match key.len() {
        0..=64 => k[..key.len()].copy_from_slice(key),
        _ => k[..32].copy_from_slice(&Sha256::digest(key)),
    }
    let mut inner = Sha256::new();
    inner.update(k.map(|b| b ^ 0x36));
    inner.update(msg);
    let mut outer = Sha256::new();
    outer.update(k.map(|b| b ^ 0x5c));
    outer.update(inner.finalize());
    outer.finalize()
}

/// `X-Signature: sha256=<hex>` over the body, in constant time.
pub fn signed(secret: &[u8], body: &[u8], header: Option<&str>) -> bool {
    let Some(theirs) = header.and_then(|h| h.strip_prefix("sha256=")) else { return false };
    let ours = hex(&hmac_sha256(secret, body));
    ours.len() == theirs.len() && ours.bytes().zip(theirs.bytes()).fold(0, |d, (a, b)| d | (a ^ b)) == 0
}

#[aio::data]
#[derive(Clone, Debug, Default, PartialEq)]
struct PaidEvent {
    id: String,
    cents: u64,
}

/// The app's HTTP: the webhook, uploads, and the blobs served back.
pub fn http(secret: Vec<u8>, blobs: Blobs) -> Http {
    let webhook = Route::post("/hooks/payment", move |req| {
        let proven = signed(&secret, req.body(), req.header("x-signature"));
        let event = req.json::<PaidEvent>();
        async move {
            if !proven {
                return Reply::text("bad signature").status(401);
            }
            let Ok(e) = event else { return Reply::text("not a payment event").status(400) };
            match hub().paid(e.id, e.cents) {
                Ok(()) => Reply::text("ok"),
                Err(r) => Reply::text(r.why).status(422),
            }
        }
    })
    .public()
    .max_body(64 * 1024);

    let store = blobs.clone();
    let upload = Route::post("/upload", move |req| {
        let name = req.query("name").unwrap_or("upload").to_string();
        let (body, store) = (req.body_reader(), store.clone());
        async move {
            let Some(body) = body else { return Reply::text("no body").status(400) };
            let at = name.clone();
            let put = aio::rt().spawn_blocking(move || store.put(body, Some(&at))).await;
            let Ok(Ok(b)) = put else { return Reply::text("the upload failed").status(500) };
            match hub().uploaded(b.id.clone(), name, b.size) {
                Ok(()) => Reply::json(&b),
                Err(r) => Reply::text(r.why).status(422),
            }
        }
    })
    .streamed()
    .max_body(10 << 20);

    Http::default().route(webhook).route(upload).blobs(blobs)
}

# fn main() {
// RFC 4231, test case 2: the HMAC is the standard one.
assert_eq!(
    hex(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
    "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
);
use aio::testing::TestServer;
let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
rt.block_on(async {
    let dir = tempfile::tempdir().unwrap();
    let h = Handle::<Hub>::memory();
    _ = HUB.set(Source::Local(h.clone()));
    let server = TestServer::start(h.clone(), http(b"whsec-test".to_vec(), Blobs::at(dir.path()))).await.unwrap();

    // The provider calls: signed, then the same event again, then a forgery.
    let body = r#"{"id":"pay_1","cents":1250}"#;
    let sig = format!("sha256={}", hex(&hmac_sha256(b"whsec-test", body.as_bytes())));
    let s = &server;
    let call = |sig: String| async move {
        s.fetch_with("POST", "/hooks/payment", &[("X-Signature", sig.as_str()), ("Content-Type", "application/json")], body).await.unwrap().status
    };
    assert_eq!(call(sig.clone()).await, 200);
    assert_eq!(call(sig).await, 200, "a repeat is fine");
    assert_eq!(call("sha256=00".into()).await, 401);
    assert_eq!(h.state().payments, [Payment { id: "pay_1".into(), cents: 1250 }]);

    // An upload: its bytes to the blob store, its metadata to the cell.
    let got = server.fetch("POST", "/upload?name=notes.txt", "hello").await.unwrap();
    assert_eq!(got.status, 200, "{}", got.body);
    let up = h.state().uploads[0].clone();
    assert_eq!((up.name.as_str(), up.size), ("notes.txt", 5));
    let back = server.fetch("GET", &aio::blobs::url(&up.blob), "").await.unwrap();
    assert_eq!(back.body, "hello", "served back, behind the key");
    let stranger = server.fetch_with("GET", "/upload?name=x", &[], "").await.unwrap();
    assert_eq!(stranger.status, 401, "the upload route needs the key");
});
# }
```

- **Prove, then act.** The route reads the body and the header before
  anything else and answers 401 to a bad signature: an unproven request
  changes nothing. `max_body` keeps a flood of bytes out (413).
- **Idempotent on purpose.** Providers retry until they get a 2xx, so
  `paid` takes the event id and does nothing the second time. Answer
  200 for a duplicate, or the provider retries for days.
- **The cell is the record.** The webhook becomes a call: logged, synced
  to every screen, visible in `ax events hub`. It is not `#[client]`, so
  no client can claim a payment.
- **Uploads stream.** `Route::streamed()` hands the body over as it
  arrives (`req.body_reader()`, read off the async threads) straight into
  `Blobs::put`, which hashes and writes it as it comes: a 10 MB upload
  costs a few chunks of memory. The cell keeps the id, the name and the
  size; the bytes never enter the state, the log or the sync wire.
- **Names are metadata.** The upload's `name` is only stored and shown;
  the blob's path is its SHA-256, so no name can reach outside the store.
- **Serving them back**: `Http::blobs(blobs)` serves
  `/__aio/blobs/<id>` with Range and long caching, behind the app's key
  (`aio::blobs::url(id)` is the path). In the app: `Blobs::of(&dirs)`.
- The webhook secret comes from the environment
  (`std::env::var("PAYMENTS_WEBHOOK_SECRET")`), never `aio.toml`.

In the launcher, the app's HTTP goes in with the cell, and the source is
kept for the routes:
`_ = app::HUB.set(aio::Source::for_app_with(&cfg, &dirs, app::http(secret, aio::blobs::Blobs::of(&dirs)))?);`

## 3. Uploads

Covered with the webhook above: a streamed route, the blob store, the
metadata in the cell. A client uploads with a plain
`POST /upload?name=<file>` and its key (or session); a browser page uses
`fetch` with the same headers. What comes after the upload (a thumbnail,
a scan) is an effect the `uploaded` call asks for, its result another
call.

## 4. Sign in with an identity provider

With accounts on (`[accounts] on = true`), an app can let people sign in
with an OpenID Connect provider. The provider's host goes in
`[capabilities] network`; the client secret comes from the environment.

```rust
use aio::auth::oidc::{Oidc, account_id};
use aio::http::Http;

/// Sign-in with the company's provider: new accounts allowed, staff by
/// their verified email's domain, everyone else a viewer.
pub fn sign_in(http: Http, client_secret: String) -> Http {
    let provider = Oidc::new("https://login.example.org", "hub")
        .secret(client_secret)
        .signup(true)
        .role(|claims| {
            let staff = claims["email"].as_str().is_some_and(|e| e.ends_with("@example.org"));
            Ok(if staff { "staff" } else { "viewer" }.to_string())
        });
    http.oidc(provider)
}

# fn main() {
let _http = sign_in(Http::default(), "from-the-environment".into());
assert_eq!(account_id("https://login.example.org", "248289761001"), "oidc:login.example.org:248289761001");
assert_eq!(aio::auth::oidc::return_path(Some("//evil.example.net")), "/", "no open redirect");
# }
```

- A page links to `/__aio/auth/oidc/start?redirect=/orders`; the app
  answers with a redirect to the provider (authorization code flow with
  PKCE), and `/__aio/auth/oidc/callback` checks the answer, makes the
  session and sends the browser back to `/orders`. A `redirect` that
  isn't a path on this site becomes `/`.
- The provider is found from `<issuer>/.well-known/openid-configuration`;
  ID tokens are checked (RS256 or ES256, `iss`, `aud`, `azp`, expiry,
  nonce) against its keys, which are kept an hour and read again when a
  token names a new one.
- Accounts are `oidc:<issuer>:<sub>`, apart from local ones: no local
  account is taken over by a provider's identity. `role(f)` sets a new
  account's role from its claims (an `Err` refuses the sign-in); an
  account keeps its role after that (`ax auth hub role <id> <role>`).
- `.redirect_uri(uri)` when the provider must see the exact registered
  URL (behind a proxy).
- Without accounts, the app can resolve its own tokens:
  `Http::users(aio::auth::Users::resolve(|token| ..))` returns the user
  and role for an API key ([accounts](../auth/README.md#accounts)).

## 5. Monitoring

A served app answers two monitoring paths, behind its key (or a
signed-in session):

| Path | Answers |
|---|---|
| `GET /__aio/health` | JSON: healthy or degraded, each part's state, the served cell |
| `GET /__aio/metrics` | Prometheus text: uptime, memory, connected clients, calls, the cell's failures, degraded parts |

```sh
ax health hub      # exit 0 when healthy: for a supervisor or a script
ax metrics hub     # the same numbers, from this machine
ax errors hub      # the last failures, each with its reason
```

Point the scraper at `/__aio/metrics` with the app's key as a bearer
token. `[budgets]` turns a breach (a state too big, too many patches a
second) into a degraded health, so an alert fires before people notice
([persistence](../persistence/sql.md#blobs-files-and-big-data)).

## What maps to what

| aio | aio-rs |
|---|---|
| `routes: { "/hooks/x": (req) => .. }` | `Http::route(Route::post("/hooks/x", ..))` |
| `route(ctx => ..)` with `ctx.params`, `ctx.json()` | `aio::http::Req` (`param`, `query`, `json`, `header`, `cookie`), `Reply` |
| a webhook's own check | `Route::public()` and the app's check (here an HMAC) |
| `Deno.writeFile("uploads/" + id, ..)` | `Route::streamed()` into `Blobs::put`; served by `Http::blobs` |
| `schedule.backoff("rates:refresh", attempt, ..)` | `cx.backoff("refresh", attempt, aio::Backoff { .. }, "refresh", ())` |
| `call(() => api.rates())` | `aio::effect::retry`, or the awaited future of an async method |
| `resolveUser: async (token) => ..` | `Http::users(Users::resolve(..))`, or `[accounts]` with `Http::oidc` |
| `GET /__aio/metrics` | the same path and format |

## Limits

- No HMAC or webhook helper ships with aio-rs: the check above is the
  app's own (SHA-256 is `aio::sha256`).
- The HTTP client aio-rs holds to the network list is `ureq`, which
  blocks: run it in `spawn_blocking`. An async client of the app's own
  is not checked against `[capabilities] network`: call
  `aio::capabilities::check("what", url)` before each request.
- OIDC is the authorization code flow for browsers; there is no device
  flow for headless clients.
- A route reaches a cell through a door the app keeps (`HUB` here): the
  router hands a handler the request, not the app's cells.

## Common mistakes

- An HTTP call in a plain method, or a `cx.now()` instead of the
  answer's own time: the log can't replay it. Await it in an async method
  or an effect.
- A host missing from `[capabilities] network`: refused before it is
  sent; the error names the line to add.
- A webhook that trusts its body before checking the signature, or that
  compares signatures with `==` (a timing leak).
- A webhook that answers an error for an event it already has: the
  provider keeps sending it.
- Upload bytes in a cell (`Vec<u8>`): every call copies them and every
  client is sent them. Blobs, and the id in the cell.
- A client secret or API key in `aio.toml`: it is built into the binary.

## API

`cargo doc -p aio-rs --no-deps --all-features --open`:
`aio::capabilities` (`agent`, `held`, `check`), `aio::http` (`Http`,
`Route`, `Req`, `Reply`), `aio::blobs`, `aio::auth::oidc` (`Oidc`,
`account_id`, `return_path`), `aio::auth::Users`, `aio::sha256`,
`aio::Backoff`, `aio::effect::retry`, `aio::testing::TestServer`.
