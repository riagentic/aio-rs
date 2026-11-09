//! aio-rs's hot-patch runtime, the app's half of `ax dev`'s hot-patch
//! (docs/build/dev.md): `ax dev` builds the changed crates into a patch
//! library and a [`JumpTable`] (old function address → new), the app loads
//! it with [`apply_patch`], and each call through [`HotFn`] takes the new
//! code from then on. A port of subsecond 0.7.10's runtime (MIT OR
//! Apache-2.0), on the desktop (Linux, macOS, Windows) and in a page
//! (wasm, `ax dev --web`): the same table and the same calls.
//!
//! Only a build with aio's `dev` feature (`ax dev`'s) has it; with no
//! patch loaded, a [`HotFn`] is a plain call.
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use std::path::PathBuf;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::{Arc, Mutex};

/// A patch: its library and where each patched function moved.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct JumpTable {
    /// The patch library, loaded as it is (`dlopen`, `LoadLibrary`).
    pub lib: PathBuf,
    /// Old address → new, as the builder linked them (before the loader's
    /// offsets: [`apply_patch`] rebases both sides).
    pub map: AddressMap,
    /// `main` of the running binary as the builder saw it ([`aslr_reference`]).
    pub aslr_reference: u64,
    /// `main` of the patch library as it was linked.
    pub new_base_address: u64,
    /// Indirect functions the patch registers (a wasm patch's; 0 here).
    pub ifunc_count: u64,
}

/// Address → address, unhashed: addresses are unique already.
pub type AddressMap = HashMap<u64, u64, BuildHasherDefault<AddressHasher>>;

/// The identity hasher of [`AddressMap`].
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct AddressHasher(u64);

impl Hasher for AddressHasher {
    fn write(&mut self, bytes: &[u8]) {
        // Only integers are hashed (u64 keys); bytes fold in all the same.
        for b in bytes {
            self.0 = (self.0 << 8) | u64::from(*b);
        }
    }

    fn write_u64(&mut self, n: u64) {
        self.0 = n;
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

/// Why a patch didn't load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchError {
    /// The library didn't open, or has no `main`.
    Dlopen(String),
}

impl std::fmt::Display for PatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Dlopen(e) => write!(f, "failed to load the patch library: {e}"),
        }
    }
}

impl std::error::Error for PatchError {}

// The table in force: leaked, so a reader holds it for the process's life.
static TABLE: AtomicPtr<JumpTable> = AtomicPtr::new(std::ptr::null_mut());
static HANDLERS: Mutex<Vec<Arc<dyn Fn() + Send + Sync>>> = Mutex::new(Vec::new());

/// Is `F` a function pointer (`fn(..) -> ..`, maybe `unsafe` or `extern`),
/// not a closure or a function item?
fn is_fn_pointer<F>() -> bool {
    let name = std::any::type_name::<F>();
    size_of::<F>() == size_of::<fn()>()
        && (name.starts_with("fn(") || name.starts_with("unsafe ") || name.starts_with("extern "))
}

/// Called after each patch is applied (the views build again on it).
pub fn register_handler(handler: Arc<dyn Fn() + Send + Sync + 'static>) {
    HANDLERS.lock().unwrap_or_else(|e| e.into_inner()).push(handler);
}

/// The table in force, if a patch was applied.
///
/// # Safety
/// The table's addresses are trusted: only `ax dev`'s own patches are applied.
pub unsafe fn get_jump_table() -> Option<&'static JumpTable> {
    let t = TABLE.load(Ordering::Acquire);
    // SAFETY: a non-null table was leaked by `commit` and is never freed.
    (!t.is_null()).then(|| unsafe { &*t })
}

fn commit(table: JumpTable) {
    TABLE.store(Box::into_raw(Box::new(table)), Ordering::Release);
    let handlers = HANDLERS.lock().unwrap_or_else(|e| e.into_inner()).clone();
    handlers.iter().for_each(|h| h());
}

/// Where this process's `main` is loaded: the base a patch is linked against.
pub fn aslr_reference() -> usize {
    static MAIN: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *MAIN.get_or_init(os::main_of_process)
}

/// Load the patch `table` names and make it the one in force: its map is
/// rebased onto where the binary and the library are loaded. In a page
/// (wasm) the page loaded the patch into the module's table: its map is
/// of table slots, as they are.
///
/// # Safety
/// The library runs code as it loads, and its functions take the place of
/// the binary's: it must be `ax dev`'s patch of this very binary.
pub unsafe fn apply_patch(mut table: JumpTable) -> Result<(), PatchError> {
    if cfg!(target_arch = "wasm32") {
        commit(table);
        return Ok(());
    }
    // SAFETY: the caller's: the library is a patch of this binary.
    let main = unsafe { os::open_main(&table.lib) }.map_err(PatchError::Dlopen)?;
    let old = aslr_reference().wrapping_sub(table.aslr_reference as usize);
    let new = main.wrapping_sub(table.new_base_address as usize);
    table.map = table
        .map
        .iter()
        .map(|(k, v)| ((*k as usize).wrapping_add(old) as u64, (*v as usize).wrapping_add(new) as u64))
        .collect();
    commit(table);
    Ok(())
}

/// A function called through the table in force: the patched code once a
/// patch moved it, else its own.
pub struct HotFn<A, M, F: HotFunction<A, M>> {
    inner: F,
    _marker: std::marker::PhantomData<(A, M)>,
}

impl<A, M, F: HotFunction<A, M>> HotFn<A, M, F> {
    /// `f` (a function item, or a closure) called hot.
    pub const fn current(f: F) -> Self {
        Self { inner: f, _marker: std::marker::PhantomData }
    }

    /// Call it, through the table in force.
    pub fn call(&mut self, args: A) -> F::Return {
        // A function pointer: the table has its address. (A closure that
        // holds one pointer is pointer-sized too: not one.)
        if is_fn_pointer::<F>() {
            // SAFETY: `F` is pointer-sized: a function pointer (see `call_as_ptr`).
            return unsafe { self.inner.call_as_ptr(args) };
        }
        // A closure or a function item (ZST): its `call_it` is a function
        // of its own, which the table moves.
        let known = <F as HotFunction<A, M>>::call_it as *const () as u64;
        // SAFETY: a table entry's target is the patched `call_it` of `F`,
        // with the same signature.
        if let Some(ptr) = unsafe { get_jump_table() }.and_then(|t| t.map.get(&known).copied()) {
            let call_it = unsafe { std::mem::transmute::<*const (), fn(&mut F, A) -> F::Return>(ptr as *const ()) };
            return call_it(&mut self.inner, args);
        }
        self.inner.call_it(args)
    }
}

/// A callable [`HotFn`] takes: `FnMut` of up to 9 arguments.
pub trait HotFunction<Args, Marker> {
    /// What it returns.
    type Return;
    /// Its function-pointer type.
    type Real;
    /// Call it as it is.
    fn call_it(&mut self, args: Args) -> Self::Return;
    /// Call it as a function pointer moved by the table.
    ///
    /// # Safety
    /// `Self` is a function pointer (pointer-sized and `Copy` as one).
    unsafe fn call_as_ptr(&mut self, args: Args) -> Self::Return;
}

macro_rules! hot_function {
    ($(($marker:ident, $($arg:ident),*)),*) => {$(
        #[doc(hidden)]
        pub struct $marker;

        impl<T, $($arg,)* R> HotFunction<($($arg,)*), $marker> for T
        where
            T: FnMut($($arg),*) -> R,
        {
            type Return = R;
            type Real = fn($($arg),*) -> R;

            #[allow(non_snake_case)]
            fn call_it(&mut self, ($($arg,)*): ($($arg,)*)) -> R {
                self($($arg),*)
            }

            #[allow(non_snake_case)]
            unsafe fn call_as_ptr(&mut self, ($($arg,)*): ($($arg,)*)) -> R {
                // SAFETY: the caller's: `Self` is a `fn` pointer of this signature.
                let real = unsafe { std::mem::transmute_copy::<Self, Self::Real>(self) };
                if let Some(ptr) = unsafe { get_jump_table() }.and_then(|t| t.map.get(&(real as usize as u64)).copied()) {
                    // SAFETY: the table moved this function to `ptr`, same signature.
                    let moved = unsafe { std::mem::transmute::<usize, Self::Real>(ptr as usize) };
                    return moved($($arg),*);
                }
                real($($arg),*)
            }
        }
    )*};
}

hot_function!(
    (Fn0Marker,),
    (Fn1Marker, A),
    (Fn2Marker, A, B),
    (Fn3Marker, A, B, C),
    (Fn4Marker, A, B, C, D),
    (Fn5Marker, A, B, C, D, E),
    (Fn6Marker, A, B, C, D, E, F),
    (Fn7Marker, A, B, C, D, E, F, G),
    (Fn8Marker, A, B, C, D, E, F, G, H),
    (Fn9Marker, A, B, C, D, E, F, G, H, I)
);

#[cfg(unix)]
mod os {
    use std::ffi::{CStr, CString};

    fn last_error() -> String {
        // SAFETY: dlerror's text is valid until the next dl call on this thread.
        let e = unsafe { libc::dlerror() };
        if e.is_null() { "unknown error".into() } else { unsafe { CStr::from_ptr(e) }.to_string_lossy().into_owned() }
    }

    pub fn main_of_process() -> usize {
        // SAFETY: a lookup in the loaded images; `main` is exported by `ax dev`'s link.
        unsafe { libc::dlsym(libc::RTLD_DEFAULT, c"main".as_ptr()) as usize }
    }

    /// Open `lib` (kept open for good: its code is called from then on)
    /// and find its `main`.
    pub unsafe fn open_main(lib: &std::path::Path) -> Result<usize, String> {
        use std::os::unix::ffi::OsStrExt;
        let path = CString::new(lib.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
        // SAFETY: the caller's: the library is a patch of this binary.
        let h = unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL) };
        if h.is_null() {
            return Err(format!("{}: {}", lib.display(), last_error()));
        }
        // SAFETY: a symbol lookup in the handle just opened.
        let main = unsafe { libc::dlsym(h, c"main".as_ptr()) };
        if main.is_null() {
            return Err(format!("{}: no `main`: {}", lib.display(), last_error()));
        }
        Ok(main as usize)
    }
}

#[cfg(windows)]
mod os {
    use std::ffi::c_void;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetModuleHandleW(name: *const u16) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
        fn LoadLibraryW(name: *const u16) -> *mut c_void;
        fn GetLastError() -> u32;
    }

    pub fn main_of_process() -> usize {
        // SAFETY: the process's own module, and a lookup in it.
        unsafe { GetProcAddress(GetModuleHandleW(std::ptr::null()), c"main".as_ptr().cast()) as usize }
    }

    /// Load `lib` (kept loaded for good) and find its `main`.
    pub unsafe fn open_main(lib: &std::path::Path) -> Result<usize, String> {
        use std::os::windows::ffi::OsStrExt;
        let wide: Vec<u16> = lib.as_os_str().encode_wide().chain([0]).collect();
        // SAFETY: the caller's: the library is a patch of this binary.
        let h = unsafe { LoadLibraryW(wide.as_ptr()) };
        if h.is_null() {
            return Err(format!("{}: error {}", lib.display(), unsafe { GetLastError() }));
        }
        // SAFETY: a lookup in the module just loaded.
        let main = unsafe { GetProcAddress(h, c"main".as_ptr().cast()) };
        if main.is_null() {
            return Err(format!("{}: no `main`: error {}", lib.display(), unsafe { GetLastError() }));
        }
        Ok(main as usize)
    }
}

#[cfg(not(any(unix, windows)))]
mod os {
    pub fn main_of_process() -> usize {
        0
    }

    pub unsafe fn open_main(_: &std::path::Path) -> Result<usize, String> {
        Err("hot-patching runs on Linux, macOS and Windows".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seven() -> u32 {
        7
    }

    #[test]
    fn a_table_round_trips_as_the_builder_writes_it() {
        let mut map = AddressMap::default();
        map.insert(0x1000, 0x2000);
        let t = JumpTable { lib: "p.so".into(), map, aslr_reference: 1, new_base_address: 2, ifunc_count: 0 };
        let back: JumpTable = serde_json::from_str(&serde_json::to_string(&t).unwrap()).unwrap();
        assert_eq!(back, t);
    }

    #[test]
    fn with_no_patch_a_hot_call_is_the_functions_own() {
        assert_eq!(HotFn::current(seven).call(()), 7);
        let mut n = 0;
        assert_eq!(
            HotFn::current(|a: u32, b: u32| {
                n += 1;
                a + b
            })
            .call((2, 3)),
            5
        );
        assert_eq!(n, 1);
        let f: fn() -> u32 = seven;
        assert_eq!(HotFn::current(f).call(()), 7);
    }

    #[test]
    fn only_a_function_pointer_is_called_as_one() {
        let n = 0u64;
        let by_ref = || n + 1;
        assert_eq!(size_of_val(&by_ref), size_of::<fn()>(), "a closure of one reference is pointer-sized");
        fn kind<F>(_: &F) -> bool {
            is_fn_pointer::<F>()
        }
        assert!(!kind(&by_ref) && !kind(&seven));
        assert!(kind(&(seven as fn() -> u32)) && kind(&(seven as unsafe fn() -> u32)));
    }

    #[test]
    fn a_library_that_does_not_load_is_said_and_nothing_changes() {
        let t = JumpTable {
            lib: "/nonexistent/patch.so".into(),
            map: AddressMap::default(),
            aslr_reference: 0,
            new_base_address: 0,
            ifunc_count: 0,
        };
        let e = unsafe { apply_patch(t) }.unwrap_err();
        assert!(e.to_string().contains("nonexistent"), "{e}");
        assert!(unsafe { get_jump_table() }.is_none());
    }
}
