//! `[app] version`'s form, checked as `aio.toml` is read and read by
//! `ax build`: `major.minor` (builds are numbered from git), or a pinned
//! `major.minor.patch`, either perhaps with a stage.

/// A declared version: `major.minor`, the patch when pinned, the stage.
#[derive(Debug, PartialEq)]
pub struct Declared {
    pub base: (u64, u64),
    pub pinned: Option<u64>,
    pub stage: Option<&'static str>,
}

/// The stages, least finished first.
pub const STAGES: [&str; 3] = ["alpha", "beta", "rc"];

/// `[app] version`, strictly (pure): anything but `M.m` or `M.m.p`, with an
/// optional stage, is refused by name.
pub fn declared(v: &str) -> Result<Declared, String> {
    let raw = v.trim();
    let (core, stage) = match raw.split_once('-') {
        Some((c, s)) => (c, Some(STAGES.into_iter().find(|t| *t == s).ok_or_else(|| refusal(raw))?)),
        None => (raw, None),
    };
    let nums: Vec<u64> = core
        .split('.')
        .map(|n| n.chars().all(|c| c.is_ascii_digit()).then(|| n.parse().ok()).flatten())
        .collect::<Option<_>>()
        .ok_or_else(|| refusal(raw))?;
    match nums[..] {
        [major, minor] => Ok(Declared { base: (major, minor), pinned: None, stage }),
        [major, minor, patch] => Ok(Declared { base: (major, minor), pinned: Some(patch), stage }),
        _ => Err(refusal(raw)),
    }
}

fn refusal(shown: &str) -> String {
    format!(
        "aio.toml: [app] version {shown:?}: an app's version is \"major.minor\" (write \"1.2\"; aio numbers builds \
         from commits: 1.2.<commit count>) or a pinned \"major.minor.patch\" (used as written), either one perhaps \
         followed by a stage, -alpha, -beta or -rc (\"1.2-beta\" builds 1.2.345-beta, ranked below 1.2.345)"
    )
}
