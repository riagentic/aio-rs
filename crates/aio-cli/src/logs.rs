//! `ax logs`: the app's log as events, filtered. An event is a stamped
//! line and the lines that continue it (indented: a backtrace frame, a box).
//! `--level warn` keeps warnings and worse, `--tag net` that category and
//! its `net:*` children, `--since 15m` what is newer, a word what holds it,
//! `--client` the window's lines (`[page]`, `[aio-shell]`). An event whose
//! head can't be read passes every filter: dropping what can't be judged is
//! how a filter hides the one line that mattered (aio field reports).
use aio::log::Level;
use aio::serde_json::json;
use std::io::{Read, Seek};
use std::path::Path;

type Out = Result<String, String>;

/// What a stamped line opens with: its time, level word and `[tag]`.
#[derive(Debug, PartialEq)]
pub struct Head {
    pub at: i64,
    pub level: Option<Level>,
    pub tag: Option<String>,
}

const LEVELS: [(&str, Level); 5] = [
    ("trace", Level::Trace),
    ("debug", Level::Debug),
    ("info", Level::Info),
    ("warn", Level::Warn),
    ("error", Level::Error),
];

fn level(word: &str) -> Option<Level> {
    LEVELS.iter().find(|(w, _)| w.eq_ignore_ascii_case(word)).map(|(_, l)| *l)
}

/// Ms since the epoch of an RFC 3339 time (`2026-10-27T23:01:02.123Z`).
fn utc(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s).ok().map(|t| t.timestamp_millis())
}

/// A file line split into its stamp and its text (pure).
fn split(line: &str) -> Option<(i64, &str)> {
    let (stamp, text) = line.split_once(' ')?;
    Some((utc(stamp)?, text))
}

/// The head of a line that starts an event; `None` for one that continues
/// the event above it, or has no stamp (pure).
pub fn head(line: &str) -> Option<Head> {
    let (at, text) = split(line)?;
    if text.starts_with("  ") || text.starts_with(|c| ('\u{2500}'..='\u{257f}').contains(&c)) {
        return None;
    }
    let (level, rest) = match text.split_once(' ') {
        Some((w, rest)) if w.chars().all(|c| c.is_ascii_uppercase()) && level(w).is_some() => {
            (level(w), rest.trim_start())
        }
        _ => (None, text),
    };
    let tag = rest.strip_prefix('[').and_then(|r| r.split_once(']')).map(|(t, _)| t.to_string());
    Some(Head { at, level, tag })
}

/// Lines grouped into events, oldest first; blank lines are none (pure).
pub fn events(lines: &[&str]) -> Vec<Vec<String>> {
    let mut out: Vec<Vec<String>> = Vec::new();
    for l in lines.iter().filter(|l| !l.trim().is_empty()) {
        match out.last_mut() {
            Some(e) if head(l).is_none() && split(l).is_some() => e.push(l.to_string()),
            _ => out.push(vec![l.to_string()]),
        }
    }
    out
}

/// What `ax logs` keeps.
#[derive(Debug, Default)]
pub struct Filter {
    pub level: Option<Level>,
    pub tag: Option<String>,
    pub since: Option<i64>,
    pub text: Option<String>,
    pub client: bool,
}

impl Filter {
    /// Does `e` pass? A head that says nothing about a filter passes it.
    pub fn keeps(&self, e: &[String]) -> bool {
        let Some(h) = e.first().and_then(|l| head(l)) else { return true };
        let tag = h.tag.as_deref();
        let in_tag = |want: &str| tag.is_none_or(|t| t == want || t.starts_with(&format!("{want}:")));
        self.level.is_none_or(|min| h.level.is_none_or(|l| l >= min))
            && self.tag.as_deref().is_none_or(in_tag)
            && self.since.is_none_or(|s| h.at >= s)
            && (!self.client || matches!(tag, Some("page" | "aio-shell")))
            && self.text.as_deref().is_none_or(|w| {
                let w = w.to_lowercase();
                e.iter().any(|l| l.to_lowercase().contains(&w))
            })
    }

    /// Kept only because its head can't be judged (no level for `--level`,
    /// no tag for `--tag`): not a match, so not counted as one.
    fn unjudged(&self, e: &[String]) -> bool {
        let Some(h) = e.first().and_then(|l| head(l)) else { return true };
        (self.level.is_some() && h.level.is_none()) || (self.tag.is_some() && h.tag.is_none())
    }
}

/// `--since`: a duration back from `now` (`30s`, `15m`, `2h`, `1d`) or a
/// time (`2026-10-28`, `2026-10-28T10:00` local, `…Z`, `…+02:00`); `None`
/// for anything else, a bare number included (a unit forgotten). Pure but
/// for the local zone.
pub fn since(raw: &str, now: i64) -> Option<i64> {
    let raw = raw.trim();
    let unit = match raw.chars().last()? {
        's' => Some(1_000),
        'm' => Some(60_000),
        'h' => Some(3_600_000),
        'd' => Some(86_400_000),
        _ => None,
    };
    if let Some(unit) = unit
        && let Ok(n) = raw[..raw.len() - 1].trim().parse::<i64>()
    {
        return Some(now - n.checked_mul(unit)?);
    }
    if let Some(t) = utc(raw) {
        return Some(t);
    }
    use chrono::{NaiveDate, NaiveDateTime, TimeZone};
    let t = ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%dT%H:%M", "%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M"]
        .iter()
        .find_map(|f| NaiveDateTime::parse_from_str(raw, f).ok())
        .or_else(|| NaiveDate::parse_from_str(raw, "%Y-%m-%d").ok()?.and_hms_opt(0, 0, 0))?;
    chrono::Local.from_local_datetime(&t).earliest().map(|t| t.timestamp_millis())
}

/// The flags `ax logs` takes, parsed; unknown ones are refused (else a
/// mistyped `--levle` would read as "nothing happened").
pub fn filter(args: &[&str], now: i64) -> Result<Filter, String> {
    let value = |f: &str| args.iter().position(|a| *a == f).map(|i| args.get(i + 1).copied());
    let known = [
        "--last",
        "--level",
        "--tag",
        "--since",
        "--client",
        "--follow",
        "-f",
        "--json",
        "--home",
        "--profile",
        "--instance",
    ];
    if let Some(bad) = args.iter().find(|a| a.starts_with('-') && !known.contains(a)) {
        return Err(format!("ax logs: no flag {bad} (flags: {})", known.join(" ")));
    }
    let mut f = Filter { client: args.contains(&"--client"), ..Filter::default() };
    if let Some(v) = value("--level") {
        let v = v.ok_or("--level needs a level")?;
        f.level = Some(level(v).ok_or_else(|| {
            format!("--level {v} is not a level: trace, debug, info, warn or error (that level and worse)")
        })?);
    }
    if let Some(v) = value("--tag") {
        f.tag = Some(v.ok_or("--tag needs a category, like --tag net")?.into());
    }
    if let Some(v) = value("--since") {
        let v = v.ok_or("--since needs a time")?;
        let hint = if v.chars().all(|c| c.is_ascii_digit()) { format!(" (add a unit: {v}m)") } else { String::new() };
        f.since = Some(since(v, now).ok_or_else(|| {
            format!("--since {v} is not a time: a duration (30s, 15m, 2h, 1d) or a time (2026-10-28T10:00){hint}")
        })?);
    }
    Ok(f)
}

/// The log files in `dir`, oldest first (`app.4.log` … `app.log`).
fn files(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out: Vec<_> = (0..5)
        .map(|i| dir.join(if i == 0 { "app.log".to_string() } else { format!("app.{i}.log") }))
        .take_while(|p| p.exists())
        .collect();
    out.reverse();
    out
}

/// The last `n` events `f` keeps across the rotated files, oldest first;
/// `json`: with the counts (events: `matched` read and kept, `unclassified`
/// kept unjudged; they add up to `total`).
pub fn tail(dir: &Path, n: usize, f: &Filter, json: bool) -> Out {
    let text: String = files(dir).iter().filter_map(|p| std::fs::read_to_string(p).ok()).collect();
    if text.is_empty() {
        return Err(format!("no logs in {}", dir.display()));
    }
    let kept: Vec<Vec<String>> = events(&text.lines().collect::<Vec<_>>()).into_iter().filter(|e| f.keeps(e)).collect();
    let shown = &kept[kept.len().saturating_sub(n)..];
    let lines: Vec<&str> = shown.iter().flatten().map(String::as_str).collect();
    if !json {
        return Ok(lines.join("\n"));
    }
    let unclassified = kept.iter().filter(|e| f.unjudged(e)).count();
    Ok(json!({
        "total": kept.len(),
        "matched": kept.len() - unclassified,
        "unclassified": unclassified,
        "shown": shown.len(),
        "lines": lines,
    })
    .to_string())
}

/// What `--follow` carries from one read to the next: the last event, and
/// whether it was printed (its continuation may come in the next read).
#[derive(Debug, Default)]
pub struct Carry {
    last: Vec<String>,
    shown: bool,
}

/// The lines to print for `new` lines, judged as whole events with the
/// carried one joined back on (pure).
pub fn follow(carry: Carry, new: &[&str], f: &Filter) -> (Vec<String>, Carry) {
    let old = carry.last.len();
    let mut lines: Vec<&str> = carry.last.iter().map(String::as_str).collect();
    lines.extend(new);
    let all = events(&lines);
    let mut out = Vec::new();
    let mut next = carry;
    for (i, e) in all.into_iter().enumerate() {
        let joined = i == 0 && old > 0 && e.len() >= old;
        let ok = f.keeps(&e);
        if ok {
            out.extend(if joined && next.shown { e[old..].to_vec() } else { e.clone() });
        }
        next = Carry { shown: ok || (joined && next.shown), last: e };
    }
    (out, next)
}

/// `--follow`: where in `app.log` it is, a line not yet whole, and the last
/// event.
pub struct Tailer {
    at: u64,
    part: Vec<u8>,
    carry: Carry,
}

impl Tailer {
    /// The tail's lines (as [`tail`]), and a tailer from the end of what was read.
    pub fn start(dir: &Path, n: usize, f: &Filter) -> (Vec<String>, Self) {
        let mut bytes = Vec::new();
        for p in files(dir).iter().filter(|p| !p.ends_with("app.log")) {
            read_from(p, 0, &mut bytes);
        }
        // The bytes read, not a later size: a line written in between is not skipped.
        let at = read_from(&dir.join("app.log"), 0, &mut bytes);
        let end = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
        let part = bytes.split_off(end);
        let text = String::from_utf8_lossy(&bytes);
        let all = events(&text.lines().collect::<Vec<_>>());
        let kept: Vec<&Vec<String>> = all.iter().filter(|e| f.keeps(e)).collect();
        let out = kept[kept.len().saturating_sub(n)..].iter().flat_map(|e| e.iter().cloned()).collect();
        let carry = all.last().map_or(Carry::default(), |e| Carry { shown: n > 0 && f.keeps(e), last: e.clone() });
        (out, Tailer { at, part, carry })
    }

    /// The lines to print for what was written since. A rotation is seen as
    /// `app.log` shrinking: the rest of what it was is read from `app.1.log`.
    pub fn poll(&mut self, dir: &Path, f: &Filter) -> Vec<String> {
        let live = dir.join("app.log");
        let len = std::fs::metadata(&live).map_or(0, |m| m.len());
        if len < self.at {
            read_from(&dir.join("app.1.log"), self.at, &mut self.part);
            self.at = 0;
        }
        self.at += read_from(&live, self.at, &mut self.part);
        // Whole lines only: the rest waits for its newline.
        let Some(end) = self.part.iter().rposition(|b| *b == b'\n') else { return vec![] };
        let done: Vec<u8> = self.part.drain(..=end).collect();
        let text = String::from_utf8_lossy(&done);
        let (out, next) = follow(std::mem::take(&mut self.carry), &text.lines().collect::<Vec<_>>(), f);
        self.carry = next;
        out
    }
}

/// Print the tail, then each new event as it is written, until stopped.
pub fn watch(dir: &Path, n: usize, f: &Filter) -> Out {
    let (out, mut t) = Tailer::start(dir, n, f);
    out.iter().for_each(|l| println!("{l}"));
    loop {
        std::thread::sleep(std::time::Duration::from_millis(200));
        t.poll(dir, f).iter().for_each(|l| println!("{l}"));
    }
}

/// Append what `p` holds past `at` to `buf`; how much that was.
fn read_from(p: &Path, at: u64, buf: &mut Vec<u8>) -> u64 {
    let Ok(mut file) = std::fs::File::open(p) else { return 0 };
    if file.seek(std::io::SeekFrom::Start(at)).is_err() {
        return 0;
    }
    file.read_to_end(buf).map_or(0, |n| n as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real lines as `aio::log` writes them.
    const LOG: &str = "2026-09-07T00:56:32.797Z WARN  [checkpoint] snapshot is old
2026-09-07T00:56:32.811Z INFO  [aio] state (3 keys)
2026-09-07T00:56:32.850Z INFO  [cell:notes] ready
2026-09-07T01:10:00.000Z ERROR [cell:notes] save failed
2026-09-07T01:10:00.000Z    0: notes::save at src/notes.rs:12:3
2026-09-07T01:11:00.000Z DEBUG [cell:todo] tick
";

    fn evs() -> Vec<Vec<String>> {
        events(&LOG.lines().collect::<Vec<_>>())
    }

    fn kept(f: &Filter) -> Vec<Vec<String>> {
        evs().into_iter().filter(|e| f.keeps(e)).collect()
    }

    fn lvl(l: Level) -> Filter {
        Filter { level: Some(l), ..Filter::default() }
    }

    #[test]
    fn a_level_means_that_level_and_worse() {
        assert_eq!(evs().len(), 5, "the fixture is five events");
        let warn = kept(&lvl(Level::Warn));
        assert_eq!(
            warn.iter().map(|e| head(&e[0]).unwrap().level).collect::<Vec<_>>(),
            [Some(Level::Warn), Some(Level::Error)]
        );
        assert_eq!(kept(&lvl(Level::Error)).len(), 1);
        assert_eq!(kept(&lvl(Level::Debug)).len(), 5, "debug is everything");
    }

    #[test]
    fn an_error_keeps_its_backtrace() {
        let e = kept(&lvl(Level::Error));
        assert!(e[0].iter().any(|l| l.contains("src/notes.rs:12:3")), "{e:?}");
    }

    #[test]
    fn a_tag_is_a_namespace_not_a_substring() {
        let tag = |t: &str| kept(&Filter { tag: Some(t.into()), ..Filter::default() }).len();
        assert_eq!(tag("cell:notes"), 2);
        assert_eq!(tag("cell"), 3, "cell keeps cell:notes and cell:todo");
        assert_eq!(tag("che"), 0, "not `checkpoint`");
        assert_eq!(tag("nosuchtag"), 0);
    }

    #[test]
    fn since_takes_a_duration_or_a_time() {
        let now = utc("2026-09-07T01:00:00Z").unwrap();
        assert_eq!(since("30s", now), Some(now - 30_000));
        assert_eq!(since("15m", now), Some(now - 900_000));
        assert_eq!(since("2h", now), Some(now - 7_200_000));
        assert_eq!(since("1d", now), Some(now - 86_400_000));
        assert_eq!(since("2026-09-07T03:00:00+02:00", 0), Some(now));
        assert_eq!(since(" 2026-09-07T01:00:00Z ", 0), Some(now));
        assert!(since("2026-09-07T03:00", 0).is_some(), "a local time");
        assert!(since("2026-09-07", 0).is_some(), "a date");
        for bad in ["yesterday", "", "15", "m", "soon", "5", "90", "1 5"] {
            assert_eq!(since(bad, now), None, "{bad:?} was taken as a time");
        }
        let f = Filter { since: Some(now), ..Filter::default() };
        assert_eq!(kept(&f).len(), 2, "only 01:10 and 01:11 are newer");
    }

    #[test]
    fn what_cant_be_judged_passes_and_is_counted_apart() {
        let raw = vec!["2026-09-07T01:00:00.000Z [aio] memory is high".to_string()];
        for f in [lvl(Level::Error), Filter { since: Some(i64::MAX), ..Filter::default() }] {
            // `since` judges any stamp; the level of a raw line can't be.
            assert_eq!(f.keeps(&raw), f.since.is_none(), "{f:?}");
        }
        assert!(Filter { tag: Some("aio".into()), ..Filter::default() }.keeps(&raw));
        assert!(!Filter { tag: Some("net".into()), ..Filter::default() }.keeps(&raw));
        let unstamped = vec!["some raw write".to_string()];
        assert!(
            lvl(Level::Error).keeps(&unstamped)
                && Filter { tag: Some("x".into()), ..Filter::default() }.keeps(&unstamped)
        );
        // The counts: the raw line is kept, not matched.
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("app.log"), format!("{LOG}{}\n", raw[0])).unwrap();
        let j: aio::serde_json::Value =
            aio::serde_json::from_str(&tail(d.path(), 50, &lvl(Level::Error), true).unwrap()).unwrap();
        assert_eq!(
            (j["total"].as_u64(), j["matched"].as_u64(), j["unclassified"].as_u64()),
            (Some(2), Some(1), Some(1))
        );
        assert_eq!(j["lines"].as_array().unwrap().len(), 3, "the error, its frame and the raw line");
    }

    #[test]
    fn blank_lines_are_no_events() {
        assert!(events(&["", "  ", ""]).is_empty());
        assert_eq!(events(&LOG.split('\n').collect::<Vec<_>>()).len(), 5, "the closing newline is no sixth");
        let none = Filter { tag: Some("nosuchtag".into()), ..Filter::default() };
        assert_eq!(evs().iter().filter(|e| none.keeps(e)).count(), 0);
    }

    #[test]
    fn a_word_matches_the_whole_event_and_client_picks_the_window() {
        let f = Filter { text: Some("NOTES.RS".into()), ..Filter::default() };
        assert_eq!(kept(&f).len(), 1, "a word in the backtrace keeps the error");
        let lines = [
            "2026-09-07T01:00:00.000Z [page] Uncaught TypeError: x is undefined",
            "2026-09-07T01:00:00.001Z [aio-shell] load failed -3",
            "2026-09-07T01:00:00.002Z INFO  [app] saved",
        ];
        let client = Filter { client: true, ..Filter::default() };
        assert_eq!(events(&lines).iter().filter(|e| client.keeps(e)).count(), 2);
    }

    #[test]
    fn flags_are_read_and_unknown_ones_refused() {
        let f = filter(&["--level", "WARN", "--tag", "net", "--since", "15m", "--client"], 1_000_000_000).unwrap();
        assert_eq!(
            (f.level, f.tag.as_deref(), f.since, f.client),
            (Some(Level::Warn), Some("net"), Some(999_100_000), true)
        );
        assert!(filter(&["--levle", "warn"], 0).unwrap_err().contains("no flag --levle"));
        assert!(filter(&["--level", "loud"], 0).unwrap_err().contains("not a level"));
        assert!(filter(&["--since", "15"], 0).unwrap_err().contains("add a unit: 15m"));
        assert!(filter(&["--since"], 0).is_err());
    }

    #[test]
    fn follow_joins_an_event_split_across_reads() {
        let f = lvl(Level::Error);
        // An INFO line, then its frame in the next read: the frame isn't an error.
        let (out, carry) = follow(Carry::default(), &["2026-09-07T01:00:00.000Z INFO  [app] fine"], &f);
        assert!(out.is_empty());
        let (out, carry) = follow(carry, &["2026-09-07T01:00:00.000Z    0: frame"], &f);
        assert!(out.is_empty(), "the stack of an INFO line printed: {out:?}");
        // An error, then its frame: the frame alone follows what was printed.
        let (out, carry) = follow(carry, &["2026-09-07T01:00:01.000Z ERROR [app] broke"], &f);
        assert_eq!(out.len(), 1);
        let (out, _) =
            follow(carry, &["2026-09-07T01:00:01.000Z    0: frame", "2026-09-07T01:00:02.000Z INFO  [app] x"], &f);
        assert_eq!(out, ["2026-09-07T01:00:01.000Z    0: frame"]);
    }

    #[test]
    fn the_tail_reads_across_rotated_files_and_says_when_there_is_none() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("app.1.log"),
            "2026-09-07T01:00:00.000Z INFO  [app] a\n2026-09-07T01:00:01.000Z INFO  [app] b\n",
        )
        .unwrap();
        std::fs::write(d.path().join("app.log"), "2026-09-07T01:00:02.000Z INFO  [app] c\n").unwrap();
        let all = Filter::default();
        assert_eq!(
            tail(d.path(), 2, &all, false).unwrap().lines().map(|l| &l[l.len() - 1..]).collect::<Vec<_>>(),
            ["b", "c"]
        );
        assert_eq!(tail(d.path(), 50, &all, false).unwrap().lines().count(), 3);
        assert!(tail(&d.path().join("none"), 5, &all, false).is_err());
    }

    #[test]
    fn follow_keeps_streaming_through_a_rotation() {
        use std::io::Write;
        let d = tempfile::tempdir().unwrap();
        let (log, old) = (d.path().join("app.log"), d.path().join("app.1.log"));
        let add = |p: &Path, s: &str| {
            std::fs::OpenOptions::new().create(true).append(true).open(p).unwrap().write_all(s.as_bytes()).unwrap()
        };
        let line = |m: &str| format!("2026-09-07T01:00:00.000Z INFO  [app] {m}\n");
        add(&log, &line("first"));
        let all = Filter::default();
        let (out, mut t) = Tailer::start(d.path(), 1, &all);
        assert!(out[0].ends_with("first"), "{out:?}");
        // Half a line waits for its newline.
        add(&log, "2026-09-07T01:00:00.000Z INFO  [app] hal");
        assert!(t.poll(d.path(), &all).is_empty());
        add(&log, "f\n");
        assert!(t.poll(d.path(), &all)[0].ends_with("half"));
        // Written, then rotated by rename before the next look: nothing lost.
        add(&log, &line("last of the old file"));
        std::fs::rename(&log, &old).unwrap();
        add(&log, &line("new"));
        let out = t.poll(d.path(), &all);
        assert_eq!(out.len(), 2, "{out:?}");
        assert!(out[0].ends_with("last of the old file") && out[1].ends_with("new"), "{out:?}");
    }
}
