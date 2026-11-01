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

/// The DevTools protocol on loopback `port` (`AIORT_CDP`), or nothing: an
/// app that didn't ask opens no port, and a release build (`debug` false)
/// none at all (any JavaScript in its page, its user's data).
pub fn cdp(port: Option<&str>, debug: bool) -> Result<Vec<String>, String> {
    match port.map(str::trim).filter(|p| !p.is_empty()) {
        None => Ok(vec![]),
        Some(p) if !debug => {
            Err(format!("AIORT_CDP={p}: a release build opens no DevTools protocol (use a debug build)"))
        }
        Some(p) => match p.parse::<u16>() {
            Ok(n) if n > 0 => Ok(vec![format!("--remote-debugging-port={n}")]),
            _ => Err(format!("AIORT_CDP={p}: not a port")),
        },
    }
}

/// The switches this start adds, each refusal said once.
pub(crate) fn extra(env: &dyn Fn(&str) -> Option<String>) -> Vec<String> {
    let (mut ok, refused) = from_env(&env("AIORT_ELECTRON_ARGS").unwrap_or_default());
    for (tok, why) in refused {
        eprintln!("[aio] AIORT_ELECTRON_ARGS: {tok} is not passed: {why}");
    }
    match cdp(env("AIORT_CDP").as_deref(), cfg!(debug_assertions)) {
        Ok(c) => {
            if let Some(s) = c.first() {
                eprintln!("[aio] DevTools protocol open on 127.0.0.1 ({s}): AIORT_CDP");
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
        assert_eq!(cdp(None, true), Ok(vec![]));
        assert_eq!(cdp(Some(" "), true), Ok(vec![]));
        assert_eq!(cdp(Some("9222"), true), Ok(vec!["--remote-debugging-port=9222".to_string()]));
        assert!(cdp(Some("0"), true).is_err() && cdp(Some("x"), true).is_err() && cdp(Some("70000"), true).is_err());
        // A packaged (release) build never opens it.
        assert!(cdp(Some("9222"), false).unwrap_err().contains("release build"));
        assert_eq!(cdp(None, false), Ok(vec![]));
        let env = |k: &str| (k == "AIORT_ELECTRON_ARGS").then(|| "--disable-gpu --inspect".to_string());
        assert_eq!(extra(&env), ["--disable-gpu"]);
    }
}
