//! A cell's state machine (aio's `machine`): declared states and the methods
//! that move between them, made with [`machine!`](crate::machine!). On a
//! cell, `#[cell(machine = field)]`: a method the machine names runs only
//! from a state that has it (else it is refused, nothing changed) and moves
//! the field to its target first, so the method sees where it is going.
//! Methods it never names run in any state. A store refuses to open a cell
//! whose machine names a method the cell lacks or has a state no path
//! reaches; a state with no way out is said in the log.
use crate::{Result, reject};

/// What [`machine!`](crate::machine!) generates.
pub trait Machine: Copy + PartialEq + std::fmt::Debug + 'static {
    /// Every state; the first is where the machine starts.
    const STATES: &'static [Self];
    /// `(from, method, to)`.
    const MOVES: &'static [(Self, &'static str, Self)];
}

/// Where `method` moves the machine from `now`: `None` for a method it
/// never names (and for an async method's later steps, `m.1`, and aio's
/// own `$…` events); refused when `now` has no such move.
pub fn step<M: Machine>(now: M, method: &str) -> Result<Option<M>> {
    if method.contains('.') || method.starts_with('$') || !M::MOVES.iter().any(|(_, m, _)| *m == method) {
        return Ok(None);
    }
    match M::MOVES.iter().find(|(s, m, _)| *s == now && *m == method) {
        Some((_, _, to)) => Ok(Some(*to)),
        None => {
            let from: Vec<String> =
                M::MOVES.iter().filter(|(_, m, _)| *m == method).map(|(s, _, _)| format!("{s:?}")).collect();
            reject!("`{method}` can't run while {now:?}: only from {}", from.join(", "))
        }
    }
}

/// What is wrong with `M` on a cell with `methods` (pure): moves on
/// methods it lacks, states no path from the first reaches.
pub fn errors<M: Machine>(methods: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (s, m, _) in M::MOVES {
        if !methods.contains(m) {
            out.push(format!("{s:?} moves on `{m}`, which is not a method of the cell"));
        }
    }
    let mut seen: Vec<M> = M::STATES.first().copied().into_iter().collect();
    while let Some(next) = M::MOVES.iter().find(|(s, _, t)| seen.contains(s) && !seen.contains(t)).map(|m| m.2) {
        seen.push(next);
    }
    let start = M::STATES.first().map(|s| format!("{s:?}")).unwrap_or_default();
    out.extend(M::STATES.iter().filter(|s| !seen.contains(s)).map(|s| format!("{s:?} is unreachable from {start}")));
    out
}

/// The states with no way out (pure): fine for an end (`Delivered`),
/// worth a look otherwise.
pub fn dead_ends<M: Machine>() -> Vec<M> {
    M::STATES.iter().copied().filter(|s| !M::MOVES.iter().any(|(f, _, _)| f == s)).collect()
}

/// A state machine: an enum of plain states (the first is the default,
/// where it starts), and per state the methods that leave it and where to.
/// Serialized as the lowercase name, like [`keys!`](crate::keys!).
///
/// ```ignore
/// aio::machine! {
///     pub enum Order {
///         Draft { submit => Review },
///         Review { approve => Approved, reject => Draft },
///         Approved {},
///     }
/// }
/// ```
#[macro_export]
macro_rules! machine {
    ($(#[$m:meta])* $vis:vis enum $name:ident {
        $first:ident { $($fm:ident => $ft:ident),* $(,)? }
        $(, $state:ident { $($method:ident => $to:ident),* $(,)? })* $(,)?
    }) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, $crate::serde::Serialize, $crate::serde::Deserialize)]
        #[serde(crate = "aio::serde", rename_all = "lowercase")]
        $vis enum $name {
            #[default]
            $first,
            $($state),*
        }

        impl $crate::machine::Machine for $name {
            const STATES: &'static [Self] = &[$name::$first, $($name::$state),*];
            const MOVES: &'static [(Self, &'static str, Self)] = &[
                $(($name::$first, stringify!($fm), $name::$ft),)*
                $($(($name::$state, stringify!($method), $name::$to),)*)*
            ];
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    crate::machine! {
        pub enum Order {
            Draft { submit => Review },
            Review { approve => Approved, reject => Draft },
            Approved { ship => Shipped, cancel => Cancelled },
            Shipped {},
            Cancelled { reopen => Draft },
        }
    }

    crate::machine! {
        enum Island {
            A { go => B },
            B { back => A },
            C { go => A },
        }
    }

    const METHODS: &[&str] = &["submit", "approve", "reject", "ship", "cancel", "reopen", "rename"];

    #[test]
    fn a_machine_moves_only_where_its_state_allows() {
        assert_eq!(Order::default(), Order::Draft, "the first state is where it starts");
        assert_eq!(step(Order::Draft, "submit"), Ok(Some(Order::Review)));
        assert_eq!(step(Order::Review, "reject"), Ok(Some(Order::Draft)));
        let e = step(Order::Draft, "ship").unwrap_err();
        assert_eq!(e.why, "`ship` can't run while Draft: only from Approved");
        assert_eq!(step(Order::Shipped, "rename"), Ok(None), "a method it never names runs anywhere");
        assert_eq!(step(Order::Shipped, "submit.1"), Ok(None), "an async method moves once, on its first part");
        assert_eq!(step(Order::Shipped, "$restore"), Ok(None));
        assert_eq!(crate::json(&Order::Approved).unwrap(), "approved");
    }

    #[test]
    fn a_wrong_machine_is_said_by_name() {
        assert!(errors::<Order>(METHODS).is_empty());
        assert_eq!(dead_ends::<Order>(), [Order::Shipped]);
        assert_eq!(errors::<Order>(&["submit"])[0], "Review moves on `approve`, which is not a method of the cell");
        assert_eq!(errors::<Island>(&["go", "back"]), ["C is unreachable from A"]);
        assert!(dead_ends::<Island>().is_empty());
    }
}
