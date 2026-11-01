# A dashboard

A live monitoring dashboard as a whole app: it samples this machine's
load and memory every five seconds, keeps a history in SQL, draws it as
cards and charts, and serves it to every screen that connects, each
person seeing what their role allows. Open it in two places and both
move together.

You will use:

- a shared cell, served by the app itself (`[app] mode = "client-server"`);
- a declared schedule with an async method that does the sampling;
- a `#[table]` field: the history as SQL rows (`ax sql`);
- users known by token, and `visible` to send each viewer their part;
- an HTTP route beside the sync socket (a JSON summary for scripts);
- views with a chart drawn in SVG, and tests at every layer.

Every Rust block on this page compiles, and the logic runs, as a test of
aio-rs. Prerequisites: [the tutorial](../basics/tutorial.md) and
[sync](../state/sync.md).

Start from an app (`ax create dash`) and put the code below in
`src/lib.rs`. It needs aio-rs's features `kit` and `net`: in the app's
`Cargo.toml`, `features = ["kit", "net"]`.

## 1. The metrics cell

```rust
use aio::prelude::*;

#[data]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sample {
    pub at: i64,
    pub load: f64,
    pub mem_mb: u64,
}

/// The last 100 samples are kept; older rows leave the table with them.
pub const KEEP: usize = 100;

#[cell(persist, shared, visible = by_role, id = "metrics")]
pub struct Metrics {
    pub load: f64,
    pub mem_mb: u64,
    pub requests: u64,
    #[table("samples", columns = "at INTEGER PRIMARY KEY, load REAL NOT NULL, mem_mb INTEGER NOT NULL")]
    pub history: Vec<Sample>,
}

/// Admins see everything; anyone else the current values without the history.
fn by_role(m: &Metrics, who: &Caller) -> Metrics {
    match who.role.as_deref() {
        Some("admin") => m.clone(),
        _ => Metrics { history: vec![], ..m.clone() },
    }
}

/// This machine's 1-minute load average and memory in use, in MB. Linux
/// reads them from /proc (memory files: no disk, no wait); elsewhere zeros.
async fn sample() -> (f64, u64) {
    let read = |p: &str| std::fs::read_to_string(p).unwrap_or_default();
    let load = read("/proc/loadavg").split_whitespace().next().and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let info = read("/proc/meminfo");
    let kb = |key: &str| -> u64 {
        let line = info.lines().find(|l| l.starts_with(key)).unwrap_or_default();
        line.split_whitespace().nth(1).and_then(|v| v.parse().ok()).unwrap_or(0)
    };
    (load, kb("MemTotal:").saturating_sub(kb("MemAvailable:")) / 1024)
}

#[methods]
impl Metrics {
    /// Every 5 s: one sample. Its I/O is the await; the result is logged.
    #[every("5s", skip_if_running)]
    pub async fn poll(&mut self, cx: &Cx) -> Result {
        let got: (f64, u64) = sample().await;
        (self.load, self.mem_mb) = got;
        self.history.push(Sample { at: cx.now(), load: got.0, mem_mb: got.1 });
        let over = self.history.len().saturating_sub(KEEP);
        self.history.drain(..over);
        Ok(())
    }

    /// Anyone connected may count a request.
    #[client]
    pub fn record_request(&mut self) {
        self.requests += 1;
    }

    /// The highest load in the history, for the chart's scale.
    #[derived(reads = [history])]
    pub fn peak(&self) -> f64 {
        self.history.iter().map(|s| s.load).fold(1.0, f64::max)
    }
}

# fn main() {
use aio::testing::TestCells;
let mut t = TestCells::new(); // the cell live, on a virtual clock
let m = t.cell::<Metrics>();
t.advance(5_000); // the schedule fires; its effect settles
assert_eq!(m.state().history.len(), 1);
for _ in 0..120 {
    t.advance(5_000);
}
assert_eq!(m.state().history.len(), KEEP, "the oldest go");
assert!(m.state().history.windows(2).all(|w| w[0].at < w[1].at), "in order, one per tick");

let viewer = by_role(&m.state(), &Caller { by: By::User("bo".into()), role: None });
assert!(viewer.history.is_empty() && viewer.mem_mb == m.state().mem_mb);
# }
```

- **The schedule** `#[every("5s", skip_if_running)]` is armed whenever
  the cell is live, keyed `@poll`; `skip_if_running` skips a tick while
  the last sample is still being taken. A run missed while the app was
  off fires once at start, not once per missed tick.
- **The async method** is split at its await: the part before it runs as
  the call, `sample()` runs as an effect, and the part after it is a
  second call with the result. Both are events, so a replay rebuilds the
  history without reading `/proc` again
  ([effects](../state/effects.md#async-methods)).
- **The table**: `history` is stored as the rows of SQL table `samples`
  in the app's `state.db`, written on each commit. Trimming the field
  deletes the rows; the rest of the cell is saved as usual.
- **`visible`** runs for every viewer on every change. A viewer's copy
  has an empty history, so the samples never cost them bytes.
- `#[derived(reads = [history])]` keeps `peak()` while `history` stays
  the same: a counted request doesn't compute it again.

## 2. Who connects: users and a route

A served app is closed by default. Without accounts, the app can know
its users by token: each token names a user and a role, and the app's
shared key then opens nothing. The tokens come from the environment,
never from `aio.toml` (which is built into the binary).

```rust
# use aio::prelude::*;
# #[data]
# #[derive(Clone, Debug, Default, PartialEq)]
# pub struct Sample { pub at: i64, pub load: f64, pub mem_mb: u64 }
# #[cell(persist, shared, visible = by_role, id = "metrics")]
# pub struct Metrics {
#     pub load: f64,
#     pub mem_mb: u64,
#     pub requests: u64,
#     #[table("samples", columns = "at INTEGER PRIMARY KEY, load REAL NOT NULL, mem_mb INTEGER NOT NULL")]
#     pub history: Vec<Sample>,
# }
# fn by_role(m: &Metrics, who: &Caller) -> Metrics {
#     match who.role.as_deref() { Some("admin") => m.clone(), _ => Metrics { history: vec![], ..m.clone() } }
# }
# #[methods]
# impl Metrics {
#     pub fn push(&mut self, at: i64, load: f64) {
#         self.load = load;
#         self.history.push(Sample { at, load, mem_mb: 512 });
#     }
#     #[client]
#     pub fn record_request(&mut self) { self.requests += 1; }
# }
use aio::auth::Users;
use aio::http::{Http, Reply, Route};
use std::sync::Arc;

/// The users, from `DASH_ADMIN_TOKEN` and `DASH_VIEWER_TOKEN` (each at
/// least 16 characters: a short one is said at boot).
pub fn users(env: impl Fn(&str) -> Option<String>) -> Users {
    let mut known = vec![];
    for (var, user, role) in [("DASH_ADMIN_TOKEN", "admin", "admin"), ("DASH_VIEWER_TOKEN", "viewer", "viewer")] {
        if let Some(token) = env(var) {
            known.push((token, (user.to_string(), role.to_string())));
        }
    }
    Users::map(known)
}

/// The app's HTTP: its users, and `GET /api/summary` for scripts.
/// `state` reads the served cell (in the app: `app::SOURCE`, set at boot).
pub fn http(state: impl Fn() -> Arc<Metrics> + Send + Sync + 'static, users: Users) -> Http {
    Http::default().users(users).route(Route::get("/api/summary", move |req| {
        let m = state();
        let body = serde_json::json!({
            "load": m.load, "mem_mb": m.mem_mb, "requests": m.requests,
            "samples": m.history.len(), "asked_by": req.user(),
        });
        async move { Reply::json(&body) }
    }))
}

# fn main() {
use aio::sync::Remote;
use aio::testing::TestServer;
const ADMIN: &str = "admin-token-0123456789abcdef";
const VIEWER: &str = "viewer-token-0123456789abcdef";
let env = |k: &str| match k {
    "DASH_ADMIN_TOKEN" => Some(ADMIN.to_string()),
    "DASH_VIEWER_TOKEN" => Some(VIEWER.to_string()),
    _ => None,
};
let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
rt.block_on(async {
    let h = Handle::<Metrics>::memory();
    h.push(1, 0.5).unwrap();
    h.push(2, 0.75).unwrap();
    let served = h.clone();
    let server = TestServer::start(h.clone(), http(move || served.state(), users(env))).await.unwrap();

    // Two screens: each token is a user with a role.
    let admin = Remote::<Metrics>::connect(&server.addr, ADMIN, "admin").await.unwrap();
    let viewer = Remote::<Metrics>::connect(&server.addr, VIEWER, "viewer").await.unwrap();
    assert!(Remote::<Metrics>::connect(&server.addr, VIEWER, "admin").await.is_err(), "no other name");
    assert_eq!(admin.state().history.len(), 2);
    assert!(viewer.state().history.is_empty(), "visible: no history for a viewer");
    assert_eq!(viewer.state().load, 0.75, "but the current values");

    viewer.record_request().unwrap(); // shown at once, confirmed by the server
    for _ in 0..300 {
        if admin.state().requests == 1 { break }
        aio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(admin.state().requests, 1, "every screen moves together");

    // The route: a token is needed, and the app's shared key opens nothing now.
    let bearer = format!("Bearer {VIEWER}");
    let got = server.fetch_with("GET", "/api/summary", &[("Authorization", bearer.as_str())], "").await.unwrap();
    let body: serde_json::Value = serde_json::from_str(&got.body).unwrap();
    assert_eq!((got.status, body["asked_by"].as_str(), body["samples"].as_u64()), (200, Some("viewer"), Some(2)));
    assert_eq!(server.fetch("GET", "/api/summary", "").await.unwrap().status, 401);
    assert!(Remote::<Metrics>::connect(&server.addr, &server.token, "admin").await.is_err(), "nor the socket");
});
# }
```

- `Users::map` compares tokens in constant time. `Users::resolve(f)`
  takes a function instead (a lookup in your own table). With real
  people, turn on `[accounts]`: password logins, roles and sessions
  ([accounts](../auth/README.md)).
- The user and role come from the token, never from what a client
  says: a link that names another user than its token's is refused.
- Routes sit on the same server as `/ws`, behind the same check;
  `Route::public()` opens one route (a health probe a load balancer
  calls, say). `ax health dash` and `/__aio/health` exist already.

## 3. The views

Cards for the current values, a chart of the history, and a button.
The kit has no chart component: a chart is a few lines of SVG, and a
charting library can run inside a view as a `WebIsland`
([kit](../ui/kit.md#islands-and-other-sites)).

```rust,no_run
# #[aio::data]
# #[derive(Clone, Debug, Default, PartialEq)]
# pub struct Sample { pub at: i64, pub load: f64, pub mem_mb: u64 }
# #[aio::cell(persist, shared, id = "metrics")]
# pub struct Metrics {
#     pub load: f64,
#     pub mem_mb: u64,
#     pub requests: u64,
#     pub history: Vec<Sample>,
# }
# #[aio::methods]
# impl Metrics {
#     #[client]
#     pub fn record_request(&mut self) { self.requests += 1; }
#     #[derived(reads = [history])]
#     pub fn peak(&self) -> f64 { self.history.iter().map(|s| s.load).fold(1.0, f64::max) }
# }
use aio::act;
use aio::kit::ConnectionStatus;
use aio::ui::prelude::*;

aio::client!(Metrics, metrics);

#[component]
pub fn App() -> View {
    use_live(); // once, in the root: effects, schedules and other screens re-render it
    let theme = aio::kit::theme_css(aio::kit::theme::hue("dash"));
    rsx! {
        style { {aio::ui::page_css(&[theme.as_str()], false)} }
        ConnectionStatus { status: use_status().get() }
        main { Cards {} Chart {} }
    }
}

#[component]
fn Cards() -> View {
    let load = part(|m| &m.load).get();
    let mem = part(|m| &m.mem_mb).get();
    let requests = part(|m| &m.requests).get();
    rsx! {
        section { class: "cards",
            Card { label: "Load", value: format!("{load:.2}") }
            Card { label: "Memory", value: format!("{mem} MB") }
            Card { label: "Requests", value: requests.to_string() }
        }
        button { class: aio::ui::btn("primary"), onclick: move |_| _ = act!(record_request()), "Count a request" }
    }
}

#[component]
fn Card(label: &'static str, value: String) -> View {
    rsx! {
        div { class: "card", "data-t": "{label}",
            small { "{label}" }
            strong { "{value}" }
        }
    }
}

/// The load history as a line: x is the sample's place, y its share of the peak.
fn points(loads: &[f64], peak: f64, (w, h): (f64, f64)) -> String {
    let step = w / (loads.len().max(2) - 1) as f64;
    let at = |(i, v): (usize, &f64)| format!("{:.1},{:.1}", i as f64 * step, h - v / peak * h);
    loads.iter().enumerate().map(at).collect::<Vec<_>>().join(" ")
}

#[component]
fn Chart() -> View {
    let history = part(|m| &m.history);
    if history.with(Vec::is_empty) {
        return rsx! { p { "data-t": "no-history", "No history for this account." } };
    }
    let loads: Vec<f64> = history.with(|h| h.iter().map(|s| s.load).collect());
    let line = points(&loads, metrics().peak(), (300.0, 60.0));
    rsx! {
        svg { "data-t": "chart", view_box: "0 0 300 60", width: "100%", height: "120", role: "img",
            "aria-label": "Load over the last {loads.len()} samples",
            polyline { points: "{line}", fill: "none", stroke: "currentColor", stroke_width: "2" }
        }
    }
}

# fn main() {
let mut ui = aio::testing::TestUi::new(App);
ui.click("CountARequestButton");
assert_eq!(ui.text("Requests"), "Requests1");
# }
```

- `part(|m| &m.load)` re-renders a view only when that field changes:
  a new sample re-renders the cards and the chart, a counted request
  only the cards.
- `metrics()` reads the whole state; it is used here only for the
  derived `peak()`, inside the view that re-renders on `history`
  anyway.
- A viewer has no history, so `Chart` shows its note: the projection,
  not the view, decides what a person may see.
- `ConnectionStatus` shows "offline" or "syncing" when the screen's
  link to the server is down or behind; on the server's own window it
  shows nothing.

In a real app the views' test runs against the real server and wire:
`TestServer::ui_source(user, latency)` gives the page a copy over the
socket, so `TestUi` drives two screens on one server
([testing](../testing/README.md)).

## 4. Config and launch

```toml
# aio.toml
[app]
id = "dash"
name = "Dashboard"
mode = "client-server"     # its own window, and served to other screens

[server]
listen = "0.0.0.0:8100"    # every interface: other machines connect here
tls = true                 # a key never crosses the network in the clear

[budgets]
broadcast_rate = "20/s"    # what one client may be sent: a breach is logged and turns health degraded
payload = "500KB"

[capabilities]
network = []
```

In the template's launcher, the cell opens with the app's HTTP: in place
of `aio::Source::for_app(&cfg, &dirs)`, call
`aio::Source::for_app_with(&cfg, &dirs, http)` with
`let http = app::http(|| app::SOURCE.get().expect("set at boot").state(), app::users(|k| std::env::var(k).ok()));`.
The route then reads the state the window shows, from the source the
launcher sets right after.

```sh
DASH_ADMIN_TOKEN=… DASH_VIEWER_TOKEN=… ax dev dash
ax state dash metrics load          # the live value
ax sql dash "select count(*), max(load) from samples"
ax schedules dash                   # @poll, and when it fires next
ax clients dash                     # the screens linked now, with their users
ax cost dash                        # what each viewer was sent
ax build dash                       # the installable app
```

## 5. More screens

- **Another machine's window**: the same app, built with
  `[app] mode = "client"` and `[client] server` naming the dashboard's
  address (and that host in `[capabilities] network`). It shows the
  same views on a synced copy, offline-first.
- **Browser tabs**: `[app] ui = "browser"` serves the views to the
  system browser instead of a window ([targets](../build/targets.md#web)),
  or `ax build dash --web` makes the views a page of their own.
- **Phones**: `ax build dash --android`; pair the phone with
  `ax pin dash` and the code ([sync](../state/sync.md#pairing-and-discovery)).

## What maps to what

| aio's dashboard | Here |
|---|---|
| `schedules: [{ every: 5000, action: metrics.poll.action() }]` | `#[every("5s", skip_if_running)]` on `poll` |
| `Deno.loadavg()`, `Deno.memoryUsage()` | `/proc/loadavg`, `/proc/meminfo` read in the awaited future |
| `db: { history: table({ .. }) }` | `#[table("samples", columns = "…")]` on the field |
| `users: { "admin-token-123": { id, role } }` | `Http::users(Users::map(..))`, tokens from the environment |
| `visible.forUser` | `#[cell(visible = by_role)]` |
| sparkline of flex `div`s | an SVG `polyline` |
| `testCell(metrics, …)` | `TestCells` on a virtual clock; `TestServer` for the wire |
| `build --compile --service` | `ax build`; `[app] mode = "server"` and a `cli` target for a headless one |

## Limits

- The load and memory reads are Linux's; on macOS and Windows `sample()`
  returns zeros until it is given that OS's call.
- One server serves one shared cell (`aio::sync::serve` takes one
  `Handle`); aio serves every cell on one socket. Keep what the screens
  share in one cell, or one served app per cell.
- No chart component in the kit: SVG by hand, or a library in a
  `WebIsland`.

## Common mistakes

- Reading `/proc` (or any file, or the clock) in a plain method: replay
  would differ. Put it in the awaited future of an async method, or an
  effect.
- Filtering the history in the view for viewers: they were sent it
  anyway. Filter in `visible`.
- A history that grows for ever in the cell: every call copies it. Trim
  it, and keep the archive in a table or `aio::db::Db` of its own.
- Tokens in `aio.toml` or the source: they are built into the binary.
  Read them from the environment or a file in `data/`.
- `use_live()` missing in the root: the cards miss the schedule's
  samples until something else re-renders them.

## API

`cargo doc -p aio-rs --no-deps --all-features --open`: `aio::Cx`,
`aio::Effect`, `aio::auth::Users`, `aio::http` (`Http`, `Route`,
`Reply`, `Req`), `aio::sync::Remote`, `aio::kit::ConnectionStatus`,
`aio::testing` (`TestCells`, `TestServer`, `TestUi`).
