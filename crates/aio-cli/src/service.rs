//! A server's supervisor file, beside its CLI bundle: a systemd user unit
//! (Linux), a launchd agent (macOS) or a Task Scheduler script (Windows).
//! Each starts the installed binary again when it exits, after 5 s, but
//! for a stop: it sets `AIO_SUPERVISED=1` (`aio_shell::update::SUPERVISED`),
//! so an update exits (0) for the supervisor to start the new version,
//! never starting one of its own, and `AIO_STOP_STAYS_DOWN=1`, so a stop
//! (`ax stop`, SIGTERM) exits 143 (`aio::lifecycle::STAYS_DOWN`), on which
//! each ends its restarts (aio's contract). The port is the app's `[server]
//! listen`, so no flag pins it.
//!
//! All three run as the user who installs them, from where `install.sh` (or
//! the Windows script) put the binary: nothing of the build machine (its
//! user, its home) is written in them.
use crate::build::{App, label};

/// The supervisor of `app` on `os` (`linux`, `macos`, `windows`), beside
/// its bundle `bundle`: its file name, its text and how to install it.
pub fn file(app: &App, os: &str, bundle: &str) -> Option<(String, String, String)> {
    let id = &app.id;
    Some(match os {
        "linux" => (
            format!("{bundle}.service"),
            systemd(app),
            format!(
                "service: mkdir -p ~/.config/systemd/user && cp {bundle}.service ~/.config/systemd/user/{id}.service \
                 && systemctl --user daemon-reload && systemctl --user enable --now {id} (loginctl enable-linger: \
                 it runs without a login too)"
            ),
        ),
        "macos" => (
            format!("{bundle}.plist"),
            launchd(app),
            format!(
                "service: cp {bundle}.plist ~/Library/LaunchAgents/app.aio.{id}.plist && launchctl bootstrap \
                 gui/$(id -u) ~/Library/LaunchAgents/app.aio.{id}.plist"
            ),
        ),
        "windows" => (
            format!("{bundle}.ps1"),
            // A BOM: Windows PowerShell 5.1 reads a file without one in the
            // ANSI code page, and the title may be any text.
            format!("\u{feff}{}", windows(app)),
            format!("service: powershell -ExecutionPolicy Bypass -File .\\{bundle}.ps1 -Install (as an administrator)"),
        ),
        _ => return None,
    })
}

/// The title on one line: a newline would start a new directive.
fn one_line(s: &str) -> String {
    s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>().trim().to_string()
}

/// A systemd user unit. Comments stand on their own lines: systemd has no
/// trailing comments (a `#` after a value is part of it).
fn systemd(app: &App) -> String {
    // `%` starts a specifier (`%h`) in Description.
    let title = one_line(label(app)).replace('%', "%%");
    let id = &app.id;
    format!(
        "[Unit]
Description={title} (aio-rs)
After=network.target

[Service]
Type=simple
# Where install.sh puts it (through the installed version's `current`).
ExecStart=%h/.local/bin/{id}
# Any exit starts it again (an update exits 0 for the new version), but
# `ax stop {id}`'s 143; `systemctl --user stop {id}` holds too.
Restart=always
RestartPreventExitStatus=143
SuccessExitStatus=143
RestartSec=5
Environment=AIO_SUPERVISED=1
Environment=AIO_STOP_STAYS_DOWN=1

[Install]
WantedBy=default.target
"
    )
}

/// Text safe inside an XML element.
fn xml(s: &str) -> String {
    one_line(s).replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// The loop a launchd agent runs the app in (`sh -c`, the command its
/// arguments): launchd can't tell exit codes apart, so this starts it again
/// after any exit but 143 and then ends with 0, under `KeepAlive` {
/// SuccessfulExit: false }, which starts the loop again only if it died.
/// `launchctl bootout` sends its SIGTERM to the loop: it passes it on (each
/// second until the app is gone: one sent right after the fork can be lost
/// there) and ends once the app has.
const LAUNCHD_LOOP: &str = r#"term() { while kill -TERM "$1" 2>/dev/null; do sleep 1; done; }
stop= sent= t= p=
trap 'stop=1; if [ -n "$p" ]; then sent=$p; term "$p" & t=$!; fi' TERM INT
while [ -z "$stop" ]; do
  p=; "$@" & p=$!
  if [ -n "$stop" ] && [ "$sent" != "$p" ]; then sent=$p; term "$p" & t=$!; fi
  wait "$p"; c=$?
  while kill -0 "$p" 2>/dev/null; do wait "$p"; c=$?; done
  [ -n "$t" ] && kill "$t" 2>/dev/null; t= p=
  [ -n "$stop" ] && exit 0
  [ "$c" -eq 143 ] && exit 0
  sleep 5
done
exit 0"#;

/// A launchd agent: the app in [`LAUNCHD_LOOP`].
fn launchd(app: &App) -> String {
    let id = &app.id;
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<!-- {title} (aio-rs): keeps the server running.
     exit 0 (an update)             started again
     exit 143 (ax stop, SIGTERM)   stays down until the next login
     anything else (a crash)        started again after 5 s
     Unload it: launchctl bootout gui/$(id -u)/app.aio.{id} -->
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>app.aio.{id}</string>
  <key>ProgramArguments</key>
  <array>
    <string>/bin/sh</string>
    <string>-c</string>
    <string>{script}</string>
    <string>aio-supervise</string>
    <string>/bin/sh</string>
    <string>-c</string>
    <string>exec "$HOME/.local/bin/{id}"</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>AIO_SUPERVISED</key>
    <string>1</string>
    <key>AIO_STOP_STAYS_DOWN</key>
    <string>1</string>
  </dict>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key>
    <false/>
  </dict>
  <key>ThrottleInterval</key>
  <integer>5</integer>
</dict>
</plist>
"#,
        title = xml(label(app)).replace("--", "- -"),
        script = LAUNCHD_LOOP.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;"),
    )
}

/// A PowerShell single-quoted literal.
fn ps(s: &str) -> String {
    format!("'{}'", one_line(s).replace('\'', "''"))
}

/// The Windows supervisor: Task Scheduler can't start a program again when
/// it exits, so this script is the loop, and `-Install` registers it as a
/// logon task.
fn windows(app: &App) -> String {
    let (id, title) = (&app.id, one_line(label(app)));
    let exe = ps(&format!("Programs\\{id}\\{id}.exe"));
    let (task, description, copy) = (ps(id), ps(&format!("{title} (aio-rs)")), ps(&format!("{id}-supervise.ps1")));
    format!(
        r#"# {title} (aio-rs): keeps the server running under Windows Task Scheduler.
#
# Install, once, in an administrator's PowerShell (a logon task needs one):
#   powershell -ExecutionPolicy Bypass -File .\<this file> -Install
# It runs %LOCALAPPDATA%\Programs\{id}\{id}.exe (updates install there), or
# -Exe <path>. A copy of this script goes beside the .exe, so this folder can go.
# exit 0 (an update) starts it again, exit 143 (ax stop) ends the loop until
# the next logon, anything else (a crash) starts it again after 5 s.
# Stop it: ax stop {id} (Stop-ScheduledTask -TaskName {id} ends it the hard way)
# Remove it: powershell -ExecutionPolicy Bypass -File .\<this file> -Uninstall
param(
  [switch]$Install,
  [switch]$Uninstall,
  [string]$Exe = (Join-Path $env:LOCALAPPDATA {exe})
)
$Task = {task}

if ($Uninstall) {{
  Stop-ScheduledTask -TaskName $Task -ErrorAction SilentlyContinue
  Unregister-ScheduledTask -TaskName $Task -Confirm:$false -ErrorAction Stop
  Write-Host "removed the scheduled task '$Task' (the app and its data are untouched)"
  exit 0
}}

if ($Install) {{
  $ErrorActionPreference = 'Stop'
  if (-not (Test-Path -LiteralPath $Exe -PathType Leaf)) {{
    Write-Host "no program at $Exe - copy the .exe there, or pass -Exe <path>"
    exit 1
  }}
  $Exe = (Resolve-Path -LiteralPath $Exe).Path
  $self = Join-Path (Split-Path -Parent $Exe) {copy}
  if ($PSCommandPath -ne $self) {{ Copy-Item -LiteralPath $PSCommandPath -Destination $self -Force }}
  $action = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument ('-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File "{{0}}" -Exe "{{1}}"' -f $self, $Exe)
  $trigger = New-ScheduledTaskTrigger -AtLogOn -User $env:USERNAME
  # Else Task Scheduler ends a task after three days, and not on battery.
  $settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit 0 -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
  Register-ScheduledTask -TaskName $Task -Action $action -Trigger $trigger -Settings $settings -Description {description} -Force | Out-Null
  Start-ScheduledTask -TaskName $Task
  Write-Host "registered and started the scheduled task '$Task': it runs $Exe at every logon"
  exit 0
}}

# An update exits for the new version to start; a stop exits 143 and stays down.
$env:AIO_SUPERVISED = '1'
$env:AIO_STOP_STAYS_DOWN = '1'
do {{
  & $Exe
  $code = $LASTEXITCODE
  if ($code -ne 143) {{ Start-Sleep -Seconds 5 }}
}} while ($code -ne 143)
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(name: &str) -> App {
        App { id: "relay".into(), name: name.into(), version: "1.2.0".into(), icon: None }
    }

    #[test]
    fn the_unit_runs_the_installed_binary_again_after_any_exit_supervised() {
        let (name, unit, how) = file(&app("Relay"), "linux", "relay-1.2.0-linux-x64-cli").unwrap();
        assert_eq!(name, "relay-1.2.0-linux-x64-cli.service");
        for line in [
            "Description=Relay (aio-rs)",
            "ExecStart=%h/.local/bin/relay",
            "Restart=always",
            "RestartPreventExitStatus=143",
            "SuccessExitStatus=143",
            "RestartSec=5",
            "Environment=AIO_SUPERVISED=1",
            "Environment=AIO_STOP_STAYS_DOWN=1",
            "WantedBy=default.target",
        ] {
            assert!(unit.lines().any(|l| l == line), "{line} in\n{unit}");
        }
        assert!(!unit.contains("User=") && !unit.contains("HOME="), "nothing of the build machine");
        assert!(!unit.contains("--port"), "the port is the app's own [server] listen");
        assert!(unit.lines().filter(|l| !l.starts_with('#')).all(|l| !l.contains('#')), "comments on their own lines");
        assert!(how.contains("cp relay-1.2.0-linux-x64-cli.service ~/.config/systemd/user/relay.service"), "{how}");
        assert!(how.contains("systemctl --user enable --now relay"), "{how}");
    }

    #[test]
    fn a_title_cannot_add_a_directive_or_a_specifier_or_break_the_xml() {
        let evil = app("50% <b>&\nExecStartPre=/bin/sh -c 'x'\r\nUser=root");
        let (_, unit, _) = file(&evil, "linux", "b").unwrap();
        assert!(unit.lines().all(|l| !l.starts_with("ExecStartPre") && !l.starts_with("User=")), "{unit}");
        assert!(unit.contains("Description=50%% <b>&"), "{unit}");
        let (name, plist, _) = file(&evil, "macos", "b").unwrap();
        assert_eq!(name, "b.plist");
        assert!(plist.contains("50% &lt;b&gt;&amp; ExecStartPre") && !plist.contains("<b>"), "{plist}");
        let (_, ps1, _) = file(&app("it's\nexit 1"), "windows", "b").unwrap();
        assert!(ps1.contains("-Description 'it''s exit 1 (aio-rs)'") && !ps1.contains("\nexit 1\n"), "{ps1}");
    }

    #[test]
    fn launchd_and_windows_start_it_again_after_any_exit_but_a_stop_supervised() {
        let (_, plist, how) = file(&app(""), "macos", "b").unwrap();
        assert!(how.contains("cp b.plist ~/Library/LaunchAgents/app.aio.relay.plist"), "{how}");
        assert!(plist.contains("<key>KeepAlive</key>\n  <dict>\n    <key>SuccessfulExit</key>\n    <false/>"));
        assert!(plist.contains("<string>app.aio.relay</string>"));
        assert!(plist.contains("<key>AIO_SUPERVISED</key>\n    <string>1</string>"), "{plist}");
        assert!(plist.contains("<key>AIO_STOP_STAYS_DOWN</key>\n    <string>1</string>"), "{plist}");
        assert!(plist.contains("[ \"$c\" -eq 143 ] &amp;&amp; exit 0") && !plist.contains("&& "), "the loop, as XML");
        assert!(plist.contains("exec \"$HOME/.local/bin/relay\""), "where install.sh puts it");
        let (name, ps1, how) = file(&app(""), "windows", "b").unwrap();
        assert_eq!(name, "b.ps1");
        assert!(ps1.starts_with('\u{feff}'), "a BOM for PowerShell 5.1");
        assert!(ps1.contains(
            "$env:AIO_STOP_STAYS_DOWN = '1'\ndo {\n  & $Exe\n  $code = $LASTEXITCODE\n  if ($code -ne 143) { Start-Sleep -Seconds 5 }\n} while ($code -ne 143)"
        ));
        assert!(ps1.contains("'Programs\\relay\\relay.exe'"), "where updates install");
        assert!(ps1.contains("# relay (aio-rs)"), "no name: the id");
        assert!(how.contains("-Install"));
        assert_eq!(file(&app(""), "android", "b"), None);
    }
}
