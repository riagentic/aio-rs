//! The open dialog when there is no window to own it (a CLI, a server, a
//! window not up yet): the desktop's own tool, spawned. Linux `zenity`, else
//! `kdialog`; macOS `osascript`; Windows PowerShell's common dialog.
//!
//! The one thing to get right (aio's field reports): a tool that is not
//! installed, a dead display and a pressed Cancel can all exit 1 with no
//! output. Here a cancel is `None` and the others are errors that say so.
use crate::page::Pick;
use std::io;

/// The paths picked (at least one); `None` if cancelled. `p` is already
/// resolved (`page::resolved`).
pub(crate) async fn spawned(p: &Pick, many: bool) -> io::Result<Option<Vec<String>>> {
    let os = std::env::consts::OS;
    crate::os::desktop(os, &|k| std::env::var(k).ok())?;
    first_of(providers(os), &|tool| spec(tool, p, many)).await
}

/// The path to save to; `None` if cancelled. `p` is resolved, its
/// `start_in` the folder to open in and `name` the file name suggested.
pub(crate) async fn spawned_save(p: &Pick, name: &str) -> io::Result<Option<String>> {
    let os = std::env::consts::OS;
    crate::os::desktop(os, &|k| std::env::var(k).ok())?;
    Ok(first_of(providers(os), &|tool| save_spec(tool, p, name)).await?.and_then(|v| v.into_iter().next()))
}

/// The answer of the first of `tools` that is installed, each asked with
/// `args(tool)`.
async fn first_of(tools: &[&str], args: &dyn Fn(&str) -> Vec<String>) -> io::Result<Option<Vec<String>>> {
    let mut missing = Vec::new();
    for &tool in tools {
        let mut cmd = tokio::process::Command::new(tool);
        cmd.args(args(tool)).stdin(std::process::Stdio::null());
        aio::lifecycle::scrub(cmd.as_std_mut(), false);
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: no console flashes up
        match cmd.output().await {
            Ok(out) => {
                let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
                return interpret(tool, out.status.code(), &stdout, &stderr);
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => missing.push(tool),
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other(format!(
        "no file dialog: tried {}; install zenity (GNOME, most desktops) or kdialog (KDE). \
         This is not a cancel (that is no path).",
        missing.join(", ")
    )))
}

fn providers(os: &str) -> &'static [&'static str] {
    match os {
        "macos" => &["osascript"],
        "windows" => &["powershell"],
        _ => &["zenity", "kdialog"],
    }
}

/// `tool`'s arguments for `p`: one file, several (`many`) or a folder. Each
/// prints one path per line (a `|` is a legal file name character, so never
/// a separator).
fn spec(tool: &str, p: &Pick, many: bool) -> Vec<String> {
    let dir = p.start_in.as_deref();
    let globs = |ext: &[String], by: &str| ext.iter().map(|e| format!("*.{e}")).collect::<Vec<_>>().join(by);
    let mut a: Vec<String> = Vec::new();
    match tool {
        "zenity" => {
            a.extend(["--file-selection".into(), format!("--title={}", p.title)]);
            if p.folder {
                a.push("--directory".into());
            }
            if many {
                a.extend(["--multiple".into(), "--separator=\n".into()]);
            }
            // Only a trailing separator opens IN the folder (else it is a name).
            if let Some(d) = dir {
                a.push(format!("--filename={}/", d.trim_end_matches('/')));
            }
            a.extend(p.filters.iter().map(|(name, ext)| format!("--file-filter={name} | {}", globs(ext, " "))));
        }
        "kdialog" => {
            let verb = if p.folder { "--getexistingdirectory" } else { "--getopenfilename" };
            a.extend(["--title".into(), p.title.clone(), verb.into(), dir.unwrap_or(".").into()]);
            // Patterns first, the name after a pipe; every group in one argument.
            let filter: Vec<_> = p.filters.iter().map(|(name, ext)| format!("{}|{name}", globs(ext, " "))).collect();
            if !filter.is_empty() {
                a.push(filter.join("\n"));
            }
            if many && !p.folder {
                a.extend(["--multiple".into(), "--separate-output".into()]);
            }
        }
        "osascript" => {
            let chooser = if p.folder { "choose folder" } else { "choose file" };
            let types: Vec<_> = p.filters.iter().flat_map(|(_, ext)| ext).map(|e| applescript(e)).collect();
            let of_type =
                if p.folder || types.is_empty() { String::new() } else { format!(" of type {{{}}}", types.join(", ")) };
            let at = dir.map(|d| format!(" default location POSIX file {}", applescript(d))).unwrap_or_default();
            let ask = format!("{chooser} with prompt {}{of_type}{at}", applescript(&p.title));
            // `POSIX path of` turns AppleScript's alias into a path, one by one.
            let script = if many {
                format!(
                    "set fs to ({ask} with multiple selections allowed)\nset out to \"\"\n\
                     repeat with f in fs\n  set out to out & POSIX path of f & linefeed\nend repeat\nreturn out"
                )
            } else {
                format!("return POSIX path of ({ask})")
            };
            a.extend(["-e".into(), script]);
        }
        _ => {
            // -STA: the common dialogs are STA COM objects (else they never
            // open). The TopMost owner keeps the dialog in front of the app
            // (unowned, it opened behind it: aio field report #10). UTF-8 out,
            // or `Résumé.txt` comes back in the OEM code page.
            let mut ps = String::from(
                "[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding $false;\
                 Add-Type -AssemblyName System.Windows.Forms;\
                 $o = New-Object System.Windows.Forms.Form -Property @{TopMost=$true;ShowInTaskbar=$false};",
            );
            if p.folder {
                ps += &format!(
                    "$d = New-Object System.Windows.Forms.FolderBrowserDialog;$d.Description = {};",
                    ps_quote(&p.title)
                );
                if let Some(d) = dir {
                    ps += &format!("$d.SelectedPath = {};", ps_quote(d));
                }
                ps += "if ($d.ShowDialog($o) -eq 'OK') { Write-Output $d.SelectedPath }";
            } else {
                ps += &format!(
                    "$d = New-Object System.Windows.Forms.OpenFileDialog;$d.Title = {};$d.Multiselect = ${many};",
                    ps_quote(&p.title)
                );
                if let Some(d) = dir {
                    ps += &format!("$d.InitialDirectory = {};", ps_quote(d));
                }
                if !p.filters.is_empty() {
                    let f: Vec<_> = p.filters.iter().map(|(name, ext)| format!("{name}|{}", globs(ext, ";"))).collect();
                    ps += &format!("$d.Filter = {};", ps_quote(&f.join("|")));
                }
                ps += "if ($d.ShowDialog($o) -eq 'OK') { $d.FileNames | ForEach-Object { Write-Output $_ } }";
            }
            a.extend(["-NoProfile", "-NonInteractive", "-STA", "-Command"].map(String::from));
            a.push(ps);
        }
    }
    a
}

/// `tool`'s arguments for a save dialog in `p.start_in`, suggesting
/// `name`. Each asks before replacing a file (zenity only when told), so a
/// path that comes back is one the user agreed to write.
fn save_spec(tool: &str, p: &Pick, name: &str) -> Vec<String> {
    let dir = p.start_in.as_deref();
    let globs = |ext: &[String], by: &str| ext.iter().map(|e| format!("*.{e}")).collect::<Vec<_>>().join(by);
    let full = match dir {
        Some(d) => format!("{}/{name}", d.trim_end_matches(['/', '\\'])),
        None => name.to_string(),
    };
    let mut a: Vec<String> = Vec::new();
    match tool {
        "zenity" => {
            a.extend(["--file-selection", "--save", "--confirm-overwrite"].map(String::from));
            a.push(format!("--title={}", p.title));
            if !full.is_empty() {
                a.push(format!("--filename={full}"));
            }
            a.extend(p.filters.iter().map(|(n, ext)| format!("--file-filter={n} | {}", globs(ext, " "))));
        }
        "kdialog" => {
            let at = if full.is_empty() { ".".to_string() } else { full };
            a.extend(["--title".into(), p.title.clone(), "--getsavefilename".into(), at]);
            let filter: Vec<_> = p.filters.iter().map(|(n, ext)| format!("{}|{n}", globs(ext, " "))).collect();
            if !filter.is_empty() {
                a.push(filter.join("\n"));
            }
        }
        "osascript" => {
            // A save panel has no type filter here.
            let named = if name.is_empty() { String::new() } else { format!(" default name {}", applescript(name)) };
            let at = dir.map(|d| format!(" default location POSIX file {}", applescript(d))).unwrap_or_default();
            let ask = format!("choose file name with prompt {}{named}{at}", applescript(&p.title));
            a.extend(["-e".into(), format!("return POSIX path of ({ask})")]);
        }
        _ => {
            let mut ps = String::from(
                "[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding $false;\
                 Add-Type -AssemblyName System.Windows.Forms;\
                 $o = New-Object System.Windows.Forms.Form -Property @{TopMost=$true;ShowInTaskbar=$false};",
            );
            ps += &format!(
                "$d = New-Object System.Windows.Forms.SaveFileDialog;$d.Title = {};$d.OverwritePrompt = $true;",
                ps_quote(&p.title)
            );
            if let Some(d) = dir {
                ps += &format!("$d.InitialDirectory = {};", ps_quote(d));
            }
            if !name.is_empty() {
                ps += &format!("$d.FileName = {};", ps_quote(name));
            }
            if !p.filters.is_empty() {
                let f: Vec<_> = p.filters.iter().map(|(n, ext)| format!("{n}|{}", globs(ext, ";"))).collect();
                ps += &format!("$d.Filter = {};", ps_quote(&f.join("|")));
            }
            ps += "if ($d.ShowDialog($o) -eq 'OK') { Write-Output $d.FileName }";
            a.extend(["-NoProfile", "-NonInteractive", "-STA", "-Command"].map(String::from));
            a.push(ps);
        }
    }
    a
}

/// An AppleScript string.
fn applescript(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// A PowerShell single-quoted string. PowerShell ends one at `'` and at each
/// typographic single quote too (`Dan’s PC` ran the rest as code): all doubled.
fn ps_quote(s: &str) -> String {
    let mut q = String::from("'");
    for c in s.chars() {
        if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
            q.push(c);
        }
        q.push(c);
    }
    q + "'"
}

/// `tool`'s exit as the answer. Nothing on stdout is a cancel only when the
/// tool says so its way; stderr alone never fails a pick (GTK warns on
/// healthy desktops). Only line ends are cut: ` report ` is a real name.
fn interpret(tool: &str, code: Option<i32>, stdout: &str, stderr: &str) -> io::Result<Option<Vec<String>>> {
    let paths: Vec<String> = stdout
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .filter(|l| !l.trim().is_empty())
        .map(String::from)
        .collect();
    if !paths.is_empty() {
        return Ok(Some(paths));
    }
    if cancelled(tool, code, stderr) {
        return Ok(None);
    }
    let tail: Vec<_> = stderr.trim().lines().rev().take(3).collect::<Vec<_>>().into_iter().rev().collect();
    let code = code.map_or("a signal".into(), |c| format!("exit {c}"));
    Err(io::Error::other(format!("the {tool} dialog failed ({code}): {}", tail.join(" "))))
}

/// The tools disagree on a cancel: zenity and kdialog exit 1, osascript exits
/// 1 and says "User canceled", PowerShell exits 0. A display that can't be
/// opened looks like zenity's cancel and is not one.
fn cancelled(tool: &str, code: Option<i32>, stderr: &str) -> bool {
    let e = stderr.to_lowercase();
    if ["cannot open display", "unable to init server", "no protocol specified"].iter().any(|s| e.contains(s)) {
        return false;
    }
    match tool {
        "zenity" | "kdialog" => code == Some(1),
        "osascript" => code == Some(1) && (e.contains("user canceled") || e.contains("user cancelled")),
        _ => code == Some(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pick(folder: bool) -> Pick {
        Pick {
            title: "Open".into(),
            folder,
            start_in: Some("/home/u/docs".into()),
            filters: if folder { vec![] } else { vec![("Video".into(), vec!["mp4".into(), "mkv".into()])] },
        }
    }

    #[test]
    fn the_four_endings_are_told_apart() {
        // Picked, with GTK noise on stderr.
        let one = interpret("zenity", Some(0), "/home/u/clip.mp4\n", "Gtk-Message: Failed to load module");
        assert_eq!(one.unwrap(), Some(vec!["/home/u/clip.mp4".into()]));
        // Cancelled, each tool its way.
        assert_eq!(interpret("zenity", Some(1), "", "").unwrap(), None);
        assert_eq!(interpret("kdialog", Some(1), "", "").unwrap(), None);
        assert_eq!(interpret("osascript", Some(1), "", "execution error: User canceled. (-128)").unwrap(), None);
        assert_eq!(interpret("powershell", Some(0), "", "").unwrap(), None);
        // Broken: the tool's own words, never a cancel.
        let e = interpret("zenity", Some(2), "", "GLib: fatal boom\n").unwrap_err().to_string();
        assert!(e.contains("exit 2") && e.contains("fatal boom"), "{e}");
        assert!(interpret("osascript", Some(1), "", "syntax error").is_err());
        assert!(interpret("powershell", Some(1), "", "").is_err());
        // A dead display looks like zenity's cancel: it is a failure.
        let dead = interpret("zenity", Some(1), "", "Unable to init server: Could not connect: Connection refused");
        assert!(dead.unwrap_err().to_string().contains("Unable to init server"));
    }

    #[test]
    fn several_paths_come_one_a_line_and_keep_their_spaces_and_pipes() {
        let many = interpret("powershell", Some(0), "C:\\a.mp4\r\n C:\\b|c.mp4 \r\n", "").unwrap();
        assert_eq!(many, Some(vec!["C:\\a.mp4".into(), " C:\\b|c.mp4 ".into()]));
    }

    #[test]
    fn zenity_opens_in_the_folder_with_its_filters() {
        let a = spec("zenity", &pick(false), true);
        assert_eq!(
            a,
            [
                "--file-selection",
                "--title=Open",
                "--multiple",
                "--separator=\n",
                "--filename=/home/u/docs/",
                "--file-filter=Video | *.mp4 *.mkv"
            ]
        );
        assert!(spec("zenity", &pick(true), false).contains(&"--directory".into()));
    }

    #[test]
    fn kdialogs_filter_is_zenitys_mirror() {
        let a = spec("kdialog", &pick(false), true);
        assert_eq!(
            a,
            [
                "--title",
                "Open",
                "--getopenfilename",
                "/home/u/docs",
                "*.mp4 *.mkv|Video",
                "--multiple",
                "--separate-output"
            ]
        );
        assert_eq!(spec("kdialog", &pick(true), false), ["--title", "Open", "--getexistingdirectory", "/home/u/docs"]);
    }

    #[test]
    fn osascript_returns_posix_paths_and_escapes_the_title() {
        let mut p = pick(false);
        p.title = r#"a "b""#.into();
        let s = &spec("osascript", &p, false)[1];
        assert_eq!(
            s,
            r#"return POSIX path of (choose file with prompt "a \"b\"" of type {"mp4", "mkv"} default location POSIX file "/home/u/docs")"#
        );
        let many = &spec("osascript", &p, true)[1];
        assert!(many.contains("with multiple selections allowed") && many.contains("repeat with f in fs"), "{many}");
        assert!(spec("osascript", &pick(true), false)[1].starts_with("return POSIX path of (choose folder"));
    }

    #[test]
    fn the_windows_dialog_is_sta_owned_utf8_and_quote_safe() {
        let mut p = pick(false);
        p.title = "Dan’s it's".into();
        let a = spec("powershell", &p, true);
        assert_eq!(a[..4], ["-NoProfile", "-NonInteractive", "-STA", "-Command"]);
        let ps = &a[4];
        assert!(ps.starts_with("[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding $false;"));
        assert!(ps.contains("TopMost=$true") && ps.contains("ShowDialog($o)") && !ps.contains("ShowDialog()"));
        assert!(ps.contains("$d.Title = 'Dan’’s it''s';"), "{ps}");
        assert!(ps.contains("$d.Multiselect = $true;") && ps.contains("$d.Filter = 'Video|*.mp4;*.mkv';"));
        assert!(spec("powershell", &pick(true), false)[4].contains("FolderBrowserDialog"));
    }

    #[test]
    fn every_save_dialog_asks_before_replacing() {
        // aio tests/pick-save.test.ts.
        let p = Pick {
            title: "Save it".into(),
            start_in: Some("/tmp/no-such-dir-aio".into()),
            filters: vec![("Text".into(), vec!["txt".into()])],
            ..Default::default()
        };
        assert_eq!(
            save_spec("zenity", &p, "out.txt"),
            [
                "--file-selection",
                "--save",
                "--confirm-overwrite",
                "--title=Save it",
                "--filename=/tmp/no-such-dir-aio/out.txt",
                "--file-filter=Text | *.txt"
            ]
        );
        assert_eq!(
            save_spec("kdialog", &p, "out.txt"),
            ["--title", "Save it", "--getsavefilename", "/tmp/no-such-dir-aio/out.txt", "*.txt|Text"]
        );
        assert_eq!(
            save_spec("osascript", &p, "out.txt")[1],
            r#"return POSIX path of (choose file name with prompt "Save it" default name "out.txt" default location POSIX file "/tmp/no-such-dir-aio")"#
        );
        let win = Pick { start_in: Some("C:\\Users\\Dan’s".into()), ..p.clone() };
        let ps = &save_spec("powershell", &win, "out.txt")[4];
        for want in [
            "SaveFileDialog",
            "$d.OverwritePrompt = $true;",
            "$d.InitialDirectory = 'C:\\Users\\Dan’’s';",
            "$d.FileName = 'out.txt';",
            "$d.Filter = 'Text|*.txt';",
            "ShowDialog($o)",
        ] {
            assert!(ps.contains(want), "{want}: {ps}");
        }
        // Nothing suggested: no name, kdialog starts in "."; a folder opens in it.
        let bare = Pick { title: "Save as".into(), ..Default::default() };
        assert_eq!(
            save_spec("zenity", &bare, ""),
            ["--file-selection", "--save", "--confirm-overwrite", "--title=Save as"]
        );
        assert_eq!(save_spec("kdialog", &bare, ""), ["--title", "Save as", "--getsavefilename", "."]);
        let dir = Pick { start_in: Some("/docs".into()), ..bare };
        assert_eq!(save_spec("zenity", &dir, "").last().unwrap(), "--filename=/docs/");
    }

    #[tokio::test]
    async fn no_tool_installed_is_an_error_never_a_cancel() {
        let e = first_of(&["aio-no-such-dialog", "aio-nor-this"], &|t| spec(t, &pick(false), false))
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("tried aio-no-such-dialog, aio-nor-this") && e.contains("not a cancel"), "{e}");
        assert_eq!(providers("linux"), ["zenity", "kdialog"]);
    }
}
