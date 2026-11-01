//! Working with aio, the Deno-based framework, from an aio-rs app.
//!
//! Every native build has [`data`]: reading and writing a ported app's
//! `state.db` in aio's layout (`aio_kv`, `#[aio_cell]` slices, the home
//! claim; docs/migration/data.md). Feature `compat` adds the rest: aio's
//! protocol v3 on `/ws` for aio pages and tools, an aio-rs app calling an
//! aio app ([`AioLink`]), aio's one-time move of a scattered layout into
//! its home ([`scatter`]) and `#[on_aio]`. Without the feature a `/ws`
//! client must say `x-aio-wire: 2` (or `?wire=2`) and `#[on_aio]` is a
//! build error.
#[cfg(all(feature = "compat", feature = "server", not(target_arch = "wasm32")))]
pub(crate) mod crdt;
#[cfg(not(target_arch = "wasm32"))]
pub mod data;
#[cfg(all(feature = "compat", not(target_arch = "wasm32")))]
pub mod scatter;
#[cfg(all(feature = "compat", feature = "server", not(target_arch = "wasm32")))]
pub(crate) mod wire;
#[cfg(all(feature = "compat", feature = "server", not(target_arch = "wasm32")))]
pub use wire::AioLink;

/// `#[on_aio] fn m` names `m` here, so the hook builds only with the feature.
#[cfg(feature = "compat")]
#[doc(hidden)]
pub const fn on_aio(method: &'static str) -> &'static str {
    method
}
/// Without feature `compat`: `#[on_aio]` says what it needs as the app builds.
#[cfg(not(feature = "compat"))]
#[doc(hidden)]
pub const fn on_aio(_: &'static str) -> &'static str {
    panic!("#[on_aio] needs aio-rs's feature `compat`: aio-rs = {{ features = [\"compat\"] }}")
}
