//! The log macros, the same natively and in a page (`crate::log` writes the line).

/// Log at a level: `aio::log!(Level::Info, "msg {x}")`, with a category and
/// fields `aio::info!(cat: "net", peer = id, n = 3, "msg")`. Fields are
/// `Display`; nothing is evaluated below the level.
#[macro_export]
macro_rules! log {
    (@f $l:expr, $cat:expr, [$($f:tt)*], $k:ident = $v:expr, $($t:tt)+) => {
        $crate::log!(@f $l, $cat, [$($f)* (stringify!($k), $v)], $($t)+)
    };
    (@f $l:expr, $cat:expr, [$(($k:expr, $v:expr))*], $($fmt:tt)+) => {
        if $crate::log::enabled($l) {
            $crate::log::emit($l, module_path!(), $cat, format_args!($($fmt)+), &[$(($k, &$v as &dyn ::std::fmt::Display)),*])
        }
    };
    ($l:expr, cat: $cat:expr, $($t:tt)+) => { $crate::log!(@f $l, Some($cat), [], $($t)+) };
    ($l:expr, $($t:tt)+) => { $crate::log!(@f $l, None, [], $($t)+) };
}

/// [`log!`] at `Trace`.
#[macro_export]
macro_rules! trace { ($($t:tt)+) => { $crate::log!($crate::log::Level::Trace, $($t)+) }; }
/// [`log!`] at `Debug`.
#[macro_export]
macro_rules! debug { ($($t:tt)+) => { $crate::log!($crate::log::Level::Debug, $($t)+) }; }
/// [`log!`] at `Info`.
#[macro_export]
macro_rules! info { ($($t:tt)+) => { $crate::log!($crate::log::Level::Info, $($t)+) }; }
/// [`log!`] at `Warn`.
#[macro_export]
macro_rules! warn { ($($t:tt)+) => { $crate::log!($crate::log::Level::Warn, $($t)+) }; }
/// [`log!`] at `Error`.
#[macro_export]
macro_rules! error { ($($t:tt)+) => { $crate::log!($crate::log::Level::Error, $($t)+) }; }
