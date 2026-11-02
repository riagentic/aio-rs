//! `hold dx|air [keys] [gap ms] [rows]`: the held-key harness
//! (`crates/air-bench/src/hold_unix.rs`); its page talks over a Unix socket.
#[cfg(unix)]
include!("../hold_unix.rs");

#[cfg(not(unix))]
fn main() {
    eprintln!("hold: the harness talks to its page over a Unix socket");
    std::process::exit(2);
}
