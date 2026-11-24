//! The relay: the calls an app's installs make to a partner's API, sent on
//! with the partner's key, which never leaves this server. Only the calls
//! in `CALLS` pass, their path parts and query keys checked; an install has
//! `per_install` calls a minute, its address `per_address`. It keeps
//! nothing and writes nothing of a call: no key, no body, no query.
use aio::http::{Http, Reply, Req, Route};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The partner's API (its host is aio.toml's `[capabilities] network`).
pub const API: &str = "https://api.example.org";
/// The header the partner reads its key from.
pub const KEY_HEADER: &str = "x-api-key";

/// A call installs may make: its method, its path (`:name`, a part the
/// install fills in) and the query keys passed on (any other is dropped).
pub struct Call {
    pub method: &'static str,
    pub path: &'static str,
    pub query: &'static [&'static str],
}

/// The calls relayed. Any other path is 404 and reaches no one.
pub const CALLS: &[Call] = &[
    Call { method: "GET", path: "/v1/quote", query: &["from", "to", "amount"] },
    Call { method: "POST", path: "/v1/swap", query: &[] },
    Call { method: "GET", path: "/v1/swap/:id", query: &[] },
];

/// A server serves a cell; the relay's holds nothing (`[data] persist = false`).
#[aio::cell(id = "relay")]
pub struct Relay {}

#[aio::methods]
impl Relay {}

/// Where calls go, with which key, how many.
pub struct Partner {
    /// The API's base: `API`, but in tests.
    pub api: String,
    /// The partner's key: from the environment (`RELAY_KEY`), never
    /// aio.toml, the code or a log.
    pub key: String,
    /// Calls a minute for one install (its `x-install-id`; without one, its address).
    pub per_install: u32,
    /// Calls a minute for one address, all its installs together.
    pub per_address: u32,
    /// Minutes since the epoch: the limits' window (a test fixes it).
    pub minute: fn() -> u64,
}

impl Partner {
    /// The partner at `API` with `key`: 30 calls a minute an install, 300 an address.
    pub fn new(key: String) -> Self {
        let minute =
            || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() / 60);
        Self { api: API.into(), key, per_install: 30, per_address: 300, minute }
    }
}

/// The relay's routes: one per call in `CALLS`, open to installs (an
/// install holds no secret), bodies of 64 KB at most.
pub fn http(p: Partner) -> Http {
    let (p, counts) = (Arc::new(p), Arc::new(Mutex::new(Counts::default())));
    // No redirect followed: the key goes to the partner's host and no other.
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .http_status_as_error(false)
        .max_redirects(0)
        .build();
    let agent = aio::capabilities::agent("relay", config);
    CALLS.iter().fold(Http::default(), |h, call| {
        let (p, counts, agent) = (p.clone(), counts.clone(), agent.clone());
        let route = Route::new(&[call.method], call.path, move |req| {
            relay(p.clone(), counts.clone(), agent.clone(), call, req)
        });
        h.route(route.public().max_body(64 << 10))
    })
}

/// Most keys counted in a minute: past it, a new one waits (memory stays bounded).
const MAX_KEYS: usize = 100_000;

/// The calls of this minute, by install and by address.
#[derive(Default)]
struct Counts {
    minute: u64,
    seen: HashMap<String, u32>,
}

impl Counts {
    /// Count a call against each (key, limit) at `minute`, if none is spent.
    fn take(&mut self, minute: u64, keys: &[(String, u32)]) -> bool {
        if minute != self.minute {
            (self.minute, self.seen) = (minute, HashMap::new());
        }
        let full = self.seen.len() + keys.len() > MAX_KEYS;
        let ok = keys.iter().all(|(k, limit)| self.seen.get(k).map_or(!full, |n| n < limit));
        if ok {
            keys.iter().for_each(|(k, _)| *self.seen.entry(k.clone()).or_default() += 1);
        }
        ok
    }
}

/// The install's id, when it sent a sound one: 8 to 64 letters, digits, `-` or `_`.
fn install(req: &Req) -> Option<&str> {
    let sound = |s: &&str| {
        (8..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    };
    req.header("x-install-id").filter(sound)
}

/// `s` percent-encoded but for unreserved characters: one path part or
/// query value, whatever it holds.
fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// The partner's URL for `call` asked as `req`: its path, each part the
/// install filled in encoded, then the query keys it lists. `None`: a part
/// that is `.` or `..`, which would leave its place in the path.
fn target(api: &str, call: &Call, req: &Req) -> Option<String> {
    let mut url = api.trim_end_matches('/').to_string();
    for part in call.path.split('/').skip(1) {
        url.push('/');
        match part.strip_prefix(':') {
            Some(name) => match req.param(name)? {
                "." | ".." => return None,
                v => url += &encode(v),
            },
            None => url += part,
        }
    }
    let query: Vec<String> =
        call.query.iter().filter_map(|k| req.query(k).map(|v| format!("{}={}", encode(k), encode(v)))).collect();
    if !query.is_empty() {
        url = format!("{url}?{}", query.join("&"));
    }
    Some(url)
}

/// `req` sent on as `call`, with the key and its content type alone; the
/// partner's answer back (its status, type and at most 1 MB of body).
async fn relay(
    p: Arc<Partner>,
    counts: Arc<Mutex<Counts>>,
    agent: ureq::Agent,
    call: &'static Call,
    req: Req,
) -> Reply {
    let address = format!("a {}", req.ip());
    let keys = match install(&req) {
        Some(id) => vec![(format!("i {id}"), p.per_install), (address, p.per_address)],
        None => vec![(address, p.per_install)],
    };
    if !counts.lock().unwrap_or_else(|e| e.into_inner()).take((p.minute)(), &keys) {
        return Reply::text("too many calls: try again in a minute").status(429).header("retry-after", "60");
    }
    let Some(url) = target(&p.api, call, &req) else {
        return Reply::text("a part of the path may not be . or ..").status(400);
    };
    let (key, kind, body) = (p.key.clone(), req.header("content-type").map(String::from), req.body().to_vec());
    let sent = aio::rt().spawn_blocking(move || send(&agent, call.method, &url, &key, kind.as_deref(), body)).await;
    match sent {
        Ok(Ok((status, kind, body))) => {
            let mut r =
                Reply::text("").status(status).header("content-type", &kind).header("cache-control", "no-store");
            r.body = body;
            r
        }
        // The call's route only: never its path parts, query, body or the key.
        _ => {
            aio::warn!(cat: "relay", "{} {}: the partner API did not answer", call.method, call.path);
            Reply::text("the partner API did not answer").status(502)
        }
    }
}

/// One call to the partner: its status, content type and body.
fn send(
    agent: &ureq::Agent,
    method: &str,
    url: &str,
    key: &str,
    kind: Option<&str>,
    body: Vec<u8>,
) -> Result<(u16, String, Vec<u8>), ureq::Error> {
    let mut rq = ureq::http::Request::builder().method(method).uri(url).header(KEY_HEADER, key);
    if let Some(kind) = kind {
        rq = rq.header("content-type", kind);
    }
    let mut res = match method {
        "GET" | "DELETE" => agent.run(rq.body(())?)?,
        _ => agent.run(rq.body(body)?)?,
    };
    let kind = res.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("application/octet-stream");
    let kind = kind.to_string();
    Ok((res.status().as_u16(), kind, res.body_mut().with_config().limit(1 << 20).read_to_vec()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aio::testing::TestServer;

    /// The partner, played on loopback: each call it got, as a line.
    async fn partner() -> (TestServer<Relay>, Arc<Mutex<Vec<String>>>) {
        let seen = Arc::new(Mutex::new(vec![]));
        let got = seen.clone();
        let any = Route::new(&[], "/*", move |req| {
            let (q, key) = (|k| req.query(k).unwrap_or("-"), req.header(KEY_HEADER).unwrap_or("-"));
            let body = String::from_utf8_lossy(req.body());
            let line = format!(
                "{} /{} {key} from={} junk={} {body}",
                req.method(),
                req.param("*").unwrap_or(""),
                q("from"),
                q("junk")
            );
            got.lock().unwrap().push(line);
            async { Reply::text("ANSWER-MARK") }
        });
        (TestServer::start(aio::Handle::memory(), Http::default().route(any.public())).await.unwrap(), seen)
    }

    /// The relay in front of `api`, as `limits` (per install, per address).
    async fn relay_to(api: &str, limits: (u32, u32)) -> TestServer<Relay> {
        let p = Partner {
            api: api.into(),
            per_install: limits.0,
            per_address: limits.1,
            minute: || 7,
            ..Partner::new("sk-SECRET".into())
        };
        TestServer::start(aio::Handle::memory(), http(p)).await.unwrap()
    }

    #[test]
    fn only_the_listed_calls_reach_the_partner_and_with_its_key_alone() {
        aio::rt().block_on(async {
            let (api, seen) = partner().await;
            let relay = relay_to(&format!("http://{}", api.addr), (100, 100)).await;
            let call = |m: &'static str, path: &'static str| {
                let r = &relay;
                async move { r.fetch_with(m, path, &[(KEY_HEADER, "mine"), ("cookie", "s=1")], "").await.unwrap() }
            };
            let ok = call("GET", "/v1/quote?from=SOL&to=USDC&junk=1").await;
            assert_eq!(
                (ok.status, ok.body.as_str(), ok.header("cache-control")),
                (200, "ANSWER-MARK", Some("no-store"))
            );
            assert_eq!(call("GET", "/v1/swap/a%2Fb").await.status, 200, "a part holding / stays one part");
            // Never sent on: an unlisted path (behind the app's key, which
            // installs don't hold) or method, a part that climbs out.
            assert_eq!(call("GET", "/v1/admin").await.status, 401);
            assert_eq!(call("GET", "/v1").await.status, 401);
            assert_eq!(call("DELETE", "/v1/quote").await.status, 401);
            assert_eq!(call("GET", "/v1/swap/..").await.status, 400);
            assert_eq!(call("GET", "/v1/swap/%2E%2E").await.status, 400);
            let r = relay
                .fetch_with("POST", "/v1/swap", &[("content-type", "application/json")], "{\"n\":1}")
                .await
                .unwrap();
            assert_eq!(r.status, 200);
            assert_eq!(
                *seen.lock().unwrap(),
                [
                    "GET /v1/quote sk-SECRET from=SOL junk=- ",
                    "GET /v1/swap/a%2Fb sk-SECRET from=- junk=- ",
                    "POST /v1/swap sk-SECRET from=- junk=- {\"n\":1}",
                ],
                "the relay's key, never the caller's; listed query keys alone"
            );
        });
    }

    #[test]
    fn an_install_has_its_calls_a_minute_and_an_address_those_of_all_its_installs() {
        aio::rt().block_on(async {
            let (api, seen) = partner().await;
            let relay = relay_to(&format!("http://{}", api.addr), (2, 3)).await;
            let call = |id: &'static str| {
                let r = &relay;
                async move { r.fetch_with("GET", "/v1/quote", &[("x-install-id", id)], "").await.unwrap() }
            };
            assert_eq!([call("install-a").await.status, call("install-a").await.status], [200, 200]);
            let spent = call("install-a").await;
            assert_eq!((spent.status, spent.header("retry-after")), (429, Some("60")));
            assert_eq!(call("install-b").await.status, 200);
            assert_eq!(call("install-c").await.status, 429, "the address had its 3");
            assert_eq!(call("x").await.status, 429, "no sound id: the address is the install");
            assert_eq!(seen.lock().unwrap().len(), 3, "a refused call reaches no one");
        });
        let mut c = Counts::default();
        let one = [("i a".to_string(), 1)];
        assert!(c.take(1, &one) && !c.take(1, &one) && c.take(2, &one), "a new minute, new calls");
        c.seen.extend((0..MAX_KEYS).map(|i| (format!("i {i}"), 1)));
        assert!(!c.take(2, &[("i new".to_string(), 1)]), "full: a new key waits");
    }

    #[test]
    fn the_relay_writes_nothing_of_a_call_neither_the_key_nor_a_body() {
        let lines = Arc::new(Mutex::new(Vec::<String>::new()));
        let to = lines.clone();
        let was = aio::log::set_sink(Some(Arc::new(move |_, l: &str| to.lock().unwrap().push(l.to_string()))));
        aio::rt().block_on(async {
            let (api, _) = partner().await;
            let relay = relay_to(&format!("http://{}", api.addr), (100, 100)).await;
            let json = [("content-type", "application/json")];
            let r = relay.fetch_with("POST", "/v1/swap?from=WALLET-MARK", &json, "\"BODY-MARK\"").await.unwrap();
            assert_eq!(r.body, "ANSWER-MARK");
            // The partner down: said, by the call's route alone.
            let down = relay_to("http://127.0.0.1:9", (100, 100)).await;
            let r = down.fetch_with("GET", "/v1/swap/WALLET-MARK?from=WALLET-MARK", &[], "").await.unwrap();
            assert_eq!(r.status, 502);
        });
        aio::log::set_sink(was);
        let all = lines.lock().unwrap().join("\n");
        assert!(all.contains("GET /v1/swap/:id: the partner API did not answer"), "{all}");
        for secret in ["sk-SECRET", "BODY-MARK", "ANSWER-MARK", "WALLET-MARK"] {
            assert!(!all.contains(secret), "{secret} written:\n{all}");
        }
    }
}
