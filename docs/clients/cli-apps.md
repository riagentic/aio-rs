# Command-line apps

A command-line tool is an aio-rs app whose cell is saved like any other's:
`ax state`, `ax events` and `ax call` see it, its history replays, and it
can sync. `ax create <name> --template cli` makes one (`[app] mode =
"cli"`, no window, no `kit`).

`aio::cli` (aio's `aio/cli`) is its command line: it parses arguments
against a spec, prints `--help`, refuses a wrong invocation with exit code
2 and a did-you-mean, answers `--json` with `{"error"}`, asks a person
without hanging a script, and draws tables, progress bars and spinners for
a terminal, plain lines for a pipe. Parsing and drawing are pure, so they
test without a terminal.

## Parse, refuse, draw

```rust
use aio::cli::{Column, Flag, Spec, Stop, Style, parse, table};

fn spec() -> Spec {
    Spec::new("notes")
        .about("notes on the command line")
        .version("0.1.0")
        .command("add", "add a note")
        .command("list", "list the notes")
        .rest("text")
        .flag(Flag::switch("json").help("answer as JSON"))
        .flag(Flag::text("tag").short('t').help("its tag"))
        .flag(Flag::number("limit").default("20").help("at most this many"))
}

# fn main() {
let a = parse(&spec(), &["add", "--tag", "home", "buy", "milk"]).unwrap();
assert_eq!(a.command.as_deref(), Some("add"));
assert_eq!((a.text("tag"), a.rest.join(" ")), (Some("home"), "buy milk".to_string()));
assert_eq!(a.number("limit"), Some(20.0), "its default");

match parse(&spec(), &["ad"]) {
    Err(Stop::Refused { why, .. }) => assert!(why.contains("add"), "{why}"), // did you mean
    other => panic!("{other:?}"),
}
assert!(matches!(parse(&spec(), &["--help"]), Err(Stop::Say(_))));
assert!(matches!(parse(&spec(), &["--version"]), Err(Stop::Say(v)) if v.contains("0.1.0")));

let rows = vec![vec!["1", "buy milk"], vec!["2", "call Ada"]];
let out = table(&[Column::right("id"), Column::left("text")], &rows, Style { color: false });
assert!(out.contains("buy milk"));
# }
```

| Spec | What it reads |
|---|---|
| `.command(name, help)` | a sub-command: the first word must be one (`Args::command`) |
| `.positional(name)` | the next named word (`Args::pos(name)`) |
| `.rest(name)` | every word after the positionals (`Args::rest`); without it a stray word is refused |
| `Flag::switch(name)` | `--name`, on or off (`Args::on`) |
| `Flag::text(name)` | `--name v` or `--name=v` (`Args::text`) |
| `Flag::number(name)` | the same, refused unless a number (`Args::number`) |
| `Flag::list(name)` | `--name a --name b`: every value (`Args::list`) |
| `.short('t')`, `.default("v")`, `.required()`, `.help("…")` | on any flag |
| `.typed::<u16>()` | on any flag: each value refused unless it parses as that type (`--port expects a u16, got "70000"`) |
| `.about(..)`, `.version(..)` | the first line of `--help`; what `--version` prints |

A switch named `json` turns refusals into `{"error": "…"}` on stdout, the
stream a script reads. `--` ends the flags.

Each flag is read as the kind the spec gives it: `a.text("n")` of a
number flag, or `a.on("jsn")` the spec doesn't name, panics naming it
(`--n is a Number flag, read as Text`), so a misread fails the first run
instead of reading `None` for good. `Args::pos` holds to the spec's
positionals the same way.

### Options as a typed struct

`#[derive(aio::cli::Options)]` makes the spec from a struct and reads
the command line into it (aio inferred the flags' types from its spec):
each field is a flag, its type says its kind and the type each value
parses as, so a read is a field, checked by the compiler, and a value of
the wrong type is refused like any wrong invocation.

```rust
use aio::cli::{Options, Stop};

/// notes on the command line
#[derive(Options, Debug)]
#[cli(name = "notes", version = "0.1.0", command("add", "add a note"), command("list", "list the notes"))]
struct Notes {
    #[cli(command)]
    command: String,
    /// its tag
    #[cli(short = 't')]
    tag: Option<String>,
    /// at most this many
    #[cli(default = 20)]
    limit: u32,
    /// answer as JSON
    json: bool,
    #[cli(rest)]
    text: Vec<String>,
}

# fn main() {
let n = Notes::parse(&["add", "-t", "home", "buy", "milk"]).unwrap();
assert_eq!((n.command.as_str(), n.tag.as_deref(), n.limit, n.text.join(" ")), ("add", Some("home"), 20, "buy milk".into()));
assert!(matches!(Notes::parse(&["list", "--limit", "-1"]), Err(Stop::Refused { why, .. }) if why.starts_with("--limit expects a u32")));
# }
```

In `main`: `let n = Notes::args();` (`--help` and a refusal end the
process, as [`args`](#parse-refuse-draw) does).

| Field | Flag |
|---|---|
| `bool` | a switch, `--dry-run` for `dry_run` |
| `T` | a flag that must be given, unless `#[cli(default = v)]` (`v` of type `T`) |
| `Option<T>` | a flag that may be absent |
| `Vec<T>` | a list |
| `#[cli(positional)] Option<String>`, `#[cli(rest)] Vec<String>` | the next word, the words after |
| `#[cli(command)] String` | the sub-command, one of the struct's `#[cli(command("name", "help"))]` |

`T` is any `FromStr` type: a number makes a number flag, anything else
text. `#[cli(short = 'x')]` gives the short name; the doc comment is the
help, the struct's its about line; `#[cli(name = "..")]` names the
program (unset: the crate), `#[cli(version)]` gives `--version` (the
crate's, or `version = ".."`). A field of a type no flag reads, or a
default of another type, is a compile error at the field.

### How a command line is read

- Spellings: `--tag=home`, `--tag home`, `-t home` (a short name takes
  its value as the next word; `-thome` is not read as one).
- Everything not starting with `-` is a word: the sub-command first (when
  the spec has commands), then the positionals in order, then the rest.
  A lone `-` is a word too (stdin, by convention).
- Refused, with exit code 2, the reason pointing at `notes --help`: an unknown flag
  (with a did-you-mean, `unknown flag: --tga (did you mean --tag?)`), an
  unknown or missing command, a flag without its value, a switch given a
  value (`--json=1`), a number flag given text, a missing `.required()`
  flag, and a stray word when the spec has no `.rest(..)`. A typo is never
  ignored: an app that runs with a setting nobody meant is worse than one
  that stops.
- A default applies when the flag isn't given; for a list, the first
  value given replaces the default list, later ones add to it.
- `--help` (and `-h`, unless a flag of the spec takes `.short('h')`)
  gives `Stop::Say(help)`; `--version` gives `name version` only when the
  spec has `.version(..)`, else it is an unknown flag.
- `--json` is looked for before anything else, so even a refusal of an
  earlier word comes as JSON.
- `Spec::help()` is the help text (pure), for a test to pin:

```text
notes on the command line

usage: notes <command> [text...] [flags]

commands:
  add   add a note
  list  list the notes

flags:
      --json            answer as JSON
  -t, --tag=<text>      its tag
      --limit=<number>  at most this many (default: "20")
      --help            show this help
      --version         print the version
```

## In main

```rust,no_run
use aio::cli::{Flag, Spec, fail, ERROR};

fn main() {
    let spec = Spec::new("notes").command("list", "list the notes").flag(Flag::switch("json"));
    // Parses std::env::args; prints --help / --version and exits 0,
    // or refuses a wrong invocation and exits 2.
    let args = aio::cli::args(&spec);
    if args.command.as_deref() != Some("list") {
        fail("nothing to do", ERROR, args.json); // stderr, or {"error"} under --json
    }
}
```

- `aio::cli::args` leaves aio's own launch flags out (`--home`,
  `--profile`, `--instance`, `--port`, `--host`, `--expose`,
  `--takeover`, `--no-persist`, `--no-tls`, `--tls-cert`, `--tls-key`, `--channel`, `--width`, `--height`,
  `--cdp`, `--server-url`, `--connect`, `--strict-pin`, `--client`, `--transport`), so the same binary answers both and neither refuses the
  other's.
- `aio::cli::fail(why, code, json)` ends with `error: why` on stderr (or
  `{"error"}` on stdout under `--json`) and the exit code.
- Exit codes, as `ax` uses them: `aio::cli::OK` (0), `ERROR` (1),
  `USAGE` (2: a wrong flag, a missing argument, a prompt with no terminal).

The template's launcher boots the cell first, then runs one command on it:

```rust,ignore
// ax create's launcher (app/src/main.rs), compiled by ax's own tests:
let cfg = aio::config!();
let notes = aio::Source::<app::Notes>::for_app(&cfg, &dirs)?;
let args: Vec<String> = std::env::args().skip(1).collect();
Ok(app::run(&notes, &args))
```

`app::run` takes any door (`&impl aio::Door<Notes>`), so its test calls it
on `Handle::memory()` and the tool calls it on the app's store.

## Asking a person

```rust
use aio::cli::Prompt;

# fn main() {
// Any streams: a test gives the answers; Prompt::std() is the terminal's.
let mut p = Prompt { input: &b"\ny\n"[..], out: Vec::new(), tty: true };
assert!(p.confirm("Delete all notes?", true).unwrap(), "Enter takes the default");
assert!(p.confirm("Really?", false).unwrap());

// With no terminal a prompt refuses instead of hanging a script.
let mut piped = Prompt { input: &b""[..], out: Vec::new(), tty: false };
assert!(piped.ask("Title", None).unwrap_err().why.contains("needs a terminal"));
# }
```

| Prompt | Answers |
|---|---|
| `ask(q, default)` | free text; Enter takes `default`, or asks again when there is none |
| `confirm(q, default)` | `y`/`yes`/`n`/`no` (any case); Enter takes `default`; anything else asks again |
| `select(q, choices, default)` | a numbered list; an answer by number or by name; no choices is refused |
| `password(q)` | not echoed on the terminal (Unix and Windows), never printed back |

With no terminal (`tty: false`) every prompt refuses at once (`ask("Title")
needs a terminal, and stdin is not one: pass the value as a flag`); a
closed stdin refuses too (`no answer`). Take each value as a flag too, so
a script never needs the prompt.

## Progress, spinners, live views

- `Progress::new(total, label)` (`tick(k)`, `set(n)`, `done()`): one line
  redrawn on a terminal, a plain line at each tenth on a pipe. On stderr,
  so stdout stays the command's answer.
- `Spinner::new(label)` (`update(label)`, `stop(last)`): the same for work
  of unknown length.
- `aio::cli::watch(&h.state(), |f| h.listen(f), render)`: redraws
  `render(state)` on the changes of the cell (a `top`-like screen), also
  on a synced copy (`Remote::listen`): one frame per `TICK` (16 ms), the
  tick's last state. On a terminal the cursor is hidden while it draws and
  shown again however the process ends (its exit, Ctrl+C, SIGTERM; an
  app's own handler still runs).
- `Style::stdout()` colours for a terminal without `NO_COLOR`, or
  whenever `FORCE_COLOR` is set (not `0`/`false`); `Style { color }`
  paints `bold`, `dim`, `underline`, `red`, `green`, `yellow`, `blue`,
  `magenta`, `cyan`, or returns the text as it is. `aio::cli::width(s)`
  counts terminal cells (CJK and emoji two, colour codes none).
- `table` aligns by those cells, so colour inside a cell shifts nothing;
  its header is bold when colour is on.

## Testing a CLI

Everything that decides what a person sees is pure, and everything that
writes takes its stream, so a test runs with strings and no terminal:

| Pure | Gives |
|---|---|
| `parse(&spec, &argv)` | `Ok(Args)`, or `Err(Stop::Say(text))` (help, version) or `Err(Stop::Refused { why, json })` |
| `Spec::help()` | the `--help` text |
| `failure(why, json)` | what `fail` prints, and whether on stdout (`(true, "{\"error\":..}")`) |
| `table(&columns, &rows, Style { color })` | the table |
| `frame(tty, text)` | one frame of a live view: the screen cleared first on a terminal |
| `width(s)` | terminal cells, colour codes counted as none |
| `near(bad, names)` | the did-you-mean |

| On a stream | For a test |
|---|---|
| `Prompt { input, out, tty }` | answers from a byte slice; `tty: false` to check the refusal |
| `Progress::to(out, redraw, total, label)` | any `Write + Send`; `redraw: false` draws the pipe form |
| `Spinner::to(out, redraw, label)` | likewise |

```rust
use aio::cli::{Progress, Spec, failure, frame};
use std::io::Write;
use std::sync::{Arc, Mutex};

/// A Write a test reads back.
#[derive(Clone, Default)]
struct Buf(Arc<Mutex<Vec<u8>>>);
impl Write for Buf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

# fn main() {
// On a pipe a progress bar is a plain line at each tenth, never \r.
let out = Buf::default();
let mut p = Progress::to(Box::new(out.clone()), false, 4, "import");
p.tick(2);
p.done();
let text = String::from_utf8(out.0.lock().unwrap().clone()).unwrap();
assert_eq!(text, "import 0/4 (0%)\nimport 2/4 (50%)\nimport 4/4 (100%)\n");

assert_eq!(failure("no such note", true), (true, r#"{"error":"no such note"}"#.to_string()));
assert_eq!(failure("no such note", false), (false, "error: no such note".to_string()));
assert_eq!(frame(false, "3 notes"), "3 notes\n", "a pipe appends");
assert!(frame(true, "3 notes").starts_with("\x1b[2J"), "a terminal redraws");
assert!(Spec::new("notes").help().contains("--help"));
# }
```

A test of the tool's commands calls `app::run(&Handle::memory(), &args)`
(any door), so it checks what the cell holds after, not only what was
printed.

## Its own flags and aio's

A tool's binary takes aio's launch flags too: `--home`, `--profile`,
`--instance` (where its data lives), `--takeover`, `--port`, `--host`,
`--expose`, `--no-persist`, `--no-tls`, `--tls-cert`, `--tls-key`, `--channel`, `--width`, `--height`,
`--cdp`, `--server-url`, `--connect`, `--strict-pin`, `--client`, `--transport` (`aio::paths::FLAGS`). `aio::cli::args` removes those (and
their values) before parsing, so the spec never lists them and a typo in
either is refused with a did-you-mean. `aio::paths::Ask::app_args(&argv)`
does the same for an app that parses its arguments some other way.

## Shapes this fits

- **A tool on its own store** (`ax create <name> --template cli`): the
  binary boots the cell, runs one command, exits; the next run finds the
  cell as the last one left it.
- **A command line for a running app or server**: the launcher's
  `aio::Source::for_app` gives a synced copy when `[app] mode = "client"`,
  so `notes list` reads the server's cell and `notes add` calls it; with
  `watch` that is a live dashboard (`notes list --watch`).
- **A script's tool**: every prompt has a flag behind it, `--json` on the
  answer and on refusals, exit codes 0, 1, 2.

## Gaps against aio

- aio's `testIO()` records the raw-mode toggles of an app's own secret
  prompt; aio-rs's only one is `Prompt::password`, which turns the echo
  off itself, so there is nothing of the app's to record.
- aio's `connectCli` client prints the notifications the server sends;
  an aio-rs client shows them only once the app says how
  (`aio::tell::show_with(f)`).

## Common mistakes

- Printing progress on stdout: a pipe then reads it as the answer. Use
  `Progress` / `Spinner` (stderr).
- A prompt with no flag behind it: the tool can't run in a script or CI.
- Reading `std::env::args()` raw in an app that also takes aio's launch
  flags: use `aio::cli::args` (or `aio::paths::Ask::app_args`).
- Doing the work outside the cell: what isn't a call isn't in the log,
  `ax events` can't show it and a replay can't make it again.

API: `cargo doc -p aio-rs --no-deps`, module `aio::cli`.
