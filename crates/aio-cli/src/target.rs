//! Which home of an app the verbs look at: the one a launch with the same
//! `--home`, `--profile` or `--instance` (or `<app>@<profile>`) would run
//! on (`aio::paths::plan`, the app's own rule). Asked for nothing, and the
//! app isn't running from its own home but from exactly one other (a
//! profile, an instance), the verbs follow it and say so.
use aio::paths::{Ask, Plan};
use std::path::{Path, PathBuf};

/// The app a verb is about: the word that named it, its id, and where it
/// runs.
#[derive(Debug)]
pub struct Target {
    pub name: String,
    pub id: String,
    pub plan: Plan,
    /// Its package, when run in its workspace.
    pub pkg: Option<crate::dev::Pkg>,
}

impl Target {
    pub fn store(&self) -> PathBuf {
        self.plan.store.clone()
    }
}

fn env(k: &str) -> Option<PathBuf> {
    std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// The other homes app `id` may run from beside `plan`'s: its profiles
/// (`<home>-<name>`), its instances and a ported app's debug-build home
/// (`[home] dev`), as (flag, plan).
fn others(id: &str, port_of: Option<&str>, rules: &aio::config::Home, plan: &Plan) -> Vec<(String, Plan)> {
    let mut out = Vec::new();
    let home = plan.data.parent().unwrap_or(Path::new(""));
    let stem = home.file_name().map(|n| format!("{}-", n.to_string_lossy())).unwrap_or_default();
    let names = |dir: &Path| -> Vec<String> {
        let mut n: Vec<String> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        n.sort();
        n
    };
    let store = plan.store.file_name().unwrap_or_default().to_owned();
    let mut try_ask = |flag: String, ask: Ask| {
        if let Ok(p) = aio::paths::plan_in(id, port_of, rules, &ask, &env)
            && p.data != plan.data
            && !out.iter().any(|(_, q): &(String, Plan)| q.data == p.data)
        {
            out.push((flag, Plan { store: p.data.join(&store), ..p }));
        }
    };
    if port_of.is_some() && rules.dev.is_some() {
        try_ask("a debug build".into(), Ask { dev: true, ..Ask::default() });
    }
    for n in names(home.parent().unwrap_or(Path::new(""))) {
        if let Some(p) = n.strip_prefix(&stem) {
            try_ask(format!("--profile {p}"), Ask { profile: Some(p.into()), ..Ask::default() });
        }
    }
    let probe = Ask { instance: Some("x".into()), ..Ask::default() };
    if let Some(root) = aio::paths::plan_in(id, port_of, rules, &probe, &env)
        .ok()
        .and_then(|p| Some(p.data.parent()?.parent()?.parent()?.to_path_buf()))
    {
        for n in names(&root) {
            try_ask(format!("--instance {n}"), Ask { instance: Some(n), ..Ask::default() });
        }
    }
    out
}

/// Pick the home among `own` (not running) and the running `others`: own,
/// unless exactly one other runs (pure).
pub fn follow(own: Plan, mut running: Vec<(String, Plan)>, name: &str) -> Result<(Plan, Option<String>), String> {
    match running.len() {
        0 => Ok((own, None)),
        1 => {
            let (flag, p) = running.remove(0);
            Ok((p, Some(format!("ax: {name} runs from {flag}, not its own home: following it"))))
        }
        _ => {
            let flags: Vec<&str> = running.iter().map(|(f, _)| f.as_str()).collect();
            Err(format!("{name} isn't running from its own home but from {}: name one", flags.join(", ")))
        }
    }
}

/// App `word` (`name@profile` names a profile), where a launch with `ask`
/// (`--home`, `--profile`, `--instance`) has it run: by its `aio.toml` in a
/// workspace (a ported app on its aio home), else by its id.
pub fn of(word: &str, ask: &Ask) -> Result<Target, String> {
    let (name, at) = word.split_once('@').map_or((word, None), |(n, p)| (n, Some(p)));
    let mut ask = ask.clone();
    if let Some(p) = at {
        if ask.profile.as_deref().is_some_and(|q| q != p) {
            return Err(format!("{word} and --profile both name a profile: give one"));
        }
        ask.profile = Some(p.into());
    }
    let pkg = crate::dev::metadata().ok().and_then(|m| crate::dev::locate(&m, name).ok());
    let (id, cfg, plan) = match &pkg {
        Some(pkg) => {
            let (app, cfg) = crate::build::app_of(pkg)?;
            let plan = aio::paths::plan_app(&cfg, &ask).map_err(|e| e.to_string())?;
            (app.id, Some(cfg), plan)
        }
        None => (name.to_string(), None, aio::paths::plan(name, None, &ask, &env)?),
    };
    let target = |plan| Target { name: word.into(), id: id.clone(), plan, pkg };
    if !matches!(plan.from, "default" | "[home] dir") || aio::paths::running(&plan.data) {
        return Ok(target(plan));
    }
    let none = aio::config::Home::default();
    let (port_of, rules) = cfg.as_ref().map_or((None, &none), |c| (c.app.port_of.as_deref(), &c.home));
    let running: Vec<(String, Plan)> =
        others(&id, port_of, rules, &plan).into_iter().filter(|(_, p)| aio::paths::running(&p.data)).collect();
    let (plan, said) = follow(plan, running, name)?;
    if let Some(s) = said {
        eprintln!("{s}");
    }
    Ok(target(plan))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(dir: &str) -> Plan {
        let p = PathBuf::from(dir);
        Plan {
            data: p.join("data"),
            logs: p.join("logs"),
            cache: p.join("cache"),
            store: p.join("data/state.db"),
            profile: None,
            port_of: None,
            from: "default",
            asked: false,
        }
    }

    #[test]
    fn its_own_home_unless_exactly_one_other_runs() {
        assert_eq!(follow(at("/a"), vec![], "notes").unwrap().0.data, PathBuf::from("/a/data"));
        let (p, said) = follow(at("/a"), vec![("--profile dev".into(), at("/a-dev"))], "notes").unwrap();
        assert_eq!(p.data, PathBuf::from("/a-dev/data"));
        assert!(said.unwrap().contains("notes runs from --profile dev"));
        let two = vec![("--profile dev".into(), at("/a-dev")), ("--instance agent1".into(), at("/i/agent1/a"))];
        let e = follow(at("/a"), two, "notes").unwrap_err();
        assert!(e.contains("from --profile dev, --instance agent1: name one"), "{e}");
    }

    #[test]
    fn profiles_and_instances_beside_a_home_are_found() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().join("apps/notes");
        for p in [home.join("data"), d.path().join("apps/notes-dev/data"), d.path().join("apps/other/data")] {
            std::fs::create_dir_all(p).unwrap();
        }
        let plan = Plan { data: home.join("data"), ..at("/") };
        let found: Vec<String> =
            others("notes", None, &Default::default(), &plan).into_iter().map(|(f, _)| f).collect();
        assert!(found.contains(&"--profile dev".to_string()), "{found:?}");
        assert!(!found.iter().any(|f| f.contains("other")), "{found:?}");
    }

    #[test]
    fn at_profile_is_the_profile_flag() {
        let ask = |profile: Option<&str>, instance: Option<&str>| Ask {
            profile: profile.map(String::from),
            instance: instance.map(String::from),
            ..Ask::default()
        };
        let a = of("no-such-app-0a1b@dev", &Ask::default()).unwrap();
        let b = of("no-such-app-0a1b", &ask(Some("dev"), None)).unwrap();
        assert_eq!(a.plan.data, b.plan.data);
        // A profile counts from the home: the app's own, or a caller's AIORT_DATA.
        assert!(a.plan.data.parent().unwrap().to_string_lossy().ends_with("-dev"), "{:?}", a.plan.data);
        assert_eq!(of("no-such-app-0a1b@dev", &ask(Some("dev"), None)).unwrap().plan.data, a.plan.data);
        let e = of("no-such-app-0a1b@dev", &ask(Some("qa"), None)).unwrap_err();
        assert!(e.contains("both name a profile"), "{e}");
        let i = of("no-such-app-0a1b", &ask(None, Some("agent1"))).unwrap();
        assert!(i.plan.data.to_string_lossy().contains("agent1"), "{:?}", i.plan.data);
    }
}
