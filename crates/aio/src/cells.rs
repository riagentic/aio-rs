//! Many cells in one app: the one list of them, started in dependency order
//! and shut down in reverse, each enabled or disabled, with its health. A
//! cell that keeps failing (a panic, or a refused call of its own schedules
//! and effects) is disabled by the breaker, its data kept.
use crate::{Cell, Drained, Handle, Result};
use std::{
    future::Future,
    pin::Pin,
    time::{Duration, Instant},
};

/// One cell's row in [`Cells::health`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellHealth {
    pub id: &'static str,
    /// `disabled`, or the cell's own `status` field when it is a string.
    pub status: Option<String>,
    pub enabled: bool,
    /// Failures since it was last enabled (within the breaker's window).
    pub failures: usize,
    /// The last committed call's method and time (ms since the epoch).
    pub last_call: Option<(String, i64)>,
}

/// A cell of any type, as the list holds it.
trait Part: Send + Sync {
    fn id(&self) -> &'static str;
    fn fire(&self);
    fn start(&self);
    fn switch(&self, on: bool);
    fn trip_after(&self, max: u32, window: Option<Duration>);
    fn shutdown(&self, budget: Duration) -> Pin<Box<dyn Future<Output = Drained> + Send>>;
    fn health(&self) -> CellHealth;
}

impl<T: Cell> Part for Handle<T> {
    fn id(&self) -> &'static str {
        T::ID
    }

    fn fire(&self) {
        Handle::fire(self);
    }

    fn start(&self) {
        drop(Handle::start(self));
    }

    fn switch(&self, on: bool) {
        Handle::switch(self, on);
    }

    fn trip_after(&self, max: u32, window: Option<Duration>) {
        Handle::trip_after(self, max, window);
    }

    fn shutdown(&self, budget: Duration) -> Pin<Box<dyn Future<Output = Drained> + Send>> {
        let h = self.clone();
        Box::pin(async move { h.shutdown(budget).await })
    }

    fn health(&self) -> CellHealth {
        let enabled = self.enabled();
        let own = || serde_json::to_value(&*self.state()).ok()?.get("status")?.as_str().map(String::from);
        CellHealth {
            id: T::ID,
            status: if enabled { own() } else { Some("disabled".into()) },
            enabled,
            failures: self.failures(),
            last_call: self.last_call(),
        }
    }
}

/// The app's cells: `Cells::default().with(todo, &[]).with(mail, &[Todo::ID]).start()?`.
#[derive(Default)]
pub struct Cells {
    /// In dependency order once started.
    parts: Vec<(Box<dyn Part>, Vec<&'static str>)>,
    trip: Option<(u32, Option<Duration>)>,
}

impl Cells {
    /// Add a cell that starts after the cells `after` names (by `ID`).
    pub fn with<T: Cell>(mut self, h: Handle<T>, after: &[&'static str]) -> Self {
        self.parts.push((Box::new(h), after.to_vec()));
        self
    }

    /// `other`'s cells too (a plugin's, [`crate::plugin`]).
    pub(crate) fn and(mut self, other: Cells) -> Self {
        self.parts.extend(other.parts);
        self
    }

    /// Disable a cell after `max` failures (within `window`, if set); the
    /// trip is said in the log, the app's health and a problem report.
    pub fn trip_after(mut self, max: u32, window: Option<Duration>) -> Self {
        self.trip = Some((max, window));
        self
    }

    /// Check the list (a duplicate, an unknown dependency or a cycle
    /// refuses the boot), then in dependency order fire each cell's missed
    /// schedules and start its scheduler (inside a tokio runtime).
    pub fn start(mut self) -> Result<Self, crate::Error> {
        let ids: Vec<_> = self.parts.iter().map(|(p, a)| (p.id(), a.as_slice())).collect();
        let order = order(&ids).map_err(crate::Error)?;
        let mut parts: Vec<_> = self.parts.into_iter().map(Some).collect();
        self.parts = order.into_iter().filter_map(|i| parts[i].take()).collect();
        for (p, _) in &self.parts {
            if let Some((max, window)) = self.trip {
                p.trip_after(max, window);
            }
            p.fire();
            p.start();
        }
        Ok(self)
    }

    /// The cells' ids, in dependency order once started.
    pub fn ids(&self) -> Vec<&'static str> {
        self.parts.iter().map(|(p, _)| p.id()).collect()
    }

    fn part(&self, id: &str) -> Result<&dyn Part> {
        match self.parts.iter().find(|(p, _)| p.id() == id) {
            Some((p, _)) => Ok(p.as_ref()),
            None => crate::reject!("unknown cell `{id}`; known: {}", self.ids().join(", ")),
        }
    }

    /// Take calls and fire schedules again; failures count from zero.
    pub fn enable(&self, id: &str) -> Result {
        self.part(id).map(|p| p.switch(true))
    }

    /// Refuse its calls, pause its schedules, abort its effects and drop
    /// what it owns. Its state stays: enabling brings it back as it was.
    pub fn disable(&self, id: &str) -> Result {
        self.part(id).map(|p| p.switch(false))
    }

    pub fn enabled(&self, id: &str) -> bool {
        self.part(id).is_ok_and(|p| p.health().enabled)
    }

    /// `disabled`, or the cell's own `status` field.
    pub fn status(&self, id: &str) -> Option<String> {
        self.part(id).ok()?.health().status
    }

    /// Every cell's row, in dependency order.
    pub fn health(&self) -> Vec<CellHealth> {
        self.parts.iter().map(|(p, _)| p.health()).collect()
    }

    /// Shut every cell down in reverse dependency order (a cell's effects
    /// still reach the cells it depends on), all within `budget`; the
    /// effects aborted at the deadline, as `cell: key`.
    pub async fn shutdown(&self, budget: Duration) -> Vec<String> {
        let deadline = Instant::now() + budget;
        let mut aborted = vec![];
        for (p, _) in self.parts.iter().rev() {
            let d = p.shutdown(deadline.saturating_duration_since(Instant::now())).await;
            aborted.extend(d.aborted.into_iter().map(|k| format!("{}: {k}", p.id())));
        }
        aborted
    }
}

/// The list's indexes in dependency order: each after the ones it names,
/// otherwise as listed.
fn order(parts: &[(&str, &[&str])]) -> std::result::Result<Vec<usize>, String> {
    let at = |id: &str| parts.iter().position(|(o, _)| *o == id);
    for (i, (id, after)) in parts.iter().enumerate() {
        if at(id) != Some(i) {
            return Err(format!("duplicate cell name `{id}`: two cells in the list share it"));
        }
        if let Some(d) = after.iter().find(|d| at(d).is_none()) {
            let known: Vec<_> = parts.iter().map(|(o, _)| *o).collect();
            return Err(format!("`{id}` depends on unknown cell `{d}`; known: {}", known.join(", ")));
        }
    }
    fn place(
        i: usize,
        parts: &[(&str, &[&str])],
        path: &mut Vec<usize>,
        out: &mut Vec<usize>,
    ) -> std::result::Result<(), String> {
        if out.contains(&i) {
            return Ok(());
        }
        if let Some(from) = path.iter().position(|&j| j == i) {
            let names: Vec<_> = path[from..].iter().chain([&i]).map(|&j| parts[j].0).collect();
            return Err(format!("dependency cycle: {}", names.join(" → ")));
        }
        path.push(i);
        for d in parts[i].1 {
            let j = parts.iter().position(|(o, _)| o == d).unwrap_or(i);
            place(j, parts, path, out)?;
        }
        path.pop();
        out.push(i);
        Ok(())
    }
    let mut out = vec![];
    for i in 0..parts.len() {
        place(i, parts, &mut vec![], &mut out)?;
    }
    Ok(out)
}

/// A cell's failures, and the breaker that disables it (`max` 0: never).
#[derive(Debug, Default)]
pub(crate) struct Tally {
    at: Vec<Instant>,
    max: u32,
    window: Option<Duration>,
}

impl Tally {
    pub(crate) fn trip_after(&mut self, max: u32, window: Option<Duration>) {
        (self.max, self.window) = (max, window);
    }

    /// Count a failure at `now`; true when the breaker trips.
    pub(crate) fn add(&mut self, now: Instant) -> bool {
        self.at.push(now);
        self.max > 0 && self.count(now) >= self.max as usize
    }

    /// Failures counted, within the window when there is one.
    pub(crate) fn count(&mut self, now: Instant) -> usize {
        if let Some(w) = self.window {
            self.at.retain(|t| now.duration_since(*t) < w);
        }
        self.at.len()
    }

    pub(crate) fn clear(&mut self) {
        self.at.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dependencies_come_first_otherwise_as_listed() {
        let parts: [(&str, &[&str]); 3] = [("c", &["a", "b"]), ("b", &["a"]), ("a", &[])];
        assert_eq!(order(&parts), Ok(vec![2, 1, 0]));
        let parts: [(&str, &[&str]); 3] = [("x", &[]), ("b", &["a"]), ("a", &[])];
        assert_eq!(order(&parts), Ok(vec![0, 2, 1]));
    }

    #[test]
    fn a_duplicate_an_unknown_dependency_or_a_cycle_refuses() {
        let dup: [(&str, &[&str]); 2] = [("dup", &[]), ("dup", &[])];
        assert!(order(&dup).unwrap_err().contains("duplicate cell name `dup`"));
        let unknown: [(&str, &[&str]); 1] = [("a", &["missing"])];
        assert_eq!(order(&unknown).unwrap_err(), "`a` depends on unknown cell `missing`; known: a");
        let cycle: [(&str, &[&str]); 2] = [("x", &["y"]), ("y", &["x"])];
        assert_eq!(order(&cycle).unwrap_err(), "dependency cycle: x → y → x");
        let long: [(&str, &[&str]); 4] = [("top", &["a"]), ("a", &["b"]), ("b", &["c"]), ("c", &["a"])];
        assert_eq!(order(&long).unwrap_err(), "dependency cycle: a → b → c → a", "only the cycle is named");
        let own: [(&str, &[&str]); 1] = [("a", &["a"])];
        assert_eq!(order(&own).unwrap_err(), "dependency cycle: a → a");
    }

    #[test]
    fn the_breaker_trips_at_max_and_counts_only_its_window() {
        let t0 = Instant::now();
        let mut t = Tally::default();
        assert!(!(0..10).any(|_| t.add(t0)), "max 0: never trips");
        let mut t = Tally::default();
        t.trip_after(3, None);
        assert_eq!([t.add(t0), t.add(t0), t.add(t0)], [false, false, true]);
        t.clear();
        assert_eq!(t.count(t0), 0);

        let mut t = Tally::default();
        t.trip_after(2, Some(Duration::from_secs(1)));
        assert!(!t.add(t0));
        assert!(!t.add(t0 + Duration::from_secs(2)), "the first one is out of the window");
        assert!(t.add(t0 + Duration::from_millis(2500)));
    }
}
