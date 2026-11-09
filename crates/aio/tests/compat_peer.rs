//! An aio-rs app calling a real aio app over aio's local socket as itself,
//! proven (`aio::compat::AioPeer`): run when `AIO_DENO_SRC` names a copy of
//! aio's `src` and `deno` is on PATH, else it says it skipped. Its socket
//! is a Unix socket, or on Windows a named pipe.
use aio::compat::AioPeer;
use serde_json::json;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread")]
async fn an_aio_app_sees_this_app_proven_on_its_socket() {
    let Some(src) = std::env::var_os("AIO_DENO_SRC").map(std::path::PathBuf::from) else {
        eprintln!("skipped: set AIO_DENO_SRC to a copy of aio's src (and have deno) to call a real aio app");
        return;
    };
    let _world = aio::paths::sandbox().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let (run, apps) = (tmp.path().join("run"), tmp.path().join("apps"));
    aio::paths::private_dir(&run).unwrap();
    // aio's lock dir (under XDG_RUNTIME_DIR, on Windows TEMP) and its apps'
    // homes, both in the temp dir, for aio and for this app.
    unsafe {
        std::env::set_var(if cfg!(windows) { "TEMP" } else { "XDG_RUNTIME_DIR" }, &run);
        std::env::set_var("AIO_APPS_DIR", &apps);
    }
    let id = format!("aiorspeer{}", std::process::id());
    // An aio app is its folder: the script and its (empty) page.
    let script = tmp.path().join("callee.ts");
    std::fs::write(&script, include_str!("aio_client/callee.ts")).unwrap();
    std::fs::write(tmp.path().join("App.tsx"), "export default function App() { return null; }\n").unwrap();
    let _aio_app = Stop(
        std::process::Command::new("deno")
            .args(["run", "--quiet", "--no-lock", "-A"])
            .arg(format!("--config={}", src.join("../deno.json").display()))
            .arg(script)
            .arg(&src)
            .arg(&id)
            .envs([("DISPLAY", ""), ("WAYLAND_DISPLAY", ""), ("AIO_NO_OPEN", "1")])
            .current_dir(tmp.path())
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("deno runs"),
    );
    let peer = async {
        for _ in 0..300 {
            if let Some(p) = AioPeer::find(&id).unwrap() {
                return p;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("the aio app never started");
    };
    let checked = async {
        let p = peer.await;
        assert_eq!(p.app(), id);
        assert_eq!(p.call("svc", "add", json!([2])).await.unwrap(), json!(2));
        let mut link = p.link().await.unwrap();
        assert_eq!(link.call("svc", "add", json!([3])).await.unwrap(), json!(5));
        assert_eq!(link.cells()["svc"]["n"], 5, "its state, live");
        // The sandbox's app is `app`: each link proved it, and only it.
        assert_eq!(link.call("svc", "who", json!([])).await.unwrap(), json!(["app", "app"]));
        // A proof under another key: refused, in aio's words.
        let key = aio::paths::app_dirs().unwrap().data.join("control.key");
        let real = std::fs::read(&key).unwrap();
        std::fs::write(&key, "0".repeat(64)).unwrap();
        let e = p.link().await.err().unwrap();
        assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied, "{e}");
        assert!(e.to_string().contains("does not verify"), "{e}");
        std::fs::write(&key, &real).unwrap();
        // Served to aio's tools now too (am state): one record, one key, so
        // the proofs still verify and `am` reads the same key.
        let dirs = aio::paths::app_dirs().unwrap();
        aio::compat::local::serve(&dirs.id, &dirs.data, dirs.profile.as_deref(), "x", "unused".into());
        assert_eq!(std::fs::read(&key).unwrap(), real, "the run's one key");
        assert_eq!(p.call("svc", "add", json!([1])).await.unwrap(), json!(6));
        let none = AioPeer::find("aiorsnosuchapp").unwrap();
        assert!(none.is_none());
        assert!(AioPeer::find("../x").is_err(), "an id is checked");
    };
    tokio::time::timeout(Duration::from_secs(60), checked).await.expect("done in time");
    // The stop takes back this app's lock record and control key.
    let records = || {
        std::fs::read_dir(&run)
            .unwrap()
            .flat_map(|d| std::fs::read_dir(d.unwrap().path()).into_iter().flatten())
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("app@") && n.ends_with(".lock"))
            .count()
    };
    assert_eq!(records(), 1);
    let rec = std::fs::read_dir(&run)
        .unwrap()
        .flat_map(|d| std::fs::read_dir(d.unwrap().path()).into_iter().flatten())
        .map(|e| e.unwrap().path())
        .find(|p| p.file_name().unwrap().to_string_lossy().starts_with("app@"))
        .unwrap();
    let rec: serde_json::Value = serde_json::from_slice(&std::fs::read(rec).unwrap()).unwrap();
    assert!(
        rec["socketPath"].as_str().is_some_and(|s| s.ends_with(".sock") || s.starts_with(r"\\.\pipe\")),
        "now with its socket: {rec}"
    );
    let key = aio::paths::app_dirs().unwrap().data.join("control.key");
    assert!(key.exists());
    std::thread::spawn(|| aio::shutdown(Duration::from_secs(2))).join().unwrap();
    assert_eq!((records(), key.exists()), (0, false));
}

/// The aio app, stopped as `am stop` stops it (SIGTERM), when the test ends
/// or fails.
struct Stop(std::process::Child);

impl Drop for Stop {
    fn drop(&mut self) {
        // aio's clean stop where there are signals; Windows has none to send.
        if cfg!(unix) {
            _ = std::process::Command::new("kill").arg("-TERM").arg(self.0.id().to_string()).status();
        }
        let ended = (0..100).any(|_| {
            std::thread::sleep(Duration::from_millis(100));
            self.0.try_wait().is_ok_and(|s| s.is_some())
        });
        if !ended {
            _ = self.0.kill();
            _ = self.0.wait();
        }
    }
}
