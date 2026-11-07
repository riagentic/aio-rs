//! aio's Android host: the app's view tree lives in Rust (the app's own
//! library, loaded by a small Java activity) and the system WebView draws
//! it. DOM edits and events travel over a WebMessage port as base64 text:
//! the page has no network, loads no file and sees nothing else of the app.
//!
//! An app's library says what it is with [`android_app!`](crate::android_app);
//! `ax build <app> --android` makes the APKs.
use crate::{Window, page_with};
use aio::ui::View;
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
pub use jni;
use jni::objects::{GlobalRef, JClass, JIntArray, JObjectArray, JString, JValue};
use jni::sys::jstring;
use jni::{JNIEnv, JavaVM};
use std::sync::{Mutex, OnceLock};
use tokio_util::codec::{Framed, LengthDelimitedCodec};

/// What an app's library hands the host.
pub struct App {
    /// The app's `aio.toml`, built in: `|| aio::config!()`.
    pub config: fn() -> aio::config::Config,
    /// `env!("CARGO_PKG_VERSION")`.
    pub version: &'static str,
    pub root: fn() -> View,
    pub window: Window,
    /// Runs once the app's dirs are locked, before the first page: open the
    /// app's cells here (as desktop `main` does).
    pub setup: fn(&aio::config::Config, &aio::paths::AppDirs) -> std::io::Result<()>,
}

/// The page's side of the port: `window.__aioWire`, as the desktop preload
/// gives it, over the port the activity posts as `aio-port`.
const BRIDGE: &str = "window.__aioWire = (() => {
  let port = null, onFrame = null;
  const early = [], out = [];
  const bytes = (s) => { const b = atob(s), u = new Uint8Array(b.length); for (let i = 0; i < b.length; i++) u[i] = b.charCodeAt(i); return u; };
  const text = (u) => { let s = ''; for (let i = 0; i < u.length; i += 0x8000) s += String.fromCharCode.apply(null, u.subarray(i, i + 0x8000)); return btoa(s); };
  // The first frame drawn into the page says so: the splash can go.
  let painted = false;
  const paint = () => { if (!painted && onFrame && port) { painted = true; port.postMessage('aio-painted'); } };
  window.addEventListener('message', (e) => {
    if (e.data !== 'aio-port' || !e.ports[0]) return;
    port = e.ports[0];
    port.onmessage = (m) => { const f = bytes(m.data); onFrame ? (onFrame(f), paint()) : early.push(f); };
    out.splice(0).forEach((s) => port.postMessage(s));
  });
  return {
    send: (u) => { const s = text(u); port ? port.postMessage(s) : out.push(s); },
    listen: (f) => { onFrame = f; const e = early.splice(0); e.forEach(f); if (e.length) paint(); },
  };
})();";

struct Host {
    vm: JavaVM,
    /// `aio.rt.Host`, found on the activity's thread (a native thread's
    /// class loader can't).
    class: GlobalRef,
    rt: tokio::runtime::Runtime,
    views: crate::views::Views,
    root: fn() -> View,
    page: String,
    /// Frames from the page now showing, to its view.
    to_view: Mutex<Option<tokio::sync::mpsc::UnboundedSender<bytes::Bytes>>>,
    /// What the page may hold beyond the basics (`Window::permissions`).
    permissions: &'static [&'static str],
    /// The system picker's questions still open, by id.
    asking: Mutex<Vec<(u64, tokio::sync::oneshot::Sender<Answer>)>>,
    dirs: aio::paths::AppDirs,
}

/// The system picker's answer: each document's name and descriptor (none:
/// cancelled), or why it failed.
type Answer = Result<Vec<(String, std::os::fd::OwnedFd)>, String>;

static HOST: OnceLock<Host> = OnceLock::new();

fn b64() -> &'static base64::engine::GeneralPurpose {
    &base64::engine::general_purpose::STANDARD
}

/// Boot the app once per process (`aio.rt.Host.start`): its dirs under the
/// app's private files dir, the log, its cells. Returns the page to load.
pub fn start(env: &mut JNIEnv, class: &JClass, files: &JString, app: &App) -> Result<String, String> {
    if let Some(h) = HOST.get() {
        return Ok(h.page.clone());
    }
    let files: String = env.get_string(files).map_err(|e| e.to_string())?.into();
    // SAFETY: before any thread of this library reads the environment.
    unsafe { std::env::set_var("AIORT_DATA", &files) };
    let cfg = (app.config)();
    // A port (`[app] port_of`) runs on aio's layout in its files dir, its
    // `[data] store`, as on the desktop.
    let plan = aio::paths::plan_app(&cfg, &aio::paths::Ask::default()).map_err(|e| e.to_string())?;
    // Installed over aio's APK (the same id): aio's state comes in, once,
    // before the home is claimed (its files dir then holds aio-rs's data).
    let imported = match cfg.app.port_of.as_deref() {
        Some(aio_app) => aio::compat::data::import_android(std::path::Path::new(&files), aio_app, &plan.store)
            .map_err(|e| format!("aio's state of {aio_app}: {e}"))?,
        None => false,
    };
    let dirs = aio::paths::AppDirs::planned(&cfg.app.id, plan).map_err(|e| e.to_string())?;
    aio::boot::begin(&cfg, app.version, &dirs).map_err(|e| e.to_string())?;
    if imported {
        eprintln!("[aio] the aio APK's state is now {}", dirs.store.display());
    }
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().map_err(|e| e.to_string())?;
    {
        let _in = rt.enter();
        (app.setup)(&cfg, &dirs).map_err(|e| e.to_string())?;
        // Its releases are offered as the APK to open (`update::apk`).
        #[cfg(feature = "updates")]
        if let Err(e) = crate::update::open(&cfg, app.version, &dirs) {
            eprintln!("[aio] no updates cell: {e}");
        }
        #[cfg(feature = "updates")]
        crate::update::watch(&cfg, app.version, &dirs.data);
        #[cfg(not(feature = "updates"))]
        if let Some(w) = crate::update::left_out(cfg.updates.url.is_some(), aio::update::installed()) {
            eprintln!("{w}");
        }
        aio::health::watch(rt.handle().clone());
    }
    let host = Host {
        vm: env.get_java_vm().map_err(|e| e.to_string())?,
        class: env.new_global_ref(class).map_err(|e| e.to_string())?,
        rt,
        views: crate::views::Views::new(),
        root: crate::connect::root(app.root, &dirs),
        page: page_with(&app.window, BRIDGE),
        to_view: Mutex::new(None),
        permissions: app.window.permissions,
        asking: Mutex::new(vec![]),
        dirs,
    };
    eprintln!("[aio] {} started on Android", cfg.app.id);
    Ok(HOST.get_or_init(|| host).page.clone())
}

/// A page (re)loaded and posted its port (`aio.rt.Host.connect`): it gets
/// a fresh view; the one before, if any, ends.
pub fn connect() {
    let Some(h) = HOST.get() else { return };
    static READY: std::sync::Once = std::sync::Once::new();
    READY.call_once(|| eprintln!("[aio] window ready"));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<bytes::Bytes>();
    *h.to_view.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);
    let (view, ours) = tokio::io::duplex(1 << 20);
    let (views, root) = (h.views.clone(), h.root);
    h.rt.spawn(async move {
        if let Err(e) = views.launch(Box::new(view), root).await {
            eprintln!("[aio-shell] window: {e}");
        }
    });
    let codec = LengthDelimitedCodec::builder().max_frame_length(256 << 20).new_codec();
    let (mut sink, mut frames) = Framed::new(ours, codec).split();
    h.rt.spawn(async move {
        while let Some(Ok(f)) = frames.next().await {
            to_page(&b64().encode(&f));
        }
    });
    h.rt.spawn(async move {
        while let Some(m) = rx.recv().await {
            if sink.send(m).await.is_err() {
                break;
            }
        }
    });
}

/// One frame for the page, through the activity's port.
fn to_page(frame: &str) {
    let Some(h) = HOST.get() else { return };
    let sent = (|| -> jni::errors::Result<()> {
        let mut env = h.vm.attach_current_thread_permanently()?;
        let s = env.new_string(frame)?;
        env.call_static_method(&h.class, "toPage", "(Ljava/lang/String;)V", &[JValue::Object(&s)])?;
        env.delete_local_ref(s)
    })();
    if let Err(e) = sent {
        eprintln!("[aio-shell] a frame did not reach the page: {e}");
    }
}

/// A frame from the page (`aio.rt.Host.fromPage`).
pub fn from_page(env: &mut JNIEnv, frame: &JString) {
    let Some(h) = HOST.get() else { return };
    let Ok(text) = env.get_string(frame) else { return };
    let Ok(bytes) = b64().decode(String::from(text)) else { return };
    if let Some(tx) = h.to_view.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        _ = tx.send(bytes.into());
    }
}

/// Back was pressed (`aio.rt.Host.back`): the views' handlers are asked
/// (`aio::kit::use_back_button`); none took it, the activity does its
/// default (`aio.rt.Host.notTaken`).
pub fn back() {
    let Some(h) = HOST.get() else { return };
    h.rt.spawn(async move {
        if aio::ui::back_pressed().await {
            return;
        }
        let told = (|| -> jni::errors::Result<()> {
            let mut env = h.vm.attach_current_thread_permanently()?;
            env.call_static_method(&h.class, "notTaken", "()V", &[]).map(drop)
        })();
        if let Err(e) = told {
            eprintln!("[aio-shell] Back's default did not run: {e}");
        }
    });
}

/// The app was opened with a link (`aio.rt.Host.link`: `[build.android]
/// schemes`), cold or while it runs.
pub fn link(env: &mut JNIEnv, url: &JString) {
    if let Ok(url) = env.get_string(url) {
        aio::ui::open_link(&String::from(url));
    }
}

/// May the page hold `permission` (`aio.rt.Host.allows`)? The app's window
/// says, as on the desktop (`Window::permissions`: `"media"` is the camera).
pub fn allows(env: &mut JNIEnv, permission: &JString) -> bool {
    let Some(h) = HOST.get() else { return false };
    env.get_string(permission).is_ok_and(|p| h.permissions.contains(&String::from(p).as_str()))
}

/// What the system picker is asked (`aio.rt.Host.ask`'s `what`).
#[derive(Clone, Copy)]
enum Ask {
    Open = 0,
    OpenMany = 1,
    Save = 2,
}

/// Ask the system picker (`aio.rt.Host.ask`): its answer, or why there is none.
async fn ask(
    what: Ask,
    title: &str,
    extensions: &[String],
    name: &str,
) -> Result<Vec<(String, std::os::fd::OwnedFd)>, String> {
    let h = HOST.get().ok_or("the app has not started")?;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = tokio::sync::oneshot::channel();
    h.asking.lock().unwrap_or_else(|e| e.into_inner()).push((id, tx));
    let asked = (|| -> jni::errors::Result<bool> {
        let mut env = h.vm.attach_current_thread_permanently()?;
        let (title, name) = (env.new_string(title)?, env.new_string(name)?);
        let exts = env.new_object_array(extensions.len() as i32, "java/lang/String", jni::objects::JObject::null())?;
        for (i, e) in extensions.iter().enumerate() {
            let e = env.new_string(e)?;
            env.set_object_array_element(&exts, i as i32, e)?;
        }
        let args = [JValue::Long(id as i64), JValue::Int(what as i32), (&title).into(), (&exts).into(), (&name).into()];
        env.call_static_method(&h.class, "ask", "(JILjava/lang/String;[Ljava/lang/String;Ljava/lang/String;)Z", &args)?
            .z()
    })();
    if !asked.unwrap_or(false) {
        answered(id, Err("no window to ask in".into()));
    }
    rx.await.map_err(|e| e.to_string())?
}

fn answered(id: u64, a: Answer) {
    let Some(h) = HOST.get() else { return };
    let mut all = h.asking.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(i) = all.iter().position(|(k, _)| *k == id) {
        _ = all.swap_remove(i).1.send(a);
    }
}

/// The system picker answered (`aio.rt.Host.answer`): the documents' names
/// and descriptors, now this library's (none: cancelled), or `error`.
pub fn answer(env: &mut JNIEnv, id: i64, names: &JObjectArray, fds: &JIntArray, error: &JString) {
    use std::os::fd::FromRawFd;
    let got = (|| -> jni::errors::Result<Answer> {
        if !error.is_null() {
            return Ok(Err(env.get_string(error)?.into()));
        }
        if names.is_null() {
            return Ok(Ok(vec![]));
        }
        let mut raw = vec![0; env.get_array_length(fds)? as usize];
        env.get_int_array_region(fds, 0, &mut raw)?;
        let mut out = vec![];
        for (i, fd) in raw.into_iter().enumerate() {
            let name = JString::from(env.get_object_array_element(names, i as i32)?);
            let name: String = env.get_string(&name)?.into();
            // SAFETY: the activity detached each descriptor for this library.
            out.push((name, unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) }));
        }
        Ok(Ok(out))
    })();
    answered(id as u64, got.unwrap_or_else(|e| Err(e.to_string())));
}

/// Where the picked and saved files of this run are: under the app's cache,
/// the last run's cleared first.
fn handed(what: &str) -> std::path::PathBuf {
    let dir = HOST.get().map(|h| h.dirs.cache.join(what)).unwrap_or_default();
    static CLEARED: std::sync::Once = std::sync::Once::new();
    CLEARED.call_once(|| {
        let cache = dir.parent().unwrap_or(&dir);
        _ = std::fs::remove_dir_all(cache.join("picked"));
        _ = std::fs::remove_dir_all(cache.join("saving"));
    });
    dir
}

/// `page::pick` on Android: the system's document picker, each file copied
/// into the app's cache (a document has no path of its own). A folder is
/// never a path on Android: refused.
pub(crate) async fn pick(p: &crate::page::Pick, many: bool) -> std::io::Result<Option<Vec<String>>> {
    if p.folder {
        return Err(std::io::Error::other(
            "a folder can't be picked on Android (its documents have no paths): pick files",
        ));
    }
    let exts: Vec<String> = p.filters.iter().flat_map(|(_, e)| e.iter().cloned()).collect();
    let what = if many { Ask::OpenMany } else { Ask::Open };
    let files = ask(what, &p.title, &exts, "").await.map_err(std::io::Error::other)?;
    if files.is_empty() {
        return Ok(None);
    }
    let dir = handed("picked");
    let paths = tokio::task::spawn_blocking(move || crate::handover::take_in(&dir, files))
        .await
        .map_err(std::io::Error::other)??;
    Ok(Some(paths))
}

/// How long a saved file may take to be written after its path is given.
const SAVE_WAIT: std::time::Duration = std::time::Duration::from_secs(600);

/// `page::save` on Android: the system's "save as" picker; the path given
/// is written once, and its bytes reach the document chosen when closed.
pub(crate) async fn save(title: &str, name: &str, extensions: &[String]) -> std::io::Result<Option<String>> {
    let mut files = ask(Ask::Save, title, extensions, name).await.map_err(std::io::Error::other)?;
    let Some((chosen, fd)) = files.pop() else { return Ok(None) };
    let name = if chosen.is_empty() { name.to_string() } else { chosen };
    crate::handover::save_through(&handed("saving"), &name, fd, SAVE_WAIT).map(Some)
}

/// The activity is going away for good (`aio.rt.Host.stop`): the stores
/// finish their effects. Android may also just end the process: every
/// call is on disk when it returns (phones save hard).
pub fn stop() {
    let aborted = aio::shutdown(std::time::Duration::from_secs(5));
    if !aborted.is_empty() {
        eprintln!("[aio] stopped (aborted: {})", aborted.join(", "));
    }
    aio::health::stop();
    aio::log::finish();
}

/// [`start`] for JNI: the page, or a Java exception.
pub fn start_jni(mut env: JNIEnv, class: JClass, files: JString, app: &App) -> jstring {
    match start(&mut env, &class, &files, app).and_then(|p| env.new_string(p).map_err(|e| e.to_string())) {
        Ok(s) => s.into_raw(),
        Err(e) => {
            _ = env.throw_new("java/lang/RuntimeException", format!("aio: {e}"));
            std::ptr::null_mut()
        }
    }
}

/// The app's Android entry points (`aio.rt.Host`'s natives), in its library:
///
/// ```ignore
/// aio_shell::android_app!(aio_shell::android::App {
///     config: || aio::config!(),
///     version: env!("CARGO_PKG_VERSION"),
///     root: App,
///     window: aio_shell::Window { title: "Todo", ..Default::default() },
///     setup: |cfg, dirs| { _ = SOURCE.set(aio::Source::for_app(cfg, dirs)?); Ok(()) },
/// });
/// ```
#[macro_export]
macro_rules! android_app {
    ($app:expr) => {
        fn aio_android_app() -> $crate::android::App {
            $app
        }

        #[unsafe(no_mangle)]
        pub extern "system" fn Java_aio_rt_Host_start<'l>(
            env: $crate::android::jni::JNIEnv<'l>,
            class: $crate::android::jni::objects::JClass<'l>,
            files: $crate::android::jni::objects::JString<'l>,
        ) -> $crate::android::jni::sys::jstring {
            $crate::android::start_jni(env, class, files, &aio_android_app())
        }

        #[unsafe(no_mangle)]
        pub extern "system" fn Java_aio_rt_Host_connect<'l>(
            _: $crate::android::jni::JNIEnv<'l>,
            _: $crate::android::jni::objects::JClass<'l>,
        ) {
            $crate::android::connect()
        }

        #[unsafe(no_mangle)]
        pub extern "system" fn Java_aio_rt_Host_fromPage<'l>(
            mut env: $crate::android::jni::JNIEnv<'l>,
            _: $crate::android::jni::objects::JClass<'l>,
            frame: $crate::android::jni::objects::JString<'l>,
        ) {
            $crate::android::from_page(&mut env, &frame)
        }

        #[unsafe(no_mangle)]
        pub extern "system" fn Java_aio_rt_Host_back<'l>(
            _: $crate::android::jni::JNIEnv<'l>,
            _: $crate::android::jni::objects::JClass<'l>,
        ) {
            $crate::android::back()
        }

        #[unsafe(no_mangle)]
        pub extern "system" fn Java_aio_rt_Host_link<'l>(
            mut env: $crate::android::jni::JNIEnv<'l>,
            _: $crate::android::jni::objects::JClass<'l>,
            url: $crate::android::jni::objects::JString<'l>,
        ) {
            $crate::android::link(&mut env, &url)
        }

        #[unsafe(no_mangle)]
        pub extern "system" fn Java_aio_rt_Host_allows<'l>(
            mut env: $crate::android::jni::JNIEnv<'l>,
            _: $crate::android::jni::objects::JClass<'l>,
            permission: $crate::android::jni::objects::JString<'l>,
        ) -> $crate::android::jni::sys::jboolean {
            $crate::android::allows(&mut env, &permission).into()
        }

        #[unsafe(no_mangle)]
        pub extern "system" fn Java_aio_rt_Host_answer<'l>(
            mut env: $crate::android::jni::JNIEnv<'l>,
            _: $crate::android::jni::objects::JClass<'l>,
            id: $crate::android::jni::sys::jlong,
            names: $crate::android::jni::objects::JObjectArray<'l>,
            fds: $crate::android::jni::objects::JIntArray<'l>,
            error: $crate::android::jni::objects::JString<'l>,
        ) {
            $crate::android::answer(&mut env, id, &names, &fds, &error)
        }

        #[unsafe(no_mangle)]
        pub extern "system" fn Java_aio_rt_Host_stop<'l>(
            _: $crate::android::jni::JNIEnv<'l>,
            _: $crate::android::jni::objects::JClass<'l>,
        ) {
            $crate::android::stop()
        }
    };
}
