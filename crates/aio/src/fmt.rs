//! One presentation vocabulary for numbers a human reads (all pure): sizes,
//! durations, counts. Every line the framework prints uses these, so an app's
//! own output can line up with it. `perf::over` prints a value over a limit.

/// Bytes in binary units, always one decimal above bytes so a column lines
/// up on the point: `912 B`, `214.3 KB`, `15.0 GB`.
pub fn bytes(n: u64) -> String {
    if n < 1024 {
        return format!("{n} B");
    }
    const UNITS: [&str; 5] = ["KB", "MB", "GB", "TB", "PB"];
    let (mut v, mut i) = (n as f64 / 1024.0, 0);
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.1} {}", UNITS[i])
}

/// A duration compared at a glance: `840ms`, `8.4s`, `45s`, `2m 03s`,
/// `13h 33m`, `4d 4h`.
pub fn dur(d: std::time::Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 {
        return format!("{ms}ms");
    }
    let s = d.as_secs_f64();
    if s < 10.0 {
        return format!("{s:.1}s");
    }
    if s < 60.0 {
        return format!("{}s", s.round());
    }
    let (s, m) = (d.as_secs() % 60, d.as_secs() / 60);
    if m < 60 {
        return format!("{m}m {s:02}s");
    }
    let h = m / 60;
    if h < 48 { format!("{h}h {:02}m", m % 60) } else { format!("{}d {}h", h / 24, h % 24) }
}

/// `1 app`, `4 apps`, `2 entries` (`many` for a plural that is not `+s`).
pub fn count(n: u64, one: &str, many: Option<&str>) -> String {
    match (n, many) {
        (1, _) => format!("{n} {one}"),
        (_, Some(m)) => format!("{n} {m}"),
        _ => format!("{n} {one}s"),
    }
}

/// `3 ok · 1 failed`: zero parts dropped, so a clean run says `5 ok`, and
/// nothing at all says "".
pub fn tally(parts: &[(u64, &str)]) -> String {
    parts.iter().filter(|(n, _)| *n > 0).map(|(n, w)| format!("{n} {w}")).collect::<Vec<_>>().join(" · ")
}

/// A long list as its head and a count of the rest: `a b c +28`, never an
/// ellipsis that hides how much is hidden.
pub fn fold<S: AsRef<str>>(items: &[S], max: usize) -> String {
    let mut out: Vec<&str> = items.iter().take(max).map(AsRef::as_ref).collect();
    let more = items.len().saturating_sub(max);
    let rest = format!("+{more}");
    if more > 0 {
        out.push(&rest);
    }
    out.join(" ")
}

/// Blocks with one blank line between them, empty ones dropped.
pub fn stack(blocks: &[&str]) -> String {
    blocks.iter().filter(|b| !b.is_empty()).copied().collect::<Vec<_>>().join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn units_a_human_can_compare() {
        assert_eq!(bytes(912), "912 B");
        assert_eq!(bytes(219_442), "214.3 KB");
        assert_eq!(bytes(92_274_688), "88.0 MB");
        assert_eq!(bytes(15 << 30), "15.0 GB");
        assert_eq!(bytes(u64::MAX), "16384.0 PB");
        let ms = Duration::from_millis;
        let all = [840, 8400, 45_000, 123_000, 48_780_000, 360_000_000].map(|m| dur(ms(m)));
        assert_eq!(all, ["840ms", "8.4s", "45s", "2m 03s", "13h 33m", "4d 4h"]);
        assert_eq!((count(1, "app", None), count(4, "app", None)), ("1 app".into(), "4 apps".into()));
        assert_eq!(count(2, "entry", Some("entries")), "2 entries");
        assert_eq!(count(0, "app", None), "0 apps");
    }

    #[test]
    fn lists_fold_tallies_drop_zeros_blocks_stack() {
        assert_eq!(tally(&[(3, "ok"), (0, "skipped"), (1, "failed")]), "3 ok · 1 failed");
        assert_eq!((tally(&[(5, "ok")]), tally(&[(0, "ok")])), ("5 ok".into(), String::new()));
        let v: Vec<String> = (0..34).map(|i| format!("a{i}")).collect();
        assert_eq!(fold(&v, 6), "a0 a1 a2 a3 a4 a5 +28");
        assert_eq!((fold(&["a", "b"], 6), fold::<&str>(&[], 6)), ("a b".into(), String::new()));
        assert_eq!(stack(&["a", "", "b", "c"]), "a\n\nb\n\nc");
        assert_eq!(stack(&["", ""]), "");
    }
}
