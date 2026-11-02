//! Form state (aio's `useForm`, `useFieldArray`, `bindInput`): the values as
//! one typed struct, checked by one function that names the field each
//! problem is about (per-field rules, cross-field rules and a schema are
//! all that function), plus rules that wait (a uniqueness lookup).
//! A field's error shows once it was left or a submit was tried; a list of
//! rows is a `Vec` field ([`move_item`] reorders it).
use crate::ui::prelude::*;
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::time::Duration;

/// What a form's check found: `(field, message)`, the first per field
/// wins (put a field's own rules before cross-field ones); field `""` is
/// about the form as a whole.
pub type Problems = Vec<(&'static str, String)>;

#[derive(Clone, PartialEq)]
struct State<T> {
    initial: T,
    values: T,
    touched: BTreeSet<&'static str>,
    edited: BTreeSet<&'static str>,
    tried: bool,
    /// What the waiting rules said, by field.
    late: BTreeMap<&'static str, String>,
    /// Waiting rules running, by field: the run that may answer.
    checking: BTreeMap<&'static str, u64>,
}

impl<T: Clone> State<T> {
    fn new(initial: T) -> Self {
        Self {
            values: initial.clone(),
            initial,
            touched: BTreeSet::new(),
            edited: BTreeSet::new(),
            tried: false,
            late: BTreeMap::new(),
            checking: BTreeMap::new(),
        }
    }
}

/// A form's handle ([`use_form`]): `Copy`, for every handler.
pub struct Form<T: 'static> {
    state: Signal<State<T>>,
    check: fn(&T) -> Problems,
}

impl<T> Clone for Form<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Form<T> {}

impl<T: Clone + PartialEq + 'static> Form<T> {
    /// The values now (reading subscribes the view).
    pub fn values(&self) -> T {
        self.state.read().values.clone()
    }

    /// Change `field` (`f` edits the values): it is edited, what a waiting
    /// rule said about it is stale, and so is the form's own error.
    pub fn edit(self, field: &'static str, f: impl FnOnce(&mut T)) {
        let mut s = self.state.write();
        f(&mut s.values);
        s.edited.insert(field);
        s.late.remove(field);
        s.late.remove("");
    }

    /// The field was left: its error may show now.
    pub fn touch(self, field: &'static str) {
        if !self.state.read_untracked().touched.contains(field) {
            self.state.write().touched.insert(field);
        }
    }

    fn own(&self, field: &'static str) -> Option<String> {
        let s = self.state.read();
        let now = (self.check)(&s.values);
        now.into_iter().find(|(f, _)| *f == field).map(|(_, m)| m).or_else(|| s.late.get(field).cloned())
    }

    /// `field`'s error, once it was left or a submit was tried.
    pub fn error(&self, field: &'static str) -> Option<String> {
        let shown = {
            let s = self.state.read();
            s.tried || s.touched.contains(field)
        };
        shown.then(|| self.own(field)).flatten()
    }

    /// The form's own error (no single field's), once a submit was tried.
    pub fn form_error(&self) -> Option<String> {
        if self.state.read().tried { self.own("") } else { None }
    }

    /// No problem, and no waiting rule still running or refusing.
    pub fn valid(&self) -> bool {
        let s = self.state.read();
        (self.check)(&s.values).is_empty() && s.late.is_empty() && s.checking.is_empty()
    }

    pub fn checking(&self, field: &'static str) -> bool {
        self.state.read().checking.contains_key(field)
    }

    /// Any field changed from where it started.
    pub fn dirty(&self) -> bool {
        let s = self.state.read();
        s.values != s.initial
    }

    /// Was `field` edited?
    pub fn edited(&self, field: &'static str) -> bool {
        self.state.read().edited.contains(field)
    }

    /// Try a submit: every error shows; the values if valid.
    pub fn submit(self) -> Option<T> {
        self.state.write().tried = true;
        self.valid().then(|| self.values())
    }

    /// Back to the initial values, nothing touched.
    pub fn reset(self) {
        let initial = self.state.read_untracked().initial.clone();
        self.state.set(State::new(initial));
    }
}

/// A form starting at `initial`, checked by `check` (a hook).
pub fn use_form<T: Clone + PartialEq + 'static>(initial: impl FnOnce() -> T, check: fn(&T) -> Problems) -> Form<T> {
    let state = signal(|| State::new(initial()));
    Form { state, check }
}

/// A rule that waits, on `field` (`part` of the values) (a hook): run
/// `debounce` after the last edit of it, unless the field's own check
/// already fails; an answer for a value since changed is dropped. The form
/// is not valid while it runs or refuses.
pub fn use_async_rule<T, V, F>(
    form: Form<T>,
    field: &'static str,
    part: fn(&T) -> V,
    debounce: Duration,
    rule: impl Fn(V) -> F + 'static,
) where
    T: Clone + PartialEq + 'static,
    V: Clone + PartialEq + 'static,
    F: Future<Output = Option<String>> + 'static,
{
    let state = form.state;
    let rule = std::rc::Rc::new(rule);
    let value = memo(move || part(&state.read().values));
    let runs = signal(|| 0u64);
    let started = once(|| std::rc::Rc::new(std::cell::Cell::new(false)));
    effect(move || {
        let v = value.get();
        // Not the starting value: only an edit asks.
        if !started.replace(true) {
            return;
        }
        let run = *runs.read_untracked() + 1;
        runs.set(run);
        if (form.check)(&state.read_untracked().values).iter().any(|(f, _)| *f == field) {
            state.write().checking.remove(field);
            return;
        }
        state.write().checking.insert(field, run);
        let rule = rule.clone();
        spawn(async move {
            super::sleep(debounce).await;
            if *runs.read_untracked() != run {
                return;
            }
            let said = rule(v).await;
            if *runs.read_untracked() != run {
                return;
            }
            let mut s = state.write();
            s.checking.remove(field);
            match said {
                Some(m) => _ = s.late.insert(field, m),
                None => _ = s.late.remove(field),
            }
        });
    });
}

/// Row `from` of a list moved to `to` (aio's `useFieldArray().move`); out of
/// range: nothing moves.
pub fn move_item<V>(rows: &mut Vec<V>, from: usize, to: usize) {
    if from < rows.len() && to < rows.len() {
        let r = rows.remove(from);
        rows.insert(to, r);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_row_moves_and_out_of_range_nothing_does() {
        let mut v = vec!['a', 'b', 'c'];
        super::move_item(&mut v, 0, 2);
        assert_eq!(v, ['b', 'c', 'a']);
        super::move_item(&mut v, 2, 9);
        assert_eq!(v, ['b', 'c', 'a']);
    }
}
