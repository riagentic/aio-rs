//! The Chromium switches a host may add (`AIORT_ELECTRON_ARGS`, aio
//! `AIO_ELECTRON_ARGS`): an allow-list of the display, GPU, locale and
//! logging ones a headless, VM or GPU-less host needs, none of which changes
//! who may reach the page, what runs in it or where its traffic goes.
//! Anything else is refused and said, with the reason where there is a
//! better way. The DevTools protocol opens only on purpose, `AIORT_CDP`, and
//! only in a debug build: a packaged app's page is never reachable by it.

/// Switch names (left of `=`) the variable may carry.
const ALLOWED: &[&str] = &[
    // GPU and rendering: the headless and VM remedies.
    "disable-gpu",
    "disable-gpu-compositing",
    "disable-software-rasterizer",
    "disable-accelerated-2d-canvas",
    "disable-accelerated-video-decode",
    "enable-unsafe-swiftshader",
    "ignore-gpu-blocklist",
    "use-gl",
    "use-angle",
    "force-device-scale-factor",
    "force-color-profile",
    "disable-lcd-text",
    "disable-smooth-scrolling",
    // A container's 64 MB /dev/shm otherwise kills the renderer.
    "disable-dev-shm-usage",
    "ozone-platform",
    "ozone-platform-hint",
    // A window nobody watches must not be throttled into looking hung.
    "disable-background-timer-throttling",
    "disable-backgrounding-occluded-windows",
    "disable-renderer-backgrounding",
    "lang",
    "enable-logging",
    "log-level",
];

/// Why a switch someone may reach for is refused, and the way instead.
fn refused_why(name: &str) -> Option<&'static str> {
    Some(match name {
        "remote-debugging-port" | "remote-debugging-pipe" | "remote-allow-origins" | "inspect" | "inspect-brk" => {
            "it opens the DevTools protocol (any JavaScript in the page): AIORT_CDP=<port> opens it on purpose"
        }
        "disable-web-security" | "allow-running-insecure-content" | "unsafely-treat-insecure-origin-as-secure" => {
            "it switches off the origin rules the page relies on"
        }
        "ignore-certificate-errors" => "it trusts any certificate",
        "allow-file-access-from-files" => "it lets a file:// page read the disk",
        "js-flags" => "it hands arbitrary V8 flags to the renderer",
        "no-sandbox" | "disable-setuid-sandbox" | "disable-gpu-sandbox" | "no-zygote" | "single-process" => {
            "aio-rs decides the sandbox itself, after measuring the host (AIORT_SANDBOX=1 keeps it strict)"
        }
        "disable-features" | "enable-features" => "it switches Chromium's own protections by name",
        "load-extension" | "disable-extensions-except" => "it loads someone else's code into the app",
        "user-data-dir" => "it moves the app's own profile",
        "proxy-server" | "host-rules" | "host-resolver-rules" => "it redirects the app's traffic",
        "gpu-launcher" | "renderer-cmd-prefix" | "utility-cmd-prefix" | "browser-subprocess-path" => {
            "it runs another program as the app's child"
        }
        _ => return None,
    })
}

/// `raw` (space-separated) as (the switches passed, each refused one with
/// why). Pure.
pub fn from_env(raw: &str) -> (Vec<String>, Vec<(String, String)>) {
    let (mut ok, mut refused) = (vec![], vec![]);
    for tok in raw.split_whitespace() {
        let name = tok.strip_prefix("--").map(|t| t.split_once('=').map_or(t, |(n, _)| n));
        let well_formed = name.is_some_and(|n| {
            n.starts_with(|c: char| c.is_ascii_alphanumeric())
                && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                && !tok.chars().any(char::is_control)
        });
        match name {
            Some(n) if well_formed && ALLOWED.contains(&n) => ok.push(tok.to_string()),
            Some(n) if well_formed => refused.push((
                tok.to_string(),
                refused_why(n).unwrap_or("not one of the display, GPU, locale and logging switches it carries").into(),
            )),
            _ => refused.push((tok.to_string(), "not a switch (--name or --name=value)".into())),
        }
    }
    (ok, refused)
}

/// The DevTools protocol on loopback (any JavaScript in the page, its
/// user's data), or nothing: the launch's `--cdp=<port>` (`typed`), else
/// `AIORT_CDP` (`env`). An app that didn't ask opens no port. A build
/// outside `ax dev` (`debug` false: no `dev` feature, a release build or a
/// plain `cargo build` alike) opens one only on the typed flag and only when its
/// app allows DevTools (`devtools`: `Window { devtools }`, aio's `--cdp`);
/// it never reads the variable (aio's `ignoreDebugEnv`, always on): one
/// left over from another app opens nothing here.
pub fn cdp(typed: Option<&str>, env: Option<&str>, debug: bool, devtools: bool) -> Result<Vec<String>, String> {
    let (p, by) = match (asked(typed), asked(env)) {
        (Some(p), _) => (p, "--cdp"),
        (None, Some(p)) => (p, "AIORT_CDP"),
        (None, None) => return Ok(vec![]),
    };
    match p.parse::<u16>() {
        _ if !debug && by == "AIORT_CDP" => Err(format!(
            "AIORT_CDP={p}: a build outside ax dev reads no debug setting from its environment; --cdp=<port> asks \
             for one launch, in an app that allows DevTools (Window {{ devtools: true }})"
        )),
        _ if !debug && !devtools => Err(format!(
            "--cdp={p}: a build outside ax dev opens no DevTools protocol unless its app allows it (Window {{ devtools: \
             true }})"
        )),
        Ok(n) if n > 0 => Ok(vec![format!("--remote-debugging-port={n}")]),
        _ => Err(format!("{by}={p}: not a port")),
    }
}

/// A value given, trimmed; blank is none.
fn asked(v: Option<&str>) -> Option<&str> {
    v.map(str::trim).filter(|p| !p.is_empty())
}

/// The switches this start adds, each refusal said once: `typed` is the
/// launch's `--cdp`, `devtools` the app's `Window { devtools }`.
pub(crate) fn extra(env: &dyn Fn(&str) -> Option<String>, typed: Option<&str>, devtools: bool) -> Vec<String> {
    let (mut ok, refused) = from_env(&env("AIORT_ELECTRON_ARGS").unwrap_or_default());
    for (tok, why) in refused {
        eprintln!("[aio] AIORT_ELECTRON_ARGS: {tok} is not passed: {why}");
    }
    // `ax dev`'s tooling, not `debug_assertions`: ax dev builds without them
    // (they run as the release does), and a plain debug build is no dev tool.
    let debug = cfg!(feature = "dev");
    match cdp(typed, env("AIORT_CDP").as_deref(), debug, devtools) {
        Ok(c) => {
            if let Some(s) = c.first() {
                let by = if typed.is_some() { "--cdp" } else { "AIORT_CDP" };
                let warn = if debug { "" } else { "WARN " };
                eprintln!(
                    "[aio] {warn}DevTools protocol open on 127.0.0.1 ({s}): {by}; any process of this machine can \
                     drive the window while it is open"
                );
            }
            ok.extend(c);
        }
        Err(e) => eprintln!("[aio] {e}: no DevTools protocol"),
    }
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_display_and_gpu_switches_pass_and_each_refusal_says_why() {
        // aio tests/electron-args-env.test.ts.
        let (ok, no) = from_env(
            "--disable-gpu --use-gl=swiftshader  --remote-debugging-port=9222 --no-sandbox --frobnicate x --a\u{0}b --toString",
        );
        assert_eq!(ok, ["--disable-gpu", "--use-gl=swiftshader"]);
        let why = |t: &str| no.iter().find(|(k, _)| k == t).map(|(_, w)| w.clone()).unwrap();
        assert!(why("--remote-debugging-port=9222").contains("AIORT_CDP"));
        assert!(why("--no-sandbox").contains("AIORT_SANDBOX"));
        assert!(why("--frobnicate").contains("not one of"));
        assert!(why("--toString").contains("not one of"), "no reason nobody wrote");
        assert!(why("x").contains("not a switch") && why("--a\u{0}b").contains("not a switch"));
        assert_eq!(from_env(""), (vec![], vec![]));
        // Every allowed name is inert: none reads like a way in.
        for n in ALLOWED {
            assert!(
                refused_why(n).is_none() && !["debug", "sandbox", "inspect", "security"].iter().any(|b| n.contains(b))
            );
        }
    }

    #[test]
    fn devtools_open_only_when_asked() {
        let port = Ok(vec!["--remote-debugging-port=9222".to_string()]);
        assert_eq!(cdp(None, None, true, false), Ok(vec![]));
        assert_eq!(cdp(None, Some(" "), true, false), Ok(vec![]));
        assert_eq!(cdp(None, Some("9222"), true, false), port);
        assert_eq!(cdp(Some("9222"), Some("1"), true, false), port, "the typed flag wins");
        for bad in ["0", "x", "70000"] {
            assert!(cdp(None, Some(bad), true, false).unwrap_err().contains("not a port"));
        }
        // A release build: never from the environment, the typed flag only
        // where its app allows DevTools (aio's --cdp, never on by default).
        assert!(cdp(None, Some("9222"), false, true).unwrap_err().contains("outside ax dev reads no debug setting"));
        assert!(cdp(Some("9222"), None, false, false).unwrap_err().contains("Window { devtools: true }"));
        assert_eq!(cdp(Some("9222"), None, false, true), port);
        assert_eq!(cdp(None, None, false, true), Ok(vec![]));
        let env = |k: &str| (k == "AIORT_ELECTRON_ARGS").then(|| "--disable-gpu --inspect".to_string());
        assert_eq!(extra(&env, None, false), ["--disable-gpu"]);
    }
}
