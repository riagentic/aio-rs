//! macOS: the links the OS opens the app with (`[app] schemes`). macOS
//! hands a link to the app's `.app` as an Apple event (`GURL`), at a cold
//! start and while it runs, never as an argument, and only to a process
//! that pumps AppKit's events on its main thread. So an app with schemes,
//! started from its bundle, makes itself an AppKit app with no UI of its
//! own (`LSUIElement`: its window is the runtime's), and its main thread
//! pumps those events while it waits for its window or server ([`wait`]).
//! AppKit is loaded then only: an app without schemes carries none of it.
use std::ffi::{c_char, c_void};
use std::sync::OnceLock;

type Id = *mut c_void;

/// `NSApp`, `nextEventMatchingMask:…` and `sendEvent:`, once loaded.
struct Kit {
    app: Id,
    next: extern "C" fn(Id, Id, u64, Id, Id, bool) -> Id,
    send: extern "C" fn(Id, Id, Id) -> Id,
    sel_next: Id,
    sel_send: Id,
    past: Id,
    mode: Id,
}
// SAFETY: its objects are used on the main thread only (`pump` checks).
unsafe impl Send for Kit {}
// SAFETY: as above.
unsafe impl Sync for Kit {}

static KIT: OnceLock<Kit> = OnceLock::new();

/// An Apple event descriptor (`AEDesc`).
#[repr(C)]
struct Desc {
    kind: u32,
    data: *mut c_void,
}

type Handler = extern "C" fn(*const Desc, *mut Desc, isize) -> i16;

const fn four(c: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*c)
}

/// `name` in the library at `path` (loaded once), or null.
fn sym(path: &str, name: &str) -> *mut c_void {
    let (path, name) = (format!("{path}\0"), format!("{name}\0"));
    // SAFETY: two NUL-terminated strings; a system library, never unloaded.
    unsafe {
        let lib = libc::dlopen(path.as_ptr().cast::<c_char>(), libc::RTLD_LAZY);
        if lib.is_null() { lib } else { libc::dlsym(lib, name.as_ptr().cast::<c_char>()) }
    }
}

/// `p` as the function type `F`.
///
/// SAFETY: the caller's: `p` is a function of that exact signature.
unsafe fn as_fn<F: Copy>(p: *mut c_void) -> F {
    // SAFETY: the function's contract, above (a pointer is a function pointer's size).
    unsafe { std::mem::transmute_copy(&p) }
}

const APPKIT: &str = "/System/Library/Frameworks/AppKit.framework/AppKit";
const SERVICES: &str = "/System/Library/Frameworks/CoreServices.framework/CoreServices";
const CF: &str = "/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation";
const OBJC: &str = "/usr/lib/libobjc.A.dylib";

/// The `GURL` event's link, to the app's links (`aio::ui::links`).
extern "C" fn got(event: *const Desc, _reply: *mut Desc, _: isize) -> i16 {
    type Param = unsafe extern "C" fn(*const Desc, u32, u32, *mut Desc) -> i16;
    type Size = unsafe extern "C" fn(*const Desc) -> isize;
    type Data = unsafe extern "C" fn(*const Desc, *mut c_void, isize) -> i16;
    type Dispose = unsafe extern "C" fn(*mut Desc) -> i16;
    let f = |n| sym(SERVICES, n);
    let (param, size, data, dispose) =
        (f("AEGetParamDesc"), f("AEGetDescDataSize"), f("AEGetDescData"), f("AEDisposeDesc"));
    if [param, size, data, dispose].iter().any(|p| p.is_null()) {
        return -1;
    }
    // SAFETY: CoreServices' own functions, as declared in AEDataModel.h,
    // on the event it hands this handler; the buffer is the size it says.
    let url = unsafe {
        let (param, size, data, dispose) =
            (as_fn::<Param>(param), as_fn::<Size>(size), as_fn::<Data>(data), as_fn::<Dispose>(dispose));
        let mut d = Desc { kind: 0, data: std::ptr::null_mut() };
        // keyDirectObject, as typeUTF8Text.
        if param(event, four(b"----"), four(b"utf8"), &mut d) != 0 {
            return -1;
        }
        let mut b = vec![0u8; size(&d).max(0) as usize];
        let ok = data(&d, b.as_mut_ptr().cast(), b.len() as isize) == 0;
        dispose(&mut d);
        if !ok {
            return -1;
        }
        String::from_utf8_lossy(&b).into_owned()
    };
    crate::instance::open_links(&[url]);
    0
}

/// Listen for the OS's links: the `GURL` handler, then AppKit started (the
/// launch's own link, if any, waits for the first [`pump`]). Only on the
/// main thread of an app run from its `.app`; once.
pub(crate) fn listen() {
    // SAFETY: a libc call with no arguments.
    if unsafe { libc::pthread_main_np() } == 0 || KIT.get().is_some() {
        return;
    }
    let exe = std::env::current_exe().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
    if crate::macmove::app_dir(&exe).is_none() {
        return;
    }
    match load() {
        Some(k) => _ = KIT.set(k),
        None => eprintln!("[aio] macOS: AppKit could not be loaded: links to the app's schemes are not heard"),
    }
}

fn load() -> Option<Kit> {
    type Install = unsafe extern "C" fn(u32, u32, Handler, isize, u8) -> i16;
    let nn = |p: *mut c_void| (!p.is_null()).then_some(p);
    let install = nn(sym(SERVICES, "AEInstallEventHandler"))?;
    // AppKit's classes exist once it is loaded.
    nn(sym(APPKIT, "NSApplicationMain"))?;
    let (class, sel, msg) =
        (nn(sym(OBJC, "objc_getClass"))?, nn(sym(OBJC, "sel_registerName"))?, nn(sym(OBJC, "objc_msgSend"))?);
    // NSDefaultRunLoopMode is CF's default mode (toll-free bridged).
    let mode = nn(sym(CF, "kCFRunLoopDefaultMode"))?;
    // SAFETY: the runtime's and CoreServices' functions as their headers
    // declare them; `objc_msgSend` cast to each message's exact signature.
    unsafe {
        let install = as_fn::<Install>(install);
        let class = as_fn::<unsafe extern "C" fn(*const c_char) -> Id>(class);
        let sel = as_fn::<unsafe extern "C" fn(*const c_char) -> Id>(sel);
        let send0 = as_fn::<extern "C" fn(Id, Id) -> Id>(msg);
        // kInternetEventClass, kAEGetURL.
        if install(four(b"GURL"), four(b"GURL"), got, 0, 0) != 0 {
            return None;
        }
        let app = send0(class(c"NSApplication".as_ptr()), sel(c"sharedApplication".as_ptr()));
        if app.is_null() {
            return None;
        }
        send0(app, sel(c"finishLaunching".as_ptr()));
        Some(Kit {
            app,
            next: as_fn(msg),
            send: as_fn(msg),
            sel_next: sel(c"nextEventMatchingMask:untilDate:inMode:dequeue:".as_ptr()),
            sel_send: sel(c"sendEvent:".as_ptr()),
            past: send0(class(c"NSDate".as_ptr()), sel(c"distantPast".as_ptr())),
            mode: *mode.cast::<Id>(),
        })
    }
}

/// Handle the events waiting (a link's `GURL` among them), on the main thread.
fn pump() {
    let Some(k) = KIT.get() else { return };
    // SAFETY: a libc call with no arguments.
    if unsafe { libc::pthread_main_np() } == 0 {
        return;
    }
    loop {
        // NSEventMaskAny, nothing waited for, dequeued.
        let e = (k.next)(k.app, k.sel_next, u64::MAX, k.past, k.mode, true);
        if e.is_null() {
            return;
        }
        (k.send)(k.app, k.sel_send, e);
    }
}

/// `f` on this thread, as `rt.block_on`; listening ([`listen`]), the
/// thread pumps AppKit's events meanwhile, every 200 ms.
pub(crate) fn wait<F: Future>(rt: &tokio::runtime::Runtime, f: F) -> F::Output {
    if KIT.get().is_none() {
        return rt.block_on(f);
    }
    rt.block_on(async {
        let pumping = async {
            loop {
                pump();
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
        };
        tokio::select! {
            r = f => r,
            _ = pumping => unreachable!("it pumps until `f` is done"),
        }
    })
}
