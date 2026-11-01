//! Per-address budgets for the login doors, as aio's: what one address
//! may try before it is told `too_many_attempts`. The per-account lockout
//! (`Auth`) is the defence rotating addresses can't sidestep; these bound
//! the work and the guessing one address can cause. Pure: time is an argument.
use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;

/// Failed logins: 10 per 5 minutes.
pub const FAILS: (usize, i64) = (10, 5 * 60_000);
/// Password checks started (each a PBKDF2): 30 per minute; a success refunds its own.
pub const WORK: (usize, i64) = (30, 60_000);
/// New accounts: 10 per hour.
pub const SIGNUPS: (usize, i64) = (10, 3_600_000);
/// Mails one door may trigger per key (an inbox, an address): 10 per 5 minutes.
pub const MAILS: (usize, i64) = (10, 5 * 60_000);
/// Addresses remembered per meter; past it the oldest are forgotten.
const MAX_KEYS: usize = 10_000;

/// The bucket an address is counted in: IPv4 (and IPv4-mapped IPv6) as is,
/// IPv6 by its /64, which one machine usually holds whole.
pub fn bucket(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => v4.to_string(),
            None if v6.is_loopback() => v6.to_string(),
            None => {
                let s = v6.segments();
                format!("{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3])
            }
        },
    }
}

/// One meter: when each counted thing happened, per bucket.
#[derive(Default)]
pub struct Meter(HashMap<String, VecDeque<i64>>);

impl Meter {
    fn fresh(&mut self, key: &str, (max, window): (usize, i64), now: i64) -> &mut VecDeque<i64> {
        if self.0.len() >= MAX_KEYS && !self.0.contains_key(key) {
            self.0.retain(|_, ts| ts.back().is_some_and(|t| now - t < window));
            while self.0.len() >= MAX_KEYS * 9 / 10 {
                let Some(k) = self.0.keys().next().cloned() else { break };
                self.0.remove(&k);
            }
        }
        let ts = self.0.entry(key.into()).or_default();
        while ts.front().is_some_and(|t| now - t >= window) {
            ts.pop_front();
        }
        while ts.len() > max {
            ts.pop_front();
        }
        ts
    }

    /// Count one; false when that is past the budget.
    pub fn charge(&mut self, key: &str, budget: (usize, i64), now: i64) -> bool {
        let ts = self.fresh(key, budget, now);
        ts.push_back(now);
        ts.len() <= budget.0
    }

    /// Is the budget used up (without counting)?
    pub fn spent(&mut self, key: &str, budget: (usize, i64), now: i64) -> bool {
        self.fresh(key, budget, now).len() >= budget.0
    }

    /// Give back the last one counted (a login that succeeded).
    pub fn refund(&mut self, key: &str) {
        if let Some(ts) = self.0.get_mut(key) {
            ts.pop_back();
        }
    }
}

/// The meters the login doors use.
#[derive(Default)]
pub struct Budgets {
    pub fails: Meter,
    pub work: Meter,
    pub signups: Meter,
    pub mails: Meter,
}

/// Wrong keys (or sessions) one address may show: 10 per 5 minutes.
pub const GUESSES: (usize, i64) = (10, 5 * 60_000);
/// A strike is forgotten after this long without another.
const STRIKE_MEMORY_MS: i64 = 10 * 60_000;

/// How long strike `n` (from 1) blocks an address, as aio's: 5 s, 10 s,
/// 20 s, 40 s, then 60 s. A first burst (one noisy tab behind a NAT) is
/// short; an address that keeps guessing waits longer.
pub fn block_ms(n: u32) -> i64 {
    (5_000i64 << n.saturating_sub(1).min(4)).min(60_000)
}

struct Strikes {
    until: i64,
    n: u32,
    last: i64,
}

/// Key guessing on the app's doors (routes, files, `/ws`, health): an
/// address past [`GUESSES`] is blocked for [`block_ms`] (aio's denylist, with
/// its TTL). The block meets only wrong or missing credentials: a caller
/// showing a valid one is always served, so nobody can lock others out by
/// guessing from their address (or their proxy's).
#[derive(Default)]
pub struct Guesses {
    wrong: Meter,
    blocked: HashMap<String, Strikes>,
}

impl Guesses {
    /// How long `ip` is still blocked, in ms, or `None`.
    pub fn blocked(&mut self, ip: IpAddr, now: i64) -> Option<i64> {
        let key = bucket(ip);
        let s = self.blocked.get(&key)?;
        if now - s.last > STRIKE_MEMORY_MS && now > s.until {
            self.blocked.remove(&key);
            return None;
        }
        (now < s.until).then(|| s.until - now)
    }

    /// Count a wrong credential from `ip`: how long it is now blocked, if
    /// that was one too many.
    pub fn wrong(&mut self, ip: IpAddr, now: i64) -> Option<i64> {
        if let Some(left) = self.blocked(ip, now) {
            return Some(left);
        }
        let key = bucket(ip);
        if self.wrong.charge(&key, GUESSES, now) {
            return None;
        }
        if self.blocked.len() >= MAX_KEYS && !self.blocked.contains_key(&key) {
            self.blocked.retain(|_, s| now <= s.until || now - s.last <= STRIKE_MEMORY_MS);
            while self.blocked.len() >= MAX_KEYS * 9 / 10 {
                let Some(k) = self.blocked.keys().next().cloned() else { break };
                self.blocked.remove(&k);
            }
        }
        let n = self.blocked.get(&key).map_or(1, |s| s.n + 1);
        let ms = block_ms(n);
        self.blocked.insert(key, Strikes { until: now + ms, n, last: now });
        Some(ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets_group_one_machine() {
        let b = |s: &str| bucket(s.parse().unwrap());
        assert_eq!(b("10.0.0.9"), "10.0.0.9");
        assert_eq!(b("::ffff:10.0.0.9"), "10.0.0.9");
        assert_eq!(b("::1"), "::1");
        assert_eq!(b("2001:db8:1:2:aaaa::1"), b("2001:db8:1:2:ffff::9"));
        assert_eq!(b("2001:db8:1:2::1"), "2001:db8:1:2::/64");
        assert_ne!(b("2001:db8:1:2::1"), b("2001:db8:1:3::1"));
    }

    #[test]
    fn a_meter_counts_in_its_window_and_refunds() {
        let mut m = Meter::default();
        for i in 0..10 {
            assert!(m.charge("a", FAILS, i));
        }
        assert!(m.spent("a", FAILS, 10));
        assert!(!m.charge("a", FAILS, 10));
        assert!(!m.spent("b", FAILS, 10), "another address is untouched");
        // Five minutes after the first ones, they no longer count.
        assert!(!m.spent("a", FAILS, 5 * 60_000 + 9));
        let mut w = Meter::default();
        for _ in 0..30 {
            assert!(w.charge("a", WORK, 0));
            w.refund("a");
        }
        assert!(w.charge("a", WORK, 0), "refunded work leaves the budget whole");
    }

    #[test]
    fn guessing_blocks_an_address_longer_each_strike_and_is_forgotten() {
        assert_eq!((1..=7).map(block_ms).collect::<Vec<_>>(), [5_000, 10_000, 20_000, 40_000, 60_000, 60_000, 60_000]);
        let (ip, other): (IpAddr, IpAddr) = ("10.0.0.9".parse().unwrap(), "10.0.0.10".parse().unwrap());
        let mut g = Guesses::default();
        for t in 0..10 {
            assert_eq!(g.wrong(ip, t), None, "within the budget");
        }
        assert_eq!(g.wrong(ip, 10), Some(5_000), "one too many: the first strike");
        assert_eq!((g.blocked(ip, 4_000), g.blocked(other, 4_000)), (Some(1_010), None));
        assert_eq!(g.wrong(ip, 4_000), Some(1_010), "a guess while blocked does not lengthen it");
        assert_eq!(g.blocked(ip, 5_010), None, "its TTL ran out");
        assert_eq!(g.wrong(ip, 5_010), Some(10_000), "still guessing: a longer block");
        // An IPv6 /64 is one address; ten quiet minutes forget the strikes.
        let (a, b): (IpAddr, IpAddr) = ("2001:db8::1".parse().unwrap(), "2001:db8::2".parse().unwrap());
        for t in 0..10 {
            g.wrong(a, t);
        }
        assert_eq!(g.wrong(b, 10), Some(5_000));
        let later = 10 + 5_000 + STRIKE_MEMORY_MS + 1;
        assert_eq!((g.blocked(b, later), g.wrong(b, later)), (None, None));
        assert!(!g.blocked.contains_key(&bucket(b)));
    }

    #[test]
    fn a_meter_stays_bounded() {
        let mut m = Meter::default();
        for i in 0..(MAX_KEYS + 5) {
            m.charge(&i.to_string(), FAILS, 0);
        }
        assert!(m.0.len() <= MAX_KEYS);
    }
}
