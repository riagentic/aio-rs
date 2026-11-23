//! The calendar (proleptic Gregorian, Howard Hinnant's algorithms): days
//! since 1970-01-01 to and from a date, and this machine's UTC offset at an
//! instant. Pure but for [`offset_s`], the one thing the OS knows.

/// The date `days` after 1970-01-01: (year, month 1-12, day 1-31).
pub(crate) fn date(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// Days from 1970-01-01 to `y-m-d` (`m` 1-12, `d` 1-31).
pub(crate) fn days(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = i64::from(if m > 2 { m - 3 } else { m + 9 });
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + (153 * mp + 2) / 5 + i64::from(d) - 1;
    era * 146_097 + doe - 719_468
}

/// The days in month `m` of year `y`.
pub(crate) fn month_len(y: i64, m: u32) -> u32 {
    match m {
        2 if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// The weekday of day `days`: Sunday 0 to Saturday 6.
pub(crate) fn weekday(days: i64) -> u32 {
    (days + 4).rem_euclid(7) as u32
}

/// This machine's UTC offset at instant `ms` (ms since the epoch), in
/// seconds east: its zone's rules (`TZ`, else the system's zone).
#[cfg(unix)]
pub(crate) fn offset_s(ms: i64) -> Option<i64> {
    let secs = libc::time_t::try_from(ms.div_euclid(1000)).ok()?;
    let mut tm = std::mem::MaybeUninit::<libc::tm>::zeroed();
    // SAFETY: `secs` and `tm` are valid for the call; localtime_r writes only
    // `tm` (the reentrant form: no shared buffer) and returns null on failure.
    let ok = unsafe { !libc::localtime_r(&secs, tm.as_mut_ptr()).is_null() };
    #[allow(clippy::useless_conversion, reason = "c_long: 32 bits on 32-bit targets")]
    // SAFETY: localtime_r filled `tm` when it returned non-null.
    let east = ok.then(|| i64::from(unsafe { tm.assume_init() }.tm_gmtoff));
    east
}

/// This machine's UTC offset at instant `ms` (ms since the epoch), in
/// seconds east: Windows' zone, with the rules of that instant's year.
#[cfg(windows)]
pub(crate) fn offset_s(ms: i64) -> Option<i64> {
    use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows_sys::Win32::System::Time::{
        DYNAMIC_TIME_ZONE_INFORMATION, FileTimeToSystemTime, GetDynamicTimeZoneInformation,
        SystemTimeToTzSpecificLocalTimeEx,
    };
    let secs = ms.div_euclid(1000);
    // 100 ns ticks since 1601-01-01.
    let ticks = u64::try_from((secs + 11_644_473_600) * 10_000_000).ok()?;
    let ft = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
    // SAFETY: plain C structs, all-zero is a valid value of each.
    let (mut utc, mut local, mut tz): (SYSTEMTIME, SYSTEMTIME, DYNAMIC_TIME_ZONE_INFORMATION) =
        unsafe { std::mem::zeroed() };
    // SAFETY: every pointer is to a live local of the type the call takes;
    // each returns 0 on failure (and an invalid zone id), checked.
    let ok = unsafe {
        GetDynamicTimeZoneInformation(&mut tz) != u32::MAX
            && FileTimeToSystemTime(&ft, &mut utc) != 0
            && SystemTimeToTzSpecificLocalTimeEx(&tz, &utc, &mut local) != 0
    };
    let wall = |t: &SYSTEMTIME| {
        days(i64::from(t.wYear), u32::from(t.wMonth), u32::from(t.wDay)) * 86_400
            + i64::from(t.wHour) * 3600
            + i64::from(t.wMinute) * 60
            + i64::from(t.wSecond)
    };
    ok.then(|| wall(&local) - wall(&utc))
}

/// The page's UTC offset at instant `ms` (ms since the epoch), in seconds
/// east: the browser's `Date`.
#[cfg(target_arch = "wasm32")]
pub(crate) fn offset_s(ms: i64) -> Option<i64> {
    let m = js_sys::Date::new(&(ms as f64).into()).get_timezone_offset();
    m.is_finite().then(|| -(m as i64) * 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_and_dates_are_one_calendar() {
        assert_eq!((date(0), weekday(0)), ((1970, 1, 1), 4), "a Thursday");
        assert_eq!(days(2000, 2, 29), 11_016);
        assert_eq!(date(-1), (1969, 12, 31));
        for d in -800_000..800_000 {
            let (y, m, dd) = date(d);
            assert!((1..=12).contains(&m) && dd >= 1 && dd <= month_len(y, m), "{d}");
            assert_eq!(days(y, m, dd), d);
        }
        assert_eq!((month_len(1900, 2), month_len(2000, 2), month_len(2028, 2), month_len(2026, 2)), (28, 29, 29, 28));
    }

    /// The OS's offsets are the tz database's for the same zone.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn this_machines_offset_is_its_zones() {
        let tz = jiff::tz::TimeZone::system();
        for ms in [0, 1_774_746_000_000, 1_792_890_000_000, 1_784_000_000_000, -86_400_000 * 365] {
            let want = tz.to_offset(jiff::Timestamp::from_millisecond(ms).unwrap()).seconds();
            assert_eq!(offset_s(ms), Some(i64::from(want)), "{ms}");
        }
    }
}
