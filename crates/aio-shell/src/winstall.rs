//! Windows: the app's `.exe` installs itself for this user, no admin rights
//! (`<id>.exe --install`): into `%LOCALAPPDATA%\Programs\<id>`, a Start menu
//! entry, and an entry in Settings › Apps whose Uninstall runs
//! `--uninstall`. The app's data stays on uninstall, as on Linux. The
//! one-click download (`oneclick`) installs the same way on a double-click.
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The Settings › Apps entry: `HKCU\…\Uninstall\aio.rt.<id>`.
fn uninstall_key(id: &str) -> String {
    format!(r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\aio.rt.{id}")
}

/// The Start menu shortcut.
fn shortcut(name: &str, id: &str) -> PathBuf {
    let roaming = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_default();
    let file = format!("{}.lnk", crate::oneclick::shortcut_name(name, id));
    roaming.join(r"Microsoft\Windows\Start Menu\Programs").join(file)
}

fn run(cmd: &mut Command, what: &str) -> io::Result<()> {
    let out = cmd.output()?;
    if out.status.success() {
        return Ok(());
    }
    Err(io::Error::other(format!("{what}: {}", String::from_utf8_lossy(&out.stderr).trim())))
}

/// Write the shortcut `at` to `target` (started in `dir`) through the
/// shell's own `IShellLinkW`, carrying the AppUserModelID `aumid`: the one
/// the app's window takes (`--aio-id`), so the taskbar files the window
/// under this entry (its name and icon) and a pin of it opens the app, not
/// Electron.
fn link(at: &Path, target: &Path, dir: &Path, aumid: &str) -> io::Result<()> {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
    };
    use windows_sys::core::GUID;
    const IID_SHELL_LINK_W: GUID = GUID::from_u128(0x000214f9_0000_0000_c000_000000000046);
    const IID_PERSIST_FILE: GUID = GUID::from_u128(0x0000010b_0000_0000_c000_000000000046);
    const IID_PROPERTY_STORE: GUID = GUID::from_u128(0x886d8eeb_8cf2_4446_8d02_cdba1dbdcf99);
    /// PKEY_AppUserModel_ID.
    #[repr(C)]
    struct Key(GUID, u32);
    const AUMID: Key = Key(GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3), 5);
    /// A PROPVARIANT holding a VT_LPWSTR.
    #[repr(C)]
    struct Value(u16, [u16; 3], *const u16, usize);
    const VT_LPWSTR: u16 = 31;
    // Method slots (shobjidl_core.h, objidl.h, propsys.h).
    const QUERY: usize = 0;
    const RELEASE: usize = 2;
    const SET_WORKING_DIRECTORY: usize = 9;
    const SET_PATH: usize = 20;
    const SAVE: usize = 6;
    const SET_VALUE: usize = 6;
    const COMMIT: usize = 7;
    type Query = unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> i32;
    type Plain = unsafe extern "system" fn(*mut c_void) -> i32;
    type SetText = unsafe extern "system" fn(*mut c_void, *const u16) -> i32;
    type Save = unsafe extern "system" fn(*mut c_void, *const u16, i32) -> i32;
    type SetValue = unsafe extern "system" fn(*mut c_void, *const Key, *const Value) -> i32;
    /// SAFETY: `this` is a live COM interface whose table has slot `n` of type `F`.
    unsafe fn method<F: Copy>(this: *mut c_void, n: usize) -> F {
        unsafe { *(*(this as *const *const F)).add(n) }
    }
    let ok = |what: &str, hr: i32| match hr {
        0.. => Ok(()),
        _ => Err(io::Error::other(format!("{what} failed ({hr:#010x})"))),
    };
    let wide = |s: &std::ffi::OsStr| s.encode_wide().chain([0]).collect::<Vec<u16>>();
    let (at, target, dir, aumid) =
        (wide(at.as_os_str()), wide(target.as_os_str()), wide(dir.as_os_str()), wide(aumid.as_ref()));
    // SAFETY: each interface pointer is used between the call that gave it
    // and its Release; every string is NUL-terminated and outlives the calls.
    unsafe {
        let com = CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
        let made = (|| {
            let mut link = std::ptr::null_mut();
            ok(
                "CoCreateInstance(ShellLink)",
                CoCreateInstance(
                    &windows_sys::Win32::UI::Shell::ShellLink,
                    std::ptr::null_mut(),
                    CLSCTX_INPROC_SERVER,
                    &IID_SHELL_LINK_W,
                    &mut link,
                ),
            )?;
            let saved = (|| {
                ok("SetPath", method::<SetText>(link, SET_PATH)(link, target.as_ptr()))?;
                ok("SetWorkingDirectory", method::<SetText>(link, SET_WORKING_DIRECTORY)(link, dir.as_ptr()))?;
                let mut store = std::ptr::null_mut();
                ok("IPropertyStore", method::<Query>(link, QUERY)(link, &IID_PROPERTY_STORE, &mut store))?;
                let value = Value(VT_LPWSTR, [0; 3], aumid.as_ptr(), 0);
                let set = ok("SetValue(AppUserModelID)", method::<SetValue>(store, SET_VALUE)(store, &AUMID, &value))
                    .and_then(|()| ok("Commit", method::<Plain>(store, COMMIT)(store)));
                method::<Plain>(store, RELEASE)(store);
                set?;
                let mut file = std::ptr::null_mut();
                ok("IPersistFile", method::<Query>(link, QUERY)(link, &IID_PERSIST_FILE, &mut file))?;
                let saved = ok("Save", method::<Save>(file, SAVE)(file, at.as_ptr(), 1));
                method::<Plain>(file, RELEASE)(file);
                saved
            })();
            method::<Plain>(link, RELEASE)(link);
            saved
        })();
        if com >= 0 {
            CoUninitialize();
        }
        made
    }
}

/// The installed exe of app `id`.
fn installed_exe(id: &str) -> PathBuf {
    crate::instance::programs(id).join(format!("{id}.exe"))
}

/// Unpacks a runtime into the folder it is given.
type Unpack<'a> = &'a dyn Fn(&Path) -> io::Result<()>;

/// `bin` installed as `exe` in `dir`, with (`unpack`: it unpacks into the
/// folder it is given) an offline build's runtime as `<dir>\electron`. One
/// install at a time: another (a second double-click) waits for this one.
/// While the installed app runs nothing is touched (its exe can't be
/// written: `ERROR_SHARING_VIOLATION`). The old runtime is set aside, not
/// deleted, and put back when the new one or the exe can't take its place,
/// so the app is always one version whole.
fn replace(dir: &Path, exe: &Path, bin: &[u8], unpack: Option<Unpack>) -> io::Result<()> {
    let lock = std::fs::File::create(dir.join(".install.lock"))?;
    lock.lock()?;
    match std::fs::OpenOptions::new().write(true).open(exe) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    let rt = dir.join(crate::runtime::BUNDLED);
    let (stage, old) = (dir.join(".electron.next"), dir.join(".electron.old"));
    for d in [&stage, &old] {
        _ = std::fs::remove_dir_all(d);
    }
    let had = match unpack {
        None => false,
        Some(unpack) => {
            let swapped = unpack(&stage).and_then(|()| {
                let had = match std::fs::rename(&rt, &old) {
                    Ok(()) => true,
                    Err(e) if e.kind() == io::ErrorKind::NotFound => false,
                    Err(e) => return Err(e),
                };
                match std::fs::rename(stage.join(crate::runtime::BUNDLED), &rt) {
                    Ok(()) => Ok(had),
                    Err(e) => {
                        if had {
                            _ = std::fs::rename(&old, &rt);
                        }
                        Err(e)
                    }
                }
            });
            _ = std::fs::remove_dir_all(&stage);
            swapped?
        }
    };
    // Whole or not at all: a second double-click never sees half an exe.
    let next = exe.with_extension("exe.next");
    let copied = std::fs::write(&next, bin).and_then(|_| std::fs::rename(&next, exe));
    if copied.is_err() {
        _ = std::fs::remove_file(&next);
        if had {
            _ = std::fs::remove_dir_all(&rt).and_then(|()| std::fs::rename(&old, &rt));
        }
    }
    _ = std::fs::remove_dir_all(&old);
    copied
}

/// This exe copied into place, with an offline build's runtime beside it:
/// the one the download carries (`oneclick::split_runtime`), or the one
/// beside this exe (an unzipped bundle's, `--install`). Then its Settings
/// entry and (`shortcut`) its Start menu entry.
fn place(cfg: &aio::config::Config, version: &str, shortcut: bool) -> io::Result<()> {
    let (id, name) = (&cfg.app.id, &cfg.app.name);
    let (dir, exe) = (crate::instance::programs(id), installed_exe(id));
    std::fs::create_dir_all(&dir)?;
    let me = std::env::current_exe()?;
    if me.canonicalize()? != exe.canonicalize().unwrap_or_default() {
        let file = std::fs::read(&me)?;
        let (bin, runtime) = crate::oneclick::split_runtime(&file);
        let beside =
            me.parent().filter(|d| crate::runtime::carries_runtime(d)).map(|d| d.join(crate::runtime::BUNDLED));
        let unpack = |stage: &Path| match (runtime, &beside) {
            (Some(zip), _) => {
                zip::ZipArchive::new(io::Cursor::new(zip)).and_then(|mut z| z.extract(stage)).map_err(io::Error::other)
            }
            (None, Some(from)) => crate::swap::copy_dir(from, &stage.join(crate::runtime::BUNDLED)),
            (None, None) => Ok(()),
        };
        let carries = runtime.is_some() || beside.is_some();
        replace(&dir, &exe, bin, carries.then_some(&unpack as Unpack))?;
    }
    if shortcut {
        let lnk = self::shortcut(name, id);
        lnk.parent().map(std::fs::create_dir_all).transpose()?;
        link(&lnk, &exe, &dir, id).map_err(|e| io::Error::other(format!("the Start menu entry: {e}")))?;
    }
    let key = uninstall_key(id);
    let values = [
        ("DisplayName", "REG_SZ", name.to_string()),
        ("DisplayVersion", "REG_SZ", version.to_string()),
        ("DisplayIcon", "REG_SZ", exe.display().to_string()),
        ("InstallLocation", "REG_SZ", dir.display().to_string()),
        ("UninstallString", "REG_SZ", format!("\"{}\" --uninstall", exe.display())),
        ("NoModify", "REG_DWORD", "1".into()),
        ("NoRepair", "REG_DWORD", "1".into()),
    ];
    for (v, t, d) in values {
        run(Command::new("reg").args(["add", &key, "/v", v, "/t", t, "/d", &d, "/f"]), "the Settings entry")?;
    }
    Ok(())
}

pub fn install(cfg: &aio::config::Config, version: &str) -> io::Result<()> {
    place(cfg, version, true)?;
    let exe = installed_exe(&cfg.app.id);
    eprintln!("[aio] {} {version} is installed: {} and the Start menu", cfg.app.name, exe.display());
    Command::new(&exe).spawn().map(drop)
}

/// The double-clicked download: installed (or upgraded) unless as new a
/// version is there, then the installed app started with `args`. A failure
/// is said in a message box too: nobody reads a double-click's stderr.
pub fn one_click(cfg: &aio::config::Config, version: &str, args: &[String]) -> io::Result<()> {
    let exe = installed_exe(&cfg.app.id);
    let done = (|| {
        let installed = exe.exists().then(|| installed_version(&exe, &cfg.app.id));
        if let crate::oneclick::Plan::Install { shortcut } = crate::oneclick::plan(version, installed.as_deref()) {
            place(cfg, version, shortcut).map_err(|e| match e.raw_os_error() {
                // ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION: replacing
                // the installed exe while it runs.
                Some(5 | 32) => {
                    io::Error::other(format!("{} is running: close it, then open this again", cfg.app.name))
                }
                _ => e,
            })?;
        }
        Command::new(&exe).args(args).spawn().map(drop)
    })();
    if let Err(e) = &done {
        message_box(&cfg.app.name, &e.to_string());
    }
    done
}

/// What the installed exe says its version is; `""` if it can't say.
fn installed_version(exe: &Path, id: &str) -> String {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let out = Command::new(exe).arg("--version").creation_flags(CREATE_NO_WINDOW).output();
    out.ok().and_then(|o| crate::oneclick::version_of(&String::from_utf8_lossy(&o.stdout), id)).unwrap_or_default()
}

fn message_box(title: &str, text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
    let w = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let (title, text) = (w(title), w(text));
    // SAFETY: two NUL-terminated UTF-16 strings that outlive the call.
    unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), MB_OK | MB_ICONERROR) };
}

pub fn uninstall(cfg: &aio::config::Config) -> io::Result<()> {
    let (id, name) = (&cfg.app.id, &cfg.app.name);
    _ = std::fs::remove_file(shortcut(name, id));
    _ = Command::new("reg").args(["delete", &uninstall_key(id), "/f"]).output();
    // This .exe is running: a detached shell removes it once it has exited,
    // and its versions (the updater's), as uninstall.sh does on Linux.
    let (dir, versions) = (crate::instance::programs(id), crate::update::dir(id));
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x8;
    Command::new("cmd")
        .arg("/C")
        .raw_arg(format!(
            "ping -n 3 127.0.0.1 >nul & rmdir /S /Q \"{}\" & rmdir /S /Q \"{}\"",
            dir.display(),
            versions.display()
        ))
        .creation_flags(DETACHED_PROCESS)
        .spawn()?;
    eprintln!("[aio] {name} is removed; its data stays in {}", aio::paths::tool_home().join("apps").join(id).display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the shell itself reads from a shortcut, through PowerShell.
    fn read(lnk: &Path, what: &str) -> String {
        let (dir, file) = (lnk.parent().unwrap(), lnk.file_name().unwrap().to_string_lossy());
        let q = |s: &str| format!("'{}'", s.replace('\'', "''"));
        let out = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command"])
            .arg(format!(
                "(New-Object -ComObject Shell.Application).NameSpace({}).ParseName({}).ExtendedProperty({})",
                q(&dir.display().to_string()),
                q(&file),
                q(what)
            ))
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// An unpack that writes `mark` into the new runtime, slowly.
    fn runtime(mark: &'static str) -> impl Fn(&Path) -> io::Result<()> {
        move |stage: &Path| {
            let rt = stage.join(crate::runtime::BUNDLED);
            std::fs::create_dir_all(&rt)?;
            std::thread::sleep(std::time::Duration::from_millis(200));
            std::fs::write(rt.join("mark"), mark)
        }
    }

    #[test]
    fn installs_at_once_wait_for_each_other_and_a_running_app_is_never_touched() {
        let t = tempfile::tempdir().unwrap();
        let (dir, exe) = (t.path().to_path_buf(), t.path().join("notes.exe"));
        let mark = || std::fs::read_to_string(dir.join(r"electron\mark")).unwrap();
        // Two at once (a double-click twice): each whole, one after the other.
        std::thread::scope(|s| {
            for m in ["one", "two"] {
                let (dir, exe) = (&dir, &exe);
                s.spawn(move || replace(dir, exe, m.as_bytes(), Some(&runtime(m))).unwrap());
            }
        });
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), mark(), "the exe and its runtime are one version");
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(left.len(), 3, "{left:?}: notes.exe, electron and the lock, nothing staged or set aside");
        // While it runs: refused before anything is touched.
        let was = mark();
        std::fs::copy(r"C:\Windows\System32\PING.EXE", &exe).unwrap();
        let mut running =
            Command::new(&exe).args(["-n", "30", "127.0.0.1"]).stdout(std::process::Stdio::null()).spawn().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let e = replace(&dir, &exe, b"three", Some(&runtime("three"))).unwrap_err();
        running.kill().unwrap();
        running.wait().unwrap();
        assert_eq!(e.raw_os_error(), Some(32), "{e}");
        assert_eq!(mark(), was, "the runtime is the running version's");
        // A runtime that can't be unpacked leaves the old one in place.
        let broken = |_: &Path| Err(io::Error::other("a torn download"));
        assert!(replace(&dir, &exe, b"four", Some(&broken)).is_err());
        assert_eq!(mark(), was);
        replace(&dir, &exe, b"five", Some(&runtime("five"))).unwrap();
        assert_eq!((std::fs::read_to_string(&exe).unwrap(), mark()), ("five".into(), "five".into()));
    }

    #[test]
    fn the_start_menu_entry_opens_the_app_and_carries_its_windows_id() {
        let t = tempfile::tempdir().unwrap();
        let (lnk, exe) = (t.path().join("Notes it's.lnk"), t.path().join("notes.exe"));
        std::fs::copy(std::env::current_exe().unwrap(), &exe).unwrap();
        link(&lnk, &exe, t.path(), "com.example.notes").unwrap();
        assert_eq!(read(&lnk, "System.AppUserModel.ID"), "com.example.notes");
        assert_eq!(
            read(&lnk, "System.Link.TargetParsingPath").to_lowercase(),
            exe.display().to_string().to_lowercase()
        );
        // Written again (an upgrade), it is replaced, not refused.
        link(&lnk, &exe, t.path(), "com.example.notes2").unwrap();
        assert_eq!(read(&lnk, "System.AppUserModel.ID"), "com.example.notes2");
        let e = link(&t.path().join("no/such/dir/x.lnk"), &exe, t.path(), "a").unwrap_err();
        assert!(e.to_string().starts_with("Save failed"), "{e}");
    }
}
