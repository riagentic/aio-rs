//! The verb table (aio: `am-flags.ts` + `am-help-text.ts` in one): each
//! verb's usage line is the one source of its help, the flags it takes and
//! how many words it reads. One gate reads it before any verb runs: an
//! unknown or misplaced flag, a missing value or a word too many is refused
//! with the one right answer (exit 2), never dropped or read as a value.

/// One verb: `usage` after `ax ` (`<x>` needed, `[x]` optional, `…` any
/// number, `[--flag]` a switch, `[--flag v]` a flag with a value) and what it
/// does (lines after the first are indented under it).
pub struct Verb {
    pub name: &'static str,
    pub usage: &'static str,
    pub about: &'static str,
}

const fn v(name: &'static str, usage: &'static str, about: &'static str) -> Verb {
    Verb { name, usage, about }
}

pub const VERBS: &[Verb] = &[
    v(
        "create",
        "create <name> [--template t] [--id id] [--aio dir] [--force]",
        "a new app in ./<name> that runs, tests and builds as it is (its own
workspace, aio.toml, the agent brief's pointer); --template app (a
window, the default), cli (a command-line tool) or service (served, no
window); --id: its id (its data's home; default: the name, refused when an
installed app has it); --aio: the aio-rs checkout it builds on (default: this ax's)",
    ),
    v(
        "dev",
        "dev <app> [args…]",
        "run an app; edits hot-patch in (with dx 0.7.10), else rebuild + restart;
args go to the app (--instance <n>: a private copy; --fresh: on new data);
--display auto|isolated|current|:N: with no terminal (an agent) the window
goes on a nested display, not the person's, and opens no browser tab (auto);
--size 390x844: the window at that size for this run (its saved one untouched);
--watch=false: build and run it once; --watch=src/ui,assets: rebuild
on changes there only (dirs of the app);
--cdp[=port]: its window opens the DevTools protocol on loopback (a free
port, for other tools; ax shot, eval, surface and trigger need none);
--tui: dx's own screen in a terminal (its keys), not ax's compact lines
(dx's whole output: logs/dev.log);
--android [--device serial] [--native]: on the attached emulator or phone
(ANDROID_SERIAL; none: AIO_AVD booted): its web build in a dev APK installed
once beside the app, each edit reloading it (--native, or no web build: the
app itself for that CPU, again on every saved change); its log in the terminal;
--web: its web build (debug) served on loopback, its tabs live: a saved
stylesheet shows at once, any other edit builds anew and the tabs reload",
    ),
    v(
        "keygen",
        "keygen <app> [--rotate] | keygen [--aio] (aio-rs's own key; --rotate too)",
        "the app's release signing key (in $XDG_CONFIG_HOME/aio-rt/keys, else ~/.config/aio-rt/keys) and its aio.toml lines;
--aio: aio-rs's own update key, which signs the SHA256SUMS of ax release --aio",
    ),
    v(
        "release",
        "release <app> [--channel c] [--notes t] [--windows] [--android] [--appimage] [--platforms a,b] [--patch-from versions] [--dir d] [--data file] [--no-build] [--allow-dirty] [--unsigned] [--workflow] | release [--aio] [--url u] (aio-rs itself; --windows, --dir too)",
        "build, sign and lay out a release in dist/<channel>/ for [updates] url, with patches from earlier ones;
--platforms: one for each (as ax build; --windows: windows); --android: its signed APKs, offered to open on the device; --appimage: the AppImage, which an app run from one replaces itself with;
--dir: in d/<channel>/; --data: what it reads, from a contract file, not asked of the build;
--no-build: the bundles ax build left in dist/; --allow-dirty: a build of uncommitted edits too; --unsigned: signed by a key made for it alone, no ax keygen needed (to look at locally; no app takes it); --workflow: the GitHub workflow that does it on a tag;
--aio (in an aio-rs checkout): aio-rs itself prebuilt for this machine (--windows: Windows too) in dist/aio-rs/v<version>/ (--dir: in d), with install.sh / install.ps1 pinning each archive's SHA-256 (archives a Mac made, put in d first, too) and SHA256SUMS of every file in it; each signature made when its keys are set, else said unsigned (CONTRIBUTING.md: AIO_WINDOWS_CERT, AIO_MACOS_SIGN_IDENTITY, AIO_RELEASE_KEY or keygen --aio); --url: where they will be downloaded from (default: the GitHub release of github.com/riagentic/aio-rs tagged v<version>)",
    ),
    v(
        "update",
        "update <app> [--from-git <ref>]",
        "the installed app's versions and update state; --from-git: build branch or tag <ref> of its git source (or of its checkout) and stage it for its next start, UNSIGNED (a developer's path: never the update checks)",
    ),
    v("installed", "installed", "the apps installed from a bundle on this machine"),
    v(
        "remove",
        "remove <app> [--data]",
        "take an installed app away (not while it runs); its data stays, unless --data",
    ),
    v(
        "smoke",
        "smoke <artifact>",
        "start a window app's Linux bundle, .deb or AppImage headless as a user would,
then stop it: its boot time, its window's memory (PSS of the app and its
runtime), a clean stop, nothing left running (as ax build --smoke does)",
    ),
    v(
        "build",
        "build <app> [--targets a,b] [--platforms a,b] [--list] [--sizes] [--smoke] [--deb] [--appimage] [--android] [--debug] [--windows] [--web] [--dev]",
        "the app's [build] targets (aio.toml), else an installable app for
this OS: dist/<id>-<version>-<os>-<arch>.* (Linux .tar.gz, macOS
zipped .app, Windows one-click .exe + zip; --windows: cross-built from Linux);
--targets: some of them, or kinds; --list: the kinds and targets;
--platforms ([build] platforms): each desktop target for each of linux,
linux-arm64, windows, windows-arm64, macos, macos-arm64 or host;
--sizes: the release binary against its budget, packed nowhere, and which
crates take its space (over budget: always); size it so, not with a plain
`cargo build --release` (no ax link flags: some % bigger);
--smoke ([build] smoke: always): then start each Linux bundle, .deb
and AppImage headless, stop it, check its log; a Windows bundle on
Windows (AIO_WINDOWS_SSH: one over ssh), stopped with <app> --quit;
--deb: a Debian package ([app] maintainer); --appimage: one portable file;
--android: signed APKs instead (the app's lib calls android_app!; --debug:
debug APKs, the page in chrome://inspect);
--web: an installable web app (PWA) in dist/<id>-web/ instead: the app's
lib for wasm32, a manifest, Apple tags, icons and an offline service worker
(--web --dev: a debug build, its page showing its errors on it)",
    ),
    v(
        "lab",
        "lab <app> <os> [--stop]",
        "the app on a real windows, macos or linux machine's desktop, for a
person to click around in: the machine is AIO_LAB_WINDOWS, AIO_LAB_MACOS
or AIO_LAB_LINUX ('[ssh options] [user@]host'); built for it, copied over
ssh into aio-rs-test/lab/<id> there and started with its data, runtime
and caches inside that folder; --stop: what runs from it ended, the
folder removed",
    ),
    v(
        "install",
        "install <app> [--android] [--build] [--emulator] [--device serial] [--apk file] [--no-launch]",
        "build this OS's bundle from source and install it for this user (Linux: its
install.sh run, nothing left aside; macOS, Windows: how to); --android:
the newest APK for the attached phone's CPU onto it, then
start it; refuses an APK older than the sources (--build: build first);
--emulator: an emulator may be the device; --device (else ANDROID_SERIAL): that one",
    ),
    v(
        "lint",
        "lint [dir] [--json] [--no-hints] [--no-clippy] [--safe-fix]",
        "what types can't check: credentials, impure methods, hooks,
a11y, cell ids, plus clippy (exit 1: errors; `ax lint --help`)",
    ),
    v(
        "test",
        "test <app> [filter] [--video out] [--pace ms]",
        "the app's tests (cargo test); --video records each UI test, no test
changed: out a folder (a video per test) or one .mp4/.webm, a frame per
step, each --pace ms (800) on screen",
    ),
    v("apps", "apps", "the aio-rs apps and private instances on this machine, and which run"),
    v(
        "ui",
        "ui [--window] [--browser]",
        "the visual app manager: the apps on this machine, each
one's status, start and stop, memory and CPU, state, logs, metrics,
components and calls; opened detached (--window: here, until it closes;
--browser: in the browser, served from here on a loopback port behind a
key made for the run, until Ctrl-C)",
    ),
    v("open", "open <app> [--print]", "the running app's page in the browser, once it answers (--print: its address)"),
    v("data", "data <app>", "where the app keeps everything, and what to back up"),
    v("config", "config <app>", "the app's aio.toml as its build bakes it in, defaults in"),
    v(
        "events",
        "events <app> [cell] [--last N] [--diff] [--from file]",
        "the newest events (default 20), oldest first; --diff: each with the
changes it made (one cell, the app running); --from: of another store
or backup file",
    ),
    v(
        "state",
        "state <app> <cell> [path] [--watch] [--as user]",
        "a cell's state, live from the running app (as user, default
you), else its newest stored snapshot; path: keys, indexes,
* (notes.*.text), filters (notes[?done=true].text, [?k], [?k!=v]),
picks (owner.{id,name}); --watch: a line now and after each change",
    ),
    v("snapshot", "snapshot <app> [--as user]", "every cell's state as one JSON object (live, else stored)"),
    v(
        "snapshot save",
        "snapshot save <app> [file] [--as user]",
        "the snapshot as a new file (default <id>-snapshot.json)",
    ),
    v(
        "snapshot load",
        "snapshot load <app> <file>",
        "put a snapshot file's cells in place on the running app, each as a
new event (ax timetravel goto steps back); a snapshot taken --as
someone holds only what they saw",
    ),
    v(
        "replay",
        "replay <app> [range] [--from file] [--dry]",
        "call a log's inputs again on the running app, in order, to
reproduce a bug (range: N, N..M, N.., ..M; --from: another store or
backup); what they caused (schedules, effects) the app makes again;
--dry: what would be sent",
    ),
    v(
        "record",
        "record <app> <cell> <file> [range] [--from file] [--type path]",
        "a Rust test from what the running app's cell just did: booted on the
state before the range, its calls made again, the state they left
asserted (--type: the cell's type, default <crate>::<Cell>)",
    ),
    v(
        "migrations",
        "migrations <app>",
        "each stored cell's version against the running app's, and its shape
drift: fields stored but no longer declared, or of another type",
    ),
    v(
        "call",
        "call <app> <cell> <method> [arg…] [--args json] [--as user]",
        "call a method of the running app, print its answer; each arg
is JSON or text; --args '[..]' exact (@file, - for stdin)",
    ),
    v(
        "expect",
        "expect <app> <cell> <path> <op> [value] [--wait s] [--as user]",
        "assert on the live state (exit 1: it fails); op: eq ne gt gte lt
lte contains exists absent; --wait: until it passes, s seconds at most",
    ),
    v(
        "clients",
        "clients <app> [cell]",
        "the running app's windows (their --window number, title and page), or
the links open to its cell",
    ),
    v(
        "shot",
        "shot <app> [file] [--cdp port] [--window n] [--check base] [--update base] [--threshold n] [--max-diff r] [--selector css] [--full] [--size WxH] [--video file] [--duration s] [--markers]",
        "a PNG of the app's window (default <id>-<time>.png), over the DevTools
protocol (a dev build's own door: no flag; --cdp port, else AIORT_CDP,
else the port a launch with --cdp opened, when there is one);
--check: exit 1 unless it looks like base (a channel may move --threshold,
default 2; a share --max-diff of pixels may differ, default 0);
--update: record base; --selector: one element; --full: past the viewport;
--size 390x844: the page laid out at that size for the shot, then as it was;
--window: which of its windows (0, the first);
--video file.mp4|.webm: record the window until Ctrl-C (or --duration
seconds), encoded by ffmpeg (none: by the window itself, slower);
--markers: each ax trigger meanwhile in
file.markers.json, [{t, path, action, rect}] (ms, video px)",
    ),
    v(
        "eval",
        "eval <app> <code> [--cdp port] [--window n]",
        "JavaScript in the app's window, its value as JSON: an expression, or
statements and the last one's value ('const f = …; f(2)', top-level await),
as the DevTools console; promises awaited; a DOM node summarised; a throw
fails: geometry, computed styles, a fetch from the page's origin;
--window: which one (0, the first);
the window's DevTools protocol as for ax shot",
    ),
    v(
        "components",
        "components <app> [--renders] [--wait-client ms]",
        "the running app's mounted components, nested, each with how many
times it rendered and the time those renders took (aio DevTools' tree
and profile); --renders: its last 200 renders as they happened (aio
DevTools' render events); --json: all of it as JSON. A dev build's, through
its own door",
    ),
    v(
        "surface",
        "surface <app> [path] [--names] [--rects] [--headless] [--root c] [--cdp port] [--window n] [--wait-client ms]",
        "what a person can see and use in the app's window, one line each,
nested: role \"name\" [handle] state, named as TestUi names them (a
data-t handle, else SaveButton, EmailInput, SaveButton2…); path: that
element and what is under it (row-2, list/row-3; none there: exit 1);
--names: the auto names, in order;
--rects: where each is, @x,y wxh in CSS px; --wait-client: wait so long
for the window (right after a start or reload); --window, the protocol:
as ax eval; --headless: with no window open (a server), the app's views
(--root, default App) mounted headless on its running cells, built by
cargo as its tests are",
    ),
    v(
        "preview",
        "preview <app> <component> [props] [--html]",
        "one component of the app mounted alone with the props given, as Rust
fields ('name: \"Ada\".into(), times: 2'), and what it shows: its
surface, or its HTML; component: its path in the app's crate (ui::Card);
type-checked and rendered as the app's tests are, by cargo",
    ),
    v(
        "trigger",
        "trigger <app> <path> <action> [text] [--cdp port] [--window n] [--wait-client ms]",
        "act on the element at path as a person does, with real mouse and
keys: click dblclick hover focus blur, type <text> (appends), setValue
<text> (replaces), clear, press|keyDown|keyUp <key> (Enter, ctrl+k, mod+s),
select <value>, check uncheck, scroll <top[,left]>, dragTo <path>; path
window: a key to the window itself (a global key); --wait-client,
--window, the protocol: as ax surface",
    ),
    v(
        "timetravel",
        "timetravel <app> <cell> [step] [seq]",
        "where the running cell is in its history, its newest calls and their
time; step: undo, redo, goto <seq> (the numbers ax events lists; each
pauses the cell: calls refused, schedules held), pause, resume. Every
step is a new event: none is lost",
    ),
    v(
        "doctor",
        "doctor <app>",
        "is the running app the code on disk (version, build, edits since), and
who decided each setting (config, dirs, environment)",
    ),
    v(
        "heap",
        "heap <app> [--watch]",
        "what the app holds: its memory, each cell's state and events;
--watch: again every 2 s (a top view)",
    ),
    v("schedules", "schedules <app>", "saved schedules, soonest first"),
    v(
        "sql",
        "sql <app> [query…] [--tables]",
        "a read-only query of the store, as JSON rows (at most 10000);
--tables: the store's tables",
    ),
    v("tables", "tables <app>", "the store's tables"),
    v(
        "logs",
        "logs <app> [word] [--last N] [--level l] [--tag t] [--since s] [--client] [--follow]",
        "the newest log events (default 50), oldest first: those holding
word, at level l or worse, of category t (and t:*), newer than s
(15m, 2h, 2026-10-28T10:00); --client: the window's lines;
--follow (-f): then each new one; --json: counted",
    ),
    v(
        "metrics",
        "metrics <app> [--as user]",
        "uptime, open links, schedules and each cell's size (live when the
app runs, else stored)",
    ),
    v(
        "peer",
        "peer <app> [--as user] [--role r]",
        "what a caller (user, role) may call of the running app over the
network: each method's Rust signature and what its #[client] rule says",
    ),
    v(
        "cost",
        "cost <app> [cell] [--keys]",
        "what each cell of the running app sent its clients (pushes, bytes per
second, mean, full resends) and its methods' p95 time; its top keys by
bytes (--keys: all of them)",
    ),
    v(
        "errors",
        "errors <app> [--last N]",
        "ax dev's failed build, then the newest warnings and errors
(default 20), each under the start of the app it came from",
    ),
    v("health", "health <app>", "healthy, or why not (exit 1: degraded, stuck or not running)"),
    v(
        "status",
        "status <app> [app…]",
        "started (exit 0), starting, stopping, maintenance (exit 2), stopped
(exit 1; a crash or kill says so); one line an app",
    ),
    v(
        "stop",
        "stop <app> [app…] [--all] [--wait s]",
        "ask the app to close (any OS) and wait until it is gone; still there
after s (15) seconds: SIGTERM, then SIGKILL; --all: every app of this
workspace",
    ),
    v(
        "start",
        "start <app> [arg…] [--wait s] [--fresh] [--display d] [--cdp] [--all]",
        "run the app in the background (built first in its workspace, else
the installed one) and wait until it is up, at most s (30) seconds; args
after -- go to the app; its output: logs/start.log; --fresh, --display,
--cdp: as ax dev; --all: every app of this workspace (its components)",
    ),
    v(
        "restart",
        "restart <app> [arg…] [--wait s] [--cdp] [--all]",
        "stop it if it runs, then start it again with the flags it was
started with (a flag given here replaces its recorded one); --all:
every app of this workspace",
    ),
    v("kill", "kill <app>", "end the app now, no asking (its pid from its health.json)"),
    v("report", "report <app> [id]", "the app's problem reports, newest first; one in full"),
    v(
        "setup",
        "setup [--check]",
        "what making apps needs: checks Rust and the C toolchain, fetches the
shared runtime and dx (ax dev's hot-patching) prebuilt and checked, and
lists the optional tools; each missing one with the command that gets it;
exit 1 while a needed one is missing; --check: look only, fetch nothing",
    ),
    v("runtime", "runtime [--fetch]", "where the shared runtime is (fetch it if missing); --prune: ax prune"),
    v("pin", "pin <app> [--plain]", "a pairing code from the running app (8 digits; 6 with --plain)"),
    v("pair", "pair <app> <host:port> <code>", "pair this device with an app's server"),
    v(
        "prune",
        "prune [--yes] [--days N] [--keep a,b]",
        "what of the shared runtime store may go (also: ax runtime --prune):
what no installed app names and nothing started from in N (30) days;
the one this ax ships, --keep ones and unknown ones stay; --yes (-y)
deletes what the plan names",
    ),
    v(
        "theme adopt",
        "theme adopt <app> [--force]",
        "aio-rs's theme into the app as assets/aio-theme.css, its own file from
then on (no upgrade moves its look); --force: write over it",
    ),
    v(
        "trust",
        "trust [path]",
        "this machine's root, and how to make browsers trust it (it explains;
it installs nothing); path: the file alone",
    ),
    v(
        "auth",
        "auth <app> <do> [id] [value] [--role r] [--email e] [--password p]",
        "the app's accounts, running or not: users · create <id> · passwd <id>
· role <id> <role> · totp <id> off · unlock <id> · verify <id> · revoke <id>
· rm <id> (no --password: one is made and shown once)",
    ),
    v(
        "profile",
        "profile <app> [--out file]",
        "the running app's profile (.aioapp): host, port, root, key: a
credential (--out writes it 0600); hand it to a client",
    ),
    v("import", "import <app> <file>", "link this device to an app by its profile, as pair does"),
    v("backup", "backup <app> [file]", "copy the app's data (safe while it runs)"),
    v("restore", "restore <app> <file>", "put a backup back (the app must be closed; the data it replaces is kept)"),
    v("discover", "discover", "the aio-rs apps on this LAN that answer ([server] discover = true)"),
    v("feedback", "feedback [app] [--create]", "where findings about aio-rs go; one app's file (--create: make it)"),
    v("agent", "agent [section] [--list] [--min] [--max]", "the brief for agents: the rules first, then how aio-rs works"),
    v("help", "help [verb]", "this, or one verb in full"),
    v("version", "version", "the aio-rs version of this ax"),
];

/// Verbs that moved or aren't there, and what to do instead.
const MOVED: &[(&str, &str)] = &[
    ("dispatch", "ax calls a method with `ax call <app> <cell> <method> [arg…]`"),
    ("prune versions", "ax has no version store to prune: an app pins aio-rs in its Cargo.lock"),
    ("theme", "ax theme adopt <app> [--force]: aio-rs's theme into the app, as its own file"),
];

/// Other spellings of a verb: these words, then this flag, read as that
/// verb (`ax runtime --prune --yes` is `ax prune --yes`).
const ALIASES: &[(&str, &str, &str)] = &[("runtime", "--prune", "prune")];

/// A verb's one-dash short flags, and the flag each stands for.
const SHORT: &[(&str, &str, &str)] = &[("logs", "-f", "--follow"), ("prune", "-y", "--yes")];

/// A verb that names no app when given this flag (`stop --all`).
const NO_APP_WITH: &[(&str, &str)] =
    &[("stop", "--all"), ("start", "--all"), ("restart", "--all"), ("release", "--aio"), ("keygen", "--aio")];

/// Flags every verb takes.
const GLOBAL: &[&str] =
    &["--help", "--version", "--json", "--app <name>", "--profile <name>", "--instance <name>", "--home <dir>"];

/// Flags that pick where the app runs (its home), and the verbs about an
/// app that take none: they work on its source, not a running copy.
const HOMED: [&str; 4] = ["--app", "--profile", "--instance", "--home"];
const UNHOMED: [&str; 9] =
    ["dev", "keygen", "release", "update", "build", "config", "remove", "install", "theme adopt"];

const FOOT: &str = "Inside an app's folder <app> may be left out; --app <name> names it anywhere.
<app>@<profile>, --profile, --instance, --home: that copy of the app (as its launch takes them);
asked for none, and the app runs from exactly one other home, the verbs follow it.
--json: one JSON object on stdout ({\"ok\":…}); exit 0 ok, 1 failed, 2 bad usage.
AIORT_DATA=<dir> points at one app's dirs instead of the tool home.";

pub fn find(name: &str) -> Option<&'static Verb> {
    VERBS.iter().find(|v| v.name == name)
}

/// What one usage line says.
#[derive(Debug, PartialEq)]
pub struct Shape {
    /// Flag names, and whether each takes a value.
    pub flags: Vec<(&'static str, bool)>,
    /// The words: needed, optional, and whether any number may follow.
    pub need: usize,
    pub optional: usize,
    pub any: bool,
    /// Whether the first word is `<app>`.
    pub app: bool,
}

pub fn shape(usage: &'static str) -> Shape {
    let mut s = Shape { flags: vec![], need: 0, optional: 0, any: false, app: false };
    // After the verb's own name (`snapshot save`).
    let mut words = usage.split(' ').skip_while(|w| !w.starts_with(['<', '['])).peekable();
    while let Some(w) = words.next() {
        if let Some(f) = w.strip_prefix("[--") {
            let name = &w[1..w.len() - usize::from(f.ends_with(']'))];
            let valued = !f.ends_with(']');
            if valued {
                words.next();
            }
            s.flags.push((name, valued));
        } else if w.contains('…') {
            s.any = true;
        } else if w.starts_with('<') {
            s.app |= s.need == 0 && s.optional == 0 && w == "<app>";
            s.need += 1;
        } else {
            s.optional += 1;
        }
    }
    s
}

fn global(name: &str) -> Option<bool> {
    GLOBAL.iter().find(|g| g.split(' ').next() == Some(name)).map(|g| g.contains(' '))
}

/// What the command line asks for.
#[derive(Debug, PartialEq)]
pub enum Ask {
    /// Print this (help, version).
    Say(String),
    /// Run the verb with these args: `--x=v` split, `--app` and the app of
    /// the folder put in as the first word.
    Run(Vec<String>),
}

fn version() -> String {
    format!("aio-rs {}", env!("CARGO_PKG_VERSION"))
}

/// The whole help.
pub fn help() -> String {
    let mut out = String::from("ax — the aio-rs command\n\n");
    for v in VERBS {
        let usage = format!("ax {}", v.usage);
        let about: Vec<&str> = v.about.lines().collect();
        let rest = if usage.len() < 34 {
            out.push_str(&format!("  {usage:<34}{}\n", about[0]));
            &about[1..]
        } else {
            out.push_str(&format!("  {usage}\n"));
            &about[..]
        };
        for l in rest {
            out.push_str(&format!("{:36}{l}\n", ""));
        }
    }
    out.push('\n');
    out.push_str(FOOT);
    out
}

/// One verb in full.
pub fn verb_help(v: &Verb) -> String {
    let s = shape(v.usage);
    let own: Vec<_> = s.flags.iter().map(|(f, _)| *f).collect();
    let takes = match (own.is_empty(), s.any) {
        // Its flags are among its args (`dev <app> [args…]`): those its text names.
        (true, true) => "the flags named above (the rest of its args go on as they are)".into(),
        (true, false) => "no flags of its own".into(),
        _ => own.join(" "),
    };
    format!(
        "ax {}\n\n{}\n\n{} takes {takes}; every verb also: {}\n{FOOT}",
        v.usage,
        v.about.lines().map(|l| format!("  {l}")).collect::<Vec<_>>().join("\n"),
        v.name,
        GLOBAL.join(" ")
    )
}

/// Read the command line: help and version wherever asked, every flag
/// known to its verb, every word counted. `here` names the app of the
/// folder (asked only when `<app>` is left out). `Err` is bad usage.
pub fn read(args: &[String], here: &dyn Fn() -> Result<String, String>) -> Result<Ask, String> {
    if let Some(first) = args.first()
        && let Some((_, flag, to)) = ALIASES.iter().find(|(v, f, _)| v == first && args.iter().any(|a| a == f))
    {
        let rest = args[1..].iter().filter(|a| a != flag).cloned();
        return read(&std::iter::once(to.to_string()).chain(rest).collect::<Vec<_>>(), here);
    }
    // A two-word verb (`snapshot save`) reads as one word.
    if let [first, second, rest @ ..] = args {
        let two = format!("{first} {second}");
        if find(&two).is_some() || MOVED.iter().any(|(m, _)| *m == two) {
            return read_one(&[vec![two], rest.to_vec()].concat(), here);
        }
    }
    read_one(args, here)
}

fn read_one(args: &[String], here: &dyn Fn() -> Result<String, String>) -> Result<Ask, String> {
    let Some(first) = args.first() else { return Ok(Ask::Say(help())) };
    let first = first.as_str();
    // Up to a bare `--`; after it every word is a word.
    let end = args.iter().position(|a| a == "--").unwrap_or(args.len());
    let asked = |f: &[&str]| args[..end].iter().any(|a| f.contains(&a.as_str()));
    if let Some((_, to)) = MOVED.iter().find(|(m, _)| *m == first) {
        return Err((*to).into());
    }
    if matches!(first, "--version" | "-V") || (asked(&["--version", "-V"]) && first != "dev") {
        return Ok(Ask::Say(version()));
    }
    if matches!(first, "--help" | "-h") {
        return Ok(Ask::Say(help()));
    }
    let Some(verb) = find(first) else {
        let hint = aio::cli::near(first, VERBS.iter().map(|v| v.name))
            .map_or(String::new(), |n| format!(" (did you mean {n}?)"));
        return Err(format!("unknown command: {first}{hint}\n  ax help lists them"));
    };
    let see = format!("\n  ax {}\n  ax help {}", verb.usage, verb.name);
    // `dev` hands what follows the app to the app; `lint` reads its own.
    let own = match verb.name {
        "dev" => args.len().min(2),
        "lint" => return Ok(Ask::Run(args.to_vec())),
        _ => end,
    };
    if args[1..own].iter().any(|a| a == "--help" || a == "-h") {
        return Ok(Ask::Say(verb_help(verb)));
    }
    if verb.name == "help" {
        return match args.get(1) {
            None => Ok(Ask::Say(help())),
            Some(_) => {
                let name = args[1..own].join(" ");
                find(&name).map(|v| Ask::Say(verb_help(v))).ok_or_else(|| format!("no verb `{name}`"))
            }
        };
    }
    let s = shape(verb.usage);
    let mut out = vec![first.to_string()];
    let mut words: Vec<String> = vec![];
    let mut app: Option<String> = None;
    let mut i = 1;
    while i < own {
        // A short flag reads as the one it stands for.
        let a = &SHORT
            .iter()
            .find(|(v, f, _)| *v == verb.name && *f == args[i])
            .map_or_else(|| args[i].clone(), |(.., l)| l.to_string());
        i += 1;
        let (name, inline) = a.split_once('=').map_or((a.as_str(), None), |(f, v)| (f, Some(v.to_string())));
        if !name.starts_with("--") {
            // `-x` is a flag nobody has (but `-3` and `-` are words).
            if verb.name != "call"
                && a.len() > 1
                && a.starts_with('-')
                && a[1..].starts_with(|c: char| c.is_ascii_alphabetic())
            {
                return Err(format!("unknown flag {a}: ax's flags are --words{see}"));
            }
            words.push(a.clone());
            continue;
        }
        let valued = match s.flags.iter().find(|(f, _)| *f == name) {
            Some((_, valued)) => *valued,
            None => match global(name) {
                Some(valued) => valued,
                None => {
                    let pool = s.flags.iter().map(|(f, _)| *f).chain(GLOBAL.iter().filter_map(|g| g.split(' ').next()));
                    let hint = aio::cli::near(name, pool).map_or(String::new(), |n| format!(" (did you mean {n}?)"));
                    let own: Vec<_> = s.flags.iter().map(|(f, _)| *f).collect();
                    let takes = if own.is_empty() { "no flags of its own".into() } else { own.join(" ") };
                    return Err(format!(
                        "{}: unknown flag {name}{hint}\n  {} takes {takes}{see}",
                        verb.name, verb.name
                    ));
                }
            },
        };
        let value = match (valued, inline) {
            (false, Some(_)) => return Err(format!("{name} takes no value{see}")),
            (false, None) => None,
            (true, Some(v)) => Some(v),
            (true, None) if i < own => {
                i += 1;
                Some(args[i - 1].clone())
            }
            (true, None) => return Err(format!("{name} needs a value{see}")),
        };
        if HOMED.contains(&name) && (!s.app || (UNHOMED.contains(&verb.name) && name != "--app")) {
            return Err(format!("{} is about no running app: {name} does nothing for it{see}", verb.name));
        }
        match (name, value) {
            ("--app", Some(v)) => app = Some(v),
            (_, v) => out.extend(std::iter::once(name.to_string()).chain(v)),
        }
    }
    let rest = if verb.name == "dev" { &args[own..] } else { args.get(own + 1..).unwrap_or_default() };
    words.extend(rest.iter().cloned());
    let alone = NO_APP_WITH.iter().any(|(v, f)| *v == verb.name && out.iter().any(|o| o == f)) && words.is_empty();
    if s.app && !alone {
        match app {
            Some(a) => words.insert(0, a),
            None if words.len() < s.need => words.insert(0, here()?),
            None => {}
        }
    }
    if words.len() < s.need && !alone {
        return Err(format!("{}: too few words{see}", verb.name));
    }
    if !s.any && words.len() > s.need + s.optional {
        let extra = words[s.need + s.optional..].join(" ");
        return Err(format!("{}: one word too many: {extra}{see}", verb.name));
    }
    if verb.name == "version" {
        return Ok(Ask::Say(version()));
    }
    // Words after the flags, behind `--` so none is read as a flag again.
    out.push("--".into());
    out.extend(words);
    Ok(Ask::Run(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(line: &str) -> Result<Ask, String> {
        let args: Vec<String> = line.split(' ').filter(|w| !w.is_empty()).map(String::from).collect();
        read(&args, &|| Ok("here-app".into()))
    }

    fn run(line: &str) -> Vec<String> {
        match r(line) {
            Ok(Ask::Run(a)) => a,
            other => panic!("{line}: {other:?}"),
        }
    }

    fn said(line: &str) -> String {
        match r(line) {
            Ok(Ask::Say(s)) => s,
            other => panic!("{line}: {other:?}"),
        }
    }

    #[test]
    fn a_usage_line_says_flags_and_words() {
        let s = shape("build <app> [--targets a,b] [--list] [--smoke]");
        assert_eq!(s.flags, vec![("--targets", true), ("--list", false), ("--smoke", false)]);
        assert_eq!((s.need, s.optional, s.any, s.app), (1, 0, false, true));
        let s = shape("call <app> <cell> <method> [arg…] [--args json] [--as user]");
        assert_eq!((s.need, s.optional, s.any), (3, 0, true));
        assert_eq!(shape("events <app> [cell] [--last N]").optional, 1);
        assert!(!shape("pair x <host:port>").app && !shape("runtime [--fetch]").app);
        // Every verb's line reads: each flag once, no flag a global one.
        for v in VERBS.iter().filter(|v| v.name != "lint") {
            let f = shape(v.usage).flags;
            assert!(f.iter().all(|(n, _)| n.starts_with("--") && global(n).is_none()), "{}", v.usage);
            assert!(v.usage == v.name || v.usage.starts_with(&format!("{} ", v.name)), "{}", v.name);
        }
    }

    #[test]
    fn an_unknown_flag_is_refused_with_what_was_meant() {
        // aio am-unknown-flags: every gated verb refuses --zzz.
        for v in VERBS.iter().filter(|v| !["dev", "lint", "help", "version"].contains(&v.name)) {
            let e = r(&format!("{} a b c d --zzz", v.name)).unwrap_err();
            assert!(e.contains("unknown flag --zzz"), "{}: {e}", v.name);
        }
        let e = r("events app --lats 3").unwrap_err();
        assert!(e.contains("unknown flag --lats (did you mean --last?)") && e.contains("events takes --last"), "{e}");
        assert!(r("health app --watch").unwrap_err().contains("health takes no flags of its own"));
        let dev = verb_help(find("dev").unwrap());
        assert!(dev.contains("--display") && !dev.contains("no flags of its own"), "its flags are in its args: {dev}");
        // A one-dash flag is a typo too; a negative number is a word.
        assert!(r("state app notes -w").unwrap_err().contains("unknown flag -w"));
        assert_eq!(run("call app c m -3 -x"), ["call", "--", "app", "c", "m", "-3", "-x"]);
        // Unknown command: did you mean.
        assert!(r("evnts app").unwrap_err().contains("did you mean events?"));
        assert!(r("dispatch notes:set x").unwrap_err().contains("ax call"));
    }

    #[test]
    fn values_words_and_the_end_of_flags() {
        assert_eq!(run("events app --last=5"), ["events", "--last", "5", "--", "app"]);
        assert_eq!(run("events app --last 5 notes"), ["events", "--last", "5", "--", "app", "notes"]);
        assert_eq!(run("timetravel app board goto 3"), ["timetravel", "--", "app", "board", "goto", "3"]);
        assert!(r("events app --last").unwrap_err().contains("--last needs a value"));
        assert!(r("health app --json=1").unwrap_err().contains("--json takes no value"));
        assert!(r("health app extra").unwrap_err().contains("one word too many: extra"));
        assert!(r("pair host").unwrap_err().contains("too few words"));
        // After `--`, a word that looks like a flag is a word.
        assert_eq!(run("call app c m -- --x"), ["call", "--", "app", "c", "m", "--x"]);
        assert_eq!(run("health app --json"), ["health", "--json", "--", "app"]);
    }

    #[test]
    fn the_app_comes_from_app_or_the_folder() {
        assert_eq!(run("health"), ["health", "--", "here-app"]);
        assert_eq!(run("events --app notes cell"), ["events", "--", "notes", "cell"]);
        // Given as a word, the folder isn't asked.
        let args: Vec<String> = ["health", "mine"].map(String::from).to_vec();
        assert_eq!(
            read(&args, &|| Err("not asked".into())),
            Ok(Ask::Run(vec!["health".into(), "--".into(), "mine".into()]))
        );
        let none = read(&["logs".to_string()], &|| Err("no app here: give one".into()));
        assert_eq!(none, Err("no app here: give one".into()));
        assert!(r("apps --app x").unwrap_err().contains("--app does nothing"));
        // --all: every app of the workspace, no folder asked.
        assert_eq!(run("start --all"), ["start", "--all", "--"]);
        assert_eq!(run("restart --all --wait 5"), ["restart", "--all", "--wait", "5", "--"]);
        assert_eq!(run("logs notes --instance a1"), ["logs", "--instance", "a1", "--", "notes"]);
        assert!(r("build notes --profile p").unwrap_err().contains("--profile does nothing"));
        assert_eq!(run("build --app notes"), ["build", "--", "notes"]);
    }

    #[test]
    fn help_and_version_on_any_verb() {
        let all = said("help");
        assert!(all.contains("ax state <app> <cell> [path] [--watch] [--as user]"), "{all}");
        assert!(all.lines().any(|l| l.starts_with("  ax data <app>") && l.ends_with("and what to back up")));
        assert_eq!(said(""), all);
        assert_eq!(said("--help"), all);
        let one = said("events --help");
        assert!(
            one.starts_with("ax events <app> [cell] [--last N] [--diff] [--from file]\n\n  the newest events"),
            "{one}"
        );
        assert!(one.contains("events takes --last"), "{one}");
        assert_eq!(said("help events"), one);
        assert_eq!(said("state -h"), said("help state"));
        assert!(r("help nope").is_err());
        let ver = format!("aio-rs {}", env!("CARGO_PKG_VERSION"));
        assert_eq!(said("--version"), ver);
        assert_eq!(said("logs app --version"), ver);
        assert_eq!(said("version"), ver);
        // `dev` hands the app's own flags on.
        assert_eq!(run("dev app --help --x"), ["dev", "--", "app", "--help", "--x"]);
        assert!(said("dev --help").starts_with("ax dev"));
    }

    #[test]
    fn another_spelling_reads_as_its_verb() {
        assert_eq!(run("runtime --prune --yes --days 3"), run("prune --yes --days 3"));
        assert_eq!(run("runtime --prune"), ["prune", "--"]);
        assert_eq!(run("runtime --fetch"), ["runtime", "--fetch", "--"]);
        assert!(r("runtime --prune --fetch").unwrap_err().contains("prune: unknown flag --fetch"));
        assert!(said("help").contains("also: ax runtime --prune"));
    }
}
