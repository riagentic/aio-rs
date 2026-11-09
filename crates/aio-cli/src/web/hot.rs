//! `ax dev --web`'s hot-patching of Rust: the page's module set up once
//! per build so a patch can reach into it ([`prepare`]), then per edit the
//! web crate's objects from its incremental build linked against the
//! running module's addresses into a small side module ([`unbind_data`],
//! [`finish`]). The page loads it into its own memory and
//! table, and its `HotFn` calls take the new functions. wasm-bindgen is
//! not run for a patch: what the patch calls of JavaScript it calls
//! through the module wasm-bindgen bound, and a patch that needs anything
//! that module lacks is refused (a full build instead).
//!
//! The layout, after subsecond 0.7's wasm patching (MIT OR Apache-2.0):
//! - Every function of the page's module (but wasm-bindgen's descriptors,
//!   which it takes out) gets a slot in its table, and every JavaScript
//!   import a wrapper function with a slot: a patch's call into the page
//!   is a call of what that slot holds. wasm-bindgen keeps the table's
//!   slots as they are (code holds them as function pointers).
//! - A patch's data goes in the memory a dev build keeps for it (aio-rs's
//!   `AIO_PATCH_ARENA`, zeros the module's file doesn't hold), at an
//!   address [`Hot`] hands out; its functions in table slots past the
//!   module's own.
//! - The page's statics a patch's code names are the page's (the running
//!   state); its constants and the statics it adds are its own.
use super::split::{self, R, Rd, bytes, entries_of, leb, section, sleb};
use crate::hotpatch::{Refused, Rustc, canonical, read_link};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// The static of aio-rs (a dev build's, in a page) patches' data go in.
const ARENA: &str = "AIO_PATCH_ARENA";

/// The symbol flags a patch's link reads (the linking section's).
const WEAK: u32 = 0x1;
const LOCAL: u32 = 0x2;
const HIDDEN: u32 = 0x4;
const UNDEFINED: u32 = 0x10;
const EXPLICIT_NAME: u32 = 0x40;
const ABSOLUTE: u32 = 0x200;

/// A static's address in code and data: unsigned and signed LEB (padded
/// to 5 bytes), 4 bytes.
const ADDR_LEB: u8 = 3;
const ADDR_SLEB: u8 = 4;
const ADDR_I32: u8 = 5;

/// The module wasm-bindgen's intrinsics for its own externref table come
/// from: its own business, never a patch's.
const XFORM: &str = "__wbindgen_externref_xform__";

/// What a generic import's shim calls: wasm-bindgen reads its type there
/// and makes the import it calls instead.
const GENERIC: &str = "__wbindgen_describe_generic_import";

/// An import wasm-bindgen runs only while it binds (its descriptors): no
/// page has it.
fn is_describe(field: &str) -> bool {
    field.starts_with("__wbindgen_describe")
}

/// What a patch is linked against: the page's module as [`prepare`] set it up.
#[derive(Debug, Default, PartialEq)]
pub struct Base {
    /// Each function with a slot, by its symbol: its slots and its type.
    fns: HashMap<String, (Vec<u32>, Vec<u8>)>,
    /// The generic imports' shims among them (wasm-bindgen rewrote them).
    shims: HashSet<String>,
    /// Each JavaScript import's wrapper: its slot and type, by module and name.
    imports: HashMap<(String, String), (u32, Vec<u8>)>,
    /// Each static's address, by its symbol (a name at two addresses left out).
    data: HashMap<String, u32>,
    /// The free memory patches' data go in: its start and end.
    pub arena: (u32, u32),
    /// The table's size: patches' slots go past it.
    pub table: u32,
}

impl Base {
    /// Whether the page has function `name` to call.
    pub fn has_fn(&self, name: &str) -> bool {
        self.fns.contains_key(name)
    }

    /// A static's address in the page's memory.
    pub fn data(&self, name: &str) -> Option<u32> {
        self.data.get(name).copied()
    }
}

/// An import: its module, name, kind and what follows the kind.
struct Import<'a> {
    module: &'a str,
    field: &'a str,
    kind: u8,
    desc: &'a [u8],
}

fn imports(payload: &[u8]) -> R<Vec<Import<'_>>> {
    let mut s = Rd::new(payload);
    (0..s.u32()?)
        .map(|_| {
            let (module, field, kind) = (s.name()?, s.name()?, s.byte()?);
            let at = s.p;
            match kind {
                0 => _ = s.u32()?,
                1 => {
                    s.byte()?;
                    split::limits(&mut s)?;
                }
                2 => split::limits(&mut s)?,
                3 => _ = s.take(2)?,
                k => return Err(format!("wasm: import kind {k} is not patched")),
            }
            Ok(Import { module, field, kind, desc: s.since(at) })
        })
        .collect()
}

fn import(o: &mut Vec<u8>, module: &str, field: &str, kind: u8, desc: &[u8]) {
    bytes(o, module.as_bytes());
    bytes(o, field.as_bytes());
    o.push(kind);
    o.extend_from_slice(desc);
}

/// Each function type's bytes (`0x60`, its params, its results).
fn types(payload: &[u8]) -> R<Vec<&[u8]>> {
    let mut s = Rd::new(payload);
    (0..s.u32()?)
        .map(|_| {
            let at = s.p;
            if s.byte()? != 0x60 {
                return Err("wasm: a type that is not a function's".into());
            }
            for _ in 0..2 {
                let n = s.u32()? as usize;
                s.take(n)?;
            }
            Ok(s.since(at))
        })
        .collect()
}

/// A vector section's entries after its count, and the count.
fn entries(payload: &[u8]) -> R<(u32, &[u8])> {
    let mut s = Rd::new(payload);
    let n = s.u32()?;
    Ok((n, &payload[s.p..]))
}

/// Each data segment's address (an active one's, at a constant) and size.
fn segment_addresses(payload: &[u8]) -> R<Vec<(Option<u32>, u32)>> {
    let mut s = Rd::new(payload);
    let mut out = Vec::new();
    for _ in 0..s.u32()? {
        let flags = s.u32()?;
        if flags == 2 {
            s.u32()?;
        }
        let at = match flags {
            1 => None,
            _ => {
                let from = s.p;
                split::expr(&mut s)?;
                let mut e = Rd::new(&payload[from..s.p]);
                (e.byte()? == 0x41).then(|| e.i32()).transpose()?.map(|v| v as u32)
            }
        };
        let n = s.u32()?;
        s.take(n as usize)?;
        out.push((at, n));
    }
    Ok(out)
}

/// A symbol of a linking section's table: its byte span in the table.
#[derive(Debug)]
struct Symbol<'a> {
    kind: u8,
    flags: u32,
    name: Option<&'a str>,
    /// Functions, globals, tables, tags: the index; data: the segment.
    index: u32,
    /// Defined data: offset and size.
    at: Option<(u32, u32)>,
    span: (usize, usize),
}

/// A linking section: its version's byte, then each subsection as
/// (kind, payload); the symbol table's symbols, read.
struct Linking<'a> {
    subs: Vec<(u8, &'a [u8])>,
    symbols: Vec<Symbol<'a>>,
    /// The segment info's names, by segment.
    segments: Vec<&'a str>,
}

fn linking(payload: &[u8]) -> R<Linking<'_>> {
    let mut s = Rd::new(payload);
    if s.u32()? != 2 {
        return Err("the linking section is not version 2".into());
    }
    let mut l = Linking { subs: Vec::new(), symbols: Vec::new(), segments: Vec::new() };
    while !s.done() {
        let kind = s.byte()?;
        let sub = s.vec()?;
        l.subs.push((kind, sub));
        let mut t = Rd::new(sub);
        match kind {
            8 => {
                for _ in 0..t.u32()? {
                    let from = t.p;
                    let (kind, flags) = (t.byte()?, t.u32()?);
                    let defined = flags & UNDEFINED == 0;
                    let mut sym = Symbol { kind, flags, name: None, index: 0, at: None, span: (0, 0) };
                    match kind {
                        0 | 2 | 4 | 5 => {
                            sym.index = t.u32()?;
                            if defined || flags & EXPLICIT_NAME != 0 {
                                sym.name = Some(t.name()?);
                            }
                        }
                        1 => {
                            sym.name = Some(t.name()?);
                            if defined {
                                sym.index = t.u32()?;
                                sym.at = Some((t.u32()?, t.u32()?));
                            }
                        }
                        3 => sym.index = t.u32()?,
                        k => return Err(format!("symbol kind {k} is not patched")),
                    }
                    sym.span = (from, t.p);
                    l.symbols.push(sym);
                }
            }
            5 => {
                for _ in 0..t.u32()? {
                    l.segments.push(t.name()?);
                    t.u32()?;
                    t.u32()?;
                }
            }
            _ => {}
        }
    }
    Ok(l)
}

/// A custom section of `name`'s payload, after its name.
fn custom<'a>(m: &split::Module<'a>, name: &str) -> Option<&'a [u8]> {
    let p = m.sections.iter().find(|s| s.0 == 0 && s.2 == name)?.1;
    let mut s = Rd::new(p);
    s.name().ok()?;
    Some(&p[s.p..])
}

fn payload<'a>(m: &split::Module<'a>, id: u8) -> &'a [u8] {
    m.sections.iter().find(|s| s.0 == id).map_or(&[0][..], |s| s.1)
}

/// A table's or memory's first limit (its initial size).
fn initial(limits: &[u8]) -> R<u32> {
    let mut s = Rd::new(limits);
    if matches!(s.b.first(), Some(0x70 | 0x6f)) {
        s.byte()?;
    }
    s.byte()?;
    s.u32()
}

/// An active element segment of table 0 at `at`.
fn element(o: &mut Vec<u8>, at: u32, funcs: &[u32]) {
    o.extend([0x00, 0x41]);
    sleb(o, at as i32);
    o.push(0x0b);
    leb(o, funcs.len() as u32);
    funcs.iter().for_each(|&f| leb(o, f));
}

/// Where a section goes among the known ones: tags before globals, the
/// data count before the code (pure).
fn order(id: u8) -> u8 {
    match id {
        13 => 6,
        6..=9 => id + 1,
        12 => 11,
        10 | 11 => id + 2,
        _ => id,
    }
}

/// The page's module `pre` (the linker's, with `--emit-relocs`, before
/// wasm-bindgen) set up for patches, and what a patch is linked against:
/// every function but wasm-bindgen's descriptors (and what reaches them)
/// in a slot of a growable table, a wrapper in a slot for each JavaScript
/// import (wasm-bindgen rewrites what it calls, so a patch calling it
/// calls the import as bound), the table and the stack pointer exported
/// (`__aio_table`, `__aio_sp`), the relocations dropped (pure).
pub fn prepare(pre: &[u8]) -> R<(Vec<u8>, Base)> {
    let m = split::parse(pre)?;
    let link = linking(custom(&m, "linking").ok_or("no linking section (--emit-relocs)")?)?;
    let types = types(payload(&m, 1))?;
    let imports = imports(payload(&m, 2))?;
    let fn_imports: Vec<&Import> = imports.iter().filter(|i| i.kind == 0).collect();
    let n = m.types.len();
    // Who calls whom, backwards: what reaches a descriptor is wasm-bindgen's to drop.
    let mut callers: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, (_, body)) in m.bodies.iter().enumerate() {
        split::rewrite(body, &mut |c| {
            if let Some(v) = callers.get_mut(c as usize) {
                v.push(m.imported + i);
            }
            c
        })?;
    }
    // A generic import's shim (a cast's, `wbg_cast`) wasm-bindgen keeps,
    // rewritten to call the import it makes: it is a function of the page.
    let marker = (0..fn_imports.len()).find(|&k| fn_imports[k].field == GENERIC);
    let shims: HashSet<usize> = marker.map(|k| callers[k].iter().copied().collect()).unwrap_or_default();
    let shim = |f: usize| shims.contains(&f);
    let mut describes = vec![false; n];
    let mut stack: Vec<usize> = (0..fn_imports.len()).filter(|&k| is_describe(fn_imports[k].field)).collect();
    while let Some(f) = stack.pop() {
        if !std::mem::replace(&mut describes[f], true) {
            stack.extend(callers[f].iter().filter(|&&c| !shim(c)));
        }
    }
    let [table] = m.tables[..] else { return Err("the module has not one table".into()) };
    if imports.iter().any(|i| i.kind == 1) {
        return Err("the module imports its table".into());
    }
    let first = initial(table)?;
    let tabled: Vec<u32> = (m.imported..n).filter(|&f| !describes[f]).map(|f| f as u32).collect();
    let wrapped: Vec<usize> =
        (0..fn_imports.len()).filter(|&k| !describes[k] && fn_imports[k].module != XFORM).collect();
    let wrappers: Vec<u32> = (0..wrapped.len()).map(|w| (n + w) as u32).collect();
    let size = first + (tabled.len() + wrapped.len()) as u32;
    let sp = link.symbols.iter().find(|s| s.kind == 2 && s.name == Some("__stack_pointer"));
    let sp = sp.ok_or("no __stack_pointer")?.index;

    let mut base = Base { table: size, ..Default::default() };
    let type_of = |f: usize| types.get(m.types[f] as usize).map(|t| t.to_vec()).ok_or("wasm: a type out of range");
    let mut slots: HashMap<u32, Vec<u32>> = HashMap::new();
    for &(t, slot, f) in &m.slots {
        if t == 0 {
            slots.entry(f).or_default().push(slot);
        }
    }
    for (k, &f) in tabled.iter().enumerate() {
        slots.entry(f).or_default().push(first + k as u32);
    }
    for (f, s) in slots {
        if let Some(name) = m.names.get(&f) {
            base.fns.insert(name.to_string(), (s, type_of(f as usize)?));
            if shim(f as usize) {
                base.shims.insert(name.to_string());
            }
        }
    }
    for (w, &k) in wrapped.iter().enumerate() {
        let i = fn_imports[k];
        let slot = first + (tabled.len() + w) as u32;
        base.imports.insert((i.module.into(), i.field.into()), (slot, type_of(k)?));
    }
    // Each static's address; the arena's span.
    let segs = segment_addresses(payload(&m, 11))?;
    let mut twice = HashSet::new();
    for s in link.symbols.iter().filter(|s| s.kind == 1) {
        let (Some(name), Some((off, size))) = (s.name, s.at) else { continue };
        let addr = match s.flags & ABSOLUTE {
            0 => match segs.get(s.index as usize).and_then(|g| g.0) {
                Some(a) => a + off,
                None => continue,
            },
            _ => off,
        };
        if base.data.insert(name.into(), addr).is_some_and(|a| a != addr) {
            twice.insert(name);
        }
        if name == ARENA {
            base.arena = (addr.next_multiple_of(16), addr + size);
        }
    }
    base.data.retain(|n, _| !twice.contains(n.as_str()));
    if base.arena.1 == 0 {
        return Err(format!("no {ARENA} (aio-rs's `dev` feature keeps it)"));
    }

    let mut o = b"\0asm\x01\0\0\0".to_vec();
    for &(id, p, name) in &m.sections {
        match id {
            0 if name == "linking" || name.starts_with("reloc.") => {}
            // Data of zeros only (its statics that start zeroed, the arena)
            // left out: the memory starts zeroed.
            11 if !m.datacount => section(&mut o, 11, &nonzero(p)?),
            3 => {
                let (c, rest) = entries(p)?;
                let mut e = rest.to_vec();
                wrapped.iter().for_each(|&k| leb(&mut e, m.types[k]));
                section(&mut o, 3, &entries_of((c as usize) + wrapped.len(), &e));
            }
            4 => {
                let mut t = vec![1, 0x70, 0x00];
                leb(&mut t, size);
                section(&mut o, 4, &t);
            }
            7 => {
                let (c, rest) = entries(p)?;
                let mut e = rest.to_vec();
                for (name, kind, index) in [("__aio_table", 1, 0), ("__aio_sp", 3, sp)] {
                    bytes(&mut e, name.as_bytes());
                    e.push(kind);
                    leb(&mut e, index);
                }
                section(&mut o, 7, &entries_of(c as usize + 2, &e));
            }
            9 => {
                let (c, rest) = entries(p)?;
                let mut e = rest.to_vec();
                element(&mut e, first, &[tabled.as_slice(), wrappers.as_slice()].concat());
                section(&mut o, 9, &entries_of(c as usize + 1, &e));
            }
            10 => {
                let (c, rest) = entries(p)?;
                let mut e = rest.to_vec();
                for &k in &wrapped {
                    let mut body = vec![0];
                    for l in 0..params(types[m.types[k] as usize])? {
                        body.push(0x20);
                        leb(&mut body, l);
                    }
                    body.push(0x10);
                    leb(&mut body, k as u32);
                    body.push(0x0b);
                    bytes(&mut e, &body);
                }
                section(&mut o, 10, &entries_of(c as usize + wrapped.len(), &e));
            }
            _ => section(&mut o, id, p),
        }
    }
    for id in [7, 9] {
        if !m.sections.iter().any(|s| s.0 == id) {
            return Err(format!("the module has no section {id} (exports, elements) to add to"));
        }
    }
    Ok((o, base))
}

/// A data section without its active segments of zeros (pure).
fn nonzero(payload: &[u8]) -> R<Vec<u8>> {
    let mut s = Rd::new(payload);
    let (mut kept, mut n) = (Vec::new(), 0);
    for _ in 0..s.u32()? {
        let from = s.p;
        let flags = s.u32()?;
        if flags == 2 {
            s.u32()?;
        }
        if flags != 1 {
            split::expr(&mut s)?;
        }
        let data = s.vec()?;
        if flags == 1 || data.iter().any(|&b| b != 0) {
            kept.extend_from_slice(s.since(from));
            n += 1;
        }
    }
    Ok(entries_of(n, &kept))
}

/// How many params a function type takes.
fn params(ty: &[u8]) -> R<u32> {
    Rd::new(&ty[1..]).u32()
}

/// An object of the patch, as its link takes it.
#[derive(Debug, PartialEq)]
pub struct Obj {
    pub bytes: Vec<u8>,
    /// The functions it defines.
    pub fns: Vec<String>,
}

/// Whether a static's name is the same static wherever it is: a global
/// symbol's, or a mangled one's (a local constant's, `vtable.3`, is one of
/// many) (pure).
fn named(flags: u32, name: &str) -> bool {
    flags & LOCAL == 0 || name.starts_with("_R") || name.starts_with("_ZN")
}

/// Whether a data segment holds state (a static that can change, or a
/// thread-local), not a constant.
fn is_state(segment: &str) -> bool {
    (segment.starts_with(".data.") && !segment.starts_with(".data.rel.ro."))
        || [".bss.", ".tdata", ".tbss"].iter().any(|p| segment.starts_with(p))
}

/// One of the web crate's objects (rustc's, from its incremental build)
/// with each static the page has made undefined: the patch's code then
/// uses the page's (its running state, at the address [`finish`] writes),
/// not a fresh copy. Constants and the statics only the patch has stay
/// its own (pure).
pub fn unbind_data(obj: &[u8], base: &Base, lib: bool) -> R<Obj> {
    let m = split::parse(obj)?;
    let link_payload = custom(&m, "linking").ok_or("an object without a linking section")?;
    let link = linking(link_payload)?;
    let mut out = Obj { bytes: Vec::new(), fns: Vec::new() };
    let mut table = Vec::new();
    leb(&mut table, link.symbols.len() as u32);
    let sub = link.subs.iter().find(|s| s.0 == 8).map_or(&[][..], |s| s.1);
    for s in &link.symbols {
        let span = &sub[s.span.0..s.span.1];
        let name = s.name.unwrap_or_default();
        match (s.kind, s.flags & UNDEFINED != 0) {
            // A function the page has: kept by the link (a root, by name,
            // so a local one is made a weak global: local copies of one
            // inlined function are one).
            // A library's function: weak, as another library's copy of it may be linked too.
            (0, false) if lib && s.flags & LOCAL == 0 => {
                table.push(0);
                leb(&mut table, s.flags | WEAK);
                leb(&mut table, s.index);
                bytes(&mut table, name.as_bytes());
                continue;
            }
            (0, false) if !lib && s.flags & LOCAL != 0 && base.fns.contains_key(name) => {
                table.push(0);
                leb(&mut table, (s.flags & !LOCAL) | WEAK);
                leb(&mut table, s.index);
                bytes(&mut table, name.as_bytes());
                out.fns.push(name.into());
                continue;
            }
            (0, false) => out.fns.push(name.into()),
            (1, false)
                if (lib || link.segments.get(s.index as usize).is_some_and(|g| is_state(g)))
                    && named(s.flags, name)
                    && base.data.contains_key(name) =>
            {
                table.push(1);
                leb(&mut table, (s.flags & HIDDEN) | UNDEFINED);
                bytes(&mut table, name.as_bytes());
                continue;
            }
            _ => {}
        }
        table.extend_from_slice(span);
    }
    let mut l = Vec::new();
    leb(&mut l, 2);
    for &(kind, p) in &link.subs {
        l.push(kind);
        bytes(&mut l, if kind == 8 { &table } else { p });
    }
    let mut o = b"\0asm\x01\0\0\0".to_vec();
    for &(id, p, name) in &m.sections {
        match (id, name) {
            (0, "linking") => {
                let mut c = Vec::new();
                bytes(&mut c, b"linking");
                c.extend_from_slice(&l);
                section(&mut o, 0, &c);
            }
            _ => section(&mut o, id, p),
        }
    }
    out.bytes = o;
    Ok(out)
}

/// What a linked patch names that the page lacks: the functions it
/// imports by symbol, the statics it leaves undefined (pure).
pub fn missing(linked: &[u8], base: &Base) -> R<Vec<String>> {
    let m = split::parse(linked)?;
    let fns = imports(payload(&m, 2))?.into_iter().filter(|i| i.kind == 0 && i.module == "env");
    let mut out: Vec<String> = fns.filter(|i| !base.fns.contains_key(i.field)).map(|i| i.field.into()).collect();
    let link = linking(custom(&m, "linking").ok_or("the patch has no linking section (--emit-relocs)")?)?;
    let data = link.symbols.into_iter().filter(|s| s.kind == 1 && s.flags & UNDEFINED != 0);
    out.extend(data.filter_map(|s| s.name).filter(|n| base.data(n).is_none()).map(String::from));
    Ok(out)
}

/// Write `value` at `off` of `section` as relocation type `ty` puts it.
fn put(section: &mut [u8], off: usize, ty: u8, value: u32) -> R<()> {
    let at = section.get_mut(off..off + if ty == ADDR_I32 { 4 } else { 5 }).ok_or("a relocation past its section")?;
    match ty {
        ADDR_I32 => at.copy_from_slice(&value.to_le_bytes()),
        _ => {
            // Padded: 7 bits a byte, the last one's high bits the sign's (a
            // signed one's) or zero.
            let v = if ty == ADDR_SLEB { i64::from(value as i32) } else { i64::from(value) };
            for (i, b) in at.iter_mut().enumerate() {
                *b = ((v >> (7 * i)) & 0x7f) as u8 | if i < 4 { 0x80 } else { 0 };
            }
        }
    }
    Ok(())
}

/// A patch, ready for the page: its module, and where each function the
/// page has moved (each of its slots → the patch's).
#[derive(Debug, PartialEq)]
pub struct Finished {
    pub wasm: Vec<u8>,
    pub map: Vec<(u32, u32)>,
    /// The table size it needs, and where its data end.
    pub table: u32,
    pub data_end: u32,
}

/// The linker's patch module (`--import-memory --import-table
/// --emit-relocs`, its slots from `at`) made a module the page loads as
/// it is: each static of the page it names at the page's address (the
/// linker left it undefined: its relocations are applied here), each call
/// into the page (an undefined function, a JavaScript import) imported
/// from `s` by the slot that holds it, its stack pointer the page's, every
/// function of its own in a slot past the linker's, nothing exported.
/// Refused when it names what the page doesn't have, or calls a function
/// as another type: that change needs a full build (pure).
pub fn finish(linked: &[u8], base: &Base, at: u32) -> R<Finished> {
    let m = split::parse(linked)?;
    let mut payloads: Vec<Vec<u8>> = m.sections.iter().map(|s| s.1.to_vec()).collect();
    let link = linking(custom(&m, "linking").ok_or("the patch has no linking section (--emit-relocs)")?)?;
    for (_, p, name) in m.sections.iter().filter(|s| s.0 == 0 && s.2.starts_with("reloc.")) {
        let mut s = Rd::new(p);
        s.name()?;
        let target = s.u32()? as usize;
        let code_or_data = matches!(m.sections.get(target), Some((10 | 11, ..)));
        let target = payloads.get_mut(target).ok_or("a relocation of no section")?;
        for _ in 0..s.u32()? {
            let (ty, off, sym) = (s.byte()?, s.u32()? as usize, s.u32()?);
            let addend = match ty {
                3..=5 | 8 | 9 | 11 | 14..=17 | 21..=23 | 25 => s.i32()?,
                0..=2 | 6 | 7 | 10 | 12 | 13 | 18..=20 | 24 | 26 => 0,
                t => return Err(format!("relocation type {t} in {name}")),
            };
            let Some(sym) = link.symbols.get(sym as usize) else { return Err("a relocation of no symbol".into()) };
            let addr = matches!(ty, ADDR_LEB | ADDR_SLEB | ADDR_I32);
            if !code_or_data || !addr || sym.kind != 1 || sym.flags & UNDEFINED == 0 {
                continue;
            }
            let name = sym.name.unwrap_or_default();
            let addr = base.data(name).ok_or_else(|| format!("it names {name} (new to the page)"))?;
            put(target, off, ty, addr.wrapping_add_signed(addend))?;
        }
    }
    let types = types(payload(&m, 1))?;
    let imports = imports(payload(&m, 2))?;
    let defined = m.bodies.len() as u32;
    let linker_slots = m.slots.iter().filter(|s| s.0 == 0).map(|s| (s.1 + 1).saturating_sub(at)).max().unwrap_or(0);
    let own = at + linker_slots;
    let table = own + defined;
    let mut global_names = HashMap::new();
    if let Some(p) = custom(&m, "name") {
        let mut s = Rd::new(p);
        while !s.done() {
            let id = s.byte()?;
            let mut sub = Rd::new(s.vec()?);
            if id == 7 {
                for _ in 0..sub.u32()? {
                    let i = sub.u32()?;
                    global_names.insert(sub.name()?, i);
                }
            }
        }
    }
    let sp = global_names.get("__stack_pointer").copied();
    if imports.iter().any(|i| i.kind == 3) || sp.is_some_and(|g| g != 0) {
        return Err("the patch's globals are not as the linker lays them out".into());
    }
    let mut im = Vec::new();
    let mut count = 0;
    for i in &imports {
        count += 1;
        match i.kind {
            // Descriptors only describe: the page's no-ops (`d`).
            0 if is_describe(i.field) => import(&mut im, "d", i.field, 0, i.desc),
            0 => {
                let ty = Rd::new(i.desc).u32()?;
                let found = match i.module {
                    "env" => base.fns.get(i.field).map(|(slots, t)| (slots[0], t)),
                    _ => base.imports.get(&(i.module.to_string(), i.field.to_string())).map(|(s, t)| (*s, t)),
                };
                let (slot, want) = found.ok_or_else(|| format!("it calls {} (new to the page)", i.field))?;
                if types.get(ty as usize).copied() != Some(want.as_slice()) {
                    return Err(format!("it calls {} as another type", i.field));
                }
                import(&mut im, "s", &slot.to_string(), 0, i.desc);
            }
            1 => {
                let mut t = vec![0x70, 0x00];
                leb(&mut t, table);
                import(&mut im, i.module, i.field, 1, &t);
            }
            _ => import(&mut im, i.module, i.field, 2, &[0x00, 0x00]),
        }
    }
    if sp.is_some() {
        import(&mut im, "env", "__stack_pointer", 3, &[0x7f, 0x01]);
        count += 1;
    }
    let first = m.imported as u32;
    let funcs: Vec<u32> = (first..first + defined).collect();
    // A generic import's shim is the page's (wasm-bindgen rewrote it
    // there): the patch's copy calls it by its slot. Another one calls a
    // cast the page doesn't make.
    let fn_imports: Vec<&Import> = imports.iter().filter(|i| i.kind == 0).collect();
    let marker = fn_imports.iter().position(|i| i.field == GENERIC).map(|k| k as u32);
    let shim_slot = |f: u32| m.names.get(&f).filter(|n| base.shims.contains(**n)).and_then(|n| base.fns.get(*n));
    if let Some(code) = m.sections.iter().position(|s| s.0 == 10) {
        let mut s = Rd::new(&payloads[code]);
        let n = s.u32()?;
        let mut e = Vec::new();
        for f in first..first + n {
            let body = s.vec()?;
            let ty = m.types[f as usize];
            match shim_slot(f) {
                Some((slots, _)) => {
                    let mut b = vec![0];
                    for l in 0..params(types[ty as usize])? {
                        b.push(0x20);
                        leb(&mut b, l);
                    }
                    b.push(0x41);
                    sleb(&mut b, slots[0] as i32);
                    b.push(0x11);
                    leb(&mut b, ty);
                    b.extend([0, 0x0b]);
                    bytes(&mut e, &b);
                }
                None => {
                    let mut casts = false;
                    split::rewrite(body, &mut |c| {
                        casts |= Some(c) == marker;
                        c
                    })?;
                    if casts {
                        let name = m.names.get(&f).copied().unwrap_or_default();
                        return Err(format!("{name} casts to or from a type new to the page"));
                    }
                    bytes(&mut e, body);
                }
            }
        }
        payloads[code] = entries_of(n as usize, &e);
    }
    let mut map = Vec::new();
    for &f in funcs.iter().filter(|&&f| shim_slot(f).is_none()) {
        if let Some((slots, _)) = m.names.get(&f).and_then(|n| base.fns.get(*n)) {
            map.extend(slots.iter().map(|&s| (s, own + f - first)));
        }
    }
    map.sort();
    let elems = |p: Option<&[u8]>| -> R<Vec<u8>> {
        let (c, rest) = p.map_or(Ok((0, &[][..])), entries)?;
        let mut e = rest.to_vec();
        element(&mut e, own, &funcs);
        Ok(entries_of(c as usize + 1, &e))
    };
    let mut o = b"\0asm\x01\0\0\0".to_vec();
    let mut elem_done = false;
    for (&(id, _, name), p) in m.sections.iter().zip(&payloads) {
        if !elem_done && id != 0 && order(id) > order(9) {
            section(&mut o, 9, &elems(None)?);
            elem_done = true;
        }
        match id {
            0 if name != "name" => {}
            2 => section(&mut o, 2, &entries_of(count, &im)),
            6 if sp.is_some() => {
                let mut s = Rd::new(p);
                let p = p.as_slice();
                let n = s.u32()?;
                s.take(2)?;
                split::expr(&mut s)?;
                if n > 1 {
                    section(&mut o, 6, &entries_of(n as usize - 1, &p[s.p..]));
                }
            }
            7 => {}
            9 => {
                section(&mut o, 9, &elems(Some(p))?);
                elem_done = true;
            }
            _ => section(&mut o, id, p),
        }
    }
    if !elem_done {
        section(&mut o, 9, &elems(None)?);
    }
    let data_end = segment_addresses(payload(&m, 11))?.iter().map(|&(a, n)| a.unwrap_or(0) + n).max().unwrap_or(0);
    Ok(Finished { wasm: o, map, table, data_end })
}

/// A dev session's hot-patching: the web crate's rustc call (recorded by
/// the build its page runs) replayed on each edit, its objects linked
/// into a patch of that page.
pub struct Hot {
    base: Base,
    rustc: Rustc,
    /// Where a patch is made (its objects, its link): emptied each time.
    dir: PathBuf,
    lld: PathBuf,
    ax: PathBuf,
    /// The next free slot, and the next free byte of the arena.
    table: u32,
    data: u32,
    /// The libraries' members by the symbols they define, read once needed.
    index: Option<HashMap<String, (PathBuf, u64)>>,
}

/// A patch for the page: its module, where each function the page has
/// moved (slot → slot), the table size it needs; its times.
pub struct Patch {
    pub wasm: Vec<u8>,
    pub map: Vec<(u32, u32)>,
    pub table: u32,
    pub compile: Duration,
    pub link: Duration,
}

/// The rust-lld of `rustc`'s toolchain (it ships with rustc).
fn rust_lld(rustc: &Rustc) -> Result<PathBuf, String> {
    let ask = |args: &[&str]| -> Result<String, String> {
        let out = Command::new(&rustc.args[0])
            .args(args)
            .env_clear()
            .envs(rustc.envs.iter().cloned())
            .output()
            .map_err(|e| format!("{}: {e}", rustc.args[0]))?;
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let host = ask(&["-vV"])?;
    let host = host.lines().find_map(|l| l.strip_prefix("host: ")).ok_or("rustc -vV names no host")?.to_string();
    let lld = Path::new(ask(&["--print", "sysroot"])?.trim())
        .join("lib/rustlib")
        .join(host)
        .join("bin")
        .join(if cfg!(windows) { "rust-lld.exe" } else { "rust-lld" });
    lld.is_file().then_some(lld.clone()).ok_or_else(|| format!("no {}", lld.display()))
}

impl Hot {
    /// The session on the page `base` (as [`prepare`] made it) and the
    /// web crate's call `record`; `ax` is the linker that records a link.
    pub fn new(base: Base, record: &Path, dir: PathBuf, ax: PathBuf) -> Result<Hot, String> {
        let text = std::fs::read_to_string(record).map_err(|e| format!("{}: {e}", record.display()))?;
        let rustc = Rustc::from_json(&text).ok_or_else(|| format!("{}: not a recorded call", record.display()))?;
        let lld = rust_lld(&rustc)?;
        Ok(Hot { table: base.table, data: base.arena.0, base, rustc, dir, lld, ax, index: None })
    }

    /// The patch of `changed` (each the web crate's source): the crate
    /// compiled as the build compiles it, linked against the page.
    /// Refused as rustc's errors ([`Refused::Code`]) or as what needs a
    /// full build ([`Refused::Rebuild`]): another crate's file, a call the
    /// page can't take, a full arena.
    pub fn patch(&mut self, changed: &[PathBuf]) -> Result<Patch, Refused> {
        let started = Instant::now();
        let sources = self.rustc.sources();
        let crate_dir = canonical(&self.rustc.cwd);
        // (A file gone is an editor's temporary copy, renamed over the real one.)
        let own = |f: &PathBuf| {
            let c = canonical(f);
            if !c.exists() {
                return true;
            }
            sources.contains(&c) || c.extension().is_some_and(|x| x == "rs") && c.starts_with(&crate_dir)
        };
        if let Some(f) = changed.iter().find(|f| !own(f)) {
            return Err(Refused::Rebuild(format!("{} is not the web crate's own source", f.display())));
        }
        let fail = |e: &dyn std::fmt::Display| Refused::Rebuild(format!("hot-patch: {e}"));
        let out = self.dir.join("out");
        _ = std::fs::remove_dir_all(&self.dir);
        std::fs::create_dir_all(&out).map_err(|e| fail(&e))?;
        // The crate's rustc call, its outputs here, its objects kept, the
        // link recorded instead of run.
        let mut call = self.rustc.clone();
        let at = call.args.iter().position(|a| a == "--out-dir").ok_or_else(|| fail(&"no --out-dir"))?;
        call.args[at + 1] = out.display().to_string();
        call.args.extend(["-Csave-temps".into(), format!("-Clinker={}", self.ax.display())]);
        let link_file = self.dir.join("link.json");
        call.replay(Some(&link_file))?;
        let compile = started.elapsed();
        let started = Instant::now();
        let link = read_link(&link_file).map_err(|e| fail(&e))?;
        let objects = link.args.iter().filter(|a| a.ends_with(".o"));
        let objects: Vec<Obj> = objects
            .map(|o| std::fs::read(o).map_err(|e| format!("{o}: {e}")).and_then(|b| unbind_data(&b, &self.base, false)))
            .collect::<Result<_, _>>()
            .map_err(|e| fail(&e))?;
        // What the page's linker dropped (code it had inlined, a library
        // function nothing called) the patch takes from the libraries.
        let rlibs: Vec<&str> = link.args.iter().filter(|a| a.ends_with(".rlib")).map(String::as_str).collect();
        let mut libs: Vec<Obj> = Vec::new();
        let mut taken = HashSet::new();
        let linked = loop {
            let linked = self.link(&objects, &libs)?;
            let missing = missing(&linked, &self.base).map_err(|e| fail(&e))?;
            if missing.is_empty() {
                break linked;
            }
            let index = self.index.get_or_insert_with(|| index(&rlibs));
            let mut added = false;
            for name in &missing {
                let (lib, at) = index.get(name).ok_or_else(|| fail(&format!("it calls {name} (new to the page)")))?;
                if taken.insert((lib.clone(), *at)) {
                    let obj = member(lib, *at).and_then(|b| unbind_data(&b, &self.base, true));
                    libs.push(obj.map_err(|e| fail(&format!("{}: {e}", lib.display())))?);
                    added = true;
                }
            }
            if !added {
                return Err(fail(&format!("{} stays undefined", missing[0])));
            }
        };
        let f = finish(&linked, &self.base, self.table).map_err(|e| fail(&e))?;
        if f.data_end > self.base.arena.1 {
            return Err(fail(&"the patches' data filled the memory kept for them"));
        }
        _ = std::fs::remove_dir_all(&self.dir);
        (self.table, self.data) = (f.table, f.data_end.next_multiple_of(16).max(self.data));
        Ok(Patch { wasm: f.wasm, map: f.map, table: f.table, compile, link: started.elapsed() })
    }

    /// The crate's objects and the library members `libs` linked into a
    /// patch module at the next free slot and byte, the functions the page
    /// has kept (the roots: `HotFn` reaches them by the page's slots).
    fn link(&self, objects: &[Obj], libs: &[Obj]) -> Result<Vec<u8>, Refused> {
        let fail = |e: &dyn std::fmt::Display| Refused::Rebuild(format!("hot-patch: {e}"));
        let mut args: Vec<String> = ["-flavor", "wasm", "--no-entry", "--import-memory", "--import-table"]
            .into_iter()
            .chain(["--allow-undefined", "--gc-sections", "--strip-debug", "--no-stack-first", "--emit-relocs"])
            .chain(["--no-demangle"])
            .map(String::from)
            .collect();
        args.extend([format!("--table-base={}", self.table), format!("--global-base={}", self.data)]);
        let roots = objects.iter().flat_map(|o| &o.fns).filter(|f| self.base.has_fn(f));
        let rsp: String = roots.map(|f| format!("--export-if-defined={f}\n")).collect();
        let write = |name: &str, b: &[u8]| -> Result<String, Refused> {
            let p = self.dir.join(name);
            std::fs::write(&p, b).map_err(|e| fail(&e))?;
            Ok(p.display().to_string())
        };
        args.push(format!("@{}", write("roots.rsp", rsp.as_bytes())?));
        for (i, o) in objects.iter().chain(libs).enumerate() {
            args.push(write(&format!("{i}.o"), &o.bytes)?);
        }
        let linked = self.dir.join("patch.wasm");
        args.extend(["-o".into(), linked.display().to_string()]);
        let ran = Command::new(&self.lld).args(&args).output().map_err(|e| fail(&e))?;
        if !ran.status.success() {
            return Err(fail(&String::from_utf8_lossy(&ran.stderr)));
        }
        std::fs::read(&linked).map_err(|e| fail(&e))
    }
}

/// The members of the libraries `rlibs` by each symbol they define (the
/// first library's, as the linker takes them): its library and where the
/// member starts, from each archive's GNU symbol table. A library this
/// can't read is left out (a patch needing it is refused).
fn index(rlibs: &[&str]) -> HashMap<String, (PathBuf, u64)> {
    let mut out = HashMap::new();
    for lib in rlibs {
        let Ok(table) = member(Path::new(lib), 8).map(|b| symbol_table(&b)) else { continue };
        for (name, at) in table {
            out.entry(name).or_insert_with(|| (PathBuf::from(lib), at));
        }
    }
    out
}

/// A GNU archive's symbol table (the `/` member's data, as [`member`]
/// reads it): each name and its member's offset (pure).
fn symbol_table(b: &[u8]) -> Vec<(String, u64)> {
    let be = |i: usize| b.get(i..i + 4).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]));
    let Some(n) = be(0) else { return vec![] };
    let names = b.get(4 + 4 * n as usize..).unwrap_or_default().split(|&c| c == 0);
    (0..n as usize)
        .zip(names)
        .filter_map(|(i, name)| Some((String::from_utf8_lossy(name).into_owned(), u64::from(be(4 + 4 * i)?))))
        .collect()
}

/// The data of the archive member whose header is at `at` of `lib`.
fn member(lib: &Path, at: u64) -> R<Vec<u8>> {
    use std::io::{Read, Seek};
    let fail = |e: std::io::Error| format!("{}: {e}", lib.display());
    let mut f = std::fs::File::open(lib).map_err(fail)?;
    f.seek(std::io::SeekFrom::Start(at)).map_err(fail)?;
    let mut head = [0u8; 60];
    f.read_exact(&mut head).map_err(fail)?;
    let size: u64 = std::str::from_utf8(&head[48..58])
        .ok()
        .and_then(|t| t.trim().parse().ok())
        .ok_or("an archive member's size")?;
    let mut data = vec![0; size as usize];
    f.read_exact(&mut data).map_err(fail)?;
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A module (or an object) of these sections.
    fn module(sections: &[(u8, Vec<u8>)]) -> Vec<u8> {
        let mut o = b"\0asm\x01\0\0\0".to_vec();
        for (id, p) in sections {
            section(&mut o, *id, p);
        }
        o
    }

    fn custom(name: &str, body: &[u8]) -> (u8, Vec<u8>) {
        let mut p = Vec::new();
        bytes(&mut p, name.as_bytes());
        p.extend_from_slice(body);
        (0, p)
    }

    /// A vector: its count, then its items.
    fn items(v: &[Vec<u8>]) -> Vec<u8> {
        entries_of(v.len(), &v.concat())
    }

    fn name(s: &str) -> Vec<u8> {
        let mut o = Vec::new();
        bytes(&mut o, s.as_bytes());
        o
    }

    fn u(v: u32) -> Vec<u8> {
        let mut o = Vec::new();
        leb(&mut o, v);
        o
    }

    /// A body: no locals, `code`, `end`.
    fn body(code: &[u8]) -> Vec<u8> {
        let mut b = vec![0];
        b.extend_from_slice(code);
        b.push(0x0b);
        name_bytes(&b)
    }

    fn name_bytes(b: &[u8]) -> Vec<u8> {
        let mut o = Vec::new();
        bytes(&mut o, b);
        o
    }

    fn import(module: &str, field: &str, kind: u8, desc: &[u8]) -> Vec<u8> {
        [name(module), name(field), vec![kind], desc.to_vec()].concat()
    }

    /// The name section: functions' names, globals' names.
    fn names(funcs: &[(u32, &str)], globals: &[(u32, &str)]) -> (u8, Vec<u8>) {
        let map = |v: &[(u32, &str)]| items(&v.iter().map(|(i, n)| [u(*i), name(n)].concat()).collect::<Vec<_>>());
        let mut b = vec![1];
        b.extend(name_bytes(&map(funcs)));
        b.push(7);
        b.extend(name_bytes(&map(globals)));
        custom("name", &b)
    }

    fn sym_fn(flags: u32, index: u32, n: &str) -> Vec<u8> {
        [vec![0], u(flags), u(index), name(n)].concat()
    }

    fn sym_data(flags: u32, n: &str, at: Option<(u32, u32, u32)>) -> Vec<u8> {
        let mut o = [vec![1], u(flags), name(n)].concat();
        if let Some((seg, off, size)) = at {
            o.extend([u(seg), u(off), u(size)].concat());
        }
        o
    }

    fn sym_global(index: u32, n: &str) -> Vec<u8> {
        [vec![2], u(0), u(index), name(n)].concat()
    }

    /// A linking section: its symbols, and its segments' names.
    fn link(symbols: &[Vec<u8>], segments: &[&str]) -> (u8, Vec<u8>) {
        let mut b = u(2);
        b.push(8);
        b.extend(name_bytes(&items(symbols)));
        if !segments.is_empty() {
            b.push(5);
            let segs: Vec<Vec<u8>> = segments.iter().map(|s| [name(s), u(0), u(0)].concat()).collect();
            b.extend(name_bytes(&items(&segs)));
        }
        custom("linking", &b)
    }

    fn call(f: u32) -> Vec<u8> {
        [vec![0x10], u(f)].concat()
    }

    /// The page's module before wasm-bindgen: imports 0 `wbg.log`, 1 the
    /// descriptor, 2 the generic import's marker; 3 `app` (in the table
    /// at 1), 4 a descriptor (calls 1), 5 a cast's shim (calls 4 and 2),
    /// 6 calls the shim; a static at 1024, the arena's zeros at 2048.
    fn page() -> Vec<u8> {
        let (t_i32, t_none) = (vec![0x60, 1, 0x7f, 0], vec![0x60, 0, 0]);
        let ph = "__wbindgen_placeholder__";
        module(&[
            (1, items(&[t_i32, t_none])),
            (
                2,
                items(&[
                    import("wbg", "__wbg_log_1", 0, &[0]),
                    import(ph, "__wbindgen_describe", 0, &[0]),
                    import(ph, GENERIC, 0, &[0]),
                ]),
            ),
            (3, items(&[u(1), u(1), u(1), u(1)])),
            (4, items(&[vec![0x70, 0x01, 2, 2]])),
            (5, items(&[vec![0, 1]])),
            (6, items(&[vec![0x7f, 1, 0x41, 0x80, 0x80, 0x04, 0x0b]])),
            (7, items(&[[name("memory"), vec![2, 0]].concat()])),
            (9, items(&[vec![0, 0x41, 1, 0x0b, 1, 3]])),
            (
                10,
                items(&[
                    body(&call(6)),
                    body(&[vec![0x41, 1], call(1)].concat()),
                    body(&[call(4), vec![0x41, 0], call(2)].concat()),
                    body(&call(5)),
                ]),
            ),
            (
                11,
                items(&[
                    [vec![0, 0x41, 0x80, 0x08, 0x0b], name_bytes(&[7, 0, 0, 0])].concat(),
                    [vec![0, 0x41, 0x80, 0x10, 0x0b], name_bytes(&[0; 8])].concat(),
                ]),
            ),
            link(
                &[
                    sym_fn(0, 3, "app"),
                    sym_fn(0, 4, "describe_it"),
                    sym_fn(0, 5, "shim"),
                    sym_fn(0, 6, "uses_shim"),
                    sym_global(0, "__stack_pointer"),
                    sym_data(0, "_RNvCs1_3app5STATE", Some((0, 0, 4))),
                    sym_data(0, ARENA, Some((1, 0, 8))),
                ],
                &[],
            ),
            names(&[(3, "app"), (4, "describe_it"), (5, "shim"), (6, "uses_shim")], &[(0, "__stack_pointer")]),
        ])
    }

    #[test]
    fn the_page_gives_each_function_but_the_descriptors_and_each_import_a_slot() {
        let (wasm, base) = prepare(&page()).unwrap();
        // The table: 2 slots, then app, the shim, its caller, the import's wrapper.
        let slots = |n: &str| base.fns.get(n).map(|f| f.0.clone());
        assert_eq!(slots("app"), Some(vec![1, 2]), "its linker's slot and one of its own");
        assert_eq!((slots("shim"), slots("uses_shim"), slots("describe_it")), (Some(vec![3]), Some(vec![4]), None));
        assert_eq!(base.shims, HashSet::from(["shim".to_string()]));
        let wbg = |m: &str, f: &str| base.imports.get(&(m.to_string(), f.to_string())).map(|i| i.0);
        assert_eq!((wbg("wbg", "__wbg_log_1"), wbg("__wbindgen_placeholder__", GENERIC)), (Some(5), None));
        assert_eq!((base.table, base.arena), (6, (2048, 2056)));
        assert_eq!(base.data("_RNvCs1_3app5STATE"), Some(1024));
        // The module: a growable table of 6, the slots as an element segment,
        // the wrapper (local 0 to import 0), the exports, no relocations, no zeros.
        let m = split::parse(&wasm).unwrap();
        assert_eq!(m.tables, [&[0x70, 0x00, 6][..]]);
        assert_eq!(m.slots, [(0, 1, 3), (0, 2, 3), (0, 3, 5), (0, 4, 6), (0, 5, 7)]);
        assert_eq!(m.bodies[4].1, [0, 0x20, 0, 0x10, 0, 0x0b]);
        assert_eq!(m.exports, [("memory", 2, 0), ("__aio_table", 1, 0), ("__aio_sp", 3, 0)]);
        assert!(m.sections.iter().all(|s| s.2 != "linking"));
        assert_eq!(segment_addresses(payload(&m, 11)).unwrap(), [(Some(1024), 4)]);
        // Without the linker's symbols there is nothing to link against.
        let bare = module(&[(1, items(&[]))]);
        assert!(prepare(&bare).unwrap_err().contains("--emit-relocs"));
    }

    fn base() -> Base {
        prepare(&page()).unwrap().1
    }

    /// An object: one function, statics in `segments`, symbols `symbols`.
    fn object(symbols: &[Vec<u8>], segments: &[&str]) -> Vec<u8> {
        module(&[
            (1, items(&[vec![0x60, 0, 0]])),
            (3, items(&[u(0)])),
            (10, items(&[body(&[])])),
            link(symbols, segments),
        ])
    }

    fn symbols_of(obj: &[u8]) -> Vec<(u8, u32, String)> {
        let m = split::parse(obj).unwrap();
        let l = linking(custom_of(&m, "linking")).unwrap();
        l.symbols.iter().map(|s| (s.kind, s.flags, s.name.unwrap_or_default().to_string())).collect()
    }

    fn custom_of<'a>(m: &split::Module<'a>, n: &str) -> &'a [u8] {
        super::custom(m, n).unwrap()
    }

    #[test]
    fn an_object_takes_the_pages_state_keeps_its_constants_and_roots_what_the_page_calls() {
        let state = "_RNvCs1_3app5STATE";
        let symbols = [
            sym_fn(LOCAL, 0, "app"),
            sym_fn(LOCAL, 0, "new_fn"),
            sym_data(HIDDEN, state, Some((0, 0, 4))),
            sym_data(0, "_RNvCs1_3app4MINE", Some((0, 4, 4))),
            sym_data(LOCAL, "_RNvCs1_3app5TABLE", Some((1, 0, 4))),
        ];
        let segs = [&format!(".data.{state}")[..], ".rodata._RNvCs1_3app5TABLE"];
        let obj = unbind_data(&object(&symbols, &segs), &base(), false).unwrap();
        assert_eq!(obj.fns, ["app", "new_fn"]);
        let s = symbols_of(&obj.bytes);
        // `app` the page has: a weak global (rooted by name); the new one stays local.
        assert_eq!((s[0].1, s[1].1), (WEAK, LOCAL));
        // The page's static: undefined (its address written after the link).
        assert_eq!(s[2], (1, HIDDEN | UNDEFINED, state.to_string()));
        // A static the page lacks, and a constant: the patch's own.
        assert_eq!((s[3].1, s[4].1), (0, LOCAL));
        // A library's member: its constants the page's too, but not one named
        // as many are (`vtable.3`), and its functions weak.
        let symbols = [
            sym_fn(0, 0, "lib_fn"),
            sym_data(LOCAL, state, Some((1, 0, 4))),
            sym_data(LOCAL, "vtable.3", Some((1, 4, 4))),
        ];
        let mut b = base();
        b.data.insert("vtable.3".into(), 99);
        let lib = unbind_data(&object(&symbols, &segs), &b, true).unwrap();
        let s = symbols_of(&lib.bytes);
        assert_eq!((s[0].1, s[1].1, s[2].1), (WEAK, UNDEFINED, LOCAL));
    }

    /// The linker's patch: imports memory, table, `app`, `wbg.log`, the
    /// descriptor (and `extra` functions); its stack pointer; 3 its `app` (stores to the page's
    /// static), 4 its copy of the shim, 5 a function of its own; one slot
    /// of the linker's at `at`.
    fn linked(at: u32, extra: &[Vec<u8>]) -> Vec<u8> {
        let ph = "__wbindgen_placeholder__";
        let store = [vec![0x41], vec![0x80, 0x80, 0x80, 0x80, 0], vec![0x41, 1, 0x36, 2, 0]].concat();
        let mut imports = vec![
            import("env", "memory", 2, &[0, 1]),
            import("env", "__indirect_function_table", 1, &[0x70, 0, 1]),
            import("env", "app", 0, &[1]),
            import("wbg", "__wbg_log_1", 0, &[0]),
            import(ph, "__wbindgen_describe", 0, &[0]),
        ];
        imports.extend(extra.iter().cloned());
        let fns = 3 + extra.len() as u32;
        let elem = [vec![0, 0x41], u(at), vec![0x0b, 1], u(fns + 2)].concat();
        let code = items(&[body(&store), body(&call(4)), body(&[])]);
        // The store's address: in the code section (the 6th), 5 bytes at 4
        // (past the count, the body's size, its locals, `i32.const`).
        let reloc = [u(5), u(1), vec![ADDR_LEB], u(4), u(1), u(8)].concat();
        module(&[
            (1, items(&[vec![0x60, 1, 0x7f, 0], vec![0x60, 0, 0]])),
            (2, items(&imports)),
            (3, items(&[u(1), u(1), u(1)])),
            (6, items(&[vec![0x7f, 1, 0x41, 0, 0x0b]])),
            (9, items(&[elem])),
            (10, code),
            link(&[sym_fn(0, fns, "app"), sym_data(UNDEFINED, "_RNvCs1_3app5STATE", None)], &[]),
            custom("reloc.CODE", &reloc),
            names(&[(fns, "app"), (fns + 1, "shim"), (fns + 2, "mine")], &[(0, "__stack_pointer")]),
        ])
    }

    #[test]
    fn a_patch_calls_the_page_by_slot_writes_its_statics_and_slots_its_functions() {
        let f = finish(&linked(10, &[]), &base(), 10).unwrap();
        let m = split::parse(&f.wasm).unwrap();
        let im = imports(payload(&m, 2)).unwrap();
        let named: Vec<(&str, &str)> = im.iter().map(|i| (i.module, i.field)).collect();
        assert_eq!(
            named,
            [
                ("env", "memory"),
                ("env", "__indirect_function_table"),
                ("s", "1"),
                ("s", "5"),
                ("d", "__wbindgen_describe"),
                ("env", "__stack_pointer")
            ]
        );
        assert_eq!(im[1].desc, [0x70, 0, 14], "the table it needs: one linker's slot, then its 3");
        assert!(m.sections.iter().all(|s| s.0 != 6 && s.0 != 7), "no globals of its own, no exports");
        // The page's static at its address (1024 + 8), padded.
        let code = m.sections.iter().find(|s| s.0 == 10).unwrap().1;
        assert!(code.windows(5).any(|w| w == [0x88, 0x88, 0x80, 0x80, 0]), "{code:?}");
        // The shim calls the page's (slot 3); `app` moved from the page's slots 1 and 2.
        assert_eq!(m.bodies[1].1, [0, 0x41, 3, 0x11, 1, 0, 0x0b]);
        assert_eq!(m.slots.last(), Some(&(0, 13, 5)));
        assert_eq!((f.map, f.table, f.data_end), (vec![(1, 11), (2, 11)], 14, 0));
    }

    #[test]
    fn a_patch_calling_what_the_page_lacks_is_refused() {
        let new = import("wbg", "__wbg_new_import_2", 0, &[1]);
        assert!(finish(&linked(10, &[new]), &base(), 10).unwrap_err().contains("__wbg_new_import_2 (new to the page)"));
        let other_type = import("wbg", "__wbg_log_1", 0, &[1]);
        let mut wasm = linked(10, &[]);
        let at = wasm.windows(12).position(|w| w == b"\x0b__wbg_log_1").unwrap() + 12;
        wasm[at + 1] = 1;
        assert!(finish(&wasm, &base(), 10).unwrap_err().contains("as another type"), "{other_type:?}");
        assert!(
            missing(&linked(10, &[import("env", "gone", 0, &[1])]), &base()).unwrap().contains(&"gone".to_string())
        );
    }

    #[test]
    fn a_relocation_is_written_as_its_type_puts_it() {
        let mut s = [0u8; 6];
        put(&mut s, 1, ADDR_I32, 0x0102_0304).unwrap();
        assert_eq!(s, [0, 4, 3, 2, 1, 0]);
        put(&mut s, 0, ADDR_LEB, 300).unwrap();
        assert_eq!(s[..5], [0xac, 0x82, 0x80, 0x80, 0]);
        put(&mut s, 0, ADDR_SLEB, 0x7fff_ffff).unwrap();
        assert_eq!(s[..5], [0xff, 0xff, 0xff, 0xff, 0x07]);
        assert!(put(&mut s, 3, ADDR_LEB, 1).is_err(), "past its section");
    }

    #[test]
    fn an_archives_symbol_table_names_each_members_offset() {
        let mut t = 2u32.to_be_bytes().to_vec();
        t.extend(8u32.to_be_bytes());
        t.extend(200u32.to_be_bytes());
        t.extend(b"a\0bb\0");
        assert_eq!(symbol_table(&t), [("a".to_string(), 8), ("bb".to_string(), 200)]);
        assert_eq!(symbol_table(&[]), []);
    }
}
