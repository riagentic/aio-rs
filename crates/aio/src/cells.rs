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
    /// Live: its `#[on_start]`'s refusal, if any.
    fn start(&self) -> Result;
    fn any(&self) -> &dyn std::any::Any;
    fn state(&self) -> serde_json::Value;
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

    fn start(&self) -> Result {
        let r = self.live(true);
        drop(Handle::start(self));
        r
    }

    fn any(&self) -> &dyn std::any::Any {
        self
    }

    fn state(&self) -> serde_json::Value {
        serde_json::to_value(&*Handle::state(self)).unwrap_or_default()
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

/// An app-wide hook of [`Cells::on_start`] / [`Cells::on_restore`].
type Hook = Box<dyn FnOnce(&Cells) -> Result + Send>;

/// The app's cells: `Cells::default().with(todo, &[]).with(mail, &[Todo::ID]).start()?`.
#[derive(Default)]
pub struct Cells {
    /// In dependency order once started.
    parts: Vec<(Box<dyn Part>, Vec<&'static str>)>,
    trip: Option<(u32, Option<Duration>)>,
    on_restore: Vec<(&'static str, Hook)>,
    on_start: Vec<(&'static str, Hook)>,
    fatal: bool,
}

impl Cells {
    /// Add a cell that starts after the cells `after` names (by `ID`).
    pub fn with<T: Cell>(mut self, h: Handle<T>, after: &[&'static str]) -> Self {
        self.parts.push((Box::new(h), after.to_vec()));
        self
    }

    /// `other`'s cells and hooks too (a plugin's, [`crate::plugin`]), its hooks after these.
    pub(crate) fn and(mut self, other: Cells) -> Self {
        self.parts.extend(other.parts);
        self.on_restore.extend(other.on_restore);
        self.on_start.extend(other.on_start);
        self
    }

    /// Run `f` once at `start`, after every cell's state is restored and
    /// before any goes live (aio's app-wide `onRestore`): repair across
    /// cells with their methods ([`Self::get`]); what it calls is logged.
    pub fn on_restore(mut self, f: impl FnOnce(&Cells) -> Result + Send + 'static) -> Self {
        self.on_restore.push(("on_restore", Box::new(f)));
        self
    }

    /// Run `f` once at `start`, after every cell went live (its
    /// `#[on_start]` ran): seed data, start the app's own timers (aio's
    /// `aio.run({ onStart })`). A refusal is said in the log and a report;
    /// with [`Self::fatal_on_start`] it fails the start.
    pub fn on_start(mut self, f: impl FnOnce(&Cells) -> Result + Send + 'static) -> Self {
        self.on_start.push(("on_start", Box::new(f)));
        self
    }

    pub(crate) fn hooks(mut self, by: &'static str, start: Vec<Hook>) -> Self {
        self.on_start.extend(start.into_iter().map(|f| (by, f)));
        self
    }

    /// aio's `fatalOnStart`: a refused `on_restore`, `on_start` or cell's
    /// `#[on_start]` fails [`Self::start`] (the app should exit) instead of
    /// leaving a half-started app running.
    pub fn fatal_on_start(mut self) -> Self {
        self.fatal = true;
        self
    }

    /// The cell `T`'s handle, when it is in the list.
    pub fn get<T: Cell>(&self) -> Option<Handle<T>> {
        self.parts.iter().find_map(|(p, _)| p.any().downcast_ref::<Handle<T>>()).cloned()
    }

    /// Every cell's state now, by id (aio's `app.getFullState()`): read
    /// only, for a hook that looks across cells.
    pub fn view(&self) -> serde_json::Map<String, serde_json::Value> {
        self.parts.iter().map(|(p, _)| (p.id().to_string(), p.state())).collect()
    }

    /// A hook's or cell's refusal at start: said; fatal, it fails the start.
    fn refused(&self, what: &str, e: crate::Rejected) -> Result<(), crate::Error> {
        let msg = format!("{what} refused at start: {e}");
        eprintln!("[aio] {msg}");
        crate::report::problem("hook", &msg);
        match self.fatal {
            true => Err(crate::Error(format!("{msg} (fatal_on_start)"))),
            false => Ok(()),
        }
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
    ///
    /// `AIORT_ISOLATE=todo,mail` (aio's `isolate`) starts only those cells:
    /// the others stay disabled (their calls refused, their schedules
    /// held), said in the log, to look at a few cells alone.
    pub fn start(mut self) -> Result<Self, crate::Error> {
        let ids: Vec<_> = self.parts.iter().map(|(p, a)| (p.id(), a.as_slice())).collect();
        let order = order(&ids).map_err(crate::Error)?;
        let mut parts: Vec<_> = self.parts.into_iter().map(Some).collect();
        self.parts = order.into_iter().filter_map(|i| parts[i].take()).collect();
        let only = isolated(std::env::var("AIORT_ISOLATE").ok().as_deref(), &self.ids()).map_err(crate::Error)?;
        for (name, f) in std::mem::take(&mut self.on_restore) {
            if let Err(e) = f(&self) {
                self.refused(name, e)?;
            }
        }
        for (p, _) in &self.parts {
            if let Some(only) = &only
                && !only.contains(&p.id())
            {
                eprintln!("[aio] {}: not started (AIORT_ISOLATE)", p.id());
                p.switch(false);
                continue;
            }
            if let Some((max, window)) = self.trip {
                p.trip_after(max, window);
            }
            p.fire();
            if let Err(e) = p.start() {
                self.refused(&format!("{}'s #[on_start]", p.id()), e)?;
            }
        }
        for (name, f) in std::mem::take(&mut self.on_start) {
            if let Err(e) = f(&self) {
                self.refused(name, e)?;
            }
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

/// The cells `AIORT_ISOLATE` names (`None`: every cell); a name the app
/// has no cell of refuses the start (a typo would start nothing). Pure.
fn isolated(var: Option<&str>, ids: &[&'static str]) -> std::result::Result<Option<Vec<&'static str>>, String> {
    let Some(var) = var.filter(|v| !v.trim().is_empty()) else { return Ok(None) };
    let mut only = vec![];
    for n in var.split(',').map(str::trim).filter(|n| !n.is_empty()) {
        match ids.iter().find(|i| **i == n) {
            Some(i) => only.push(*i),
            None => return Err(format!("AIORT_ISOLATE names `{n}`, no cell of this app; its cells: {}", ids.join(", "))),
        }
    }
    Ok(Some(only))
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

    #[test]
    fn isolate_names_cells_the_app_has() {
        let ids = ["todo", "mail", "sync"];
        assert_eq!(isolated(None, &ids), Ok(None));
        assert_eq!(isolated(Some(" "), &ids), Ok(None), "set but empty: every cell");
        assert_eq!(isolated(Some("mail, todo"), &ids), Ok(Some(vec!["mail", "todo"])));
        let e = isolated(Some("mial"), &ids).unwrap_err();
        assert!(e.contains("names `mial`, no cell of this app; its cells: todo, mail, sync"), "{e}");
    }
}
