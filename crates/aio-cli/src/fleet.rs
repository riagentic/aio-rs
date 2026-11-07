//! `[build] targets`: what one `ax build <app>` makes (aio's `build.targets`).
//! Each target is a kind, or a labelled `{ name, kind, bin }`: two of one
//! kind, or another binary of the workspace (its own `aio.toml`, its own
//! id: one repo, several apps). `--targets a,b` picks some of them and
//! keeps their settings; nothing declared builds `desktop`. With
//! `[build] platforms` each desktop target is built for each of them
//! (`across`).
use crate::platform::Platform;
use aio::config::Target;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Desktop,
    Windows,
    Deb,
    AppImage,
    Dmg,
    Cli,
    Android,
    Web,
}

impl Kind {
    pub const ALL: [Kind; 8] =
        [Kind::Desktop, Kind::Windows, Kind::Deb, Kind::AppImage, Kind::Dmg, Kind::Cli, Kind::Android, Kind::Web];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Desktop => "desktop",
            Kind::Windows => "windows",
            Kind::Deb => "deb",
            Kind::AppImage => "appimage",
            Kind::Dmg => "dmg",
            Kind::Cli => "cli",
            Kind::Android => "android",
            Kind::Web => "web",
        }
    }

    fn about(self) -> &'static str {
        match self {
            Kind::Desktop => "this OS's bundle: Linux .tar.gz, macOS zipped .app, Windows one-click .exe",
            Kind::Windows => "the one-click Windows .exe (and its zip, for updates), cross-built from Linux",
            Kind::Deb => "a Debian package (needs [app] maintainer; on Linux)",
            Kind::AppImage => "one portable Linux file",
            Kind::Dmg => "a macOS disk image to drag into Applications (on a Mac)",
            Kind::Cli => "a headless binary on PATH, no menu entry (a cli or server app, or a remote client's bin)",
            Kind::Android => "signed APKs, one per CPU type (the app's lib calls android_app!)",
            Kind::Web => "the app as a static site, an installable PWA (the app's lib runs in the page, aio::web)",
        }
    }

    fn parse(s: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.name() == s)
    }

    /// Installed under the app's name: two apps showing one name look alike.
    fn shows_name(self) -> bool {
        !matches!(self, Kind::Android | Kind::Cli)
    }
}

/// One target to build.
#[derive(Debug, PartialEq)]
pub struct Planned {
    pub label: String,
    /// Its own `platforms` (`[build] targets`), over `[build] platforms`.
    pub platforms: Vec<String>,
    /// Its own title and ui, stamped into its build.
    pub look: Look,
    pub kind: Kind,
    /// The workspace binary it builds.
    pub bin: String,
    /// The OS and CPU it is for; None: its kind's (this machine's, the
    /// Windows kind's x64).
    pub platform: Option<&'static Platform>,
}

/// A target's own title and ui (`[build] targets`), over `[app]`'s.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Look {
    pub title: Option<String>,
    pub ui: Option<aio::config::Shown>,
}

impl Planned {
    /// Its label, with the platform when it names one.
    pub fn shown(&self) -> String {
        self.platform.map_or(self.label.clone(), |p| format!("{} [{}]", self.label, p.name))
    }
}

/// The targets to build, in order: `select` (`a,b`) of `declared`, a
/// selected label that isn't declared being a kind; all of `declared` without
/// `select`; `fallback` when neither names any (`desktop`; a server's or
/// CLI's: `cli`, a binary on PATH and a server's supervisor file). Pure.
pub fn plan(app: &str, declared: &[Target], select: Option<&str>, fallback: Kind) -> Result<Vec<Planned>, String> {
    let labels: Vec<&str> = match select {
        Some(s) => s.split(',').map(str::trim).filter(|s| !s.is_empty()).collect(),
        None => declared.iter().map(|t| t.name.as_str()).collect(),
    };
    if labels.is_empty() {
        let (label, kind, bin) = (fallback.name().into(), fallback, app.into());
        return Ok(vec![Planned { label, kind, bin, platform: None, platforms: vec![], look: Look::default() }]);
    }
    if let Some(dup) = labels.iter().enumerate().find_map(|(i, l)| labels[..i].contains(l).then_some(l)) {
        return Err(format!("target `{dup}` is named twice"));
    }
    let kinds = || Kind::ALL.map(Kind::name).join(", ");
    labels
        .into_iter()
        .map(|label| {
            let t = declared.iter().find(|t| t.name == label);
            let kind = t.and_then(|t| t.kind.as_deref()).unwrap_or(label);
            let kind = Kind::parse(kind).ok_or_else(|| match t {
                Some(Target { kind: Some(k), .. }) => format!("target `{label}`: no kind `{k}` (kinds: {})", kinds()),
                Some(_) => format!("target `{label}` needs a kind: {{ name = \"{label}\", kind = \"desktop\" }}"),
                None => format!("no target `{label}`: not in [build] targets, nor a kind ({})", kinds()),
            })?;
            let bin = t.and_then(|t| t.bin.clone()).unwrap_or_else(|| app.into());
            let platforms = t.map_or(vec![], |t| t.platforms.clone());
            let look = t.map_or_else(Look::default, |t| Look { title: t.title.clone(), ui: t.ui });
            Ok(Planned { label: label.into(), kind, bin, platform: None, platforms, look })
        })
        .collect()
}

/// `plan` across `platforms` (none: as planned): a desktop target once per
/// platform, a Windows one per Windows platform and a Linux package per
/// Linux one (none of theirs listed: as planned); Android and the web are
/// the same files everywhere, built once. Pure.
pub fn across(plan: Vec<Planned>, platforms: &[&'static Platform]) -> Vec<Planned> {
    if platforms.is_empty() {
        return plan;
    }
    plan.into_iter()
        .flat_map(|t| {
            let fits = |p: &&&Platform| match t.kind {
                Kind::Desktop | Kind::Cli => true,
                Kind::Windows => p.os == "windows",
                Kind::Deb | Kind::AppImage => p.os == "linux",
                Kind::Dmg => p.os == "macos",
                Kind::Android | Kind::Web => false,
            };
            let on: Vec<&'static Platform> = platforms.iter().filter(fits).copied().collect();
            if on.is_empty() {
                return vec![t];
            }
            on.into_iter()
                .map(|p| Planned {
                    platform: Some(p),
                    label: t.label.clone(),
                    bin: t.bin.clone(),
                    platforms: t.platforms.clone(),
                    look: t.look.clone(),
                    ..t
                })
                .collect()
        })
        .collect()
}

/// Refused: two targets making the same thing (one overwrites the other's
/// file). Returned: warnings for two apps of a kind that show one name, as
/// in two editions both in the menu as "Notes". `apps`: each target with its
/// app's (id, name). Pure.
pub fn clashes(apps: &[(&Planned, (&str, &str))]) -> Result<Vec<String>, String> {
    let mut warn = vec![];
    for (i, (a, (a_id, a_name))) in apps.iter().enumerate() {
        for (b, (b_id, b_name)) in apps[..i].iter().filter(|(b, _)| b.kind == a.kind && b.platform == a.platform) {
            if a_id == b_id {
                return Err(format!(
                    "targets `{}` and `{}` both make {a_id}'s {}: give one its own bin (with its own aio.toml)",
                    b.label,
                    a.label,
                    a.kind.name()
                ));
            }
            if a_name == b_name && a.kind.shows_name() {
                warn.push(format!(
                    "targets `{}` and `{}` are two apps that both show as \"{a_name}\": give one its own [app] name",
                    b.label, a.label
                ));
            }
        }
    }
    Ok(warn)
}

/// `--list`: the kinds, and the targets `declared`.
pub fn list(app: &str, declared: &[Target]) -> String {
    let mut out = vec!["kinds:".to_string()];
    out.extend(Kind::ALL.map(|k| format!("  {:<10}{}", k.name(), k.about())));
    if declared.is_empty() {
        out.push(format!("{app} declares no targets: `ax build {app}` makes desktop"));
    } else {
        out.push(format!("{app}'s [build] targets:"));
        out.extend(declared.iter().map(|t| {
            let kind = t.kind.as_deref().filter(|k| *k != t.name).map_or(String::new(), |k| format!(" (kind {k})"));
            let bin = t.bin.as_deref().map_or(String::new(), |b| format!(" (bin {b})"));
            format!("  {}{kind}{bin}", t.name)
        }));
    }
    out.push("add one in aio.toml: [build] targets = [\"desktop\", { name = \"pro\", kind = \"desktop\", bin = \"notes-pro\" }]".into());
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A windowed app's plan.
    fn plan(app: &str, declared: &[Target], select: Option<&str>) -> Result<Vec<Planned>, String> {
        super::plan(app, declared, select, Kind::Desktop)
    }

    fn t(name: &str, kind: Option<&str>, bin: Option<&str>) -> Target {
        Target { name: name.into(), kind: kind.map(Into::into), bin: bin.map(Into::into), ..Default::default() }
    }

    fn short(p: &[Planned]) -> Vec<(&str, Kind, &str)> {
        p.iter().map(|p| (p.label.as_str(), p.kind, p.bin.as_str())).collect()
    }

    #[test]
    fn nothing_declared_is_desktop_and_kinds_build_the_app() {
        assert_eq!(short(&plan("notes", &[], None).unwrap()), [("desktop", Kind::Desktop, "notes")]);
        // A server's or CLI's: a binary on PATH (a server's with its supervisor file), no menu entry.
        assert_eq!(short(&super::plan("board", &[], None, Kind::Cli).unwrap()), [("cli", Kind::Cli, "board")]);
        let p = plan("notes", &[t("deb", None, None), t("android", None, None)], None).unwrap();
        assert_eq!(short(&p), [("deb", Kind::Deb, "notes"), ("android", Kind::Android, "notes")]);
    }

    #[test]
    fn labelled_targets_carry_their_kind_and_bin() {
        let d = [t("desktop", None, None), t("pro", Some("desktop"), Some("notes-pro"))];
        let p = plan("notes", &d, None).unwrap();
        assert_eq!(short(&p), [("desktop", Kind::Desktop, "notes"), ("pro", Kind::Desktop, "notes-pro")]);
    }

    #[test]
    fn select_picks_without_dropping_settings() {
        let d = [t("desktop", None, None), t("pro", Some("desktop"), Some("notes-pro"))];
        assert_eq!(short(&plan("notes", &d, Some(" pro ")).unwrap()), [("pro", Kind::Desktop, "notes-pro")]);
        // A kind needs no declaring.
        assert_eq!(short(&plan("notes", &d, Some("deb,pro")).unwrap())[0], ("deb", Kind::Deb, "notes"));
        assert_eq!(plan("notes", &d, Some(",")).unwrap()[0].label, "desktop");
    }

    #[test]
    fn unknown_kinds_unkinded_labels_and_twice_named_are_refused() {
        let e = plan("notes", &[t("agent", None, None)], None).unwrap_err();
        assert!(e.contains("needs a kind") && e.contains("kind = \"desktop\""), "{e}");
        let e = plan("notes", &[t("pro", Some("electron"), None)], None).unwrap_err();
        assert!(
            e.contains("no kind `electron`") && e.contains("desktop, windows, deb, appimage, dmg, cli, android, web"),
            "{e}"
        );
        assert!(plan("notes", &[], Some("rpm")).unwrap_err().contains("no target `rpm`"));
        assert!(plan("notes", &[], Some("deb,deb")).unwrap_err().contains("named twice"));
        // A labelled target's own platforms, title and ui go with it.
        let kiosk = Target {
            platforms: vec!["linux-arm64".into()],
            title: Some("Notes Kiosk".into()),
            ui: Some(aio::config::Shown::Browser),
            ..t("kiosk", Some("desktop"), None)
        };
        let p = plan("notes", &[kiosk], None).unwrap();
        assert_eq!(p[0].platforms, ["linux-arm64"]);
        assert_eq!(p[0].look, Look { title: Some("Notes Kiosk".into()), ui: Some(aio::config::Shown::Browser) });
        let at = crate::platform::resolve(&["linux-arm64"], &crate::platform::ALL[0]).unwrap();
        assert_eq!(across(p, &at)[0].look.title.as_deref(), Some("Notes Kiosk"), "kept per platform");
    }

    #[test]
    fn two_editions_clash_only_where_they_would_collide() {
        let p = |label: &str, kind| Planned {
            label: label.into(),
            kind,
            bin: label.into(),
            platform: None,
            platforms: vec![],
            look: Look::default(),
        };
        let (free, pro, apk) = (p("desktop", Kind::Desktop), p("pro", Kind::Desktop), p("android", Kind::Android));
        // Two apps, one shown name: a warning that names the fix.
        let w = clashes(&[(&free, ("notes", "Notes")), (&pro, ("notes-pro", "Notes"))]).unwrap();
        assert_eq!(w.len(), 1);
        assert!(w[0].contains("`desktop` and `pro`") && w[0].contains("[app] name"), "{}", w[0]);
        // Their own names, or other kinds: nothing to say.
        assert_eq!(clashes(&[(&free, ("notes", "Notes")), (&pro, ("notes-pro", "Notes PRO"))]).unwrap().len(), 0);
        assert_eq!(clashes(&[(&free, ("notes", "Notes")), (&apk, ("notes", "Notes"))]).unwrap().len(), 0);
        // One app built twice as one kind: one file overwrites the other.
        let e = clashes(&[(&free, ("notes", "Notes")), (&pro, ("notes", "Notes"))]).unwrap_err();
        assert!(e.contains("both make notes's desktop"), "{e}");
    }

    #[test]
    fn each_desktop_target_is_built_for_each_platform_it_fits() {
        let at = |names: &[&str]| crate::platform::resolve(names, &crate::platform::ALL[0]).unwrap();
        let d = [
            t("desktop", None, None),
            t("deb", None, None),
            t("windows", None, None),
            t("android", None, None),
            t("web", None, None),
        ];
        let shown = |p: Vec<Planned>| p.iter().map(Planned::shown).collect::<Vec<_>>();
        let p = across(plan("notes", &d, None).unwrap(), &at(&["linux", "linux-arm64", "macos-arm64"]));
        assert_eq!(
            shown(p),
            [
                "desktop [linux]",
                "desktop [linux-arm64]",
                "desktop [macos-arm64]",
                "deb [linux]",
                "deb [linux-arm64]",
                "windows",
                "android",
                "web"
            ]
        );
        let p = across(plan("notes", &d[2..3], None).unwrap(), &at(&["windows-arm64", "windows"]));
        assert_eq!(shown(p), ["windows [windows-arm64]", "windows [windows]"]);
        assert_eq!(
            shown(across(plan("notes", &d, None).unwrap(), &[])),
            ["desktop", "deb", "windows", "android", "web"]
        );
        let p = across(plan("notes", &[t("cli", None, None)], None).unwrap(), &at(&["linux", "windows"]));
        assert_eq!(shown(p), ["cli [linux]", "cli [windows]"], "a CLI goes everywhere");
        // One app on two platforms is two files, not a clash.
        let p = across(plan("notes", &d[..1], None).unwrap(), &at(&["linux", "windows"]));
        assert!(clashes(&[(&p[0], ("notes", "Notes")), (&p[1], ("notes", "Notes"))]).unwrap().is_empty());
    }

    #[test]
    fn the_list_shows_kinds_and_declared_targets() {
        let l = list("notes", &[t("pro", Some("desktop"), Some("notes-pro"))]);
        assert!(l.contains("  appimage  ") && l.contains("  web       the app as a static site"), "{l}");
        assert!(l.contains("  pro (kind desktop) (bin notes-pro)"), "{l}");
        assert!(list("notes", &[]).contains("makes desktop"));
    }
}
