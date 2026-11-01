//! Cron schedules: `minute hour day-of-month month day-of-week`, in the
//! machine's local time. Fields take `*`, numbers, `a-b`, lists and `/step`;
//! months and weekdays take names (`jan`, `mon`); weekday 0 and 7 are Sunday.
//! As in cron, when both days are restricted, either one matching is enough.
//!
//! Clock rules: a time a DST change skips runs once, at the first minute that
//! exists after it; a time that happens twice runs once, the first time. The
//! next run is always found after the last one, so a clock step back never
//! runs a slot twice (and after sleep a missed run fires once: the scheduler).
use chrono::{Datelike, Duration, Local, LocalResult, NaiveDate, NaiveDateTime, TimeZone, Timelike};

/// A parsed expression: the allowed values of each field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cron {
    minutes: u64,
    hours: u32,
    days: u32,
    months: u16,
    weekdays: u8,
    /// Day-of-month / day-of-week given (not `*`): cron's either-or rule.
    days_set: bool,
    weekdays_set: bool,
}

const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
const DAYS: [&str; 7] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];

/// One field as a bit set over `lo..=hi`.
fn field(s: &str, lo: u32, hi: u32, names: &[&str], name_base: u32) -> Result<u64, String> {
    let num = |t: &str| -> Result<u32, String> {
        let t = t.to_ascii_lowercase();
        if let Some(i) = names.iter().position(|n| *n == t) {
            return Ok(i as u32 + name_base);
        }
        t.parse().map_err(|_| format!("`{t}` is not a number{}", if names.is_empty() { "" } else { " or name" }))
    };
    let mut bits = 0u64;
    for part in s.split(',') {
        let (range, step) = match part.split_once('/') {
            Some((r, st)) => (r, st.parse::<u32>().map_err(|_| format!("bad step in `{part}`"))?),
            None => (part, 1),
        };
        if step == 0 {
            return Err(format!("step 0 in `{part}`"));
        }
        let (a, b) = match range {
            "*" => (lo, hi),
            r => match r.split_once('-') {
                Some((a, b)) => (num(a)?, num(b)?),
                // `5/15` means from 5 on.
                None if part.contains('/') => (num(r)?, hi),
                None => (num(r)?, num(r)?),
            },
        };
        if a < lo || b > hi || a > b {
            return Err(format!("`{part}` is outside {lo}-{hi}"));
        }
        for v in (a..=b).step_by(step as usize) {
            bits |= 1 << v;
        }
    }
    Ok(bits)
}

impl Cron {
    pub fn parse(expr: &str) -> Result<Self, String> {
        let f: Vec<&str> = expr.split_whitespace().collect();
        let [mi, h, dom, mo, dow] = f[..] else {
            return Err(format!("cron `{expr}`: 5 fields (minute hour day month weekday)"));
        };
        let bad = |e: String| format!("cron `{expr}`: {e}");
        let mut weekdays = field(dow, 0, 7, &DAYS, 0).map_err(bad)?;
        if weekdays & (1 << 7) != 0 {
            weekdays |= 1; // 7 is Sunday too
        }
        Ok(Self {
            minutes: field(mi, 0, 59, &[], 0).map_err(bad)?,
            hours: field(h, 0, 23, &[], 0).map_err(bad)? as u32,
            days: field(dom, 1, 31, &[], 0).map_err(bad)? as u32,
            months: field(mo, 1, 12, &MONTHS, 1).map_err(bad)? as u16,
            weekdays: (weekdays & 0x7f) as u8,
            days_set: dom != "*",
            weekdays_set: dow != "*",
        })
    }

    fn day_ok(&self, d: NaiveDate) -> bool {
        let dom = self.days & (1 << d.day()) != 0;
        let dow = self.weekdays & (1 << d.weekday().num_days_from_sunday()) != 0;
        match (self.days_set, self.weekdays_set) {
            (true, true) => dom || dow,
            (true, false) => dom,
            (false, true) => dow,
            (false, false) => true,
        }
    }

    /// The first matching local wall-clock minute after `t` (naive local time).
    fn next_naive(&self, t: NaiveDateTime) -> Option<NaiveDateTime> {
        let mut t = t.with_second(0)?.with_nanosecond(0)? + Duration::minutes(1);
        // Five years is past any match a valid expression has (Feb 29 + a weekday).
        let end = t + Duration::days(5 * 366);
        while t < end {
            if self.months & (1 << t.month()) == 0 {
                let (y, m) = if t.month() == 12 { (t.year() + 1, 1) } else { (t.year(), t.month() + 1) };
                t = NaiveDate::from_ymd_opt(y, m, 1)?.and_hms_opt(0, 0, 0)?;
            } else if !self.day_ok(t.date()) {
                t = t.date().succ_opt()?.and_hms_opt(0, 0, 0)?;
            } else if self.hours & (1 << t.hour()) == 0 {
                t = t.with_minute(0)? + Duration::hours(1);
            } else if self.minutes & (1 << t.minute()) == 0 {
                t += Duration::minutes(1);
            } else {
                return Some(t);
            }
        }
        None
    }

    /// The next run after `after` (ms since the epoch), in `tz`'s wall time.
    pub fn next_in<Z: TimeZone>(&self, tz: &Z, after: i64) -> Option<i64> {
        let from = tz.timestamp_millis_opt(after).single()?.naive_local();
        self.next_with(from, |n| tz.from_local_datetime(&n).map(|t| t.timestamp_millis()))
    }

    /// The next run after wall time `from`; `resolve` turns a wall time into
    /// its instants (none: skipped; two: it happens twice).
    fn next_with(&self, mut from: NaiveDateTime, resolve: impl Fn(NaiveDateTime) -> LocalResult<i64>) -> Option<i64> {
        loop {
            let n = self.next_naive(from)?;
            match resolve(n) {
                LocalResult::Single(t) => return Some(t),
                // Happens twice (clocks went back): the first one.
                LocalResult::Ambiguous(first, _) => return Some(first),
                // Skipped (clocks went forward): the first minute that exists.
                LocalResult::None => {
                    let mut m = n;
                    for _ in 0..24 * 60 {
                        m += Duration::minutes(1);
                        if let Some(t) = resolve(m).earliest() {
                            return Some(t);
                        }
                    }
                    from = n;
                }
            }
        }
    }

    /// The next run after `after`, in this machine's local time.
    pub fn next(&self, after: i64) -> Option<i64> {
        self.next_in(&Local, after)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, Utc};

    fn ms(s: &str) -> i64 {
        chrono::DateTime::parse_from_rfc3339(s).unwrap().timestamp_millis()
    }

    fn at(t: i64) -> String {
        Utc.timestamp_millis_opt(t).unwrap().format("%Y-%m-%d %H:%M %a").to_string()
    }

    #[test]
    fn expressions_parse_as_cron_reads_them() {
        assert!(Cron::parse("0 9 * * 1").is_ok());
        assert!(Cron::parse("*/15 8-18 * jan-mar mon-fri").is_ok());
        for bad in ["0 9 * *", "60 * * * *", "* 24 * * *", "* * 0 * *", "*/0 * * * *", "* * * foo *", "5-1 * * * *"] {
            assert!(Cron::parse(bad).is_err(), "{bad}");
        }
        let c = Cron::parse("0 0 * * 7").unwrap();
        assert_eq!(c.weekdays, 1, "7 is Sunday");
    }

    #[test]
    fn the_next_run_follows_the_fields() {
        let utc = Utc;
        let next = |e: &str, from: &str| at(Cron::parse(e).unwrap().next_in(&utc, ms(from)).unwrap());
        assert_eq!(next("0 9 * * 1", "2026-10-28T10:00:00Z"), "2026-11-02 09:00 Mon");
        assert_eq!(next("*/15 * * * *", "2026-10-28T10:07:30Z"), "2026-10-28 10:15 Wed");
        assert_eq!(next("0 0 29 2 *", "2026-03-01T00:00:00Z"), "2028-02-29 00:00 Tue");
        // Both days given: either matches (the 1st, or a Friday).
        assert_eq!(next("0 12 1 * fri", "2026-10-28T13:00:00Z"), "2026-10-30 12:00 Fri");
        // Never the same minute again: strictly after.
        assert_eq!(next("30 10 * * *", "2026-10-28T10:30:00Z"), "2026-10-29 10:30 Thu");
    }

    /// Europe/Prague's 2026 clock changes: wall time → its instants.
    fn prague(naive: NaiveDateTime) -> LocalResult<i64> {
        let (cest, cet) = (FixedOffset::east_opt(7200).unwrap(), FixedOffset::east_opt(3600).unwrap());
        let d = |s: &str| NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap();
        let at = |o: FixedOffset| o.from_local_datetime(&naive).unwrap().timestamp_millis();
        if naive >= d("2026-03-29 02:00") && naive < d("2026-03-29 03:00") {
            LocalResult::None
        } else if naive >= d("2026-10-25 02:00") && naive < d("2026-10-25 03:00") {
            LocalResult::Ambiguous(at(cest), at(cet))
        } else if naive >= d("2026-03-29 03:00") && naive < d("2026-10-25 03:00") {
            LocalResult::Single(at(cest))
        } else {
            LocalResult::Single(at(cet))
        }
    }

    fn wall(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap()
    }

    #[test]
    fn dst_skips_run_after_the_gap_and_repeats_run_once() {
        let c = Cron::parse("30 2 * * *").unwrap();
        // Spring forward: 02:30 doesn't exist on Mar 29: it runs at 03:00 CEST.
        let t = c.next_with(wall("2026-03-29 01:00"), prague).unwrap();
        assert_eq!(at(t), "2026-03-29 01:00 Sun", "03:00 CEST is 01:00 UTC");
        // Fall back: 02:30 happens twice on Oct 25; it runs the first time…
        let t = c.next_with(wall("2026-10-25 01:00"), prague).unwrap();
        assert_eq!(at(t), "2026-10-25 00:30 Sun", "02:30 CEST");
        // …and the run after it (searched from that run's wall time, as the
        // scheduler does) is the next day, not 02:30 CET an hour later.
        let from =
            Utc.timestamp_millis_opt(t).unwrap().with_timezone(&FixedOffset::east_opt(7200).unwrap()).naive_local();
        assert_eq!(at(c.next_with(from, prague).unwrap()), "2026-10-26 01:30 Mon");
    }
}
