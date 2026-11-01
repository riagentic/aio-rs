//! The log in a page: `aio::info!` and its kin, as natively, written to the
//! page's console (a browser has no log files and drops stderr).
use std::fmt::Display;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Trace,
    Debug,
    #[default]
    Info,
    Warn,
    Error,
}

static GATE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(Level::Info as u8);

/// Lines below `level` are dropped.
pub fn set_level(level: Level) {
    GATE.store(level as u8, std::sync::atomic::Ordering::Relaxed);
}

/// Would a line at `level` be written?
pub fn enabled(level: Level) -> bool {
    level as u8 >= GATE.load(std::sync::atomic::Ordering::Relaxed)
}

/// One entry on the console, at its level: `[cat] msg k=v`.
pub fn emit(level: Level, module: &str, cat: Option<&str>, msg: std::fmt::Arguments, data: &[(&str, &dyn Display)]) {
    let cat = cat.unwrap_or(if module == "aio" || module.starts_with("aio::") { "aio" } else { "app" });
    let line = data.iter().fold(format!("[{cat}] {msg}"), |l, (k, v)| format!("{l} {k}={v}")).into();
    match level {
        Level::Trace | Level::Debug => web_sys::console::debug_1(&line),
        Level::Info => web_sys::console::info_1(&line),
        Level::Warn => web_sys::console::warn_1(&line),
        Level::Error => web_sys::console::error_1(&line),
    }
}
