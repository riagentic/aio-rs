//! The freeze guard (aio's `freeze-guard.ts`): `ax build`'s heavy steps
//! (cargo, gradle) keep the desktop responsive. A desktop freezes on memory
//! pressure, not CPU, so a step does not start on a machine already out of
//! headroom; and it never takes every core, one stays for the desktop.
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

/// A machine's memory, in bytes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Memory {
    pub total: u64,
    pub available: u64,
    pub swap_used: u64,
    pub swap_total: u64,
}

/// Below this share of RAM available, a heavy step declines.
const FLOOR_PCT: f64 = 5.0;
/// Swap this full is thrashing…
const SWAP_FULL_PCT: f64 = 80.0;
/// …but only counts while RAM is short too: full swap beside free RAM is
/// pages left from an old peak, not pressure.
const SWAP_COUNTS_BELOW_PCT: f64 = 20.0;

/// Why a heavy step should not start now, if it should not.
pub fn headroom(m: &Memory) -> Option<String> {
    let avail = 100.0 * m.available as f64 / m.total.max(1) as f64;
    if avail < FLOOR_PCT {
        return Some(format!("only {avail:.1}% RAM available"));
    }
    let swap = 100.0 * m.swap_used as f64 / m.swap_total.max(1) as f64;
    (m.swap_total > 0 && avail < SWAP_COUNTS_BELOW_PCT && swap > SWAP_FULL_PCT)
        .then(|| format!("swap {}% full with only {}% RAM available", swap.round(), avail.round()))
}

/// Linux `/proc/meminfo`, read; None when it says nothing (no total).
pub fn parse_meminfo(text: &str) -> Option<Memory> {
    let kb = |key: &str| {
        text.lines()
            .find_map(|l| {
                l.strip_prefix(key)?.strip_prefix(':')?.trim().strip_suffix(" kB")?.trim().parse::<u64>().ok()
            })
            .unwrap_or(0)
            * 1024
    };
    let m = Memory {
        total: kb("MemTotal"),
        available: kb("MemAvailable"),
        swap_used: kb("SwapTotal").saturating_sub(kb("SwapFree")),
        swap_total: kb("SwapTotal"),
    };
    (m.total > 0).then_some(m)
}

/// Jobs for a machine with `cores`: most, never all.
pub fn jobs(cores: usize) -> usize {
    cores.saturating_sub(1).max(1)
}

/// [`jobs`] on this machine.
pub fn max_jobs() -> usize {
    jobs(std::thread::available_parallelism().map_or(1, |n| n.get()))
}

/// Refuses `step` on a machine out of headroom (where there is no
/// `/proc/meminfo`, nothing to say: it never blocks).
pub fn ensure(step: &str) -> Result<(), String> {
    let Some(m) = std::fs::read_to_string("/proc/meminfo").ok().as_deref().and_then(parse_meminfo) else {
        return Ok(());
    };
    match headroom(&m) {
        Some(why) => Err(format!(
            "{step}: declined, {why}. aio does not start a memory-heavy step on a machine out of headroom: \
             free memory (or wait for the swap to drain) and build again"
        )),
        None => Ok(()),
    }
}

/// `cargo <args>` as a heavy step: refused without headroom, at most
/// [`jobs`] jobs; a release built quickly after [`quick`].
pub fn cargo(step: &str, args: &[&str]) -> Result<Command, String> {
    ensure(step)?;
    let mut c = Command::new("cargo");
    c.args(args).arg("-j").arg(max_jobs().to_string()).envs(profile(is_quick()).iter().copied());
    Ok(c)
}

static QUICK: AtomicBool = AtomicBool::new(false);

/// `ax build --quick`: every cargo this process runs builds its release
/// without LTO, in 16 codegen units (the template's: 27 → 2 s for a
/// one-line change, the binary ~37% bigger). For trying, never shipping.
pub fn quick() {
    QUICK.store(true, Ordering::Relaxed);
}

/// Whether [`quick`] was asked.
pub fn is_quick() -> bool {
    QUICK.load(Ordering::Relaxed)
}

/// The release profile's overrides, quick or not (pure). Cargo keeps
/// both builds apart, so going back and forth rebuilds nothing.
pub fn profile(quick: bool) -> &'static [(&'static str, &'static str)] {
    match quick {
        true => &[("CARGO_PROFILE_RELEASE_LTO", "false"), ("CARGO_PROFILE_RELEASE_CODEGEN_UNITS", "16")],
        false => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quick_build_overrides_the_release_profile_and_only_then() {
        assert_eq!(profile(false), []);
        assert_eq!(
            profile(true),
            [("CARGO_PROFILE_RELEASE_LTO", "false"), ("CARGO_PROFILE_RELEASE_CODEGEN_UNITS", "16")]
        );
    }

    const GB: u64 = 1 << 30;

    fn mem(available: f64, swap_used: f64, swap_total: u64) -> Memory {
        Memory {
            total: 32 * GB,
            available: (available * GB as f64).round() as u64,
            swap_used: (swap_used * GB as f64).round() as u64,
            swap_total: swap_total * GB,
        }
    }

    #[test]
    fn a_healthy_machine_is_never_slowed() {
        assert_eq!(headroom(&mem(20.0, 0.0, 8)), None);
        assert_eq!(headroom(&mem(3.2, 0.0, 8)), None, "10% is above the 5% floor");
    }

    #[test]
    fn it_declines_when_ram_is_nearly_spent_or_swap_full_and_ram_short() {
        assert!(headroom(&mem(1.0, 0.0, 8)).unwrap().contains("RAM available"));
        assert_eq!(headroom(&mem(4.0, 7.0, 8)).as_deref(), Some("swap 88% full with only 13% RAM available"));
    }

    #[test]
    fn full_swap_beside_free_ram_is_not_pressure_and_no_swap_is_ram_alone() {
        assert_eq!(headroom(&mem(11.0, 7.9, 8)), None);
        assert_eq!(headroom(&mem(6.4, 7.9, 8)), None, "a fifth of RAM is plenty");
        assert!(headroom(&mem(6.3, 7.9, 8)).is_some());
        assert_eq!(headroom(&mem(20.0, 0.0, 0)), None);
        assert!(headroom(&mem(0.5, 0.0, 0)).is_some());
    }

    #[test]
    fn meminfo_is_read_and_jobs_leave_a_core() {
        let text = "MemTotal:       32000000 kB\nMemFree:  100 kB\nMemAvailable:   1000000 kB\nSwapTotal:  8000000 kB\nSwapFree:  2000000 kB\n";
        let m = parse_meminfo(text).unwrap();
        assert_eq!(
            (m.total, m.available, m.swap_used, m.swap_total),
            (32_000_000 * 1024, 1_000_000 * 1024, 6_000_000 * 1024, 8_000_000 * 1024)
        );
        assert_eq!(parse_meminfo("nothing here"), None);
        assert_eq!((jobs(1), jobs(2), jobs(16)), (1, 1, 15));
    }
}
