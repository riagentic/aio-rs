//! The window reaches no host the app did not declare, in the real runtime
//! (this crate's main.js, headless): every request off this machine goes
//! through a proxy that only listens, from launch through typing. Chromium's
//! spellchecker fetched its dictionary from Google's CDN on first launch
//! (Linux), past the app's `[capabilities] network`. Skipped where the
//! runtime is not installed.
use aio::serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

const PATIENCE: Duration = Duration::from_secs(60);

#[tokio::test(flavor = "multi_thread")]
async fn the_window_reaches_no_other_host_on_launch_or_while_typing() {
    let rt = crate::runtime::dir();
    let exe = crate::runtime::exe(&rt);
    if !exe.exists() {
        return eprintln!("skipped: no Electron runtime in {}", rt.display());
    }
    // The proxy: what each connection asked for (its first line), never answered.
    let asked = Arc::new(Mutex::new(Vec::<String>::new()));
    let proxy = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let at = proxy.local_addr().unwrap();
    {
        let asked = asked.clone();
        tokio::spawn(async move {
            while let Ok((s, _)) = proxy.accept().await {
                let asked = asked.clone();
                tokio::spawn(async move {
                    let mut r = tokio::io::BufReader::new(s);
                    let mut line = String::new();
                    _ = r.read_line(&mut line).await;
                    asked.lock().unwrap().push(line.trim().to_string());
                    std::future::pending::<()>().await;
                });
            }
        });
    }
    let d = tempfile::tempdir_in(aio::paths::short_temp()).unwrap();
    let dir = d.path().join("shell");
    crate::install_with(&dir, "offline", &crate::Window { title: "Offline", ..Default::default() }, true).unwrap();
    let (sock, ctl_sock) = (aio::ipc::addr(d.path(), "wire"), aio::ipc::addr(d.path(), "ctl"));
    let mut wires = aio::ipc::Listener::bind(&sock).unwrap();
    let mut ctls = aio::ipc::Listener::bind(&ctl_sock).unwrap();
    let mut child = tokio::process::Command::new(&exe)
        .arg(&dir)
        .args(crate::runtime::sandbox_args(&rt))
        .args(["--ozone-platform=headless", "--aio-title=Offline", "--aio-width=640", "--aio-height=480"])
        .args(["--use-angle=swiftshader", "--enable-unsafe-swiftshader"])
        .arg("--aio-no-open=1")
        .arg(format!("--proxy-server=http://{at}"))
        .arg(format!("--user-data-dir={}", d.path().join("profile").display()))
        .arg(format!("--aio-sock={sock}"))
        .arg(format!("--aio-ctl={ctl_sock}"))
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        // The person's language, as on a desktop.
        .env("LANG", "en_US.UTF-8")
        .env("TMPDIR", d.path())
        .env("DBUS_SESSION_BUS_ADDRESS", format!("unix:path={}", d.path().join("no-bus").display()))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let said = Arc::new(Mutex::new(Vec::<String>::new()));
    {
        let (said, err) = (said.clone(), child.stderr.take().unwrap());
        tokio::spawn(async move {
            let mut lines = tokio::io::BufReader::new(err).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                said.lock().unwrap().push(l);
            }
        });
    }
    let s = tokio::time::timeout(PATIENCE, ctls.accept()).await.unwrap().unwrap();
    let (r, mut ctl) = tokio::io::split(s);
    let (tx, mut heard) = tokio::sync::mpsc::unbounded_channel::<Value>();
    tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(r).lines();
        while let Ok(Some(l)) = lines.next_line().await {
            if let Ok(v) = aio::serde_json::from_str(&l) {
                _ = tx.send(v);
            }
        }
    });
    ctl.write_all(b"{\"t\":\"place\",\"width\":640,\"height\":480}\n").await.unwrap();
    let _wire = tokio::time::timeout(PATIENCE, wires.accept()).await.unwrap().unwrap();
    let mut id = 0;
    let mut cdp = async |method: &str, params: Value| -> Value {
        id += 1;
        let m = json!({ "t": "cdp", "id": id, "method": method, "params": params });
        ctl.write_all(format!("{m}\n").as_bytes()).await.unwrap();
        loop {
            let v = tokio::time::timeout(PATIENCE, heard.recv()).await.unwrap().unwrap();
            if v["t"] == "cdp" && v["id"] == id {
                assert!(v["error"].is_null() && v["result"]["exceptionDetails"].is_null(), "{method}: {v}");
                return v["result"].clone();
            }
        }
    };
    let eval = |e: &str| json!({ "expression": e, "returnByValue": true, "awaitPromise": true });
    let end = std::time::Instant::now() + PATIENCE;
    while cdp("Runtime.evaluate", eval("location.href")).await["result"]["value"] != "aio://app/" {
        assert!(std::time::Instant::now() < end, "the page never loaded: {:?}", said.lock().unwrap());
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // A person types misspelled words into a field.
    let field = "const t = document.createElement('textarea'); document.body.append(t); t.focus(); true";
    cdp("Runtime.evaluate", eval(field)).await;
    for c in "teh quikc brwon fox ".chars() {
        cdp("Input.insertText", json!({ "text": c.to_string() })).await;
    }
    let typed = cdp("Runtime.evaluate", eval("document.querySelector('textarea').value")).await;
    assert_eq!(typed["result"]["value"], "teh quikc brwon fox ");
    // Linux: the dictionary's fetch ends, from no host, said once (or it
    // reached the proxy). macOS and Windows check en-US natively: no fetch
    // to wait for, so a while for any other to show.
    let unchecked = "[aio-shell] spellcheck: no en-US dictionary on this machine, and [capabilities] network \
                     does not reach Google's (*.gvt1.com): en-US words go unchecked";
    let linux = cfg!(target_os = "linux");
    let end = std::time::Instant::now() + if linux { PATIENCE } else { Duration::from_secs(2) };
    while !said.lock().unwrap().iter().any(|l| l == unchecked) && asked.lock().unwrap().is_empty() {
        if std::time::Instant::now() >= end {
            assert!(!linux, "never said: {:?}", said.lock().unwrap());
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    _ = child.kill().await;
    assert_eq!(*asked.lock().unwrap(), Vec::<String>::new(), "requests off this machine");
    assert_eq!(
        said.lock().unwrap().iter().filter(|l| l.contains("spellcheck")).count(),
        usize::from(linux),
        "{:?}",
        said.lock().unwrap()
    );
}
