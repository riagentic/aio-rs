//! Cron schedules: `minute hour day-of-month month day-of-week`, in UTC as
//! aio's were (`[app] tz = "local"`: this machine's local time; see
//! [`set_local`]). Fields take `*`, numbers, `a-b`, lists and `/step`;
//! months and weekdays take names (`jan`, `mon`); weekday 0 and 7 are Sunday.
//! As in cron, when both days are restricted, either one matching is enough.
//!
//! Clock rules: a time a DST change skips runs once, at the first minute that
//! exists after it; a time that happens twice runs once, the first time. The
//! next run is always found after the last one, so a clock step back never
//! runs a slot twice (and after sleep a missed run fires once: the scheduler).
use jiff::ToSpan;
use jiff::civil::{Date, DateTime};
use std::sync::atomic::{AtomicBool, Ordering};

static LOCAL: AtomicBool = AtomicBool::new(false);

/// Read cron schedules in this machine's local time (`true`) or in UTC
/// (`false`, the default, as aio): `aio::boot` sets it from `[app] tz`.
/// A schedule already armed keeps its next run; the one after follows this.
pub fn set_local(local: bool) {
    LOCAL.store(local, Ordering::Relaxed);
}

/// Are cron schedules read in local time?
pub fn local() -> bool {
    LOCAL.load(Ordering::Relaxed)
}

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

    fn day_ok(&self, d: Date) -> bool {
        let dom = self.days & (1 << d.day()) != 0;
        let dow = self.weekdays & (1 << d.weekday().to_sunday_zero_offset()) != 0;
        match (self.days_set, self.weekdays_set) {
            (true, true) => dom || dow,
            (true, false) => dom,
            (false, true) => dow,
            (false, false) => true,
        }
    }

    /// The first matching wall-clock minute after wall time `t`.
    fn next_wall(&self, t: DateTime) -> Option<DateTime> {
        let mut t = t.date().at(t.hour(), t.minute(), 0, 0).checked_add(1.minute()).ok()?;
        // Five years is past any match a valid expression has (Feb 29 + a weekday).
        let end = t.checked_add((5 * 366).days()).ok()?;
        while t < end {
            if self.months & (1 << t.month()) == 0 {
                let (y, m) = if t.month() == 12 { (t.year() + 1, 1) } else { (t.year(), t.month() + 1) };
                t = Date::new(y, m, 1).ok()?.at(0, 0, 0, 0);
            } else if !self.day_ok(t.date()) {
                t = t.date().tomorrow().ok()?.at(0, 0, 0, 0);
            } else if self.hours & (1 << t.hour()) == 0 {
                t = t.date().at(t.hour(), 0, 0, 0).checked_add(1.hour()).ok()?;
            } else if self.minutes & (1 << t.minute()) == 0 {
                t = t.checked_add(1.minute()).ok()?;
            } else {
                return Some(t);
            }
        }
        None
    }

    /// The next run after `after` (ms since the epoch), in local wall time
    /// (`true`) or UTC's.
    fn next_in(&self, local: bool, after: i64) -> Option<i64> {
        self.next_with(zone::wall(local, after)?, after, |n| zone::instants(local, n))
    }

    /// The next run after instant `after`, whose wall time is `from`;
    /// `resolve` turns a wall time into its instants (none: skipped; two: it
    /// happens twice).
    fn next_with(&self, mut from: DateTime, after: i64, resolve: impl Fn(DateTime) -> Instants) -> Option<i64> {
        loop {
            let n = self.next_wall(from)?;
            match resolve(n) {
                Instants::One(t) => return Some(t),
                // Happens twice (clocks went back): the first one. Searched
                // from inside the repeated hour, the first one is past: that
                // wall time has been, and it runs once, so on to the next.
                Instants::Two(first, _) if first > after => return Some(first),
                Instants::Two(..) => from = n,
                // Skipped (clocks went forward): the first minute that exists.
                Instants::None => {
                    let mut m = n;
                    for _ in 0..24 * 60 {
                        m = m.checked_add(1.minute()).ok()?;
                        if let Some(t) = resolve(m).first() {
                            return Some(t);
                        }
                    }
                    from = n;
                }
            }
        }
    }

    /// The next run after `after`: in UTC, or local time after [`set_local`].
    pub fn next(&self, after: i64) -> Option<i64> {
        self.next_in(local(), after)
    }
}

/// A wall time's instants (ms since the epoch): one; two, the first and the
/// second (clocks went back over it); or none (clocks went forward over it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Instants {
    One(i64),
    Two(i64, i64),
    None,
}

impl Instants {
    fn first(self) -> Option<i64> {
        match self {
            Self::One(t) | Self::Two(t, _) => Some(t),
            Self::None => None,
        }
    }
}

/// Wall times in UTC or this machine's zone: its tz database's rules.
#[cfg(not(target_arch = "wasm32"))]
mod zone {
    use super::Instants;
    use jiff::civil::DateTime;
    use jiff::tz::{AmbiguousOffset, TimeZone};

    fn tz(local: bool) -> TimeZone {
        if local { TimeZone::system() } else { TimeZone::UTC }
    }

    /// The wall time at instant `ms`.
    pub(super) fn wall(local: bool, ms: i64) -> Option<DateTime> {
        Some(tz(local).to_datetime(jiff::Timestamp::from_millisecond(ms).ok()?))
    }

    pub(super) fn instants(local: bool, t: DateTime) -> Instants {
        instants_in(&tz(local), t)
    }

    pub(super) fn instants_in(tz: &TimeZone, t: DateTime) -> Instants {
        let at = |o: jiff::tz::Offset| o.to_timestamp(t).ok().map(|t| t.as_millisecond());
        match tz.to_ambiguous_timestamp(t).offset() {
            AmbiguousOffset::Unambiguous { offset } => at(offset).map_or(Instants::None, Instants::One),
            AmbiguousOffset::Fold { before, after } => match (at(before), at(after)) {
                (Some(a), Some(b)) => Instants::Two(a, b),
                (a, b) => a.or(b).map_or(Instants::None, Instants::One),
            },
            AmbiguousOffset::Gap { .. } => Instants::None,
        }
    }
}

/// Wall times in UTC or the page's zone: the browser's `Date`, as it reads
/// them. A clock change there moves by an hour: a wall time `Date` places
/// that reads back otherwise was skipped; one an hour later that reads the
/// same happens twice.
#[cfg(target_arch = "wasm32")]
mod zone {
    use super::Instants;
    use jiff::civil::{Date, DateTime};
    use jiff::{Timestamp, tz::TimeZone};

    pub(super) fn wall(local: bool, ms: i64) -> Option<DateTime> {
        if !local {
            return Some(TimeZone::UTC.to_datetime(Timestamp::from_millisecond(ms).ok()?));
        }
        let d = js_sys::Date::new(&(ms as f64).into());
        let n = |v: u32| i8::try_from(v).ok();
        let y = i16::try_from(d.get_full_year()).ok()?;
        Some(Date::new(y, n(d.get_month() + 1)?, n(d.get_date())?).ok()?.at(
            n(d.get_hours())?,
            n(d.get_minutes())?,
            0,
            0,
        ))
    }

    pub(super) fn instants(local: bool, t: DateTime) -> Instants {
        if !local {
            return TimeZone::UTC.to_timestamp(t).map_or(Instants::None, |t| Instants::One(t.as_millisecond()));
        }
        let (mo, d, h, mi) = (t.month() - 1, t.day(), t.hour(), t.minute());
        let at =
            js_sys::Date::new_with_year_month_day_hr_min(t.year() as u32, mo.into(), d.into(), h.into(), mi.into());
        let at = at.get_time() as i64;
        const HOUR: i64 = 3_600_000;
        match (wall(true, at) == Some(t), wall(true, at + HOUR) == Some(t)) {
            (false, _) => Instants::None,
            (true, true) => Instants::Two(at, at + HOUR),
            (true, false) => Instants::One(at),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::{Timestamp, tz::Offset};

    fn ms(s: &str) -> i64 {
        s.parse::<Timestamp>().unwrap().as_millisecond()
    }

    fn at(t: i64) -> String {
        Timestamp::from_millisecond(t).unwrap().strftime("%Y-%m-%d %H:%M %a").to_string()
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
    fn cron_reads_utc_unless_told_local() {
        let c = Cron::parse("0 9 * * *").unwrap();
        assert!(!local(), "UTC by default, as aio");
        assert_eq!(at(c.next(ms("2026-10-28T10:00:00Z")).unwrap()), "2026-10-29 09:00 Thu");
        set_local(true);
        let t = ms("2026-10-28T10:00:00Z");
        assert_eq!(c.next(t), c.next_in(true, t), "[app] tz = \"local\"");
        set_local(false);
        assert_eq!(c.next(t), c.next_in(false, t));
    }

    #[test]
    fn the_next_run_follows_the_fields() {
        let next = |e: &str, from: &str| at(Cron::parse(e).unwrap().next_in(false, ms(from)).unwrap());
        assert_eq!(next("0 9 * * 1", "2026-10-28T10:00:00Z"), "2026-11-02 09:00 Mon");
        assert_eq!(next("*/15 * * * *", "2026-10-28T10:07:30Z"), "2026-10-28 10:15 Wed");
        assert_eq!(next("0 0 29 2 *", "2026-03-01T00:00:00Z"), "2028-02-29 00:00 Tue");
        // Both days given: either matches (the 1st, or a Friday).
        assert_eq!(next("0 12 1 * fri", "2026-10-28T13:00:00Z"), "2026-10-30 12:00 Fri");
        // Never the same minute again: strictly after.
        assert_eq!(next("30 10 * * *", "2026-10-28T10:30:00Z"), "2026-10-29 10:30 Thu");
    }

    /// Europe/Prague's 2026 clock changes: wall time → its instants.
    fn prague(naive: DateTime) -> Instants {
        let (cest, cet) = (Offset::constant(2), Offset::constant(1));
        let d = wall;
        let at = |o: Offset| o.to_timestamp(naive).unwrap().as_millisecond();
        if naive >= d("2026-03-29 02:00") && naive < d("2026-03-29 03:00") {
            Instants::None
        } else if naive >= d("2026-10-25 02:00") && naive < d("2026-10-25 03:00") {
            Instants::Two(at(cest), at(cet))
        } else if naive >= d("2026-03-29 03:00") && naive < d("2026-10-25 03:00") {
            Instants::One(at(cest))
        } else {
            Instants::One(at(cet))
        }
    }

    fn wall(s: &str) -> DateTime {
        DateTime::strptime("%Y-%m-%d %H:%M", s).unwrap()
    }

    #[test]
    fn dst_skips_run_after_the_gap_and_repeats_run_once() {
        let c = Cron::parse("30 2 * * *").unwrap();
        // Spring forward: 02:30 doesn't exist on Mar 29: it runs at 03:00 CEST.
        let t = c.next_with(wall("2026-03-29 01:00"), ms("2026-03-29T00:00:00Z"), prague).unwrap();
        assert_eq!(at(t), "2026-03-29 01:00 Sun", "03:00 CEST is 01:00 UTC");
        // Fall back: 02:30 happens twice on Oct 25; it runs the first time…
        let t = c.next_with(wall("2026-10-25 01:00"), ms("2026-10-24T23:00:00Z"), prague).unwrap();
        assert_eq!(at(t), "2026-10-25 00:30 Sun", "02:30 CEST");
        // …and the run after it (searched from that run's wall time, as the
        // scheduler does) is the next day, not 02:30 CET an hour later.
        let from = Offset::constant(2).to_datetime(Timestamp::from_millisecond(t).unwrap());
        assert_eq!(at(c.next_with(from, t, prague).unwrap()), "2026-10-26 01:30 Mon");
        // Searched from inside the repeated hour (02:10 CET, a start or a wake
        // then): 02:30 CEST is past, and 02:30 CET is the same wall time again:
        // the next day, never a time in the past (fired at once, over and over).
        let after = ms("2026-10-25T01:10:00Z");
        let next = c.next_with(wall("2026-10-25 02:10"), after, prague).unwrap();
        assert!(next > after, "{}", at(next));
        assert_eq!(at(next), "2026-10-26 01:30 Mon");
    }

    /// The tz database's rules are the ones the scheduler is tested with
    /// above: every minute around both of Prague's 2026 clock changes.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_real_zone_resolves_wall_times_as_its_clock_changes_say() {
        let tz = jiff::tz::TimeZone::get("Europe/Prague").unwrap();
        for day in ["2026-03-29 00:00", "2026-10-25 00:00", "2026-07-01 00:00"] {
            let mut w = wall(day);
            for _ in 0..6 * 60 {
                assert_eq!(zone::instants_in(&tz, w), prague(w), "{w}");
                w = w.checked_add(1.minute()).unwrap();
            }
        }
    }
}
