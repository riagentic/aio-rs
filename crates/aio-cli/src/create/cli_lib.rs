//! The tool: its cell (the state, saved, and the only ways to change it)
//! and its commands. `ax state`, `ax events` and `ax call` see the cell
//! as for any app.
use aio::Door;

/// What the tool keeps: saved on every change, its history kept.
#[aio::cell(persist, id = "notes")]
pub struct Notes {
    pub items: Vec<String>,
}

#[aio::methods]
impl Notes {
    /// Keep a note; an empty one is refused.
    pub fn add(&mut self, text: String) -> aio::Result {
        aio::ensure!(!text.trim().is_empty(), "a note needs some text");
        self.items.push(text);
        Ok(())
    }

    /// Forget note number `n` (from 1, as `list` shows them).
    pub fn finish(&mut self, n: usize) -> aio::Result {
        aio::ensure!((1..=self.items.len()).contains(&n), "there is no note {n}");
        self.items.remove(n - 1);
        Ok(())
    }
}

pub const USAGE: &str = "usage: add <text…> | done <n> | list";

/// Run one command on the cell behind `door` (pure but for the call): what
/// to print, or why not (then the exit code is 1; bad usage, 2).
pub fn run(door: &impl Door<Notes>, args: &[String]) -> Result<String, (u8, String)> {
    let refused = |e: aio::Rejected| (1, e.why);
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["add", text @ ..] if !text.is_empty() => door.add(text.join(" ")).map(|_| "kept".into()).map_err(refused),
        ["done", n] => {
            let n = n.parse().map_err(|_| (2, format!("done {n}: a note's number\n{USAGE}")))?;
            door.finish(n).map(|_| "done".into()).map_err(refused)
        }
        ["list"] | [] => Ok(door.read(|s| {
            s.items.iter().enumerate().map(|(i, t)| format!("{}. {t}", i + 1)).collect::<Vec<_>>().join("\n")
        })),
        _ => Err((2, USAGE.into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_are_kept_listed_and_done() {
        let h = aio::Handle::<Notes>::memory();
        let s = |v: &[&str]| v.iter().map(|a| a.to_string()).collect::<Vec<_>>();
        assert_eq!(run(&h, &s(&["add", "buy", "milk"])), Ok("kept".into()));
        assert_eq!(run(&h, &s(&["add", "call", "Ann"])), Ok("kept".into()));
        assert_eq!(run(&h, &s(&["list"])), Ok("1. buy milk\n2. call Ann".into()));
        assert_eq!(run(&h, &s(&["done", "1"])), Ok("done".into()));
        assert_eq!(run(&h, &s(&[])), Ok("1. call Ann".into()));
        assert_eq!(run(&h, &s(&["done", "7"])), Err((1, "there is no note 7".into())));
        assert_eq!(run(&h, &s(&["add", " "])).unwrap_err().0, 1);
        assert_eq!(run(&h, &s(&["done", "x"])).unwrap_err().0, 2);
        assert_eq!(run(&h, &s(&["fly"])), Err((2, USAGE.into())));
    }
}
