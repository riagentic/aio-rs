//! Plugins (`aio::plugin`, aio's `definePlugin`): a crate's cells, routes,
//! server functions and hooks added to an app as one value, the app winning.
use aio::Cells;
use aio::fns::{Access, ServerFn};
use aio::http::{Http, Presence, Reply, Route};
use aio::plugin::{Plugin, compose};
use aio::prelude::*;
use aio::sync::Remote;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[cell(shared)]
pub struct Todo {
    pub items: Vec<String>,
}

#[methods]
impl Todo {
    pub fn add(&mut self, cx: &Cx, text: String) -> Result {
        cx.run(Effect::new("saved", |_: Handle<Todo>| async {}));
        self.items.push(text);
        Ok(())
    }
}

#[cell]
pub struct Audit {
    pub seen: u32,
}

#[methods]
impl Audit {
    pub fn record(&mut self) -> Result {
        self.seen += 1;
        Ok(())
    }
}

const WHO: ServerFn<(), String> = ServerFn::new("who.am");
const ONLY: ServerFn<(), String> = ServerFn::new("who.plugin");

fn text(t: &'static str) -> impl Fn(aio::http::Req) -> std::future::Ready<Reply> + Send + Sync + 'static {
    move |_| std::future::ready(Reply::text(t))
}

async fn until(what: &str, f: impl Fn() -> bool) {
    for _ in 0..300 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out: {what}");
}

/// GET `path` from `addr` with the key; the body.
async fn get(addr: &str, path: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    let req =
        format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer secret\r\nConnection: close\r\n\r\n");
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).await.unwrap();
    out.split_once("\r\n\r\n").unwrap().1.to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_plugins_cell_routes_functions_and_hooks_are_live_and_the_app_wins() {
    let effects: Arc<Mutex<Vec<String>>> = Arc::default();
    let links: Arc<Mutex<Vec<String>>> = Arc::default();
    let (e1, e2, e3) = (effects.clone(), effects.clone(), effects.clone());
    let (l1, l2) = (links.clone(), links.clone());
    let audit = Handle::<Audit>::memory();
    let files = Plugin::new("files")
        .cell(audit.clone(), &[])
        .route(Route::get("/files/:name", text("PLUGIN files")))
        .route(Route::get("/same", text("PLUGIN same")))
        .route(Route::get("/*", text("PLUGIN catch-all")))
        .server_fn(WHO, Access::Open, |_, ()| async { Ok("plugin".to_string()) })
        .server_fn(ONLY, Access::Open, |_, ()| async { Ok("plugin only".to_string()) })
        .on_effect(move |c, k| e1.lock().unwrap().push(format!("files {c} {k}")))
        // One that panics stops no other hook.
        .on_effect(|_, _| panic!("a plugin hook exploded"))
        .on_link(move |p: &Presence| {
            l1.lock().unwrap().push(format!("files {}", p.online));
            panic!("a link hook exploded");
        });
    let stats = Plugin::new("stats").on_effect(move |c, k| e2.lock().unwrap().push(format!("stats {c} {k}")));
    aio::on_effect(move |c, k| e3.lock().unwrap().push(format!("app {c} {k}")));
    let todo = Handle::<Todo>::memory();
    let app = Http::default()
        .route(Route::get("/files/*", |r| async move { Reply::text(format!("APP files {}", r.param("*").unwrap())) }))
        .route(Route::get("/same", text("APP same")))
        .server_fn(WHO, Access::Open, |_, ()| async { Ok("app".to_string()) })
        .on_link(move |p: &Presence| l2.lock().unwrap().push(format!("app {}", p.online)));
    let (cells, http) = compose(Cells::default().with(todo.clone(), &[]), app, vec![files, stats]).unwrap();
    let cells = cells.start().unwrap();
    assert_eq!(cells.ids(), ["todo", "audit"], "the plugin's cell beside the app's");
    audit.record().unwrap();
    assert_eq!(audit.state().seen, 1);

    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let opts = aio::sync::Options { token: "secret".into(), http, ..Default::default() };
    tokio::spawn(aio::sync::serve_with(todo.clone(), l, opts));
    until("served", || std::net::TcpStream::connect(&addr).is_ok()).await;
    // The app's routes match first; what it leaves is the plugin's.
    assert_eq!(get(&addr, "/files/a.txt").await, "APP files a.txt");
    assert_eq!(get(&addr, "/same").await, "APP same");
    assert_eq!(get(&addr, "/elsewhere").await, "PLUGIN catch-all");
    let r = Remote::<Todo>::connect(&addr, "secret", "ann").await.unwrap();
    assert_eq!(r.run(WHO, ()).await, Ok("app".to_string()), "the app's function of a name");
    assert_eq!(r.run(ONLY, ()).await, Ok("plugin only".to_string()));
    until("both heard the link", || links.lock().unwrap().len() == 2).await;
    assert_eq!(*links.lock().unwrap(), ["files true", "app true"], "plugins first, then the app");

    todo.add("milk".into()).unwrap();
    until("every effect hook heard", || effects.lock().unwrap().len() == 3).await;
    assert_eq!(
        *effects.lock().unwrap(),
        ["app todo saved", "files todo saved", "stats todo saved"],
        "in the order added"
    );
}

fn names(p: Vec<Plugin>) -> String {
    match compose(Cells::default().with(Handle::<Todo>::memory(), &[]), Http::default(), p) {
        Ok(_) => "ok".into(),
        Err(e) => e.0,
    }
}

#[test]
fn a_collision_refuses_the_boot_naming_both() {
    let route =
        |name, pattern: &str, methods: &[&str]| Plugin::new(name).route(Route::new(methods, pattern, text("x")));
    let e = names(vec![route("a", "/u/:id", &["GET"]), route("b", "/u/:name", &["GET"])]);
    assert!(e.contains("route \"/u/:name\" (the same route as \"/u/:id\")"), "{e}");
    assert!(e.contains("claimed by both \"a\" and \"b\""), "{e}");
    assert_eq!(
        names(vec![route("a", "/u/:id", &["GET"]), route("b", "/u/:id/posts", &["GET"])]),
        "ok",
        "another shape"
    );
    assert_eq!(names(vec![route("a", "/u", &["GET"]), route("b", "/u", &["POST"])]), "ok", "another method");
    assert!(names(vec![route("a", "/u", &[]), route("b", "/u", &["POST"])]).contains("claimed by both"), "any method");
    assert_eq!(names(vec![route("a", "/u", &["GET"]).route(Route::get("/u", text("y")))]), "ok", "one plugin's own");

    let cell = |name| Plugin::new(name).cell(Handle::<Audit>::memory(), &[]);
    assert!(names(vec![cell("a"), cell("b")]).contains("cell `audit` is claimed by both \"a\" and \"b\""));
    let e = names(vec![Plugin::new("p").cell(Handle::<Todo>::memory(), &[])]);
    assert!(e.contains("cell `todo` is both the app's and \"p\"'s"), "{e}");

    let f = |name| Plugin::new(name).server_fn(WHO, Access::Open, |_, ()| async { Ok(String::new()) });
    assert!(names(vec![f("a"), f("b")]).contains("server function `who.am` is claimed by both \"a\" and \"b\""));

    let e = names(vec![Plugin::new("twice"), Plugin::new("twice")]);
    assert!(e.contains("\"twice\" is listed twice"), "{e}");
    assert_eq!(names(vec![]), "ok", "nothing declared, nothing changes");
}
