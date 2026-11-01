//! `ax agent` (aio: `am agent`): the brief an agent reads before it
//! touches an aio-rs app: the rules that protect the human first, then how aio-rs
//! works and the order that works. The verbs section is made from the verb
//! table, so it lists exactly what this `ax` has. Three sizes (aio's):
//! `--min` the compact forms, the page by default, `--max` the deep sections
//! too; `--json` the same sections addressable.
use crate::verbs::VERBS;

/// One section: its slug (`ax agent <slug>`), its compact form (`--min`),
/// its whole text; `deep`: on `--max` (or by its slug) only.
struct Section {
    slug: &'static str,
    title: &'static str,
    deep: bool,
    min: &'static str,
    body: &'static str,
}

const SECTIONS: &[Section] = &[
    Section {
        slug: "rules",
        title: "RULES: protect the human, and don't burn the clock",
        deep: false,
        min: "never kill by process match (ax stop <app>) · never take over the screen ·
never script around ax (ax expect, ax state --watch, --json) · learn before
editing (this, then aio-rs's docs/README.md) · check → run → test · clean up only what you made",
        body: "1. NEVER kill by process match. NO: pkill -f, killall. YES: ax stop <app>. If it
   fails, report it; never escalate.
2. NEVER take over the human's screen. Windows an agent opens go on a nested
   display (Xvfb/Xephyr), never the human's; ax build --smoke runs headless.
3. NEVER script around ax. Assert: ax expect. Watch: ax state --watch (one
   line per change, pushed). Every verb takes --json and exits 0 ok, 1 failed,
   2 bad usage.
4. LEARN BEFORE EDITING. aio-rs is Rust (views: aio::ui), not aio's TypeScript: guessed code
   compiles to the wrong thing or not at all. Read this, then the guide (docs/README.md
   in aio-rs: every part with examples that compile; a port from aio:
   docs/migration/README.md, keeping aio's optimizations: docs/performance.md), then
   the code around.
5. check → run → test: cargo check, ax dev <app>, then tests that prove it.
6. CLEAN UP only what you made. A throwaway app: AIORT_DATA=<dir> (all its dirs
   there; same value for every ax verb). Done: ax stop, delete that dir.",
    },
    Section {
        slug: "model",
        title: "MODEL: one cell drives everything",
        deep: false,
        min: "#[cell(persist, shared)] struct = state, saved and synced · #[methods] impl =
the only writes, pure, &mut self → aio::Result · effects do the I/O · views read cells",
        body: "- A cell is a struct: #[aio::cell(persist)] saves it (state.db, an event per call),
  shared sends it to clients. Its #[methods] impl is the only way to change it:
  pure functions of (&mut self, args) → aio::Result<T>; a refusal (ensure!,
  reject!) changes nothing and says why.
- Methods do no I/O: no files, clock, randomness or threads (ax lint refuses
  them). The I/O happens around them (effects, flows) and hands results
  in as arguments.
- A call commits before it answers: nothing is ever \"not saved yet\".
- Views are aio::ui components that read cells; Handle::<Cell> calls methods.
- aio.toml is the one config; ax config <app> prints it as it runs.",
    },
    Section {
        slug: "test",
        title: "TEST: in-process, against the real cell",
        deep: false,
        min: "Handle::<Cell>::memory() (or ::open(path)) → h.call(\"method\", json!([args])) →
assert on h.state() · UI tests through aio::testing",
        body: "- A cell test opens a Handle on a temp or in-memory store, calls methods by name
  with JSON args and asserts on the state: the same path the app takes.
- Refusals are values: assert the Err, never only that it didn't panic.
- UI tests go through aio::testing (auto names from the markup), headless.
- Sync under faults: aio::testing::Sim::start(h, clients, seed) then
  run(steps, calls, holds): a seeded script of calls, cuts and heals; every
  copy must settle on the server. A failure names its seed and script.
- A test reads its fixtures with include_str!/include_bytes!, never from the
  source dir at run time.
- A failure a test expects: aio::testing::fails(|| ..) returns its message
  and leaves no trace (#[should_panic] keeps one, as for any failure).
- Hardware (USB/HID, serial): aio-rs has no device API; put the transport
  behind a trait, open the real one only outside aio::paths::in_sandbox(),
  and give the test world a simulated device the test seeds.",
    },
    Section {
        slug: "debug",
        title: "DEBUG: the order that works",
        deep: false,
        min: "read the error · ax logs · ax events (what ran) · ax state / ax expect (truth) ·
reproduce as a red test → fix → green; never widen a type or swallow an error",
        body: "1. Read the error: ax names the cause and the fix.
2. ax logs <app> · ax health <app> · ax report <app> (problem reports).
3. ax events <app> [cell] (what ran, with which args, by whom).
4. ax state <app> <cell> [path] / ax expect (what the running app holds).
5. ax call <app> <cell> <method> [arg…] (act as a user would, --as <user>).
6. Reproduce as a red test, fix, green.
NO: widen a type, delete an assertion, swallow an error, sleep-and-retry, pkill.",
    },
    Section {
        slug: "ship",
        title: "SHIP: build, package, release",
        deep: false,
        min: "ax build <app> (this OS; --list the targets) · ax keygen + ax release <app> ·
ax lint before every commit",
        body: "- ax lint [dir]: what types can't check (credentials, impure methods, hooks,
  a11y), plus clippy. Exit 1 on errors.
- ax build <app>: the app's [build] targets, else an installable app for this
  OS in dist/. --smoke starts it headless and checks its log. --sizes: the
  binary against its budget, as shipped (a plain cargo build --release is
  bigger: no ax link flags), and its crates.
- ax keygen <app>, then ax release <app> [--channel c]: signed updates.",
    },
    Section {
        slug: "new",
        title: "NEW: an app, from nothing to running",
        deep: false,
        min: "a lib crate (cells, views) + a bin crate (main) + aio.toml ([app] id, never
changed) · cargo check → ax dev <app> → tests",
        body: "- First, once per machine: ax setup (checks Rust and the C toolchain, fetches
  the runtime and dx; each missing piece is said with the command that gets
  it; exit 1 until all are there; --check only looks).
- An app is a workspace member: a lib crate with its cells and views (tests use
  it), and a small bin crate whose main boots it. aio.toml at the app's root
  is its one config ([app] id: its data's home, never changed once it ran;
  ax create sets it to the name, or --id, and refuses an installed app's;
  name, mode = standalone | client-server | client | server, version = \"M.m\":
  builds are M.m.<git commit count>, -dirty.<hash> while uncommitted).
- main: let cfg = aio::config!();  (the nearest aio.toml, parsed and checked
  as the app builds: a mistake is a compile error; no build.rs)
  let aio_shell::Start::Run(dirs) = aio_shell::start(&cfg, aio::app_version!())?
  else { return Ok(()) };  (a second launch hands over and ends here), then
  SOURCE.set(aio::Source::for_app(&cfg, &dirs)?) and aio_shell::run(App,
  aio_shell::Window { width, height, ..aio_shell::Window::of(&cfg) }, &dirs)
  (title, lang and dir from [app]).
- aio-rs's features: none = the core (cells, log, replay, the local door for
  ax call/state); kit (the UI kit, client!, router!), net (sync, its server,
  TLS, pairing, sign-in), or one part: outbound, server, tls, oidc, br, imbl.
  Keep the template's set: each other set is one more build of aio.
- No dev-dependency: aio::testing (TestUi) comes with kit. Then cargo check,
  ax dev <app>, ax test <app>.
- An app of the older layout (a launcher build.rs calling aio::config::build,
  a build-dependency on aio-rs, a \"testing\" feature) still builds, warned;
  ax lint --fix moves it to this one in one step.",
    },
    Section {
        slug: "cell",
        title: "CELL: flags, methods, effects, schedules",
        deep: false,
        min: "#[cell(persist, coalesce, shared, version, migrate, restore, save, visible, machine)] ·
fields #[transient] #[server] · methods #[client(role, allow, predict)] #[derived]
#[invariant] #[every]/#[cron] #[ttl] #[long] #[redact] #[coalesce] · cx.run(Effect) cx.after/every/at",
        body: "- #[cell(...)]: persist (saved, an event per call), coalesce (every plain
  call shown at once and written within 100 ms, aio's debounced writes: the
  default for an app's cell), shared (synced to clients),
  version + migrate = f (old data up), restore = f (repair on boot), save = f
  (what is written), visible = f (what each viewer is sent: fn(&Self, &By)
  or fn(&Self, &Caller) for their role), machine = field, diagnostics = false.
- Fields: #[transient] never saved; #[server] never sent. A shared field named
  like a credential (password, api_key, token…) must be #[server]: a compile error.
  A field (or method argument) of the app's own type: #[aio::data] on that
  struct or enum (it derives serde through aio: no serde dependency).
- Methods (#[methods] impl): &mut self, plain args → () or aio::Result<T>;
  ensure!(cond, \"why\") / reject!(\"why\") refuse and change nothing.
  #[client] lets the network call it (role = \"admin\", allow = rule,
  predict = false); #[derived] cached reads (#[derived(reads = [rows])]: kept
  while those fields are, aio's memoLast); #[invariant] refuses a call
  that breaks it; #[every(\"5m\")] / #[cron(\"0 9 * * 1\")] (skip_if_running);
  #[ttl(ms)] reuses an answer; #[long] lifts the await ceiling; #[redact]
  keeps args out of logs; #[coalesce] on a cursor/scroll/selection method
  (called per keypress): each call shows at once, the run is written once,
  100 ms after it began (aio's persistDebounceMs), at a call that can't wait
  or at close. Big state: fields in Arc or aio::imbl, so a call's copy is cheap.
- cx: cx.now(), cx.id(), cx.rand() (recorded, so replay is exact), cx.by();
  cx.run(Effect::new(\"key\", |h| async move { .. h.done(x) })) does the I/O
  after the commit and hands the result back as a call (Mode Queue | Newest |
  First, .timeout(d)); cx.after/every/at/backoff(key, ms, method, args) and
  cx.cancel(key) schedule calls, saved, surviving a restart.",
    },
    Section {
        slug: "ui",
        title: "UI: aio::ui views on cells",
        deep: false,
        min: "use aio::ui::prelude::*; (never dioxus) · aio::client!(Cell, cell) → part(|c| &c.field)
a handle (.get(), \"{n}\") re-rendering only when that field changes, act!(method(args))
calls · use_live() once at the root · data-t on what tests drive",
        body: "- use aio::ui::prelude::*; is the whole view API: rsx!, #[component], View,
  signal(|| v) (a Copy Signal: .get(), .set(v) tells only on a change,
  .update(|v| ..), .read(), .write(), \"{s}\"), memo, effect, watch(deps, f),
  once(|| ..), on_cleanup, provide/context, task/spawn, resource, Global,
  ErrorBoundary, Suspense, eval (page scripts talk to `aio`). Call signal,
  memo, effect, once.. at a view's top level. Never name dioxus (ax lint
  refuses it): air replaces the renderer behind aio::ui. Rows in a for need key:.
- aio::client!(Todo, todo) makes SOURCE/HANDLE, part(|t| &t.items) (a handle:
  .get()/.with(|v| ..); the view re-renders only when that top-level field
  changes: aio's signal per cell; pick the field, then index it), todo() (the whole state: every commit
  re-renders, so handlers only), use_status(), use_pending(method), and the
  Ui door: act!(add(title)) calls a method from a handler (a refusal is
  logged and gives None); a view's call renders at once, commits from
  effects and schedules at most once per 150 ms. use_live() once in the root.
- aio's caches: #[derived(reads = [..])] (memoLast), aio::Cache (keyed caches
  on a source: Ver::of(&state) or part(..).ver()), rows as props (equal
  props skip a render, as AIR did), kit::VirtualList for long lists.
- Views are rsx! over real HTML and CSS; aio::kit has the
  components (Button, Field, Modal, VList…). Styles: style { {aio::use_css!(
  false, &aio::kit::theme_css(hue); \"assets/style.css\")} }: false is the
  light theme (true dark), the kit's theme comes first (the kit's classes,
  aio::ui::btn included, are unstyled without it), then the app's files; a
  saved edit shows at once under ax dev, no compile. Never keep CSS or
  anything an edit should change in once(..): its value is made once, so a
  hot-patched edit never shows.
- data-t=\"name\" is an element's handle for tests and agents; without one, an
  element is named from its label and role (SaveButton, EmailInput).
- TestUi::new(App): click(t), type_text(t, s), paste(t, s) (all in one
  paste), until_text(t, s) / until_contains(t, part) (t may appear later),
  expect_cell (headless, real layout); failures() takes the refusals a
  test expects. t: a handle, an auto name, outer/t, text=Skip this version
  (the one element showing that text), or css=#id / css=.parts li:nth-child(2)
  (the one element it matches).
  Text is typed literally (\\n is no Enter: press(t, \"Enter\")); focus(t)
  clicks nothing and rings :focus-visible; present(t) counts auto names,
  has_handle(t) does not; shot() is a PNG once transitions ended, a
  :hover look only where hover() put the pointer (Mount::no_scrollbars()
  draws as headless Chromium); a click whose element moved between press
  and release fails; read(&GLOBAL) reads a global.
- e.prevent_default() does nothing in a window (the browser moved on before
  the view hears the key): put \"data-aio-prevent\": \"ArrowUp ArrowDown PageUp
  PageDown Home End Tab\" on the element whose keys the view handles (fields
  and modal dialogs inside keep theirs). The keyboard cursor's row:
  \"data-aio-reveal\": selected.then(|| i.to_string()) scrolls it into view,
  no eval per step.
- Speed: cell() in a render re-renders that component on every commit (a
  dev build says where, once: [hint]); read the field with part(|c| ..),
  and build row lists once per list (aio::Cache), not per cursor step.
- Forms: Enter submits a form with no submit button (as in aio), and one whose
  button the view enables a frame later. Held keys: a binding fires once
  unless it is movement or Chord::repeating(true). WindowEvent::MouseMove
  comes at most every 100 ms.
- Keys app-wide: aio::kit::GlobalKeys around the app, use_global_key(chord,
  f) (keys pressed with nothing focused reach it too); use_key_claim(chord,
  f) takes a chord before any element hears it (aio's capture listener).
  A chord: modifiers then a key, joined by + (ctrl, shift, alt, meta, mod =
  Ctrl or ⌘ on macOS; a character or a DOM key name: Enter, esc, space,
  ArrowUp/up, F1…): \"mod+k\", \"ctrl+shift+s\". TestUi press(t, \"ctrl+n\")
  and ax trigger <app> window press ctrl+n take the same words.
- key: counts on a for body's first node only, never inside a block (a
  for over { let …; rsx! { … } } keys nothing: every row is rebuilt when
  the list shifts; yield it instead: {xs.iter().map(|x| rsx! { Row { key:
  \"{x.id}\", … } })}); ax lint warns otherwise. To remount one element on
  a new key, a one-item for.
- The app's dirs from anywhere: aio::paths::app_dirs() (the booted ones;
  in a test, the test world's own).",
    },
    Section {
        slug: "sync",
        title: "SYNC: server, clients, who sees and calls what",
        deep: false,
        min: "mode = client-server · #[cell(shared)] synced, #[server] fields never sent ·
#[client] methods only, by role · visible = f per viewer · clients predict, the server confirms",
        body: "- A shared cell is served on GET /ws: each client gets a snapshot of its view,
  then patches. A client (aio::sync::Remote) predicts a call at once and the
  server confirms it; a refusal rolls the prediction back.
- The network may call only #[client] methods, as their role/allow say; the
  app's own code and the local door's tools (ax call) may call any.
- What a client sees: the state without #[server] fields, through
  visible = f (per user, per role, read again on each change).
- Accounts: [auth] in aio.toml; ax auth <app> users | create | role …;
  a session's role counts from its next call.",
    },
    Section {
        slug: "data",
        title: "DATA: where it lives, how it changes shape",
        deep: false,
        min: "persist → state.db in the app's data dir (ax data <app>) · an event per call,
[history] keep · version + migrate for a new shape · ax backup / restore · aio's state.db opens as is",
        body: "- A persisted cell lives in state.db in the app's data dir (ax data <app> says
  where; AIORT_DATA=<dir> moves all of it): its state, and an event per call
  kept as long as [history] keep says (ax events, ax timetravel).
- A new field with a default needs nothing. A changed shape: bump
  #[cell(version = N)] and give migrate = f (fn(Value, from) → Result<Self>);
  data that doesn't fit refuses to boot, after a backup, never reset.
- ax backup <app> [file] is safe while it runs; ax restore <app> <file>
  with the app closed (the replaced data is kept aside).
- A port of an app made with aio (Deno) opens its state.db as is
  (Source::open_aio); aio, too, still reads it: every commit also writes
  aio's aio_kv row.",
    },
    Section {
        slug: "api",
        title: "API: every export, documented",
        deep: true,
        min: "cargo doc -p aio-rs --no-deps: every module, type and fn, with its docs, as HTML
files (read <target>/doc/aio/; never --open) · aio::prelude for the app's daily names",
        body: "- cargo doc -p aio-rs --no-deps documents every export from the code itself, so
  it can't drift: it ends with Generated <target>/doc/aio/index.html, files
  to read (never --open: that is a browser on the person's screen).
  use aio::prelude::* brings cell, methods, data, Cx, Effect, Handle, By,
  Caller, Rejected, Result, ensure!/reject!.
- A plugin is a crate whose fn returns an aio::plugin::Plugin (its cells,
  routes, server fns, hooks); aio::plugin::compose(cells, http, vec![..])
  adds them under the app's: the app wins, a collision refuses the boot.
- Modules by job: testing (TestCells, TestUi), sync (Remote, serve), ui and
  kit (views), cli (command lines), config (aio.toml), auth, health, report,
  log, backup, blobs, cron, http, video.
- Who calls is one type, aio::Caller { by, role }: a #[client] rule, a
  visible = f filter and a server fn (fns::Req { who, ip, headers }, from
  Http::server_fn(F, Access, |req, args| ..)) all see it.
- Refusals are aio::Rejected { why, code } (reject!/ensure!/Rejected::new
  make one): the why names the cause and the fix; code is aio::Code
  (AccessDenied, ActionRefused, ConnectionLost) for aio-rs's own, None for the
  app's. Match on the code, never on the words.",
    },
    Section {
        slug: "cli",
        title: "CLI: an app's own command line",
        deep: true,
        min: "aio::cli::Spec + args() (typed flags, --help, did-you-mean, --json refusals) ·
fail(why, code, json) · Prompt (refuses without a terminal) · table, Style, Progress, Spinner, watch",
        body: "- let a = aio::cli::args(&Spec::new(\"todo\").command(\"add\", \"add one\")
  .rest(\"text\").flag(Flag::switch(\"json\")));  a.command, a.rest, a.on(\"json\"),
  a.text(\"url\"), a.number(\"n\"), a.list(\"tag\"), a.pos(\"id\"). --help/--version
  print and exit 0; a wrong invocation exits 2 (aio::cli::USAGE).
- fail(why, ERROR, a.json): `error: why` on stderr, or {\"error\"} on stdout.
- Prompt::std().ask/confirm/select/password refuse without a terminal: take
  the value as a flag instead.
- table(&[Column::left(\"id\")], &rows, Style::stdout()), Progress::new(n, label),
  Spinner::new(label), watch(&h.state(), |f| h.listen(f), render).",
    },
    Section {
        slug: "pitfalls",
        title: "PITFALLS: what goes wrong most",
        deep: true,
        min: "I/O in a method · a HashMap in state · a changed [app] id · a test reading files
at run time · a refusal nobody took · sleeping in a test instead of TestCells::advance",
        body: "- I/O, clocks, randomness or threads in a method: ax lint refuses them; use
  cx.now()/cx.id()/cx.rand() and effects.
- State with a HashMap: order differs between runs; use BTreeMap, or `aio::imbl` (feature `imbl`).
- Changing [app] id or a cell id: it is the data's identity; a new id is a
  new app with no data.
- A test reading files through CARGO_MANIFEST_DIR at run time: use
  include_str!/include_bytes!.
- TestCells/TestUi fail at the end on refusals nobody took: take the
  expected ones with failures().
- Waiting in real time in a cell test: TestCells::advance(ms) moves the
  schedule and the effects' clock; settle() runs effects to their end.
- Calling a method from a view without act!/Ui: the view doesn't re-render.
- A signal read in an if let / while let / match scrutinee (X.peek()…,
  X.read()) stays borrowed in its block: a write to X there panics
  (already borrowed). Bind it first: let v = X.peek().map(..); if let
  Some(id) = v { .. } (ax lint warns).
- spawn(..) ends with its component: a dialog that closes drops it (a
  toast after it never comes). Work that outlives it:
  aio::ui::spawn_forever(..), writing with try_write.
- set_data in ondragstart: the drop never sees it (the view runs after the
  browser's dragstart); put \"data-drag\": \"<id>\" on the dragged element, and
  the drop reads it with e.data_transfer().get_as_text().",
    },
];

/// The verbs section, from the table.
fn verbs() -> String {
    let w = VERBS.iter().map(|v| v.usage.len()).filter(|n| *n < 48).max().unwrap_or(0) + 4;
    let rows = VERBS.iter().map(|v| {
        let about = v.about.lines().next().unwrap_or("");
        if v.usage.len() < 48 { format!("ax {:<w$}{about}", v.usage) } else { format!("ax {}\n    {about}", v.usage) }
    });
    rows.collect::<Vec<_>>().join("\n")
}

/// How much of the brief.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Size {
    Min,
    Page,
    Max,
}

impl Size {
    /// `--min` or `--max` (or neither); both is a contradiction, refused.
    pub fn of(min: bool, max: bool) -> Result<Self, String> {
        match (min, max) {
            (true, true) => Err("pick one size: --min, --max, or neither (the page)".into()),
            (true, false) => Ok(Size::Min),
            (false, true) => Ok(Size::Max),
            (false, false) => Ok(Size::Page),
        }
    }
}

/// The brief at `size`, or the one section `task` names (in full unless
/// `--min`); `list` the slugs; `json` the sections as data.
pub fn brief(task: Option<&str>, size: Size, list: bool, json: bool) -> Result<String, String> {
    let slugs: Vec<&str> = SECTIONS.iter().map(|s| s.slug).chain(["verbs"]).collect();
    if list {
        let rows = SECTIONS
            .iter()
            .map(|s| format!("  {:<8}  {:<4}  {}", s.slug, if s.deep { "deep" } else { "page" }, s.title));
        let verbs = format!("  {:<8}  {:<4}  VERBS: every ax command", "verbs", "page");
        return Ok(rows.chain([verbs]).collect::<Vec<_>>().join("\n"));
    }
    if let Some(t) = task
        && !slugs.contains(&t)
    {
        return Err(format!("no section `{t}`: {} (ax agent --list)", slugs.join(", ")));
    }
    let picked = |slug: &str, deep: bool| task.map_or(!deep || size == Size::Max, |t| t == slug);
    let mut parts: Vec<(&str, String, String)> = SECTIONS
        .iter()
        .filter(|s| picked(s.slug, s.deep))
        .map(|s| (s.slug, s.title.to_string(), (if size == Size::Min { s.min } else { s.body }).to_string()))
        .collect();
    if picked("verbs", false) {
        parts.push(("verbs", "VERBS: every ax command (ax help <verb>: one in full)".into(), verbs()));
    }
    if json {
        let sections: Vec<_> = parts
            .iter()
            .map(|(slug, title, body)| serde_json::json!({ "task": slug, "title": title, "body": body }))
            .collect();
        let size = format!("{size:?}").to_lowercase();
        return Ok(
            serde_json::json!({ "version": env!("CARGO_PKG_VERSION"), "size": size, "sections": sections }).to_string()
        );
    }
    let mut out = vec![format!("# aio-rs {} for agents (ax agent --list: the sections)", env!("CARGO_PKG_VERSION"))];
    out.extend(parts.iter().map(|(_, title, body)| format!("## {title}\n\n{body}")));
    Ok(out.join("\n\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(size: Size) -> String {
        brief(None, size, false, false).unwrap()
    }

    #[test]
    fn the_brief_opens_with_the_rules_and_lists_every_verb() {
        let page = all(Size::Page);
        let rules = page.find("## RULES").unwrap();
        let shown = SECTIONS.iter().skip(1).filter(|s| !s.deep);
        assert!(shown.clone().all(|s| page.find(&format!("## {}", s.title)).unwrap() > rules));
        assert!(page.contains("NEVER kill by process match") && page.contains("## NEW") && page.contains("## SYNC"));
        // Made from the table: it can't miss or invent a verb.
        for v in VERBS {
            assert!(page.contains(&format!("ax {}", v.usage)), "{}", v.name);
        }
        let min = all(Size::Min);
        assert!(min.len() < page.len() && min.contains("never kill by process match"));
    }

    #[test]
    fn three_sizes_the_deep_sections_on_max_only() {
        let (page, max) = (all(Size::Page), all(Size::Max));
        for s in SECTIONS.iter().filter(|s| s.deep) {
            let head = format!("## {}", s.title);
            assert!(!page.contains(&head) && max.contains(&head), "{}", s.slug);
        }
        assert!(max.len() > page.len() && max.contains("## PITFALLS") && max.contains("## CLI"));
        assert_eq!(Size::of(true, false), Ok(Size::Min));
        assert_eq!(Size::of(false, false), Ok(Size::Page));
        assert!(Size::of(true, true).unwrap_err().contains("pick one size"));
    }

    #[test]
    fn one_section_or_the_list() {
        let one = brief(Some("debug"), Size::Page, false, false).unwrap();
        assert!(one.contains("## DEBUG") && !one.contains("## RULES") && !one.contains("## VERBS"));
        // A deep section by its slug, at any size.
        assert!(brief(Some("pitfalls"), Size::Page, false, false).unwrap().contains("## PITFALLS"));
        assert!(brief(Some("verbs"), Size::Page, false, false).unwrap().contains("ax state <app>"));
        let e = brief(Some("nope"), Size::Page, false, false).unwrap_err();
        let all = "rules, model, test, debug, ship, new, cell, ui, sync, data, api, cli, pitfalls, verbs";
        assert!(e.contains(&format!("no section `nope`: {all}")), "{e}");
        let list = brief(None, Size::Page, true, false).unwrap();
        assert_eq!(list.lines().count(), SECTIONS.len() + 1);
        assert!(
            list.lines().next().unwrap().starts_with("  rules ") && list.contains("  cli       deep  CLI"),
            "{list}"
        );
    }

    #[test]
    fn json_has_the_same_sections_addressable() {
        let v: serde_json::Value = serde_json::from_str(&brief(None, Size::Min, false, true).unwrap()).unwrap();
        assert_eq!(v["size"], "min");
        let tasks: Vec<&str> = v["sections"].as_array().unwrap().iter().map(|s| s["task"].as_str().unwrap()).collect();
        assert_eq!(tasks, ["rules", "model", "test", "debug", "ship", "new", "cell", "ui", "sync", "data", "verbs"]);
        assert_eq!(v["sections"][0]["body"], SECTIONS[0].min);
        let one: serde_json::Value =
            serde_json::from_str(&brief(Some("cli"), Size::Max, false, true).unwrap()).unwrap();
        assert_eq!(
            (one["sections"].as_array().unwrap().len(), &one["sections"][0]["task"]),
            (1, &serde_json::json!("cli"))
        );
    }
}
