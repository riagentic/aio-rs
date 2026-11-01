//! What only the window can do, for views on the shared runtime: bring an
//! element into view, set a scroll position, read a text field's caret,
//! copy text, print, ask for files or a folder, or where to save. Each is one small script in
//! the page (aio did these on its DOM) or one line to the runtime; with no
//! window (headless tests) they do nothing, but a pick asks the desktop.
use aio::serde_json::json;
use std::io;

fn run(js: &str) {
    _ = dioxus_document::eval(js);
}

/// The element with this id, scrolled into view (to the top, or centred).
pub fn reveal_id(id: &str, center: bool) {
    run(&format!(
        "document.getElementById({})?.scrollIntoView({{block:'{}',behavior:'smooth'}})",
        json!(id),
        block(center)
    ));
}

/// The first element matching `selector`, scrolled into view.
pub fn reveal(selector: &str, center: bool) {
    run(&format!(
        "document.querySelector({})?.scrollIntoView({{block:'{}',behavior:'smooth'}})",
        json!(selector),
        block(center)
    ));
}

fn block(center: bool) -> &'static str {
    if center { "center" } else { "start" }
}

/// Scroll the element matching `selector` to `y` once its content is tall
/// enough (a document still laying out), trying for up to ten frames.
pub fn set_scroll(selector: &str, y: f64) {
    run(&format!(
        "let n=0;const go=()=>{{const el=document.querySelector({});if(!el)return;el.scrollTop={y};\
         if(el.scrollTop<{y}*0.9&&++n<10)requestAnimationFrame(go)}};requestAnimationFrame(go)",
        json!(selector)
    ));
}

/// Where the caret is in the text field matching `selector` (in UTF-16
/// units, as the page counts), if there is one.
pub async fn caret(selector: &str) -> Option<usize> {
    let js = format!("return document.querySelector({})?.selectionStart ?? null", json!(selector));
    dioxus_document::eval(&js).join::<Option<usize>>().await.ok().flatten()
}

/// Own part of the window's `<head>` while this component lives (aio's `useHead`).
pub use aio::head::use_head;

/// The system print dialog for the page (the runtime opens it; Electron
/// ignores the page's own `window.print()`).
pub fn print() {
    crate::tell_runtime("print".into());
}

/// `text` on the clipboard; false if the page was refused it.
pub async fn copy(text: &str) -> bool {
    let js = format!("try{{await navigator.clipboard.writeText({});return true}}catch(e){{return false}}", json!(text));
    dioxus_document::eval(&js).join::<bool>().await.unwrap_or(false)
}

/// An open dialog's question: files (of these types) or a folder.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pick {
    /// Empty: "Choose a file" / "Choose a folder".
    pub title: String,
    pub folder: bool,
    /// A folder, or a file (its folder is used: where the last pick was).
    pub start_in: Option<String>,
    /// A name and its extensions, without the dot.
    pub filters: Vec<(String, Vec<String>)>,
}

/// The path the user picked; `None` if cancelled. The runtime opens the
/// dialog, owned by the window (O3); with no window, the desktop's own tool
/// does (`dialog.rs`). An error is never a cancel: no dialog tool, no desktop
/// session, a dialog that failed. While it is open, no server fn caller's
/// clock runs (`aio::deciding::start`).
pub async fn pick(p: &Pick) -> io::Result<Option<String>> {
    Ok(ask(p, false).await?.and_then(|v| v.into_iter().next()))
}

/// [`pick`] for several files (or folders, where the dialog can): at least
/// one path, or `None` if cancelled.
pub async fn pick_many(p: &Pick) -> io::Result<Option<Vec<String>>> {
    ask(p, true).await
}

async fn ask(p: &Pick, many: bool) -> io::Result<Option<Vec<String>>> {
    let _deciding = aio::deciding::start();
    let p = resolved(p, &|d| std::path::Path::new(d).is_dir());
    #[cfg(target_os = "android")]
    return crate::android::pick(&p, many).await;
    #[cfg_attr(target_os = "android", allow(unreachable_code))]
    let (id, rx) = waiting();
    if crate::send_runtime(pick_line(id, &p, many)).await {
        return rx.await.map_err(io::Error::other)?.map_err(io::Error::other);
    }
    take(id);
    crate::dialog::spawned(&p, many).await
}

/// A save dialog's question (aio's `pickSave`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Save {
    /// Empty: "Save as".
    pub title: String,
    /// What to suggest: a file name (`draft.md`), a folder to open in, or
    /// both as one path (`/home/me/docs/draft.md`).
    pub suggest: Option<String>,
    /// A name and its extensions, without the dot.
    pub filters: Vec<(String, Vec<String>)>,
}

/// The path to write to, which may not exist yet; `None` if cancelled.
/// Every dialog asks before replacing a file, so a path that comes back is
/// one the user agreed to write; writing it is the caller's. As [`pick`]:
/// the window's dialog, else the desktop's tool; an error is never a cancel.
pub async fn save(s: &Save) -> io::Result<Option<String>> {
    let _deciding = aio::deciding::start();
    let title = if s.title.is_empty() { "Save as".to_string() } else { s.title.clone() };
    let p = resolved(&Pick { title, filters: s.filters.clone(), ..Pick::default() }, &|_| false);
    #[cfg(target_os = "android")]
    {
        let (_, name) = split_save(s.suggest.as_deref(), &|_| false);
        let exts: Vec<String> = p.filters.iter().flat_map(|(_, e)| e.iter().cloned()).collect();
        return crate::android::save(&p.title, &name, &exts).await;
    }
    #[cfg_attr(target_os = "android", allow(unreachable_code))]
    let (id, rx) = waiting();
    let filters: Vec<_> = p.filters.iter().map(|(name, ext)| json!({ "name": name, "extensions": ext })).collect();
    let line = json!({ "t": "save", "id": id, "title": p.title, "path": s.suggest, "filters": filters }).to_string();
    if crate::send_runtime(line).await {
        let paths = rx.await.map_err(io::Error::other)?.map_err(io::Error::other)?;
        return Ok(paths.and_then(|v| v.into_iter().next()));
    }
    take(id);
    let (start_in, name) = split_save(s.suggest.as_deref(), &|d| std::path::Path::new(d).is_dir());
    crate::dialog::spawned_save(&Pick { start_in, ..p }, &name).await
}

/// A suggestion as (the folder to open in, the name to suggest): a folder
/// that exists opens in it, naming nothing.
fn split_save(s: Option<&str>, is_dir: &dyn Fn(&str) -> bool) -> (Option<String>, String) {
    let Some(s) = s.filter(|s| !s.is_empty()) else { return (None, String::new()) };
    if is_dir(s) {
        return (Some(s.to_string()), String::new());
    }
    match s.rfind(['/', '\\']) {
        // The root keeps its separator: `/x.md` is in `/`, `C:\x.md` in `C:\`.
        Some(i) => {
            let dir = &s[..i];
            let dir = if dir.is_empty() || dir.ends_with(':') { &s[..=i] } else { dir };
            (Some(dir.to_string()), s[i + 1..].to_string())
        }
        None => (None, s.to_string()),
    }
}

/// A new dialog's id, and where its answer lands.
fn waiting() -> (u64, tokio::sync::oneshot::Receiver<Reply>) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = tokio::sync::oneshot::channel();
    WAITING.lock().unwrap_or_else(|e| e.into_inner()).push((id, tx));
    (id, rx)
}

/// What a dialog answers: the paths (at least one), `None` if cancelled.
type Reply = Result<Option<Vec<String>>, String>;

/// The dialogs still open, by id.
type Waiting = Vec<(u64, tokio::sync::oneshot::Sender<Reply>)>;
static WAITING: std::sync::Mutex<Waiting> = std::sync::Mutex::new(Vec::new());

fn take(id: u64) -> Option<tokio::sync::oneshot::Sender<Reply>> {
    let mut w = WAITING.lock().unwrap_or_else(|e| e.into_inner());
    w.iter().position(|(k, _)| *k == id).map(|i| w.swap_remove(i).1)
}

/// The runtime's answer to [`pick`]:
/// `{"t":"picked","id":1,"paths":["…"]}` (none: cancelled) or `"error":"…"`.
pub(crate) fn picked(m: &aio::serde_json::Value) {
    if let Some(tx) = m["id"].as_u64().and_then(take) {
        _ = tx.send(reply(m));
    }
}

fn reply(m: &aio::serde_json::Value) -> Reply {
    if let Some(e) = m["error"].as_str() {
        return Err(format!("the window's dialog failed: {e}"));
    }
    let paths = m["paths"].as_array().into_iter().flatten().filter_map(|p| p.as_str());
    let paths: Vec<String> = paths.filter(|p| !p.is_empty()).map(String::from).collect();
    Ok((!paths.is_empty()).then_some(paths))
}

/// `p` as every dialog takes it: a title, a folder to start in, extensions
/// without their dot, no filters on a folder pick. `is_dir` asks the disk.
fn resolved(p: &Pick, is_dir: &dyn Fn(&str) -> bool) -> Pick {
    let title = match (p.title.is_empty(), p.folder) {
        (false, _) => p.title.clone(),
        (true, true) => "Choose a folder".into(),
        (true, false) => "Choose a file".into(),
    };
    let start_in = p.start_in.as_deref().filter(|s| !s.is_empty()).map(|s| {
        let parent = std::path::Path::new(s).parent().map(|d| d.to_string_lossy().into_owned());
        if is_dir(s) { s.to_string() } else { parent.filter(|d| !d.is_empty()).unwrap_or_else(|| s.to_string()) }
    });
    let bare = |ext: &[String]| ext.iter().map(|e| e.trim_start_matches(['.', '*']).to_string()).collect();
    let filters = if p.folder { vec![] } else { p.filters.iter().map(|(n, ext)| (n.clone(), bare(ext))).collect() };
    Pick { title, folder: p.folder, start_in, filters }
}

/// The control line asking the runtime for an open dialog.
fn pick_line(id: u64, p: &Pick, many: bool) -> String {
    let filters: Vec<_> = p.filters.iter().map(|(name, ext)| json!({ "name": name, "extensions": ext })).collect();
    json!({ "t": "pick", "id": id, "title": p.title, "folder": p.folder, "many": many, "start": p.start_in, "filters": filters })
        .to_string()
}

/// `s`'s char index for UTF-16 offset `units` (the page counts text in
/// UTF-16, Rust in bytes and chars).
pub fn char_index(s: &str, units: usize) -> usize {
    let mut seen = 0;
    for (i, c) in s.chars().enumerate() {
        if seen >= units {
            return i;
        }
        seen += c.len_utf16();
    }
    s.chars().count()
}

#[cfg(test)]
mod tests {
    use aio::serde_json::json;

    /// aio's `print()`: the runtime opens the system dialog for the window on
    /// the app's "print" line, and says why a job failed, never a cancel.
    #[test]
    fn print_asks_the_runtime_for_the_windows_print_dialog() {
        let js = include_str!("main.js");
        let at = js.find("if (l === 'print'").expect("the runtime takes the print line");
        let then = &js[at..at + 240];
        assert!(then.contains("w.webContents.print({}") && then.contains("why !== 'Print job canceled'"), "{then}");
    }

    #[test]
    fn page_offsets_count_utf16() {
        assert_eq!(super::char_index("abc", 2), 2);
        // 😀 is two UTF-16 units, one char.
        assert_eq!(super::char_index("😀x", 2), 1);
        assert_eq!(super::char_index("ab", 9), 2);
    }

    #[test]
    fn a_pick_asks_the_runtime_and_its_answer_lands() {
        let p = super::Pick {
            title: "Open".into(),
            filters: vec![("Markdown".into(), vec!["md".into()])],
            ..Default::default()
        };
        assert_eq!(
            super::pick_line(7, &p, true),
            r#"{"filters":[{"extensions":["md"],"name":"Markdown"}],"folder":false,"id":7,"many":true,"start":null,"t":"pick","title":"Open"}"#
        );
        // An answer lands on its dialog, once; one for a dialog that isn't open is ignored.
        let (tx, mut rx) = tokio::sync::oneshot::channel();
        super::WAITING.lock().unwrap().push((1 << 40, tx));
        super::picked(&json!({ "t": "picked", "id": 99u64 << 40, "paths": ["/x"] }));
        super::picked(&json!({ "t": "picked", "id": 1u64 << 40, "paths": ["/a.md"] }));
        assert_eq!(rx.try_recv().unwrap(), Ok(Some(vec!["/a.md".into()])));
        assert!(super::take(1 << 40).is_none());
    }

    #[test]
    fn the_windows_answer_is_paths_a_cancel_or_an_error() {
        let r = |m| super::reply(&m);
        assert_eq!(r(json!({ "paths": ["/a b", "/c|d"] })), Ok(Some(vec!["/a b".into(), "/c|d".into()])));
        // Cancelled is `None`, never an empty list.
        assert_eq!(r(json!({ "paths": [] })), Ok(None));
        assert_eq!(r(json!({ "paths": [""] })), Ok(None));
        assert_eq!(r(json!({ "error": "boom" })), Err("the window's dialog failed: boom".into()));
    }

    #[test]
    fn a_save_suggestion_splits_into_where_to_open_and_what_to_name() {
        // aio tests/pick-save.test.ts `_splitSavePath`.
        let none = |_: &str| false;
        let s = |p: Option<&str>| super::split_save(p, &none);
        assert_eq!(s(None), (None, String::new()));
        assert_eq!(s(Some("draft.md")), (None, "draft.md".into()));
        assert_eq!(s(Some("/home/u/new.md")), (Some("/home/u".into()), "new.md".into()));
        assert_eq!(s(Some("/x.md")), (Some("/".into()), "x.md".into()));
        assert_eq!(s(Some("C:\\x.md")), (Some("C:\\".into()), "x.md".into()));
        assert_eq!(super::split_save(Some("/docs"), &|d| d == "/docs"), (Some("/docs".into()), String::new()));
    }

    #[tokio::test]
    async fn a_save_asks_the_window_for_the_suggestion_as_given_and_its_answer_is_one_path() {
        // No window here: the line is what `save` sends, the answer what it takes.
        let (id, rx) = super::waiting();
        super::picked(&json!({ "t": "picked", "id": id, "paths": ["/a/b.md"] }));
        assert_eq!(rx.await.unwrap(), Ok(Some(vec!["/a/b.md".into()])));
        let (id, rx) = super::waiting();
        super::picked(&json!({ "t": "picked", "id": id, "paths": [""] }));
        assert_eq!(rx.await.unwrap(), Ok(None), "an empty path is no choice, never ''");
        let js = include_str!("main.js");
        assert!(js.contains("showSaveDialog") && js.contains("'showOverwriteConfirmation'"));
    }

    #[test]
    fn every_dialog_gets_the_same_question() {
        let p = super::Pick {
            start_in: Some("/docs/last.md".into()),
            filters: vec![("Video".into(), vec![".mp4".into(), "*.mkv".into()])],
            ..Default::default()
        };
        let none = |_: &str| false;
        let q = super::resolved(&p, &none);
        assert_eq!(q.title, "Choose a file");
        // A file's folder; extensions bare.
        assert_eq!(q.start_in.as_deref(), Some("/docs"));
        assert_eq!(q.filters, [("Video".to_string(), vec!["mp4".to_string(), "mkv".to_string()])]);
        let folder = super::resolved(&super::Pick { folder: true, ..p.clone() }, &|d| d == "/docs/last.md");
        assert_eq!((folder.title.as_str(), folder.start_in.as_deref()), ("Choose a folder", Some("/docs/last.md")));
        assert!(folder.filters.is_empty(), "nothing to filter in a folder pick");
        let bare = super::resolved(&super::Pick { start_in: Some("x".into()), ..Default::default() }, &none);
        assert_eq!(bare.start_in.as_deref(), Some("x"));
    }
}
