//! A cell type as the runtime's one copy of the store, handle and sync code
//! sees it: its state behind [`AnyCell`], its constants and constructors in
//! a [`Kind`], both made once per cell type. The generic shells
//! (`Store<T>`, `Handle<T>`, `Remote<T>`) only convert to and from `T`, so
//! an app's build compiles that machinery once, not once per cell.
use crate::compat::data::{Bound, Slice};
use crate::{Caller, Cell, Cx, Declared, Field, Mode, Recovered, Result, Transaction};
use serde_json::Value;
use std::any::Any;
use std::sync::Arc;

/// A cell's state, its type erased.
pub(crate) type St = Arc<dyn AnyCell>;

/// What the runtime does with a cell's state, without knowing its type.
pub(crate) trait AnyCell: Any + Send + Sync {
    fn kind(&self) -> &'static Kind;
    /// A copy, alone in its `Arc` (so [`own`] can change it).
    fn dup(&self) -> St;
    fn same(&self, other: &dyn AnyCell) -> bool;
    fn to_json(&self) -> serde_json::Result<Value>;
    /// Its size as JSON, in bytes (0: it doesn't serialize).
    #[cfg(feature = "server")]
    fn json_len(&self) -> usize;
    fn apply(&mut self, cx: &Cx, method: &str, args: Value) -> Result<Value>;
    fn advance(&mut self, method: &str) -> Result;
    fn check(&self) -> Result;
    fn allow(&self, who: &Caller, method: &str, args: &Value) -> Result;
    fn save(&self) -> Option<St>;
    fn restore(self: Arc<Self>) -> St;
    fn recovered(self: Arc<Self>, r: &Recovered) -> St;
    fn project(&self, viewer: &Caller) -> Value;
    fn slice_json(&self, field: &str) -> Option<serde_json::Result<Value>>;
    fn changed_fields(&self, other: &dyn AnyCell) -> u64;
    fn take_fields(&mut self, from: &dyn AnyCell, mask: u64);
}

/// `st`, which nothing else holds, to change.
pub(crate) fn own(st: &mut St) -> &mut dyn AnyCell {
    Arc::get_mut(st).expect("a state copy nothing else holds")
}

/// `st` as cell `T`'s state.
pub(crate) fn typed<T: Cell>(st: St) -> Arc<T> {
    let any: Arc<dyn Any + Send + Sync> = st;
    any.downcast().unwrap_or_else(|_| panic!("{}: a state of another cell", T::ID))
}

/// `st` as cell `T`'s state, borrowed.
pub(crate) fn typed_ref<T: Cell>(st: &dyn AnyCell) -> &T {
    let any: &dyn Any = st;
    any.downcast_ref().unwrap_or_else(|| panic!("{}: a state of another cell", T::ID))
}

/// Cell `T`'s constants and constructors.
pub(crate) struct Kind {
    pub id: &'static str,
    pub version: u32,
    pub persist: bool,
    #[cfg(feature = "server")]
    pub shared: bool,
    pub diagnostics: bool,
    pub saved: bool,
    pub coalesce: bool,
    pub redact: bool,
    pub migrates: bool,
    pub screened: bool,
    pub transient: &'static [&'static str],
    pub persist_include: &'static [&'static str],
    pub server: &'static [&'static str],
    pub tables: &'static [Bound],
    pub aio_cells: &'static [Slice],
    pub retired: &'static [&'static str],
    pub tx: Option<Tx>,
    pub bound_code: Option<BoundCode>,
    pub fields: &'static [Field],
    pub methods: &'static [&'static str],
    pub signatures: &'static [&'static str],
    pub client: &'static [&'static str],
    #[cfg(feature = "server")]
    pub unpredicted: &'static [&'static str],
    pub redacted: &'static [&'static str],
    pub coalesced: &'static [&'static str],
    pub skip_if_running: &'static [&'static str],
    pub untraveled: &'static [&'static str],
    pub ttl: &'static [&'static str],
    pub on_start: Option<&'static str>,
    pub on_stop: Option<&'static str>,
    pub on_aio: Option<&'static str>,
    pub schedules: &'static [Declared],
    pub default: fn() -> St,
    pub from_json: fn(Value) -> serde_json::Result<St>,
    /// A synced copy's state of a server's view ([`crate::copy_of`]).
    #[cfg(feature = "server")]
    pub copy_of: fn(Value) -> serde_json::Result<St>,
    pub migrate: fn(Value, u32) -> std::result::Result<St, String>,
    pub machine_errors: fn(&[&str]) -> Vec<String>,
    pub machine_ends: fn() -> Vec<String>,
    pub blind: fn(&str) -> u64,
    pub async_mode: fn(&str) -> Option<Mode>,
    pub redacts: fn(&str) -> bool,
    /// Its type's name, as a test names it.
    #[cfg(feature = "testing")]
    pub name: fn() -> &'static str,
}

/// A `#[cell(transaction)]` cell's settings and its transactions' code
/// (`store/tx.rs`), reached only through here: an app without such a cell
/// links none of it.
#[derive(Clone, Copy)]
pub(crate) struct Tx {
    pub cfg: Transaction,
    pub call: crate::store::TxCall,
    pub sweep: fn(&mut crate::store::Core) -> bool,
}

/// The code of a cell's `db:` tables, its own or its `#[aio_cell]`s'
/// (declared, read back), reached only through here: an app whose cells
/// bind none links none of it.
#[derive(Clone, Copy)]
#[allow(clippy::type_complexity)]
pub(crate) struct BoundCode {
    pub declare: fn(&rusqlite::Connection, &[(&str, &Bound)]) -> std::result::Result<(), crate::store::Error>,
    pub bind: fn(&rusqlite::Connection, &str, u32, &[Bound]) -> std::result::Result<Option<(Value, u32)>, crate::store::Error>,
}

/// Whether a cell binds `db:` tables: its own, or an `#[aio_cell]`'s.
const fn binds(tables: &[Bound], slices: &[Slice]) -> bool {
    let mut i = 0;
    while i < slices.len() {
        if !slices[i].tables.is_empty() {
            return true;
        }
        i += 1;
    }
    !tables.is_empty()
}

/// [`Kind`] of each cell type, made as it builds.
pub(crate) trait KindOf {
    const KIND: &'static Kind;
}

impl<T: Cell> KindOf for T {
    const KIND: &'static Kind = &Kind {
        id: T::ID,
        version: T::VERSION,
        persist: T::PERSIST,
        #[cfg(feature = "server")]
        shared: T::SHARED,
        diagnostics: T::DIAGNOSTICS,
        saved: T::SAVED,
        coalesce: T::COALESCE,
        redact: T::REDACT,
        migrates: T::MIGRATES,
        screened: T::SCREENED,
        transient: T::TRANSIENT,
        persist_include: T::PERSIST_INCLUDE,
        server: T::SERVER,
        tables: T::TABLES,
        aio_cells: T::AIO_CELLS,
        retired: T::RETIRED,
        bound_code: match binds(T::TABLES, T::AIO_CELLS) {
            true => Some(BoundCode { declare: crate::store::declare, bind: crate::store::bind }),
            false => None,
        },
        tx: match T::TRANSACTION {
            Some(cfg) => Some(Tx { cfg, call: crate::store::Core::tx_call, sweep: crate::store::Core::tx_sweep }),
            None => None,
        },
        fields: T::FIELDS,
        methods: T::METHODS,
        signatures: T::SIGNATURES,
        client: T::CLIENT,
        #[cfg(feature = "server")]
        unpredicted: T::UNPREDICTED,
        redacted: T::REDACTED,
        coalesced: T::COALESCED,
        skip_if_running: T::SKIP_IF_RUNNING,
        untraveled: T::UNTRAVELED,
        ttl: T::TTL,
        on_start: T::ON_START,
        on_stop: T::ON_STOP,
        on_aio: T::ON_AIO,
        schedules: T::SCHEDULES,
        default: || Arc::new(T::default()),
        from_json: |v| serde_json::from_value::<T>(v).map(|t| Arc::new(t) as St),
        #[cfg(feature = "server")]
        copy_of: |v| crate::copy_of::<T>(v).map(|t| Arc::new(t) as St),
        migrate: |v, from| T::migrate(v, from).map(|t| Arc::new(t) as St),
        machine_errors: T::machine_errors,
        machine_ends: T::machine_ends,
        blind: T::blind,
        async_mode: T::async_mode,
        redacts: T::redacts,
        #[cfg(feature = "testing")]
        name: crate::testing::name::<T>,
    };
}

impl<T: Cell> AnyCell for T {
    fn kind(&self) -> &'static Kind {
        T::KIND
    }

    fn dup(&self) -> St {
        Arc::new(self.clone())
    }

    fn same(&self, other: &dyn AnyCell) -> bool {
        let other: &dyn Any = other;
        other.downcast_ref::<T>().is_some_and(|o| self == o)
    }

    fn to_json(&self) -> serde_json::Result<Value> {
        serde_json::to_value(self)
    }

    #[cfg(feature = "server")]
    fn json_len(&self) -> usize {
        serde_json::to_vec(self).map_or(0, |v| v.len())
    }

    fn apply(&mut self, cx: &Cx, method: &str, args: Value) -> Result<Value> {
        // `ax dev`'s hot-patch: with no patch applied, the function itself
        // (a release build calls it directly).
        #[cfg(feature = "dev")]
        return aio_hotpatch::HotFn::current(<T as Cell>::apply).call((self, cx, method, args));
        #[cfg(not(feature = "dev"))]
        Cell::apply(self, cx, method, args)
    }

    fn advance(&mut self, method: &str) -> Result {
        crate::CellData::advance(self, method)
    }

    fn check(&self) -> Result {
        Cell::check(self)
    }

    fn allow(&self, who: &Caller, method: &str, args: &Value) -> Result {
        Cell::allow(self, who, method, args)
    }

    fn save(&self) -> Option<St> {
        crate::CellData::save(self).map(|t| Arc::new(t) as St)
    }

    fn restore(self: Arc<Self>) -> St {
        Arc::new(crate::CellData::restore(Arc::unwrap_or_clone(self)))
    }

    fn recovered(self: Arc<Self>, r: &Recovered) -> St {
        Arc::new(crate::CellData::recovered(Arc::unwrap_or_clone(self), r))
    }

    fn project(&self, viewer: &Caller) -> Value {
        crate::CellData::project(self, viewer)
    }

    fn slice_json(&self, field: &str) -> Option<serde_json::Result<Value>> {
        crate::CellData::slice_json(self, field)
    }

    fn changed_fields(&self, other: &dyn AnyCell) -> u64 {
        crate::CellData::changed_fields(self, typed_ref::<T>(other))
    }

    fn take_fields(&mut self, from: &dyn AnyCell, mask: u64) {
        crate::CellData::take_fields(self, typed_ref::<T>(from), mask)
    }
}

/// `st` as JSON ([`crate::json`]'s words when it doesn't serialize).
pub(crate) fn json(st: &dyn AnyCell) -> Result<Value> {
    st.to_json().map_err(|e| crate::Rejected::new(format!("unserializable: {e}")))
}

/// [`crate::screened`] for cell `k`.
pub(crate) fn screened(k: &Kind) -> bool {
    crate::screened_as(k.screened)
}

/// What `viewer` may see of `st` ([`crate::shown_as`]).
pub(crate) fn shown(st: &dyn AnyCell, viewer: &Caller) -> Value {
    let k = st.kind();
    crate::shown_as(k.id, k.screened, st.project(viewer), || (k.default)().to_json(), viewer)
}

/// `st` alone in its `Arc`: itself when nothing else holds it, else a copy.
pub(crate) fn unshared(mut st: St) -> St {
    match Arc::get_mut(&mut st) {
        Some(_) => st,
        None => st.dup(),
    }
}
