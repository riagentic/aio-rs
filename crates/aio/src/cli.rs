//! An app's own command line (aio's `aio/cli`): [`args`] parses against a
//! [`Spec`] and prints it as `--help`; [`fail`] ends with a refusal
//! (`{"error"}` on stdout under `--json`, the stream a script reads); the
//! prompts refuse without a terminal instead of hanging; [`table`], [`Style`],
//! [`Progress`], [`Spinner`] and [`watch`] draw for a person on a terminal and
//! plain lines for a pipe. Parsing and drawing are pure (`parse`, `help`,
//! `table`, `frame`); the doors only add the process's streams.
pub use aio_macros::Options;
use std::collections::BTreeMap;
use std::io::{BufRead, IsTerminal, Write};
use std::sync::{Arc, Mutex};

/// Exit codes: the command worked, failed, or was asked wrongly (an unknown
/// flag, a missing argument, a prompt with no terminal).
pub const OK: i32 = 0;
pub const ERROR: i32 = 1;
pub const USAGE: i32 = 2;

/// What a flag takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `--watch`: on or off.
    Switch,
    /// `--url=<text>`.
    Text,
    /// `--n=<number>`, refused unless it is one.
    Number,
    /// `--tag=a --tag=b`: every value, in order.
    List,
}

/// One flag of a [`Spec`].
#[derive(Clone, Debug)]
pub struct Flag {
    name: String,
    kind: Kind,
    help: String,
    short: Option<char>,
    default: Option<String>,
    required: bool,
    /// The type each value must parse as, and its name ([`Flag::typed`]).
    typed: Option<Typed>,
}

/// Whether a value parses as a flag's type, and the type's name.
type Typed = (fn(&str) -> bool, &'static str);

impl Flag {
    fn of(name: &str, kind: Kind) -> Self {
        Self { name: name.into(), kind, help: String::new(), short: None, default: None, required: false, typed: None }
    }

    pub fn switch(name: &str) -> Self {
        Self::of(name, Kind::Switch)
    }

    pub fn text(name: &str) -> Self {
        Self::of(name, Kind::Text)
    }

    pub fn number(name: &str) -> Self {
        Self::of(name, Kind::Number)
    }

    pub fn list(name: &str) -> Self {
        Self::of(name, Kind::List)
    }

    /// Its line in `--help`.
    pub fn help(mut self, help: &str) -> Self {
        self.help = help.into();
        self
    }

    /// `-n` for `--n`.
    pub fn short(mut self, c: char) -> Self {
        self.short = Some(c);
        self
    }

    /// Its value when absent.
    pub fn default(mut self, v: &str) -> Self {
        self.default = Some(v.into());
        self
    }

    /// Refused when absent.
    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }

    /// Each value refused unless it parses as a `T` (`--port` as a `u16`:
    /// `70000` and `1.5` are refused naming it); `#[derive(Options)]` sets
    /// it from the field's type.
    pub fn typed<T: std::str::FromStr>(mut self) -> Self {
        let name = std::any::type_name::<T>();
        self.typed = Some((|v| v.trim().parse::<T>().is_ok(), name.rsplit("::").next().unwrap_or(name)));
        self
    }
}

/// Command-line options as a typed struct, its spec made from the fields
/// (`#[derive(aio::cli::Options)]`): each field's type is its flag's kind
/// and the type its value parses as, so reading one is a field, checked
/// by the compiler (aio inferred the flags' types from the spec).
///
/// ```
/// use aio::cli::Options;
///
/// /// copy files
/// #[derive(Options)]
/// struct Copy {
///     /// how many at once
///     #[cli(default = 4)]
///     jobs: u8,
///     verbose: bool,
/// }
///
/// let c = Copy::parse(&["--jobs", "2"]).unwrap();
/// assert_eq!((c.jobs, c.verbose), (2, false));
/// ```
///
/// A default of another type than its field is a compile error:
///
/// ```compile_fail
/// #[derive(aio::cli::Options)]
/// struct Copy {
///     #[cli(default = "four")]
///     jobs: u8,
/// }
/// ```
///
/// and so is a field no flag reads as, or a misread name (a field):
///
/// ```compile_fail
/// #[derive(aio::cli::Options)]
/// struct Copy {
///     to: std::collections::HashMap<String, String>,
/// }
/// ```
pub trait Options: Sized {
    /// The spec the fields make (add to it: `.command(..)` is for an app
    /// that reads `Args` itself).
    fn spec() -> Spec;
    /// The fields from `a`, parsed against [`spec`](Self::spec).
    fn from_args(a: &Args) -> Self;

    /// `argv` (without the program) parsed (pure).
    fn parse<S: AsRef<str>>(argv: &[S]) -> Result<Self, Stop> {
        parse(&Self::spec(), argv).map(|a| Self::from_args(&a))
    }

    /// The process's arguments, as [`args`] reads them: `--help` and a
    /// wrong invocation end the process.
    fn args() -> Self {
        Self::from_args(&args(&Self::spec()))
    }
}

/// What [`parse`] reads and [`Spec::help`] prints. A switch named `json`
/// turns refusals into `{"error"}` on stdout.
#[derive(Clone, Debug, Default)]
pub struct Spec {
    name: String,
    about: String,
    version: Option<String>,
    commands: Vec<(String, String)>,
    positional: Vec<String>,
    rest: Option<String>,
    flags: Vec<Flag>,
}

impl Spec {
    /// The program's name, as typed: the first word of its usage.
    pub fn new(name: &str) -> Self {
        Self { name: name.into(), ..Self::default() }
    }

    /// The first line of `--help`.
    pub fn about(mut self, about: &str) -> Self {
        self.about = about.into();
        self
    }

    /// What `--version` prints (none: no `--version`).
    pub fn version(mut self, v: &str) -> Self {
        self.version = Some(v.into());
        self
    }

    /// A sub-command: the first word must then be one of them.
    pub fn command(mut self, name: &str, help: &str) -> Self {
        self.commands.push((name.into(), help.into()));
        self
    }

    /// The next named positional (`Args::pos`).
    pub fn positional(mut self, name: &str) -> Self {
        self.positional.push(name.into());
        self
    }

    /// The words after the positionals (`Args::rest`); without it, a stray word is refused.
    pub fn rest(mut self, name: &str) -> Self {
        self.rest = Some(name.into());
        self
    }

    pub fn flag(mut self, f: Flag) -> Self {
        self.flags.push(f);
        self
    }

    /// The `--help` text (pure).
    pub fn help(&self) -> String {
        let mut lines = vec![];
        if !self.about.is_empty() {
            lines.extend([self.about.clone(), String::new()]);
        }
        let usage = [self.name.clone()]
            .into_iter()
            .chain((!self.commands.is_empty()).then(|| "<command>".into()))
            .chain(self.positional.iter().map(|p| format!("<{p}>")))
            .chain(self.rest.iter().map(|r| format!("[{r}...]")))
            .chain(["[flags]".into()]);
        lines.push(format!("usage: {}", usage.collect::<Vec<_>>().join(" ")));
        if !self.commands.is_empty() {
            lines.extend([String::new(), "commands:".into()]);
            lines.extend(columns(&self.commands));
        }
        let mut flags: Vec<(String, String)> = self
            .flags
            .iter()
            .map(|f| {
                let short = f.short.map_or("    ".into(), |c| format!("-{c}, "));
                let value = match (f.kind, f.typed) {
                    (Kind::Switch, _) => String::new(),
                    (Kind::Number, _) => "=<number>".into(),
                    (_, Some((_, ty))) if ty != "String" => format!("=<{}>", ty.to_lowercase()),
                    _ => "=<text>".into(),
                };
                let tail = match &f.default {
                    _ if f.required => " (required)".into(),
                    Some(d) if f.kind != Kind::Switch => format!(" (default: {d:?})"),
                    _ => String::new(),
                };
                (format!("{short}--{}{value}", f.name), format!("{}{tail}", f.help))
            })
            .collect();
        flags.push(("    --help".into(), "show this help".into()));
        if self.version.is_some() {
            flags.push(("    --version".into(), "print the version".into()));
        }
        lines.extend([String::new(), "flags:".into()]);
        lines.extend(columns(&flags));
        lines.join("\n")
    }

    fn named(&self, name: &str) -> Option<&Flag> {
        self.flags.iter().find(|f| f.name == name)
    }
}

/// `  left  right` rows, the lefts padded to one width.
fn columns(rows: &[(String, String)]) -> Vec<String> {
    let w = rows.iter().map(|(l, _)| l.chars().count()).max().unwrap_or(0);
    rows.iter().map(|(l, r)| format!("  {l:<w$}  {r}").trim_end().to_string()).collect()
}

/// A parsed command line, read by the spec's names, each flag as its kind
/// (aio inferred the types from the spec): a name the spec hasn't, or a
/// flag read as another kind (`number` of a text flag), panics naming it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Args {
    /// The sub-command (when the spec has commands).
    pub command: Option<String>,
    /// The words after the positionals.
    pub rest: Vec<String>,
    /// `--json` was given (a switch named `json`).
    pub json: bool,
    values: BTreeMap<String, Vec<String>>,
    pos: BTreeMap<String, String>,
    kinds: BTreeMap<String, Kind>,
    positional: Vec<String>,
}

impl Args {
    /// A switch is on.
    pub fn on(&self, name: &str) -> bool {
        self.read(name, Kind::Switch).is_some()
    }

    /// A text flag's value (the last one given), or its default.
    pub fn text(&self, name: &str) -> Option<&str> {
        self.read(name, Kind::Text).and_then(|v| v.last()).map(String::as_str)
    }

    /// A number flag's value, or its default.
    pub fn number(&self, name: &str) -> Option<f64> {
        self.read(name, Kind::Number).and_then(|v| v.last()).and_then(|v| v.trim().parse().ok())
    }

    /// A list flag's values (its default when none was given).
    pub fn list(&self, name: &str) -> &[String] {
        self.read(name, Kind::List).map_or(&[], Vec::as_slice)
    }

    /// Flag `name`'s last value (or its default) as a `T`, whatever its
    /// kind but a switch: what `#[derive(Options)]` reads, its type checked by [`Flag::typed`].
    #[doc(hidden)]
    pub fn get<T: std::str::FromStr>(&self, name: &str) -> Option<T> {
        self.values.get(name)?.last()?.trim().parse().ok()
    }

    /// Every value of list flag `name` as a `T`.
    #[doc(hidden)]
    pub fn get_all<T: std::str::FromStr>(&self, name: &str) -> Vec<T> {
        self.list(name).iter().filter_map(|v| v.trim().parse().ok()).collect()
    }

    /// A named positional.
    pub fn pos(&self, name: &str) -> Option<&str> {
        assert!(self.positional.iter().any(|p| p == name), "Args::pos({name:?}): the spec has no such positional");
        self.pos.get(name).map(String::as_str)
    }

    /// Flag `name`'s values, held to the spec's kind.
    fn read(&self, name: &str, kind: Kind) -> Option<&Vec<String>> {
        match self.kinds.get(name) {
            Some(k) if *k == kind => self.values.get(name),
            Some(k) => panic!("--{name} is a {k:?} flag, read as {kind:?}"),
            None => panic!("--{name}: the spec has no such flag"),
        }
    }
}

/// Why [`parse`] gave no [`Args`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stop {
    /// `--help` or `--version`: print this on stdout, exit [`OK`].
    Say(String),
    /// A wrong invocation: refuse with this, exit [`USAGE`] (`json`: as `{"error"}`).
    Refused { why: String, json: bool },
}

/// Parse `argv` (without the program) against `spec` (pure). `--x=v` and
/// `--x v`, `-s v` for a short name; `--` ends the flags.
pub fn parse<S: AsRef<str>>(spec: &Spec, argv: &[S]) -> Result<Args, Stop> {
    let argv: Vec<&str> = argv.iter().map(AsRef::as_ref).collect();
    let flag_of = |a: &str| -> Option<(&Flag, Option<String>)> {
        match a.strip_prefix("--") {
            Some(long) => {
                let (name, v) = long.split_once('=').map_or((long, None), |(n, v)| (n, Some(v.to_string())));
                spec.named(name).map(|f| (f, v))
            }
            None => {
                let c = a.strip_prefix('-')?.chars().collect::<Vec<_>>();
                spec.flags.iter().find(|f| c.len() == 1 && f.short == Some(c[0])).map(|f| (f, None))
            }
        }
    };
    // Looked for first, so a refusal before `--json` is JSON too.
    let json = spec.named("json").is_some_and(|f| f.kind == Kind::Switch) && {
        let mut it = argv.iter().take_while(|a| **a != "--");
        let mut found = false;
        while let Some(a) = it.next() {
            match flag_of(a) {
                Some((f, None)) if f.name == "json" => found = true,
                Some((f, None)) if f.kind != Kind::Switch => _ = it.next(),
                _ => {}
            }
        }
        found
    };
    let refuse = |why: String| Stop::Refused { why: format!("{why} (run `{} --help`)", spec.name), json };
    let mut values: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut given = std::collections::BTreeSet::new();
    let mut words = vec![];
    let mut it = argv.iter().copied();
    while let Some(a) = it.next() {
        if a == "--" {
            words.extend(it.by_ref());
            break;
        }
        let short_h = spec.flags.iter().any(|f| f.short == Some('h'));
        if a == "--help" || (a == "-h" && !short_h) {
            return Err(Stop::Say(spec.help()));
        }
        if a == "--version"
            && let Some(v) = &spec.version
        {
            return Err(Stop::Say(format!("{} {v}", spec.name)));
        }
        if !a.starts_with('-') || a == "-" {
            words.push(a);
            continue;
        }
        let Some((f, inline)) = flag_of(a) else {
            let bad = a.split('=').next().unwrap_or(a);
            let hint = bad
                .strip_prefix("--")
                .and_then(|b| near(b, spec.flags.iter().map(|f| f.name.as_str())))
                .map_or(String::new(), |n| format!(" (did you mean --{n}?)"));
            return Err(refuse(format!("unknown flag: {bad}{hint}")));
        };
        let name = &f.name;
        if f.kind == Kind::Switch {
            if inline.is_some() {
                return Err(refuse(format!("--{name} takes no value")));
            }
            values.insert(name.clone(), vec![]);
            continue;
        }
        let Some(v) = inline.or_else(|| it.next().map(str::to_string)) else {
            return Err(refuse(format!("--{name} needs a value")));
        };
        if f.kind == Kind::Number && !v.trim().parse::<f64>().is_ok_and(f64::is_finite) {
            return Err(refuse(format!("--{name} expects a number, got {v:?}")));
        }
        if let Some((_, ty)) = f.typed.filter(|(ok, _)| !ok(&v)) {
            return Err(refuse(format!("--{name} expects a {ty}, got {v:?}")));
        }
        // The first given replaces the default; a list keeps the rest.
        let slot = values.entry(name.clone()).or_default();
        if given.insert(name.clone()) || f.kind != Kind::List {
            slot.clear();
        }
        slot.push(v);
    }
    for f in &spec.flags {
        match &f.default {
            _ if values.contains_key(&f.name) => {}
            _ if f.required => return Err(refuse(format!("--{} is required", f.name))),
            Some(d) if f.kind != Kind::Switch => _ = values.insert(f.name.clone(), vec![d.clone()]),
            _ => {}
        }
    }
    let mut words = words.into_iter();
    let command = match spec.commands.is_empty() {
        true => None,
        false => {
            let c = words.next().ok_or_else(|| refuse("missing command".into()))?;
            if !spec.commands.iter().any(|(n, _)| n == c) {
                let hint = near(c, spec.commands.iter().map(|(n, _)| n.as_str()))
                    .map_or(String::new(), |n| format!(" (did you mean {n}?)"));
                return Err(refuse(format!("unknown command: {c}{hint}")));
            }
            Some(c.to_string())
        }
    };
    let pos = spec.positional.iter().zip(words.by_ref()).map(|(n, w)| (n.clone(), w.to_string())).collect();
    let rest: Vec<String> = words.map(str::to_string).collect();
    if let (Some(w), None) = (rest.first(), &spec.rest) {
        return Err(refuse(format!("unexpected argument: {w}")));
    }
    let kinds = spec.flags.iter().map(|f| (f.name.clone(), f.kind)).collect();
    Ok(Args { command, rest, json, values, pos, kinds, positional: spec.positional.clone() })
}

/// A run [`crate::testing::TestIo::run`] holds on this thread: its
/// arguments, where it prints, and its end, a [`Exit`] unwind instead of
/// the process's.
pub(crate) struct Held {
    pub(crate) argv: Vec<String>,
    pub(crate) out: Box<dyn std::io::Write>,
    pub(crate) err: Box<dyn std::io::Write>,
}

thread_local! {
    pub(crate) static HELD: std::cell::RefCell<Option<Held>> = const { std::cell::RefCell::new(None) };
}

/// How a held run ended: its exit code.
#[cfg_attr(not(feature = "testing"), expect(dead_code, reason = "read by testing::TestIo::run"))]
pub(crate) struct Exit(pub(crate) i32);

/// `line` to stdout (`out`) or stderr, then the process ends with `code`;
/// in a held run, its streams and its end.
fn end(out: bool, line: &str, code: i32) -> ! {
    let held = HELD.with_borrow_mut(|h| {
        h.as_mut().map(|h| {
            let to = if out { &mut h.out } else { &mut h.err };
            _ = writeln!(to, "{line}");
        })
    });
    if held.is_some() {
        std::panic::resume_unwind(Box::new(Exit(code)));
    }
    match out {
        true => println!("{line}"),
        false => eprintln!("{line}"),
    }
    std::process::exit(code)
}

/// The process's arguments parsed against `spec` (aio's own launch flags,
/// `--home` and the like, left out): `--help`/`--version` print and exit
/// 0, a wrong invocation is refused with [`USAGE`].
pub fn args(spec: &Spec) -> Args {
    let argv = HELD.with_borrow(|h| h.as_ref().map(|h| h.argv.clone()));
    let argv: Vec<String> = argv.unwrap_or_else(|| std::env::args().skip(1).collect());
    match parse(spec, &crate::paths::Ask::app_args(&argv)) {
        Ok(a) => a,
        Err(Stop::Say(text)) => end(true, &text, OK),
        Err(Stop::Refused { why, json }) => fail(&why, USAGE, json),
    }
}

/// What [`fail`] prints, and whether on stdout (pure).
pub fn failure(why: &str, json: bool) -> (bool, String) {
    match json {
        true => (true, serde_json::json!({ "error": why }).to_string()),
        false => (false, format!("error: {why}")),
    }
}

/// Say `why` and end the process with `code` ([`ERROR`] unless above 0).
pub fn fail(why: &str, code: i32, json: bool) -> ! {
    let (out, line) = failure(why, json);
    end(out, &line, if code > 0 { code } else { ERROR })
}

/// Edit distance, for the did-you-mean.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut diag = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let up = row[j + 1];
            row[j + 1] = (up + 1).min(row[j] + 1).min(diag + usize::from(ca != *cb));
            diag = up;
        }
    }
    row[b.len()]
}

/// The nearest of `pool` to `bad`, when near enough to be what was meant.
pub fn near<'a>(bad: &str, pool: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    pool.map(|p| (distance(bad, p), p)).min().filter(|(d, _)| *d <= 2.max(bad.len() / 3)).map(|(_, p)| p)
}

/// The refusal of prompt `what` asked `q` with no terminal.
fn needs_terminal(what: &str, q: &str) -> crate::Rejected {
    crate::Rejected::new(format!(
        "{what}({q:?}) needs a terminal, and stdin is not one: pass the value as a flag, or run from a terminal"
    ))
}

/// One answer read from `input`, after `ask` is written to `out`: `None` once
/// `input` closed. `\r\n` and `\n` endings are cut.
fn line(input: &mut impl BufRead, out: &mut impl Write, ask: &str, q: &str) -> crate::Result<String> {
    _ = write!(out, "{ask}").and_then(|_| out.flush());
    let mut s = String::new();
    match input.read_line(&mut s) {
        Ok(0) | Err(_) => Err(crate::Rejected::new(format!("no answer to {q:?}: stdin closed"))),
        Ok(_) => Ok(s.trim_end_matches(['\n', '\r']).to_string()),
    }
}

/// The prompts, on any streams (`tty`: `input` is a terminal). [`Prompt::std`]
/// is the process's own.
pub struct Prompt<I, O> {
    pub input: I,
    pub out: O,
    pub tty: bool,
}

impl Prompt<std::io::StdinLock<'static>, std::io::Stdout> {
    /// stdin and stdout; a terminal only if stdin is one.
    pub fn std() -> Self {
        Self { input: std::io::stdin().lock(), out: std::io::stdout(), tty: std::io::stdin().is_terminal() }
    }
}

impl<I: BufRead, O: Write> Prompt<I, O> {
    fn need(&self, what: &str, q: &str) -> crate::Result<()> {
        if self.tty { Ok(()) } else { Err(needs_terminal(what, q)) }
    }

    /// A free-text answer; an empty one is `default`, or asked again.
    pub fn ask(&mut self, q: &str, default: Option<&str>) -> crate::Result<String> {
        self.need("ask", q)?;
        let hint = default.map_or(String::new(), |d| format!(" [{d}]"));
        loop {
            let v = line(&mut self.input, &mut self.out, &format!("{q}{hint}: "), q)?;
            match (v.trim(), default) {
                ("", Some(d)) => return Ok(d.into()),
                ("", None) => {}
                (v, _) => return Ok(v.into()),
            }
        }
    }

    /// Yes or no; Enter is `default`.
    pub fn confirm(&mut self, q: &str, default: bool) -> crate::Result<bool> {
        self.need("confirm", q)?;
        loop {
            let ask = format!("{q} {}: ", if default { "[Y/n]" } else { "[y/N]" });
            match line(&mut self.input, &mut self.out, &ask, q)?.trim().to_lowercase().as_str() {
                "" => return Ok(default),
                "y" | "yes" => return Ok(true),
                "n" | "no" => return Ok(false),
                _ => _ = writeln!(self.out, "answer y or n"),
            }
        }
    }

    /// One of `choices`, by number or by name; Enter is `default`.
    pub fn select(&mut self, q: &str, choices: &[&str], default: Option<&str>) -> crate::Result<String> {
        if choices.is_empty() {
            return Err(crate::Rejected::new(format!("select({q:?}): no choices")));
        }
        self.need("select", q)?;
        loop {
            _ = writeln!(self.out, "{q}");
            for (i, c) in choices.iter().enumerate() {
                let mark = if Some(*c) == default { " (default)" } else { "" };
                _ = writeln!(self.out, "  {}) {c}{mark}", i + 1);
            }
            let v = line(&mut self.input, &mut self.out, &format!("choose 1-{}: ", choices.len()), q)?;
            let v = v.trim();
            let by_number = v.parse::<usize>().ok().filter(|n| (1..=choices.len()).contains(n)).map(|n| choices[n - 1]);
            let pick = by_number.or_else(|| choices.iter().copied().find(|c| *c == v));
            match (v, pick, default) {
                ("", _, Some(d)) => return Ok(d.into()),
                (_, Some(c), _) => return Ok(c.into()),
                _ => _ = writeln!(self.out, "not a choice: {v:?}"),
            }
        }
    }

    /// A secret: not echoed on the process's terminal, never printed back.
    pub fn password(&mut self, q: &str) -> crate::Result<String> {
        self.need("password", q)?;
        let _quiet = Quiet::stdin(self.tty);
        let v = line(&mut self.input, &mut self.out, &format!("{q}: "), q);
        _ = writeln!(self.out);
        v
    }
}

/// The terminal's echo, off while held (only when stdin is the terminal).
struct Quiet(Option<Saved>);

#[cfg(unix)]
type Saved = libc::termios;
#[cfg(windows)]
type Saved = u32;

impl Quiet {
    fn stdin(tty: bool) -> Self {
        Self(if tty && std::io::stdin().is_terminal() { echo(None) } else { None })
    }
}

impl Drop for Quiet {
    fn drop(&mut self) {
        if let Some(s) = self.0.take() {
            echo(Some(s));
        }
    }
}

/// Echo off (`None`; the mode it had), or back to `saved`.
#[cfg(unix)]
fn echo(saved: Option<Saved>) -> Option<Saved> {
    // SAFETY: termios calls on fd 0 with a struct they fill or read.
    unsafe {
        if let Some(s) = saved {
            libc::tcsetattr(0, libc::TCSANOW, &s);
            return None;
        }
        let mut t: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(0, &mut t) != 0 {
            return None;
        }
        let mut quiet = t;
        quiet.c_lflag &= !libc::ECHO;
        (libc::tcsetattr(0, libc::TCSANOW, &quiet) == 0).then_some(t)
    }
}

#[cfg(windows)]
fn echo(saved: Option<Saved>) -> Option<Saved> {
    use windows_sys::Win32::System::Console::{
        ENABLE_ECHO_INPUT, GetConsoleMode, GetStdHandle, STD_INPUT_HANDLE, SetConsoleMode,
    };
    // SAFETY: console mode calls on the process's own stdin handle.
    unsafe {
        let h = GetStdHandle(STD_INPUT_HANDLE);
        if let Some(m) = saved {
            SetConsoleMode(h, m);
            return None;
        }
        let mut m = 0;
        if GetConsoleMode(h, &mut m) == 0 {
            return None;
        }
        (SetConsoleMode(h, m & !ENABLE_ECHO_INPUT) != 0).then_some(m)
    }
}

/// Colours and weights, or nothing when colour is off. [`Style::stdout`]
/// follows the one decider ([`crate::log::colour_for`]: a terminal, no `NO_COLOR`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Style {
    pub color: bool,
}

macro_rules! paints {
    ($($name:ident $code:literal),*) => {
        impl Style {
            $(pub fn $name(&self, s: &str) -> String {
                if self.color { format!("\x1b[{}m{s}\x1b[0m", $code) } else { s.to_string() }
            })*
        }
    };
}
paints!(bold "1", dim "2", underline "4", red "31", green "32", yellow "33", blue "34", magenta "35", cyan "36");

impl Style {
    pub fn stdout() -> Self {
        Self { color: crate::log::colour_for(&std::io::stdout()) }
    }
}

/// The terminal cells `s` takes: colour codes and combining marks none,
/// CJK and most emoji two (pure).
pub fn width(s: &str) -> usize {
    let mut n = 0;
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\x1b' && it.peek() == Some(&'[') {
            // A CSI sequence: up to its final letter.
            it.by_ref().find(char::is_ascii_alphabetic);
            continue;
        }
        let cp = c as u32;
        let free =
            cp < 0x20 || cp == 0x7f || cp == 0x200d || (0x300..=0x36f).contains(&cp) || (0xfe00..=0xfe0f).contains(&cp);
        let wide = (0x1100..=0x115f).contains(&cp)
            || ((0x2e80..=0xa4cf).contains(&cp) && cp != 0x303f)
            || (0xac00..=0xd7a3).contains(&cp)
            || (0xf900..=0xfaff).contains(&cp)
            || (0xfe30..=0xfe6f).contains(&cp)
            || (0xff00..=0xff60).contains(&cp)
            || (0xffe0..=0xffe6).contains(&cp)
            || (0x1f300..=0x1f64f).contains(&cp)
            || (0x1f900..=0x1f9ff).contains(&cp);
        n += if free {
            0
        } else if wide {
            2
        } else {
            1
        };
    }
    n
}

/// A column of a [`table`]: its header, and whether it aligns right.
#[derive(Clone, Copy, Debug)]
pub struct Column<'a> {
    pub header: &'a str,
    pub right: bool,
}

impl<'a> Column<'a> {
    pub fn left(header: &'a str) -> Self {
        Self { header, right: false }
    }

    pub fn right(header: &'a str) -> Self {
        Self { header, right: true }
    }
}

/// `rows` as an aligned table under `columns` (pure; the header bold in
/// colour). Widths are terminal cells, so colour in a cell shifts nothing.
pub fn table<S: AsRef<str>>(columns: &[Column], rows: &[Vec<S>], style: Style) -> String {
    let widths: Vec<usize> = columns
        .iter()
        .enumerate()
        .map(|(i, c)| rows.iter().filter_map(|r| r.get(i)).map(|s| width(s.as_ref())).fold(width(c.header), usize::max))
        .collect();
    let line = |cells: Vec<&str>| {
        let padded = cells.iter().zip(columns).zip(&widths).map(|((s, c), w)| {
            let gap = " ".repeat(w.saturating_sub(width(s)));
            if c.right { format!("{gap}{s}") } else { format!("{s}{gap}") }
        });
        padded.collect::<Vec<_>>().join("  ").trim_end().to_string()
    };
    let head = style.bold(&line(columns.iter().map(|c| c.header).collect()));
    let body = rows.iter().map(|r| line((0..columns.len()).map(|i| r.get(i).map_or("", |s| s.as_ref())).collect()));
    [head].into_iter().chain(body).collect::<Vec<_>>().join("\n")
}

type Out = Box<dyn Write + Send>;

/// A progress bar over `total` steps. On a terminal one line drawn in
/// place; on a pipe a plain line at each tenth and at the end.
pub struct Progress {
    out: Out,
    redraw: bool,
    label: String,
    total: u64,
    n: u64,
    tenth: Option<u64>,
    done: bool,
}

impl Progress {
    /// On stderr (stdout stays the command's answer).
    pub fn new(total: u64, label: &str) -> Self {
        Self::to(Box::new(std::io::stderr()), std::io::stderr().is_terminal(), total, label)
    }

    /// On `out` (`redraw`: it is a terminal).
    pub fn to(out: Out, redraw: bool, total: u64, label: &str) -> Self {
        let label = if label.is_empty() { String::new() } else { format!("{label} ") };
        let mut p = Self { out, redraw, label, total, n: 0, tenth: None, done: false };
        p.draw();
        p
    }

    fn pct(&self) -> u64 {
        (self.n * 100).checked_div(self.total).map_or(100, |p| p.min(100))
    }

    fn draw(&mut self) {
        let (pct, label, (n, total)) = (self.pct(), &self.label, (self.n, self.total));
        if self.redraw {
            let filled = (24 * pct as usize).div_ceil(100).min(24);
            let bar = format!("{}{}", "#".repeat(filled), "-".repeat(24 - filled));
            _ = write!(self.out, "\r\x1b[2K{label}[{bar}] {n}/{total} {pct}%");
        } else if self.tenth != Some(pct / 10) {
            self.tenth = Some(pct / 10);
            _ = writeln!(self.out, "{label}{n}/{total} ({pct}%)");
        }
        _ = self.out.flush();
    }

    /// `k` steps on.
    pub fn tick(&mut self, k: u64) {
        self.set(self.n + k);
    }

    /// At step `n`.
    pub fn set(&mut self, n: u64) {
        if !self.done {
            self.n = n.min(self.total);
            self.draw();
        }
    }

    /// At the end, and the line closed; once.
    pub fn done(&mut self) {
        if !self.done {
            self.set(self.total);
            self.done = true;
            if self.redraw {
                _ = writeln!(self.out);
            }
        }
    }
}

const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// A spinner with a label. On a terminal it turns in place (its own thread,
/// which never keeps the process alive); on a pipe it is a `label...` line,
/// and the final line.
pub struct Spinner {
    state: Arc<Mutex<(Out, String, bool)>>,
    redraw: bool,
}

impl Spinner {
    /// On stderr.
    pub fn new(label: &str) -> Self {
        Self::to(Box::new(std::io::stderr()), std::io::stderr().is_terminal(), label)
    }

    /// On `out` (`redraw`: it is a terminal).
    pub fn to(mut out: Out, redraw: bool, label: &str) -> Self {
        if !redraw {
            _ = writeln!(out, "{label}...");
        }
        let state = Arc::new(Mutex::new((out, label.to_string(), false)));
        if redraw {
            let s = state.clone();
            std::thread::spawn(move || {
                for frame in FRAMES.iter().cycle() {
                    let Ok(mut s) = s.lock() else { return };
                    let (out, text, stopped) = &mut *s;
                    if *stopped {
                        return;
                    }
                    _ = write!(out, "\r\x1b[2K{frame} {text}").and_then(|_| out.flush());
                    drop(s);
                    std::thread::sleep(std::time::Duration::from_millis(80));
                }
            });
        }
        Self { state, redraw }
    }

    /// A new label while it turns.
    pub fn update(&self, label: &str) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !s.2 {
            s.1 = label.into();
            if !self.redraw {
                _ = writeln!(s.0, "{label}...");
            }
        }
    }

    /// Stopped, `last` (or the label) the final line; once.
    pub fn stop(&self, last: Option<&str>) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let (out, text, stopped) = &mut *s;
        if !std::mem::replace(stopped, true) {
            let clear = if self.redraw { "\r\x1b[2K" } else { "" };
            _ = writeln!(out, "{clear}{}", last.unwrap_or(text)).and_then(|_| out.flush());
        }
    }
}

/// Dropped, it stops as with [`Spinner::stop`]: no spinner turns on unowned.
impl Drop for Spinner {
    fn drop(&mut self) {
        self.stop(None);
    }
}

/// One frame of a live view (pure): on a terminal the screen cleared first,
/// on a pipe one more block.
pub fn frame(tty: bool, text: &str) -> String {
    if tty { format!("\x1b[2J\x1b[H{text}\n") } else { format!("{text}\n") }
}

/// How long a live view gathers changes into one frame.
pub const TICK: std::time::Duration = std::time::Duration::from_millis(16);

/// A live view on stdout: `render(now)` at once, then one frame per
/// [`TICK`] with changes (the last state of the tick) that `listen` reports
/// (`Handle::listen`, a client's `Remote::listen`):
/// `aio::cli::watch(&h.state(), |f| h.listen(f), render)`. On a terminal
/// the cursor is hidden while it draws, and shown again when the process
/// ends, Ctrl+C too.
pub fn watch<T, F>(now: &T, listen: impl FnOnce(Box<dyn Fn(&Arc<T>) + Send + Sync>), render: F)
where
    T: Send + Sync + 'static,
    F: Fn(&T) -> String + Send + Sync + 'static,
{
    let tty = std::io::stdout().is_terminal();
    let show = move |text: &str| {
        let mut out = std::io::stdout().lock();
        _ = out.write_all(text.as_bytes()).and_then(|_| out.flush());
    };
    if tty {
        cursor::hide();
    }
    show(&frame(tty, &render(now)));
    let (tx, rx) = std::sync::mpsc::channel::<Arc<T>>();
    std::thread::spawn(move || ticks(&rx, |s| show(&frame(tty, &render(s)))));
    let tx = Mutex::new(tx);
    listen(Box::new(move |s| _ = tx.lock().unwrap_or_else(|e| e.into_inner()).send(s.clone())));
}

/// Each change `rx` hears, and those of the [`TICK`] after it, drawn as one
/// frame: the last.
fn ticks<T>(rx: &std::sync::mpsc::Receiver<Arc<T>>, draw: impl Fn(&T)) {
    while let Ok(first) = rx.recv() {
        std::thread::sleep(TICK);
        draw(&rx.try_iter().last().unwrap_or(first));
    }
}

/// The terminal's cursor, hidden for a live view and shown again however
/// the process ends: its exit, Ctrl+C or SIGTERM (an app's own handler
/// still runs after).
mod cursor {
    const SHOW: &[u8] = b"\x1b[?25h";

    pub fn hide() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(on_end);
        use std::io::Write;
        let mut out = std::io::stdout().lock();
        _ = out.write_all(b"\x1b[?25l").and_then(|_| out.flush());
    }

    #[cfg(unix)]
    fn on_end() {
        use std::sync::OnceLock;
        // The handlers there before (an app's, tokio's): run after ours.
        static OLD: [OnceLock<libc::sigaction>; 2] = [OnceLock::new(), OnceLock::new()];
        extern "C" fn show() {
            // write(2) only: what a signal handler may call.
            // SAFETY: write(2) is async-signal-safe; SHOW is a static, valid for its length.
            unsafe { libc::write(1, SHOW.as_ptr().cast(), SHOW.len()) };
        }
        extern "C" fn on_signal(sig: libc::c_int, info: *mut libc::siginfo_t, cx: *mut libc::c_void) {
            show();
            let Some(old) = OLD[usize::from(sig == libc::SIGTERM)].get() else { return };
            // SAFETY: in a signal handler: only async-signal-safe calls (signal, raise) or the handler that was installed before ours, called as its flags say it takes its arguments (SA_SIGINFO: three, else one), with the arguments the kernel gave us.
            unsafe {
                match old.sa_sigaction {
                    libc::SIG_DFL => {
                        libc::signal(sig, libc::SIG_DFL);
                        libc::raise(sig);
                    }
                    libc::SIG_IGN => {}
                    f if old.sa_flags & libc::SA_SIGINFO != 0 => {
                        let f: extern "C" fn(libc::c_int, *mut libc::siginfo_t, *mut libc::c_void) = std::mem::transmute(f);
                        f(sig, info, cx)
                    }
                    f => std::mem::transmute::<libc::sighandler_t, extern "C" fn(libc::c_int)>(f)(sig),
                }
            }
        }
        // SAFETY: process-wide setup, once (Once in the caller): `show` is an extern "C" fn taking nothing, as atexit wants; the sigaction structs are zeroed C structs (all-zero is valid) filled before use.
        unsafe {
            libc::atexit(show);
            for (i, sig) in [libc::SIGINT, libc::SIGTERM].into_iter().enumerate() {
                let mut new: libc::sigaction = std::mem::zeroed();
                new.sa_sigaction = on_signal as extern "C" fn(_, _, _) as libc::sighandler_t;
                new.sa_flags = libc::SA_SIGINFO | libc::SA_RESTART;
                let mut old: libc::sigaction = std::mem::zeroed();
                if libc::sigaction(sig, &new, &mut old) == 0 {
                    if old.sa_sigaction == libc::SIG_IGN {
                        libc::sigaction(sig, &old, std::ptr::null_mut()); // ignored stays ignored
                    }
                    _ = OLD[i].set(old);
                }
            }
        }
    }

    #[cfg(windows)]
    fn on_end() {
        use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
        extern "C" fn show() {
            use std::io::Write;
            _ = std::io::stdout().write_all(SHOW).and_then(|_| std::io::stdout().flush());
        }
        unsafe extern "system" fn on_ctrl(_: u32) -> windows_sys::core::BOOL {
            show();
            0 // and on to the next handler: the app's, else the default end
        }
        unsafe extern "C" {
            fn atexit(f: extern "C" fn()) -> i32;
        }
        // SAFETY: process-wide setup, once (`hide`'s Once): `show` is an extern
        // "C" fn taking nothing, as atexit wants; the console handler is a plain fn.
        unsafe {
            atexit(show);
            SetConsoleCtrlHandler(Some(on_ctrl), 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// notes on the command line
    #[derive(Options, Debug, PartialEq)]
    #[cli(name = "notes", version = "0.1.0", command("add", "add a note"), command("list", "list them"))]
    struct Opts {
        #[cli(command)]
        cmd: String,
        /// its tag
        #[cli(short = 't')]
        tag: Option<String>,
        /// at most this many
        #[cli(default = 20)]
        limit: u32,
        /// answer as JSON
        json: bool,
        port: Option<u16>,
        ratio: Option<f64>,
        also: Vec<u8>,
        #[cli(default = "home")]
        shelf: String,
        dry_run: bool,
        #[cli(positional)]
        file: Option<String>,
        #[cli(rest)]
        text: Vec<String>,
    }

    #[test]
    fn options_derived_from_a_struct_read_as_their_types() {
        let o = Opts::parse(&["add", "-t", "x", "--port", "80", "--also", "1", "--also=2", "--dry-run", "f", "a", "b"]);
        let want = Opts {
            cmd: "add".into(),
            tag: Some("x".into()),
            limit: 20,
            json: false,
            port: Some(80),
            ratio: None,
            also: vec![1, 2],
            shelf: "home".into(),
            dry_run: true,
            file: Some("f".into()),
            text: vec!["a".into(), "b".into()],
        };
        assert_eq!(o, Ok(want));
        let refused = |argv: &[&str]| match Opts::parse(argv) {
            Err(Stop::Refused { why, .. }) => why,
            other => panic!("{other:?}"),
        };
        assert!(refused(&["add", "--port", "70000"]).starts_with("--port expects a u16, got \"70000\""));
        assert!(refused(&["add", "--limit", "1.5"]).starts_with("--limit expects a u32"));
        assert!(refused(&["add", "--also", "300"]).starts_with("--also expects a u8"));
        assert!(refused(&["ad"]).contains("did you mean add"));
        let help = Opts::spec().help();
        for line in ["notes on the command line", "-t, --tag=<text>", "--limit=<number>", "at most this many (default: \"20\")", "--dry-run", "--also=<u8>"] {
            assert!(help.contains(line), "{line} in {help}");
        }
        assert_eq!(Opts::parse(&["--version"]), Err(Stop::Say("notes 0.1.0".into())));
    }

    fn todo() -> Spec {
        Spec::new("todo")
            .about("A todo list.")
            .version("1.2.3")
            .command("list", "show")
            .command("add", "add one")
            .positional("id")
            .rest("text")
            .flag(Flag::text("url").help("server").default("ws://x/ws"))
            .flag(Flag::number("n").short('n').help("count"))
            .flag(Flag::list("tag"))
            .flag(Flag::switch("json").help("json out"))
    }

    fn refused(spec: &Spec, argv: &[&str]) -> (String, bool) {
        match parse(spec, argv) {
            Err(Stop::Refused { why, json }) => (why, json),
            other => panic!("not refused: {other:?}"),
        }
    }

    #[test]
    fn flags_command_positionals_and_rest_are_read_by_name() {
        let a = parse(&todo(), &["add", "7", "--url=ws://y", "-n", "3", "--tag", "a", "x", "--tag=b", "y"]).unwrap();
        assert_eq!(
            (a.command.as_deref(), a.pos("id"), a.rest.clone()),
            (Some("add"), Some("7"), vec!["x".to_string(), "y".into()])
        );
        assert_eq!(
            (a.text("url"), a.number("n"), a.list("tag")),
            (Some("ws://y"), Some(3.0), &["a".into(), "b".into()][..])
        );
        assert!(!a.on("json") && !a.json);
        // Defaults apply; `--` ends the flags.
        let a = parse(&todo(), &["list", "--", "--not-a-flag"]).unwrap();
        assert_eq!((a.text("url"), a.number("n"), a.pos("id")), (Some("ws://x/ws"), None, Some("--not-a-flag")));
        let tags = Spec::new("t").flag(Flag::list("tag").default("all"));
        assert_eq!(parse(&tags, &[] as &[&str]).unwrap().list("tag"), ["all"]);
        assert_eq!(parse(&tags, &["--tag=a"]).unwrap().list("tag"), ["a"], "a given value replaces the default");
    }

    #[test]
    fn a_live_view_draws_one_frame_per_tick_the_last_state() {
        // aio's watch: the changes of one tick, one frame.
        let (tx, rx) = std::sync::mpsc::channel();
        for n in 1..=50 {
            tx.send(Arc::new(n)).unwrap();
        }
        drop(tx);
        let drawn = Mutex::new(vec![]);
        ticks(&rx, |n: &i32| drawn.lock().unwrap().push(*n));
        assert_eq!(*drawn.lock().unwrap(), [50]);
    }

    /// A child test process: hides the cursor, then ends as `AIO_TEST_CURSOR` says.
    #[cfg(unix)]
    #[test]
    fn cursor_child() {
        let Ok(how) = std::env::var("AIO_TEST_CURSOR") else { return };
        cursor::hide();
        if how == "int" {
            // SAFETY: raise(3) on this process, in a child test process made for it.
            unsafe { libc::raise(libc::SIGINT) };
        }
        std::process::exit(3);
    }

    #[cfg(unix)]
    #[test]
    fn a_hidden_cursor_is_shown_again_however_the_process_ends() {
        use std::os::unix::process::{CommandExt, ExitStatusExt};
        // SIGINT as the child starts with it, whatever this test inherited
        // (nohup, a background job): an ignored one stays ignored.
        for (how, disposition, code, sig) in [
            ("exit", libc::SIG_DFL, Some(3), None),
            ("int", libc::SIG_DFL, None, Some(libc::SIGINT)),
            ("int", libc::SIG_IGN, Some(3), None),
        ] {
            let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
            cmd.args(["--exact", "cli::tests::cursor_child", "--nocapture", "--test-threads=1"]).env("AIO_TEST_CURSOR", how);
            // SAFETY: only signal(), async-signal-safe, between fork and exec.
            unsafe {
                cmd.pre_exec(move || {
                    libc::signal(libc::SIGINT, disposition);
                    Ok(())
                });
            }
            let out = cmd.output().unwrap();
            let text = String::from_utf8_lossy(&out.stdout);
            let (hid, shown) = (text.find("\x1b[?25l"), text.rfind("\x1b[?25h"));
            assert!(hid.is_some() && shown > hid, "{how}: {text:?}");
            assert_eq!((out.status.code(), out.status.signal()), (code, sig), "{how}: ends as it would");
        }
    }

    #[test]
    fn a_flag_is_read_as_the_kind_the_spec_gives_it() {
        // aio inferred `a.flags.n: number` from the spec; a misread is caught at once.
        let a = parse(&todo(), &["list", "-n", "3"]).unwrap();
        let panics = |f: fn(&Args)| {
            let a = a.clone();
            std::panic::catch_unwind(move || f(&a)).unwrap_err().downcast::<String>().map(|s| *s).unwrap_or_default()
        };
        assert_eq!(panics(|a| _ = a.text("n")), "--n is a Number flag, read as Text");
        assert_eq!(panics(|a| _ = a.on("url")), "--url is a Text flag, read as Switch");
        assert_eq!(panics(|a| _ = a.number("count")), "--count: the spec has no such flag");
        assert_eq!(panics(|a| _ = a.pos("idd")), "Args::pos(\"idd\"): the spec has no such positional");
        assert_eq!((a.number("n"), a.on("json"), a.pos("id")), (Some(3.0), false, None));
    }

    #[test]
    fn a_wrong_invocation_is_refused_naming_the_fix() {
        let spec = todo();
        let (why, json) = refused(&spec, &["list", "--urk=1"]);
        assert_eq!(why, "unknown flag: --urk (did you mean --url?) (run `todo --help`)");
        assert!(!json);
        for (argv, says) in [
            (&["list", "-q"][..], "unknown flag: -q"),
            (&["lst"], "unknown command: lst (did you mean list?)"),
            (&[], "missing command"),
            (&["list", "-n", "x"], "--n expects a number, got \"x\""),
            (&["list", "--n"], "--n needs a value"),
            (&["list", "--json=1"], "--json takes no value"),
        ] {
            assert!(refused(&spec, argv).0.starts_with(says), "{argv:?}: {}", refused(&spec, argv).0);
        }
        let one = Spec::new("one").positional("id");
        assert!(refused(&one, &["1", "2"]).0.starts_with("unexpected argument: 2"));
        let key = Spec::new("k").flag(Flag::text("key").required());
        assert!(refused(&key, &[] as &[&str]).0.starts_with("--key is required"));
        assert_eq!(parse(&key, &["--key", "k"]).unwrap().text("key"), Some("k"));
    }

    #[test]
    fn json_turns_a_refusal_into_an_error_object_on_stdout() {
        let (why, json) = refused(&todo(), &["--bad", "--json"]);
        assert!(json, "found though after the bad flag");
        assert_eq!(failure(&why, json), (true, format!("{{\"error\":{}}}", serde_json::json!(why))));
        assert_eq!(failure("boom", false), (false, "error: boom".into()));
        // `--json` is only special when the spec has it.
        assert!(!refused(&Spec::new("x"), &["--json"]).1);
        assert_eq!((OK, ERROR, USAGE), (0, 1, 2));
    }

    #[test]
    fn help_is_made_from_the_spec_and_version_says_it() {
        let Err(Stop::Say(h)) = parse(&todo(), &["--help"]) else { panic!() };
        assert_eq!(h, todo().help());
        assert!(h.starts_with("A todo list.\n\nusage: todo <command> <id> [text...] [flags]"), "{h}");
        assert!(h.contains("commands:\n  list  show\n  add   add one"), "{h}");
        assert!(h.contains("-n, --n=<number>  count"), "{h}");
        assert!(h.contains("    --url=<text>  server (default: \"ws://x/ws\")"), "{h}");
        assert!(h.contains("--json") && h.contains("--help") && h.contains("--version"), "{h}");
        assert_eq!(parse(&todo(), &["-h"]), Err(Stop::Say(h)));
        assert_eq!(parse(&todo(), &["--version"]), Err(Stop::Say("todo 1.2.3".into())));
        assert!(refused(&Spec::new("x"), &["--version"]).0.starts_with("unknown flag: --version"));
    }

    fn prompt(input: &str, tty: bool) -> Prompt<&[u8], Vec<u8>> {
        Prompt { input: input.as_bytes(), out: vec![], tty }
    }

    #[test]
    fn prompts_read_answers_with_defaults_and_ask_again() {
        let mut p = prompt("alice\ny\n3\nsecret\r\n", true);
        assert_eq!(p.ask("name", None).unwrap(), "alice");
        assert!(p.confirm("ok?", false).unwrap());
        assert_eq!(p.select("pick", &["a", "b", "c"], None).unwrap(), "c");
        assert_eq!(p.password("pw").unwrap(), "secret");
        let said = String::from_utf8(p.out).unwrap();
        assert!(said.contains("name: ") && said.contains("ok? [y/N]: ") && said.contains("  3) c\n"), "{said}");
        assert!(said.contains("choose 1-3: ") && !said.contains("secret"), "{said}");

        let mut p = prompt("\n\n\nn\n\n\nb\n\nmaybe\ny\nzz\n1\n", true);
        assert_eq!(p.ask("q", Some("d")).unwrap(), "d");
        assert!(p.confirm("c", true).unwrap());
        assert!(!p.confirm("c", false).unwrap());
        assert!(!p.confirm("c", false).unwrap(), "n");
        assert_eq!(p.select("s", &["a", "b"], Some("b")).unwrap(), "b");
        assert_eq!(p.select("s", &["a", "b"], None).unwrap(), "b", "by name, after an empty answer");
        assert_eq!(p.ask("q", None).unwrap(), "maybe", "an empty answer without a default asks again");
        assert!(p.confirm("c", false).unwrap());
        assert_eq!(p.select("s", &["a", "b"], None).unwrap(), "a");
        let said = String::from_utf8(p.out).unwrap();
        assert!(said.contains("not a choice: \"zz\"") && said.contains("  2) b (default)"), "{said}");
    }

    #[test]
    fn without_a_terminal_a_prompt_refuses_and_reads_nothing() {
        let mut p = prompt("would-be-answer\n", false);
        let e = p.ask("q", None).unwrap_err().why;
        assert_eq!(
            e,
            "ask(\"q\") needs a terminal, and stdin is not one: pass the value as a flag, or run from a terminal"
        );
        assert!(p.confirm("q", true).unwrap_err().why.starts_with("confirm(\"q\") needs a terminal"));
        assert!(p.select("q", &["a"], None).unwrap_err().why.starts_with("select(\"q\") needs a terminal"));
        assert!(p.password("q").unwrap_err().why.starts_with("password(\"q\") needs a terminal"));
        assert!(p.out.is_empty(), "a refusal asks nothing");
        assert_eq!(p.input, b"would-be-answer\n", "stdin untouched");
        // Closed input is an error, not an empty answer.
        let mut p = prompt("", true);
        assert_eq!(p.ask("q", Some("d")).unwrap_err().why, "no answer to \"q\": stdin closed");
        assert_eq!(p.select("q", &[], None).unwrap_err().why, "select(\"q\"): no choices");
    }

    #[test]
    fn a_table_aligns_by_terminal_cells_and_bolds_its_header_in_colour() {
        let cols = [Column::right("id"), Column::left("what")];
        let rows = vec![vec!["1", "a"], vec!["10", "longer"]];
        assert_eq!(table(&cols, &rows, Style { color: false }), "id  what\n 1  a\n10  longer");
        let lit = Style { color: true };
        let coloured = vec![vec![lit.green("1"), "日本".into()], vec!["10".into(), "x".into()]];
        assert_eq!(table(&cols, &coloured, Style { color: false }), "id  what\n \x1b[32m1\x1b[0m  日本\n10  x");
        assert!(table(&cols, &rows, lit).starts_with("\x1b[1mid  what\x1b[0m\n"));
        assert_eq!((width("\x1b[1mab\x1b[0m"), width("日本"), width("e\u{301}")), (2, 4, 1));
        assert_eq!(Style { color: false }.red("x"), "x");
    }

    /// A stream a test reads back.
    #[derive(Clone, Default)]
    struct Seen(Arc<Mutex<Vec<u8>>>);

    impl Write for Seen {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Seen {
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    #[test]
    fn progress_redraws_a_terminal_line_and_tells_a_pipe_each_tenth() {
        let seen = Seen::default();
        let mut p = Progress::to(Box::new(seen.clone()), false, 20, "copy");
        (0..20).for_each(|_| p.tick(1));
        p.done();
        p.done();
        let lines: Vec<String> = seen.text().lines().map(str::to_string).collect();
        assert_eq!(lines.len(), 11, "{lines:?}");
        assert_eq!((lines[0].as_str(), lines[10].as_str()), ("copy 0/20 (0%)", "copy 20/20 (100%)"));

        let seen = Seen::default();
        let mut p = Progress::to(Box::new(seen.clone()), true, 4, "");
        p.set(2);
        p.done();
        assert!(seen.text().contains("\r\x1b[2K[############------------] 2/4 50%"), "{:?}", seen.text());
        assert!(seen.text().ends_with("4/4 100%\n"));
    }

    #[test]
    fn a_spinner_on_a_pipe_is_plain_lines_and_on_a_terminal_turns_then_clears() {
        let seen = Seen::default();
        let s = Spinner::to(Box::new(seen.clone()), false, "fetching");
        s.update("parsing");
        s.stop(Some("done"));
        s.stop(None);
        assert_eq!(seen.text(), "fetching...\nparsing...\ndone\n");

        let seen = Seen::default();
        let s = Spinner::to(Box::new(seen.clone()), true, "work");
        std::thread::sleep(std::time::Duration::from_millis(200));
        s.stop(None);
        let t = seen.text();
        assert!(t.contains("\r\x1b[2K⠋ work") && t.ends_with("\r\x1b[2Kwork\n"), "{t:?}");
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(seen.text(), t, "nothing drawn after stop");
    }

    #[test]
    fn a_live_frame_clears_a_terminal_and_appends_to_a_pipe() {
        assert_eq!(frame(true, "a"), "\x1b[2J\x1b[Ha\n");
        assert_eq!(frame(false, "a"), "a\n");
    }
}
