//! `ax lab <app> <windows|macos|linux> [--stop]` (aio's `am lab`): the app
//! on a real machine of that OS, on its desktop, for a person to click
//! around in. aio booted containers (dockurr, webtop); aio-rs uses machines that
//! are there already: `AIO_LAB_WINDOWS`, `AIO_LAB_MACOS`, `AIO_LAB_LINUX`,
//! each `[ssh options] [user@]host` (as `AIO_MACOS_SSH`). The app is built
//! for it (`ax build --platforms`), its bundle copied over ssh into
//! `aio-rs-test/lab/<id>` under that machine's home, unpacked and started
//! on the desktop of the person signed in there (Windows: a one-shot
//! scheduled task, the way into the interactive session; macOS: `open`;
//! Linux: its `DISPLAY`). Its data, runtime and caches stay inside that
//! folder (its homes point there), so `--stop` ends what runs from it and
//! removes it whole: nothing else on the machine is touched.
use crate::platform::Platform;
use crate::remote::Host;
use std::process::{Command, Stdio};

/// The folder under the lab machine's home that everything goes in.
pub const DIR: &str = "aio-rs-test/lab";

/// The OSes a lab runs, with the variable naming each one's machine.
pub const LABS: [(&str, &str); 3] =
    [("windows", "AIO_LAB_WINDOWS"), ("macos", "AIO_LAB_MACOS"), ("linux", "AIO_LAB_LINUX")];

/// The machine `os`'s lab runs on (`env`: a variable's value), or why none.
pub fn machine(os: &str, env: &dyn Fn(&str) -> Option<String>) -> Result<Host, String> {
    let names: Vec<&str> = LABS.iter().map(|(o, _)| *o).collect();
    let (_, var) = LABS.iter().find(|(o, _)| *o == os).ok_or_else(|| format!("no lab {os}: {}", names.join(", ")))?;
    let v = env(var).ok_or_else(|| format!("no {os} lab machine: set {var} to its `[ssh options] [user@]host`"))?;
    crate::remote::host_of(&v).ok_or_else(|| format!("{var} names no machine"))
}

/// The platform a lab of `os` runs: Windows x64, an Apple-silicon Mac, a
/// Linux of this machine's CPU.
pub fn platform(os: &str) -> Result<&'static Platform, String> {
    let arch = match os {
        "windows" => "x86_64",
        "macos" => "aarch64",
        _ => std::env::consts::ARCH,
    };
    crate::platform::of(os, arch).ok_or_else(|| format!("ax builds no {os} {arch} app"))
}

/// `s` single-quoted for PowerShell.
fn ps(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// `s` single-quoted for a POSIX shell.
fn sh(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// What the machine runs to make app `id`'s lab folder, never over an
/// earlier one (pure).
pub fn prepare_script(os: &str, id: &str) -> String {
    match os {
        "windows" => format!(
            "$ErrorActionPreference = 'Stop'\n\
             $d = Join-Path $HOME {dir}\n\
             if (Test-Path $d) {{ throw \"$d is there already: ax lab --stop first\" }}\n\
             New-Item -ItemType Directory -Force (Join-Path $d 'app'), (Join-Path $d 'local'), (Join-Path $d 'roaming') | Out-Null\n",
            dir = ps(&format!("{DIR}/{id}").replace('/', "\\")),
        ),
        _ => format!(
            "d=\"$HOME\"/{dir}\n\
             [ -e \"$d\" ] && {{ echo \"$d is there already: ax lab --stop first\" >&2; exit 1; }}\n\
             mkdir -p \"$d/app\" \"$d/home\"\n",
            dir = sh(&format!("{DIR}/{id}")),
        ),
    }
}

/// What the machine runs to start app `id` from the bundle `file` (a
/// `.zip` or `.tar.gz`, by its name) copied into its lab folder (pure).
pub fn start_script(os: &str, id: &str, file: &str) -> String {
    match os {
        "windows" => format!(
            "$ErrorActionPreference = 'Stop'\n\
             $d = Join-Path $HOME {dir}\n\
             $zip = Join-Path $d {file}\n\
             Expand-Archive -LiteralPath $zip -DestinationPath (Join-Path $d 'app')\n\
             Remove-Item $zip\n\
             # Its homes inside the lab folder: data, runtime and caches go with it.\n\
             $cmd = Join-Path $d 'start.cmd'\n\
             Set-Content -Encoding ASCII $cmd \"@set `\"LOCALAPPDATA=$d\\local`\"`r`n@set `\"APPDATA=$d\\roaming`\"`r`n@start `\"`\" `\"$d\\app\\{id}.exe`\"\"\n\
             # A scheduled task is the way into the desktop of the person signed in.\n\
             $t = {task}\n\
             schtasks /Create /F /TN $t /SC ONCE /ST 00:00 /IT /TR \"`\"$cmd`\"\" | Out-Null\n\
             schtasks /Run /TN $t | Out-Null\n\
             $ran = $LASTEXITCODE\n\
             schtasks /Delete /F /TN $t | Out-Null\n\
             if ($ran -ne 0) {{ throw 'the task did not start: is someone signed in on the desktop?' }}\n\
             Start-Sleep 3\n\
             $p = Get-Process | Where-Object {{ $_.Path -like \"$d\\*\" }}\n\
             if (-not $p) {{ throw \"{id} is not running: see $d\" }}\n\
             \"started {id} in $d (desktop session $(@($p)[0].SessionId))\"\n",
            dir = ps(&format!("{DIR}/{id}").replace('/', "\\")),
            file = ps(file),
            task = ps(&format!("aio-lab-{id}")),
        ),
        _ => {
            let unpack = if file.ends_with(".zip") {
                "ditto -x -k \"$f\" \"$d/app\""
            } else {
                "tar -xzf \"$f\" -C \"$d/app\" --strip-components=1"
            };
            let run = match os {
                "macos" => format!(
                    "a=$(ls -d \"$d\"/app/*.app 2>/dev/null | head -n 1)\n\
                     if [ -n \"$a\" ]; then open -n --env HOME=\"$d/home\" \"$a\"; else\n  \
                     HOME=\"$d/home\" nohup \"$d/app/\"{id} > \"$d/out.log\" 2>&1 &\nfi\n",
                    id = sh(id)
                ),
                _ => format!(
                    "HOME=\"$d/home\" XDG_DATA_HOME= XDG_CACHE_HOME= XDG_CONFIG_HOME= DISPLAY=\"${{DISPLAY:-:0}}\" \
                     nohup \"$d/app/\"{id} > \"$d/out.log\" 2>&1 &\n",
                    id = sh(id)
                ),
            };
            format!(
                "set -e\nd=\"$HOME\"/{dir}\nf=\"$d\"/{file}\n{unpack}\nrm -f \"$f\"\n\
                 # Its home inside the lab folder: data, runtime and caches go with it.\n{run}\
                 echo \"started {id} in $d\"\n",
                dir = sh(&format!("{DIR}/{id}")),
                file = sh(file),
            )
        }
    }
}

/// What the machine runs to stop what runs from app `id`'s lab folder and
/// remove it, and the lab folders once empty (pure).
pub fn stop_script(os: &str, id: &str) -> String {
    match os {
        "windows" => format!(
            "$d = Join-Path $HOME {dir}\n\
             schtasks /Delete /F /TN {task} 2>$null | Out-Null\n\
             Get-Process | Where-Object {{ $_.Path -like \"$d\\*\" }} | Stop-Process -Force\n\
             Start-Sleep 2\n\
             if (Test-Path $d) {{ Remove-Item -Recurse -Force $d }}\n\
             foreach ($up in @((Join-Path $HOME {lab}), (Join-Path $HOME 'aio-rs-test'))) {{\n  \
             if ((Test-Path $up) -and -not (Get-ChildItem -Force $up)) {{ Remove-Item $up }}\n\
             }}\n\
             \"stopped {id}; $d removed\"\n",
            dir = ps(&format!("{DIR}/{id}").replace('/', "\\")),
            lab = ps(&DIR.replace('/', "\\")),
            task = ps(&format!("aio-lab-{id}")),
        ),
        _ => format!(
            "d=\"$HOME\"/{dir}\n\
             # What runs from the lab folder, by its executable's path.\n\
             ps -axo pid=,command= | awk -v d=\"$d/\" 'index($2, d) == 1 {{ print $1 }}' | xargs kill 2>/dev/null\n\
             sleep 2\nrm -rf \"$d\"\nrmdir \"$HOME\"/{lab} \"$HOME/aio-rs-test\" 2>/dev/null\n\
             echo \"stopped {id}; $d removed\"\n",
            dir = sh(&format!("{DIR}/{id}")),
            lab = sh(DIR),
        ),
    }
}

/// The command the machine runs `script` with, on its own shell: PowerShell
/// on Windows (the script encoded, never spliced into a command line).
fn remote(h: &Host, os: &str, script: &str) -> Command {
    let mut c = crate::remote::ssh(h);
    match os {
        "windows" => c.args(["powershell", "-NoProfile", "-NonInteractive", "-EncodedCommand"]).arg(encoded(script)),
        _ => c.args(["sh", "-c"]).arg(sh(script)),
    };
    c
}

/// PowerShell's `-EncodedCommand` form: UTF-16LE, base64.
fn encoded(script: &str) -> String {
    use base64::Engine;
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// scp's options for `h`'s ssh ones (`-p` is `-P` there) (pure).
pub fn scp_opts(h: &Host) -> Vec<String> {
    h.opts.iter().map(|o| if o == "-p" { "-P".into() } else { o.clone() }).collect()
}

/// Run `cmd`: what it said, or why it failed.
fn ran(mut cmd: Command) -> Result<String, String> {
    let out = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("{:?}: {e}", cmd.get_program()))?;
    let said = String::from_utf8_lossy(&out.stdout).trim().to_string();
    match out.status.success() {
        true => Ok(said),
        false => Err(format!("{said}\n{}", String::from_utf8_lossy(&out.stderr).trim()).trim().to_string()),
    }
}

/// `ax lab <app> <os>`: built, copied, started there; `stop`: what runs
/// from its lab folder there ended and the folder removed.
pub fn run(app: &str, os: &str, stop: bool) -> Result<String, String> {
    let h = machine(os, &|k| std::env::var(k).ok())?;
    let p = platform(os)?;
    let meta = crate::dev::metadata()?;
    let id = crate::build::app_of(&crate::dev::locate(&meta, app)?)?.0.id;
    if stop {
        return ran(remote(&h, os, &stop_script(os, &id)));
    }
    crate::build::run(app, None, Some(p.name), crate::smoke::Smoke::Off)?;
    let (_, bundle) = crate::build::built(app, Some(p))?;
    let file = bundle.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
    ran(remote(&h, os, &prepare_script(os, &id)))?;
    eprintln!("ax lab: {file} to {}…", h.target);
    let mut scp = Command::new("scp");
    scp.args(["-q", "-o", "BatchMode=yes"]).args(scp_opts(&h)).arg(&bundle);
    scp.arg(format!("{}:{DIR}/{id}/{file}", h.target));
    ran(scp)?;
    ran(remote(&h, os, &start_script(os, &id, &file)))
        .map(|said| format!("{said}\nstop it, and remove all of it there: ax lab {app} {os} --stop"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_lab_is_a_machine_named_in_the_environment() {
        let env = |k: &str| (k == "AIO_LAB_WINDOWS").then(|| "-i ~/.ssh/vm -o IdentitiesOnly=yes me@vm".to_string());
        let h = machine("windows", &env).unwrap();
        assert_eq!((h.opts.len(), h.target.as_str()), (4, "me@vm"));
        assert!(machine("macos", &env).unwrap_err().contains("set AIO_LAB_MACOS"));
        assert!(machine("android", &env).unwrap_err().contains("windows, macos, linux"));
        assert_eq!(platform("windows").unwrap().name, "windows");
        assert_eq!(platform("macos").unwrap().name, "macos-arm64");
    }

    #[test]
    fn the_app_starts_from_its_lab_folder_with_its_homes_inside_it() {
        let w = start_script("windows", "notes", "notes-1.0.0-windows-x64.zip");
        assert!(w.contains("Join-Path $HOME 'aio-rs-test\\lab\\notes'") && w.contains("'notes-1.0.0-windows-x64.zip'"));
        assert!(w.contains("LOCALAPPDATA=$d\\local") && w.contains("APPDATA=$d\\roaming"), "{w}");
        assert!(
            w.contains("/IT") && w.contains("schtasks /Delete /F /TN $t"),
            "into the desktop session, task gone after"
        );
        assert!(prepare_script("windows", "notes").contains("is there already"), "never over an earlier lab");
        assert!(prepare_script("linux", "notes").contains("mkdir -p \"$d/app\" \"$d/home\""));
        let h = crate::remote::host_of("-p 2222 -i k u@h").unwrap();
        assert_eq!(scp_opts(&h), ["-P", "2222", "-i", "k"]);
        let m = start_script("macos", "notes", "notes-1.0.0-macos-arm64.zip");
        assert!(m.contains("ditto -x -k") && m.contains("open -n --env HOME=\"$d/home\""), "{m}");
        let l = start_script("linux", "it's", "x.tar.gz");
        assert!(l.contains("--strip-components=1") && l.contains("\"$d/app/\"'it'\\''s'"), "{l}");
    }

    #[test]
    fn a_stop_ends_only_what_runs_from_the_lab_folder_and_removes_it() {
        let w = stop_script("windows", "notes");
        assert!(w.contains("$_.Path -like \"$d\\*\"") && w.contains("Remove-Item -Recurse -Force $d"), "{w}");
        let u = stop_script("macos", "notes");
        assert!(u.contains("index($2, d) == 1") && u.contains("rm -rf \"$d\""), "{u}");
        assert!(!u.contains("pkill") && !w.contains("taskkill"), "never by name");
    }
}
