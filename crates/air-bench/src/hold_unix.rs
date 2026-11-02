// `hold air [keys] [gap ms] [rows]`: a 1000-row cell list drawn by air, in
// the pinned Electron runtime (`AIORT_RUNTIME`) headless, over the shell's
// wire. Holds ↓ and prints keydown → DOM-applied (p50, p95, max) and the
// bytes sent to the page per key.
use std::io::{BufRead, Read, Write};
use std::path::PathBuf;

/// Rows in the list (arg 4; 1000).
static ROWS: std::sync::OnceLock<u32> = std::sync::OnceLock::new();

fn rows() -> u32 {
    *ROWS.get().unwrap_or(&1000)
}

#[aio::data]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Row {
    pub id: u32,
    pub name: String,
    pub amount: u64,
}

#[aio::cell(id = "list")]
pub struct List {
    pub rows: Vec<Row>,
    pub cursor: u32,
}

#[aio::methods]
impl List {
    pub fn fill(&mut self, n: u32) {
        self.rows = (0..n).map(|id| Row { id, name: format!("Account {id}"), amount: id as u64 * 7 }).collect();
    }

    pub fn down(&mut self) {
        self.cursor = (self.cursor + 1).min(self.rows.len().saturating_sub(1) as u32);
    }
}

mod on_air {
    use super::{List, ListCalls, rows};
    use air::{Key, KeyboardEvent, View, rsx};
    use aio::CellData;

    thread_local!(static CELL: aio::Handle<List> = aio::Handle::memory());

    /// The cell bound as `client!` binds it on air: a signal per field,
    /// set after a call for the fields its commit changed.
    pub fn app() -> View {
        let h = CELL.with(Clone::clone);
        _ = h.fill(rows());
        let st = h.state();
        let rows = air::Signal::new(st.rows.clone());
        let cursor = air::Signal::new(st.cursor);
        let sel = air::selector(move || cursor.get());
        let down = move || {
            let h = CELL.with(Clone::clone);
            let before = h.state();
            _ = h.down();
            let now = h.state();
            let changed = now.changed_fields(&before);
            if changed & 1 != 0 {
                rows.set(now.rows.clone());
            }
            if changed & 2 != 0 {
                cursor.set(now.cursor);
            }
        };
        rsx! {
            div { "data-t": "keys", tabindex: "0",
                onkeydown: move |e: KeyboardEvent| if e.key() == Key::ArrowDown { down() },
                ul {
                    for r in rows.get() {
                        li { key: r.id, "aria-selected": sel.is(r.id), "{r.name} {r.amount}" }
                    }
                }
            }
        }
    }
}

fn page() -> String {
    let body = format!(r#"<div id="main"></div><script>{}</script>"#, air::PAGE_JS);
    let html = format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><title>hold</title></head><body>{body}</body></html>"#
    );
    aio::page::sealed(&html, "'none'")
}

fn frame(s: &mut impl Read) -> Option<Vec<u8>> {
    let mut n = [0; 4];
    s.read_exact(&mut n).ok()?;
    let mut b = vec![0; u32::from_be_bytes(n) as usize];
    s.read_exact(&mut b).ok()?;
    Some(b)
}

fn send(s: &mut impl Write, b: &[u8]) {
    if !b.is_empty() {
        _ = s.write_all(&(b.len() as u32).to_be_bytes()).and_then(|_| s.write_all(b));
    }
}

/// air's loop: mount, then each event frame in, its edits out (one thread: air's runtime is the thread's).
fn serve_air(mut s: std::os::unix::net::UnixStream) {
    let t = std::time::Instant::now();
    let root = air::Root::mount(on_air::app);
    let first = air::take_edits();
    // A hot patch remounts: what that costs (the cell keeps its state).
    eprintln!("air mount: {:.2} ms, {} B", t.elapsed().as_secs_f64() * 1e3, first.len());
    send(&mut s, &first);
    while let Some(f) = frame(&mut s) {
        root.event(&f);
        send(&mut s, &air::take_edits());
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) != Some("air") {
        eprintln!("usage: hold air [keys] [gap ms] [rows]");
        std::process::exit(2);
    }
    let keys = args.get(2).map_or("200", String::as_str);
    let gap = args.get(3).map_or("40", String::as_str);
    _ = ROWS.set(args.get(4).and_then(|r| r.parse().ok()).unwrap_or(1000));
    let dir = std::env::temp_dir().join(format!("hold-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("harness dir");
    for (f, s) in [
        ("main.js", include_str!("../harness/main.js")),
        ("preload.js", include_str!("../harness/preload.js")),
        ("package.json", r#"{"name":"air-hold","main":"main.js"}"#),
    ] {
        std::fs::write(dir.join(f), s).expect("harness file");
    }
    let html = page();
    eprintln!("page: {} B", html.len());
    std::fs::write(dir.join("page.html"), html).expect("page");
    let sock = dir.join("s.sock");
    let listener = std::os::unix::net::UnixListener::bind(&sock).expect("socket");
    let rt = PathBuf::from(std::env::var_os("AIORT_RUNTIME").expect("AIORT_RUNTIME: the Electron runtime folder"));
    let mut child = std::process::Command::new(aio::paths::electron_exe_for(std::env::consts::OS, &rt))
        .arg(&dir)
        .args([
            "--no-sandbox",
            "--ozone-platform=headless",
            "--use-angle=swiftshader",
            "--enable-unsafe-swiftshader",
            "--disable-gpu-compositing",
            "--disable-gpu-vsync",
            "--disable-frame-rate-limit",
        ])
        .arg(format!("--sock={}", sock.display()))
        .arg(format!("--keys={keys}"))
        .arg(format!("--gap={gap}"))
        .env_remove("ELECTRON_RUN_AS_NODE")
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("electron");
    let (s, _) = listener.accept().expect("the page's connection");
    std::thread::spawn(move || serve_air(s));
    let out = std::io::BufReader::new(child.stdout.take().expect("stdout"));
    let line = out.lines().map_while(Result::ok).find(|l| l.starts_with(r#"{"bench""#));
    _ = child.wait();
    _ = std::fs::remove_dir_all(&dir);
    let Some(line) = line else {
        eprintln!("no result from the harness");
        std::process::exit(1);
    };
    report(&line);
}

fn report(line: &str) {
    let v: aio::serde_json::Value = aio::serde_json::from_str(line).expect("result");
    let mut lat: Vec<f64> = v["lat"].as_array().unwrap().iter().filter_map(|x| x.as_f64()).collect();
    let missed = v["keys"].as_u64().unwrap() as usize - lat.len();
    lat.sort_by(f64::total_cmp);
    let bytes: Vec<u64> = v["bytes"].as_array().unwrap().iter().filter_map(|x| x.as_u64()).collect();
    let q = |p: f64| lat[((lat.len() - 1) as f64 * p) as usize];
    println!(
        "air: rows {} selected {} keys {} (no frame: {missed}) keydown->applied p50 {:.2} ms p95 {:.2} ms max {:.2} ms, {:.0} B/key",
        v["nodes"],
        v["selected"],
        v["keys"],
        q(0.5),
        q(0.95),
        q(1.0),
        bytes.iter().sum::<u64>() as f64 / bytes.len().max(1) as f64
    );
}
