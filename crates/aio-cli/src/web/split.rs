//! Code splitting of a release web build: the code only `aio::lazy!`
//! views reach goes into chunks the page fetches when such a view first
//! shows (aio's `lazy(() => import(…))`).
//!
//! The module `wasm-bindgen` made is split after it. A moved function's
//! body in the main module becomes `unreachable`; its index and its table
//! slots stay. A chunk holds its code, imports the main module's memory,
//! tables and globals, calls the main module's functions through exports,
//! and on instantiation writes its functions into their table slots. What
//! code reaches besides its direct calls (a vtable's function, a static
//! holding a function's address) is read from the linker's relocations
//! (`--emit-relocs`) of the module before `wasm-bindgen`, matched by name:
//! a function moves only if lazy views alone reach it, by any path. Code
//! two lazy views share goes into a common chunk; data stays in the main
//! module.
use std::collections::{HashMap, HashSet};

pub(super) type R<T> = Result<T, String>;

/// Whether `name` (as the linker has it) is a lazy view's entry: the
/// `fn __aio_lazy` of an `aio::lazy!` (not a closure in it) (pure).
fn is_entry(name: &str) -> bool {
    format!("{:#}", rustc_demangle::demangle(name)).ends_with("::__aio_lazy")
}

/// The main module and its chunks, by file name.
pub struct Split {
    pub main: Vec<u8>,
    pub chunks: Vec<(String, Vec<u8>)>,
}

pub(super) struct Rd<'a> {
    pub(super) b: &'a [u8],
    pub(super) p: usize,
}

impl<'a> Rd<'a> {
    pub(super) fn new(b: &'a [u8]) -> Self {
        Self { b, p: 0 }
    }

    pub(super) fn done(&self) -> bool {
        self.p >= self.b.len()
    }

    pub(super) fn byte(&mut self) -> R<u8> {
        let c = *self.b.get(self.p).ok_or("wasm: cut short")?;
        self.p += 1;
        Ok(c)
    }

    pub(super) fn u32(&mut self) -> R<u32> {
        let (mut v, mut shift) = (0u64, 0);
        loop {
            let c = self.byte()?;
            v |= u64::from(c & 0x7f) << shift;
            if c & 0x80 == 0 {
                return u32::try_from(v).map_err(|_| "wasm: a number too large".into());
            }
            shift += 7;
            if shift > 28 {
                return Err("wasm: a number too long".into());
            }
        }
    }

    pub(super) fn i32(&mut self) -> R<i32> {
        let (mut v, mut shift) = (0i64, 0);
        loop {
            let c = self.byte()?;
            v |= i64::from(c & 0x7f) << shift;
            shift += 7;
            if c & 0x80 == 0 {
                if shift < 64 && c & 0x40 != 0 {
                    v |= -1 << shift;
                }
                return i32::try_from(v).map_err(|_| "wasm: a number too large".into());
            }
            if shift > 28 {
                return Err("wasm: a number too long".into());
            }
        }
    }

    /// Any LEB number, signed or not, up to 64 bits.
    pub(super) fn skip(&mut self) -> R<()> {
        for _ in 0..10 {
            if self.byte()? & 0x80 == 0 {
                return Ok(());
            }
        }
        Err("wasm: a number too long".into())
    }

    pub(super) fn take(&mut self, n: usize) -> R<&'a [u8]> {
        let s = self.b.get(self.p..self.p + n).ok_or("wasm: cut short")?;
        self.p += n;
        Ok(s)
    }

    pub(super) fn vec(&mut self) -> R<&'a [u8]> {
        let n = self.u32()? as usize;
        self.take(n)
    }

    pub(super) fn name(&mut self) -> R<&'a str> {
        std::str::from_utf8(self.vec()?).map_err(|_| "wasm: a name not UTF-8".into())
    }

    /// The bytes read since `from`.
    pub(super) fn since(&self, from: usize) -> &'a [u8] {
        &self.b[from..self.p]
    }
}

pub(super) fn leb(o: &mut Vec<u8>, mut v: u32) {
    loop {
        let c = (v & 0x7f) as u8;
        v >>= 7;
        o.push(if v == 0 { c } else { c | 0x80 });
        if v == 0 {
            return;
        }
    }
}

pub(super) fn sleb(o: &mut Vec<u8>, mut v: i32) {
    loop {
        let c = (v & 0x7f) as u8;
        v >>= 7;
        let last = (v == 0 && c & 0x40 == 0) || (v == -1 && c & 0x40 != 0);
        o.push(if last { c } else { c | 0x80 });
        if last {
            return;
        }
    }
}

pub(super) fn bytes(o: &mut Vec<u8>, b: &[u8]) {
    leb(o, b.len() as u32);
    o.extend_from_slice(b);
}

pub(super) fn section(o: &mut Vec<u8>, id: u8, payload: &[u8]) {
    o.push(id);
    bytes(o, payload);
}

/// A vector section's payload: its count, then its entries.
pub(super) fn entries_of(n: usize, body: &[u8]) -> Vec<u8> {
    let mut o = Vec::new();
    leb(&mut o, n as u32);
    o.extend_from_slice(body);
    o
}

/// A table's or memory's limits.
pub(super) fn limits(rd: &mut Rd) -> R<()> {
    let flags = rd.byte()?;
    rd.skip()?;
    if flags & 1 != 0 {
        rd.skip()?;
    }
    Ok(())
}

/// A value type: one byte (numbers, `funcref`, `externref`).
fn valtype(rd: &mut Rd) -> R<()> {
    match rd.byte()? {
        0x7b..=0x7f | 0x70 | 0x6f => Ok(()),
        t => Err(format!("wasm: value type {t:#x} is not split")),
    }
}

/// What an instruction names that the split cares about.
enum Op {
    Plain,
    /// A function index, read from `.0` (the reader is past it).
    Func(usize),
    /// A data or element segment (a chunk has none: the function stays).
    Segment,
}

/// A load's or store's alignment and offset.
fn memarg(rd: &mut Rd) -> R<()> {
    let align = rd.u32()?;
    if align & 0x40 != 0 {
        rd.u32()?;
    }
    rd.skip()
}

/// Read one instruction after its opcode `op`.
fn op(rd: &mut Rd, op: u8) -> R<Op> {
    match op {
        0x00 | 0x01 | 0x05 | 0x0a | 0x0b | 0x0f | 0x19 | 0x1a | 0x1b | 0x45..=0xc4 | 0xd1 | 0xd3 | 0xd5 => {}
        0x02..=0x04 | 0x06 => blocktype(rd)?,
        0x07..=0x09 | 0x0c | 0x0d | 0x14 | 0x15 | 0x18 | 0x20..=0x26 | 0xd4 | 0xd6 => _ = rd.u32()?,
        0x0e => {
            for _ in 0..=rd.u32()? {
                rd.u32()?;
            }
        }
        0x10 | 0x12 | 0xd2 => {
            let at = rd.p;
            rd.u32()?;
            return Ok(Op::Func(at));
        }
        0x11 | 0x13 => {
            rd.u32()?;
            rd.u32()?;
        }
        0x1c => {
            for _ in 0..rd.u32()? {
                valtype(rd)?;
            }
        }
        0x1f => {
            blocktype(rd)?;
            for _ in 0..rd.u32()? {
                if rd.byte()? < 2 {
                    rd.u32()?;
                }
                rd.u32()?;
            }
        }
        0x28..=0x3e => memarg(rd)?,
        0x3f | 0x40 => _ = rd.u32()?,
        0x41 | 0x42 | 0xd0 => rd.skip()?,
        0x43 => _ = rd.take(4)?,
        0x44 => _ = rd.take(8)?,
        0xfc => match rd.u32()? {
            0..=7 => {}
            8 | 12 => {
                rd.u32()?;
                rd.u32()?;
                return Ok(Op::Segment);
            }
            9 | 13 => {
                rd.u32()?;
                return Ok(Op::Segment);
            }
            10 | 14 => {
                rd.u32()?;
                rd.u32()?;
            }
            11 | 15..=17 => _ = rd.u32()?,
            n => return Err(format!("wasm: instruction 0xfc {n} is not split")),
        },
        0xfd => match rd.u32()? {
            0..=11 | 92 | 93 => memarg(rd)?,
            12 | 13 => _ = rd.take(16)?,
            21..=34 => _ = rd.byte()?,
            84..=91 => {
                memarg(rd)?;
                rd.byte()?;
            }
            _ => {}
        },
        0xfe => match rd.u32()? {
            3 => _ = rd.byte()?,
            _ => memarg(rd)?,
        },
        n => return Err(format!("wasm: instruction {n:#x} is not split")),
    }
    Ok(Op::Plain)
}

fn blocktype(rd: &mut Rd) -> R<()> {
    match rd.b.get(rd.p) {
        Some(0x40 | 0x7b..=0x7f | 0x70 | 0x6f) => rd.p += 1,
        _ => rd.skip()?,
    }
    Ok(())
}

/// A constant expression, up to its `end`.
pub(super) fn expr(rd: &mut Rd) -> R<Vec<u32>> {
    let mut funcs = Vec::new();
    loop {
        match rd.byte()? {
            0x0b => return Ok(funcs),
            o => {
                if let Op::Func(at) = op(rd, o)? {
                    funcs.push(Rd { b: rd.b, p: at }.u32()?);
                }
            }
        }
    }
}

/// A function body's code: its function indices `f` as `map(f)`; whether
/// it names a segment.
pub(super) fn rewrite(body: &[u8], map: &mut impl FnMut(u32) -> u32) -> R<(Vec<u8>, bool)> {
    let mut rd = Rd::new(body);
    for _ in 0..rd.u32()? {
        rd.u32()?;
        valtype(&mut rd)?;
    }
    let mut out = body[..rd.p].to_vec();
    let mut segment = false;
    while !rd.done() {
        let at = rd.p;
        let o = rd.byte()?;
        match op(&mut rd, o)? {
            Op::Func(i) => {
                out.extend_from_slice(&body[at..i]);
                let f = Rd { b: body, p: i }.u32()?;
                leb(&mut out, map(f));
            }
            Op::Segment => {
                segment = true;
                out.extend_from_slice(rd.since(at));
            }
            Op::Plain => out.extend_from_slice(rd.since(at)),
        }
    }
    Ok((out, segment))
}

/// What a module holds that the split reads.
#[derive(Default)]
pub(super) struct Module<'a> {
    /// Each section: its id, payload and (custom) name.
    pub(super) sections: Vec<(u8, &'a [u8], &'a str)>,
    /// Each function's type, imported ones first.
    pub(super) types: Vec<u32>,
    pub(super) imported: usize,
    /// Each global's, table's and memory's type, as its import or section has it.
    pub(super) globals: Vec<&'a [u8]>,
    pub(super) tables: Vec<&'a [u8]>,
    pub(super) memory: Option<&'a [u8]>,
    /// Whether anything but functions is imported (a chunk imports them by
    /// the main module's own index).
    pub(super) imports_more: bool,
    /// Each defined function's body, and where it starts in the code section.
    pub(super) bodies: Vec<(usize, &'a [u8])>,
    pub(super) exports: Vec<(&'a str, u8, u32)>,
    pub(super) start: Option<u32>,
    /// The table slots of plain element segments: (table, slot, function).
    pub(super) slots: Vec<(u32, u32, u32)>,
    /// The functions other element segments name.
    pub(super) named: Vec<u32>,
    pub(super) names: HashMap<u32, &'a str>,
    pub(super) datacount: bool,
}

pub(super) fn parse(wasm: &[u8]) -> R<Module<'_>> {
    let mut rd = Rd::new(wasm);
    if rd.take(8)? != b"\0asm\x01\0\0\0" {
        return Err("not a wasm module".into());
    }
    let mut m = Module::default();
    while !rd.done() {
        let id = rd.byte()?;
        let payload = rd.vec()?;
        let mut s = Rd::new(payload);
        let name = if id == 0 { s.name()? } else { "" };
        m.sections.push((id, payload, name));
        match id {
            0 if name == "name" => names(&mut s, &mut m.names)?,
            2 => {
                for _ in 0..s.u32()? {
                    s.name()?;
                    s.name()?;
                    let kind = s.byte()?;
                    let at = s.p;
                    match kind {
                        0 => {
                            m.types.push(s.u32()?);
                            m.imported += 1;
                        }
                        1 => {
                            valtype(&mut s)?;
                            limits(&mut s)?;
                            m.tables.push(s.since(at));
                        }
                        2 => {
                            limits(&mut s)?;
                            m.memory = Some(s.since(at));
                        }
                        3 => {
                            s.take(2)?;
                            m.globals.push(s.since(at));
                        }
                        _ => return Err(format!("wasm: import kind {kind} is not split")),
                    }
                    m.imports_more |= kind != 0;
                }
            }
            3 => {
                for _ in 0..s.u32()? {
                    m.types.push(s.u32()?);
                }
            }
            4 => {
                for _ in 0..s.u32()? {
                    let at = s.p;
                    valtype(&mut s)?;
                    limits(&mut s)?;
                    m.tables.push(s.since(at));
                }
            }
            5 => {
                if s.u32()? > 0 {
                    let at = s.p;
                    limits(&mut s)?;
                    m.memory = Some(s.since(at));
                }
            }
            6 => {
                for _ in 0..s.u32()? {
                    let at = s.p;
                    s.take(2)?;
                    m.globals.push(s.since(at));
                    expr(&mut s)?;
                }
            }
            7 => {
                for _ in 0..s.u32()? {
                    m.exports.push((s.name()?, s.byte()?, s.u32()?));
                }
            }
            8 => m.start = Some(s.u32()?),
            9 => elements(&mut s, &mut m)?,
            10 => {
                for _ in 0..s.u32()? {
                    let n = s.u32()? as usize;
                    m.bodies.push((s.p, s.take(n)?));
                }
            }
            12 => m.datacount = true,
            _ => {}
        }
    }
    Ok(m)
}

fn names<'a>(s: &mut Rd<'a>, out: &mut HashMap<u32, &'a str>) -> R<()> {
    while !s.done() {
        let id = s.byte()?;
        let mut sub = Rd::new(s.vec()?);
        if id == 1 {
            for _ in 0..sub.u32()? {
                let i = sub.u32()?;
                out.insert(i, sub.name()?);
            }
        }
    }
    Ok(())
}

fn elements(s: &mut Rd, m: &mut Module) -> R<()> {
    for _ in 0..s.u32()? {
        let flags = s.u32()?;
        let table = if flags & 3 == 2 { s.u32()? } else { 0 };
        // Active: a plain `i32.const` offset gives the slots.
        let offset = match flags & 1 {
            0 => {
                let at = s.p;
                let n = match s.byte()? {
                    0x41 => Some(s.i32()?),
                    _ => None,
                };
                let n = n.filter(|_| s.b.get(s.p) == Some(&0x0b));
                s.p = at;
                expr(s)?;
                n.and_then(|n| u32::try_from(n).ok())
            }
            _ => None,
        };
        if flags & 3 != 0 {
            s.byte()?;
        }
        let funcs: Vec<u32> = match flags & 4 {
            0 => (0..s.u32()?).map(|_| s.u32()).collect::<R<_>>()?,
            _ => (0..s.u32()?).map(|_| expr(s)).collect::<R<Vec<_>>>()?.concat(),
        };
        match (offset, flags & 4) {
            (Some(o), 0) => m.slots.extend(funcs.iter().enumerate().map(|(i, &f)| (table, o + i as u32, f))),
            _ => m.named.extend(funcs),
        }
    }
    Ok(())
}

/// What the module before `wasm-bindgen` says code and data reach besides
/// direct calls, by function name: its relocations.
#[derive(Default)]
struct Refs<'a> {
    /// What each function (by name) and each data symbol reaches.
    from_fn: HashMap<&'a str, Vec<To<'a>>>,
    from_data: Vec<Vec<To<'a>>>,
    /// What data no symbol covers reaches: the main module keeps it.
    kept: Vec<To<'a>>,
    known: HashSet<&'a str>,
}

#[derive(Clone, Copy)]
enum To<'a> {
    Fn(&'a str),
    Data(usize),
}

/// A symbol of the linking section.
enum Sym {
    Fn(u32),
    /// Its segment, offset and size.
    Data(u32, u32, u32),
    Other,
}

fn symbols(link: &[u8]) -> R<Vec<Sym>> {
    let mut s = Rd::new(link);
    if s.u32()? != 2 {
        return Err("the linking section is not version 2".into());
    }
    while !s.done() {
        let kind = s.byte()?;
        let mut sub = Rd::new(s.vec()?);
        if kind != 8 {
            continue;
        }
        let mut out = Vec::new();
        for _ in 0..sub.u32()? {
            let kind = sub.byte()?;
            let flags = sub.u32()?;
            let undefined = flags & 0x10 != 0;
            out.push(match kind {
                0 | 2 | 4 | 5 => {
                    let i = sub.u32()?;
                    if !undefined || flags & 0x40 != 0 {
                        sub.name()?;
                    }
                    if kind == 0 { Sym::Fn(i) } else { Sym::Other }
                }
                1 => {
                    sub.name()?;
                    match undefined {
                        true => Sym::Other,
                        false => Sym::Data(sub.u32()?, sub.u32()?, sub.u32()?),
                    }
                }
                3 => {
                    sub.u32()?;
                    Sym::Other
                }
                k => return Err(format!("symbol kind {k} is not split")),
            });
        }
        return Ok(out);
    }
    Err("no symbol table".into())
}

/// Each data segment's bytes: where they start in the data section, and how many.
pub(super) fn segments(data: &[u8]) -> R<Vec<(usize, usize)>> {
    let mut s = Rd::new(data);
    let mut out = Vec::new();
    for _ in 0..s.u32()? {
        let flags = s.u32()?;
        if flags == 2 {
            s.u32()?;
        }
        if flags != 1 {
            expr(&mut s)?;
        }
        let n = s.u32()? as usize;
        out.push((s.p, n));
        s.take(n)?;
    }
    Ok(out)
}

fn refs<'a>(pre: &Module<'a>) -> R<Refs<'a>> {
    let custom = |n: &str| pre.sections.iter().find(|(id, _, name)| *id == 0 && *name == n).map(|s| s.1);
    let after_name = |p: &'a [u8]| -> R<&'a [u8]> {
        let mut s = Rd::new(p);
        s.name()?;
        Ok(&p[s.p..])
    };
    let link = custom("linking").ok_or("no relocations (--emit-relocs)")?;
    let syms = symbols(after_name(link)?)?;
    let section = |id: u8| pre.sections.iter().find(|s| s.0 == id).map_or(&[][..], |s| s.1);
    let segs = segments(section(11))?;
    // The data symbols, by segment and offset (aliases overlap).
    let data: Vec<(usize, u32, u32, u32)> = syms
        .iter()
        .enumerate()
        .filter_map(|(i, s)| match *s {
            Sym::Data(seg, off, size) => Some((i, seg, off, size)),
            _ => None,
        })
        .collect();
    let covering = |seg: u32, off: u32| -> Vec<usize> {
        data.iter().filter(|d| d.1 == seg && d.2 <= off && off < d.2 + d.3.max(1)).map(|d| d.0).collect()
    };
    let fn_name = |i: u32| pre.names.get(&i).copied();
    let mut r = Refs { from_data: vec![Vec::new(); syms.len()], ..Default::default() };
    r.known = pre.names.values().copied().collect();
    let target = |ty: u8, sym: u32, addend: i32| -> R<Vec<To<'a>>> {
        let s = syms.get(sym as usize).ok_or("a relocation names no symbol")?;
        Ok(match (ty, s) {
            (0 | 1 | 2 | 12 | 18 | 19 | 24 | 26, Sym::Fn(f)) => fn_name(*f).map(To::Fn).into_iter().collect(),
            (_, Sym::Data(seg, off, _)) => {
                let at = off.checked_add_signed(addend).unwrap_or(*off);
                let mut to = vec![To::Data(sym as usize)];
                to.extend(covering(*seg, at).into_iter().map(To::Data));
                to
            }
            _ => Vec::new(),
        })
    };
    for (which, body_of) in [("reloc.CODE", true), ("reloc.DATA", false)] {
        let Some(sec) = custom(which) else { continue };
        let mut s = Rd::new(after_name(sec)?);
        s.u32()?;
        for _ in 0..s.u32()? {
            let ty = s.byte()?;
            let at = s.u32()? as usize;
            let sym = s.u32()?;
            let addend = match ty {
                3 | 4 | 5 | 8 | 9 | 11 | 14..=17 | 21..=23 | 25 => s.i32()?,
                0..=2 | 6 | 7 | 10 | 12 | 13 | 18..=20 | 24 | 26 => 0,
                t => return Err(format!("relocation type {t} is not split")),
            };
            // Direct calls are read from the code itself.
            if ty == 0 {
                continue;
            }
            let to = target(ty, sym, addend)?;
            match body_of {
                true => {
                    let i = pre.bodies.partition_point(|b| b.0 <= at).checked_sub(1);
                    let name = i.and_then(|i| fn_name((pre.imported + i) as u32));
                    match name {
                        Some(n) => r.from_fn.entry(n).or_default().extend(to),
                        None => r.kept.extend(to),
                    }
                }
                false => {
                    let seg = segs.iter().position(|&(o, n)| o <= at && at < o + n);
                    let syms = seg.map(|g| covering(g as u32, (at - segs[g].0) as u32)).unwrap_or_default();
                    match syms.is_empty() {
                        true => r.kept.extend(to),
                        false => syms.into_iter().for_each(|d| r.from_data[d].extend(to.iter().copied())),
                    }
                }
            }
        }
    }
    Ok(r)
}

/// Which chunk each function goes to, from the graph's edges (`edges[n]`,
/// functions first, then data), what the main module keeps (`roots`) and
/// the lazy entries (function indices): `None` stays in the main module,
/// `Some(i)` goes to entry `i`'s chunk, `Some(entries.len())` to the
/// common chunk (more than one entry reaches it); and whether each entry
/// reaches the common chunk (by any path: its chunk loads first) (pure).
fn partition(
    edges: &[Vec<usize>],
    roots: &[usize],
    entries: &[usize],
    funcs: usize,
) -> (Vec<Option<usize>>, Vec<bool>) {
    let walk = |from: &[usize], skip: &[bool]| {
        let mut seen = vec![false; edges.len()];
        let mut stack: Vec<usize> = from.iter().copied().filter(|&n| !skip[n]).collect();
        while let Some(n) = stack.pop() {
            if !std::mem::replace(&mut seen[n], true) {
                stack.extend(edges[n].iter().copied().filter(|&t| !seen[t] && !skip[t]));
            }
        }
        seen
    };
    let main = walk(roots, &vec![false; edges.len()]);
    let reached: Vec<Vec<bool>> = entries.iter().map(|&e| walk(&[e], &main)).collect();
    let common = entries.len();
    let out: Vec<Option<usize>> = (0..funcs)
        .map(|f| match reached.iter().enumerate().filter(|(_, r)| r[f]).map(|(i, _)| i).collect::<Vec<_>>()[..] {
            [] => None,
            [i] => Some(i),
            _ => Some(common),
        })
        .collect();
    let needs = reached.iter().map(|r| (0..funcs).any(|f| r[f] && out[f] == Some(common))).collect();
    (out, needs)
}

/// Split `post` (what `wasm-bindgen` made) by what `pre` (the linker's
/// module, with `--emit-relocs`) says code reaches. `None`: nothing is
/// lazy only, or the module is not one this split reads (said as `Err`);
/// either way the one module stays as it is.
pub fn split(pre: &[u8], post: &[u8]) -> R<Option<Split>> {
    let m = parse(post)?;
    let mut entries: Vec<(u32, u32)> = Vec::new();
    for s in m.slots.iter().filter(|s| s.0 == 0 && m.names.get(&s.2).is_some_and(|n| is_entry(n))) {
        if !entries.iter().any(|e| e.0 == s.2) {
            entries.push((s.2, s.1));
        }
    }
    if entries.is_empty() {
        return Ok(None);
    }
    if m.imports_more {
        return Err("the module imports more than functions".into());
    }
    let pre = parse(pre)?;
    let refs = refs(&pre)?;
    let funcs = m.types.len();
    let mut by_name: HashMap<&str, Vec<usize>> = HashMap::new();
    for (&i, &n) in &m.names {
        by_name.entry(n).or_default().push(i as usize);
    }
    // A lazy entry's own address is the boundary: holding it is not reaching it.
    let entry = |f: usize| entries.iter().any(|e| e.0 as usize == f);
    let node = |t: &To| -> Vec<usize> {
        match t {
            To::Fn(n) => by_name.get(n).into_iter().flatten().copied().filter(|&f| !entry(f)).collect(),
            To::Data(d) => vec![funcs + d],
        }
    };
    let mut edges: Vec<Vec<usize>> = vec![Vec::new(); funcs + refs.from_data.len()];
    // The imported functions stay where they are, and the exported ones.
    let mut roots: Vec<usize> = (0..m.imported).collect();
    roots.extend(m.exports.iter().filter(|e| e.1 == 0).map(|e| e.2 as usize));
    roots.extend(m.start.map(|s| s as usize));
    roots.extend(m.named.iter().map(|&f| f as usize));
    roots.extend(refs.kept.iter().flat_map(node));
    // What wasm-bindgen took out of the module (its descriptors) it told the
    // glue: a function they name may be called from JS by its table slot.
    for (n, to) in &refs.from_fn {
        if !by_name.contains_key(n) {
            roots.extend(to.iter().flat_map(node));
        }
    }
    for (i, (_, body)) in m.bodies.iter().enumerate() {
        let f = m.imported + i;
        let mut calls = Vec::new();
        let (_, segment) = rewrite(body, &mut |c| {
            calls.push(c as usize);
            c
        })?;
        edges[f] = calls;
        let name = m.names.get(&(f as u32));
        // The relocations of a function wasm-bindgen made are unknown: one
        // in a table stays, as does one that names a segment.
        let in_table = m.slots.iter().any(|s| s.2 as usize == f);
        if segment || (in_table && !name.is_some_and(|n| refs.known.contains(n))) {
            roots.push(f);
        }
        if let Some(to) = name.and_then(|n| refs.from_fn.get(n)) {
            edges[f].extend(to.iter().flat_map(node));
        }
    }
    for (d, to) in refs.from_data.iter().enumerate() {
        edges[funcs + d] = to.iter().flat_map(node).collect();
    }
    let ids: Vec<usize> = entries.iter().map(|e| e.0 as usize).collect();
    let (chunk_of, needs_common) = partition(&edges, &roots, &ids, funcs);
    if chunk_of.iter().all(Option::is_none) {
        return Ok(None);
    }
    Ok(Some(emit(&m, post, &chunk_of, &entries, &needs_common)?))
}

/// The main module and the chunks of a partition.
fn emit(
    m: &Module,
    post: &[u8],
    chunk_of: &[Option<usize>],
    entries: &[(u32, u32)],
    needs_common: &[bool],
) -> R<Split> {
    let hash: String = aio::sha256::Sha256::digest(post)[..4].iter().map(|b| format!("{b:02x}")).collect();
    let common = entries.len();
    let members =
        |c: usize| -> Vec<u32> { (0..chunk_of.len() as u32).filter(|&f| chunk_of[f as usize] == Some(c)).collect() };
    let body = |f: u32| m.bodies[f as usize - m.imported].1;
    let mut chunks = Vec::new();
    // What the main module and the common chunk export to the chunks.
    let mut from_main: HashSet<u32> = HashSet::new();
    let mut from_common: HashSet<u32> = HashSet::new();
    let mut made = Vec::new();
    // The common chunk last: what the entries import from it is known by then.
    for c in (0..entries.len()).chain([common]) {
        let own = members(c);
        if own.is_empty() {
            continue;
        }
        let local: HashSet<u32> = own.iter().copied().collect();
        let mut ext: Vec<u32> = Vec::new();
        for &f in &own {
            rewrite(body(f), &mut |g| {
                if !local.contains(&g) {
                    ext.push(g);
                }
                g
            })?;
        }
        ext.sort_unstable();
        ext.dedup();
        let mut index: HashMap<u32, u32> = HashMap::new();
        let mut imports = Vec::new();
        for (i, &g) in ext.iter().enumerate() {
            let in_common = chunk_of[g as usize] == Some(common);
            if in_common {
                from_common.insert(g);
            } else {
                from_main.insert(g);
            }
            bytes(&mut imports, if in_common { b"c" } else { b"m" });
            bytes(&mut imports, format!("${g}").as_bytes());
            imports.push(0);
            leb(&mut imports, m.types[g as usize]);
            index.insert(g, i as u32);
        }
        for (i, &f) in own.iter().enumerate() {
            index.insert(f, (ext.len() + i) as u32);
        }
        for (i, t) in m.globals.iter().enumerate() {
            bytes(&mut imports, b"m");
            bytes(&mut imports, format!("$g{i}").as_bytes());
            imports.push(3);
            imports.extend_from_slice(t);
        }
        for (i, t) in m.tables.iter().enumerate() {
            bytes(&mut imports, b"m");
            bytes(&mut imports, format!("$t{i}").as_bytes());
            imports.push(1);
            imports.extend_from_slice(t);
        }
        let memory = m.memory.ok_or("the module has no memory")?;
        bytes(&mut imports, b"m");
        bytes(&mut imports, format!("$m{hash}").as_bytes());
        imports.push(2);
        imports.extend_from_slice(memory);
        let n_imports = ext.len() + m.globals.len() + m.tables.len() + 1;
        let mut funcs = Vec::new();
        let mut code = Vec::new();
        for &f in &own {
            leb(&mut funcs, m.types[f as usize]);
            let (b, _) = rewrite(body(f), &mut |g| index[&g])?;
            bytes(&mut code, &b);
        }
        let mut elems = Vec::new();
        let slots: Vec<&(u32, u32, u32)> = m.slots.iter().filter(|s| local.contains(&s.2)).collect();
        for &&(table, slot, f) in &slots {
            if table == 0 {
                elems.push(0);
            } else {
                elems.push(2);
                leb(&mut elems, table);
            }
            elems.push(0x41);
            sleb(&mut elems, slot as i32);
            elems.push(0x0b);
            if table != 0 {
                elems.push(0);
            }
            leb(&mut elems, 1);
            leb(&mut elems, index[&f]);
        }
        made.push((c, own, imports, n_imports, funcs, code, elems, slots.len(), index));
    }
    for (c, own, imports, n_imports, funcs, code, elems, n_elems, index) in made {
        let mut o = b"\0asm\x01\0\0\0".to_vec();
        let types = m.sections.iter().find(|s| s.0 == 1).map_or(&[][..], |s| s.1);
        section(&mut o, 1, types);
        section(&mut o, 2, &entries_of(n_imports, &imports));
        section(&mut o, 3, &entries_of(own.len(), &funcs));
        if c == common {
            let mut ex = Vec::new();
            let mut shared: Vec<u32> = from_common.iter().copied().collect();
            shared.sort_unstable();
            for &g in &shared {
                bytes(&mut ex, format!("${g}").as_bytes());
                ex.push(0);
                leb(&mut ex, index[&g]);
            }
            section(&mut o, 7, &entries_of(shared.len(), &ex));
        }
        if n_elems > 0 {
            section(&mut o, 9, &entries_of(n_elems, &elems));
        }
        section(&mut o, 10, &entries_of(own.len(), &code));
        let slot = (c != common).then(|| entries[c].1);
        chunks.push((aio::web::chunk_file(&hash, slot), o));
    }
    // The main module: moved bodies `unreachable`, what the chunks import
    // exported, which entries have a chunk (and need the common one), no names.
    let mut ex = Vec::new();
    let mut n_ex = m.exports.len();
    let mut add = |ex: &mut Vec<u8>, name: String, kind: u8, i: u32| {
        bytes(ex, name.as_bytes());
        ex.push(kind);
        leb(ex, i);
        n_ex += 1;
    };
    let mut shared: Vec<u32> = from_main.into_iter().collect();
    shared.sort_unstable();
    for g in shared {
        add(&mut ex, format!("${g}"), 0, g);
    }
    for i in 0..m.globals.len() {
        add(&mut ex, format!("$g{i}"), 3, i as u32);
    }
    for i in 0..m.tables.len() {
        add(&mut ex, format!("$t{i}"), 1, i as u32);
    }
    add(&mut ex, format!("$m{hash}"), 2, 0);
    for (i, &(f, slot)) in entries.iter().enumerate() {
        if chunk_of.contains(&Some(i)) {
            add(&mut ex, format!("aio-lazy-{slot}{}", if needs_common[i] { "-c" } else { "" }), 0, f);
        }
    }
    let mut main = b"\0asm\x01\0\0\0".to_vec();
    for &(id, payload, name) in &m.sections {
        match id {
            0 if name == "name" => {}
            7 => {
                let mut s = Rd::new(payload);
                s.u32()?;
                let mut p = Vec::new();
                leb(&mut p, n_ex as u32);
                p.extend_from_slice(&payload[s.p..]);
                p.extend_from_slice(&ex);
                section(&mut main, 7, &p);
            }
            10 => {
                let mut p = Vec::new();
                leb(&mut p, m.bodies.len() as u32);
                for (i, (_, b)) in m.bodies.iter().enumerate() {
                    match chunk_of[m.imported + i] {
                        Some(_) => bytes(&mut p, &[0, 0x00, 0x0b]),
                        None => bytes(&mut p, b),
                    }
                }
                section(&mut main, 10, &p);
            }
            _ => section(&mut main, id, payload),
        }
    }
    if !m.sections.iter().any(|s| s.0 == 7) {
        return Err("the module has no exports".into());
    }
    Ok(Split { main, chunks })
}

/// `wasm` without its relocation sections, its name section unless
/// `names`, and data segments of zeros only (its memory starts zeroed):
/// none of them is used by the page (pure).
pub fn strip(wasm: &[u8], names: bool) -> R<Vec<u8>> {
    let m = parse(wasm)?;
    let mut o = b"\0asm\x01\0\0\0".to_vec();
    for &(id, payload, name) in &m.sections {
        match id {
            0 if (name == "name" && !names) || name == "linking" || name.starts_with("reloc.") => {}
            11 if !m.datacount && !m.imports_more => {
                let mut kept = Vec::new();
                let mut n = 0;
                let mut s = Rd::new(payload);
                s.u32()?;
                for (at, len) in segments(payload)? {
                    let from = s.p;
                    s.p = at + len;
                    if payload[at..at + len].iter().any(|&b| b != 0) || payload[from] != 0 {
                        kept.extend_from_slice(s.since(from));
                        n += 1;
                    }
                }
                section(&mut o, 11, &entries_of(n, &kept));
            }
            _ => section(&mut o, id, payload),
        }
    }
    Ok(o)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn a_bodys_function_indices_are_mapped_and_its_segments_said() {
        // Two i32 locals; call 5; i32.const -1; call_indirect 0 0;
        // ref.func 300; return_call 7; end.
        let body = [1, 2, 0x7f, 0x10, 5, 0x41, 0x7f, 0x11, 0, 0, 0xd2, 0xac, 0x02, 0x12, 7, 0x0b];
        let (out, segment) = rewrite(&body, &mut |f| f + 1000).unwrap();
        let want = [1, 2, 0x7f, 0x10, 0xed, 0x07, 0x41, 0x7f, 0x11, 0, 0, 0xd2, 0x94, 0x0a, 0x12, 0xef, 0x07, 0x0b];
        assert_eq!((out, segment), (want.to_vec(), false));
        // memory.init names a data segment: the function stays in the main module.
        let init = [0, 0x41, 0, 0x41, 0, 0x41, 0, 0xfc, 8, 0, 0, 0x0b];
        assert_eq!(rewrite(&init, &mut |f| f).unwrap(), (init.to_vec(), true));
        // An instruction the split does not know: no split, rather than a wrong one.
        assert!(rewrite(&[0, 0xfb, 0, 0x0b], &mut |f| f).is_err());
    }

    #[test]
    fn a_function_goes_to_the_chunk_of_the_one_lazy_entry_that_alone_reaches_it() {
        // 0 a root, 1 and 6 it calls; 2 and 5 entries; 3 only 2's; 4 both
        // entries'; 7 reached by 5 through datum 8 (a vtable).
        let edges = [vec![1, 6], vec![], vec![3, 4, 6], vec![], vec![], vec![4, 8], vec![], vec![], vec![7]];
        let (chunks, needs) = partition(&edges, &[0], &[2, 5], 8);
        assert_eq!(chunks, [None, None, Some(0), Some(0), Some(2), Some(1), None, Some(1)]);
        assert_eq!(needs, [true, true]);
        // 5 reaches the common 4 only through a datum (a vtable): it needs the common chunk too.
        let edges = [vec![], vec![], vec![3, 4], vec![], vec![], vec![6], vec![4]];
        assert_eq!(
            partition(&edges, &[0], &[2, 5], 6),
            (vec![None, None, Some(0), Some(0), Some(2), Some(1)], vec![true, true])
        );
        // An entry the main module reaches stays, with all it reaches.
        let edges = [vec![2], vec![], vec![3], vec![]];
        assert_eq!(partition(&edges, &[0], &[2], 4), (vec![None; 4], vec![false]));
    }

    #[test]
    fn only_the_fn_a_lazy_macro_makes_is_an_entry() {
        assert!(is_entry("_RNvNCNCNCNCNCNvCsilORWv1Nt9r_9splitpage3Apps0_0s0_000010___aio_lazy"));
        assert!(is_entry("_ZN4page3App10__aio_lazy17h0123456789abcdefE"));
        // A closure's method in it, or another name: not one.
        let hole = "_RNvXs7_NtCsf6FCipdida1_3air3domNCNvNCNCNCNCNCNvCsilORWv1Nt9r_9splitpage3Apps0_0s0_000010___aio_lazy0INtB5_4HoleNtB5_5RangeE3runBJ_";
        assert!(!is_entry(hole));
        assert!(!is_entry("_ZN4page3App12__aio_lazy_x17h0123456789abcdefE"));
    }

    #[test]
    fn a_release_loses_its_names_relocations_and_zero_data() {
        // Memory; data: 3 zeros at 0, [1, 2] at 16.
        let data = vec![2, 0, 0x41, 0, 0x0b, 3, 0, 0, 0, 0, 0x41, 0x10, 0x0b, 2, 1, 2];
        let wasm = module(&[
            (5, vec![1, 0, 1]),
            (11, data),
            custom("name", &[1, 1, 0]),
            custom("linking", &[2]),
            custom("reloc.CODE", &[0]),
            custom("producers", &[0]),
        ]);
        let kept = vec![1, 0, 0x41, 0x10, 0x0b, 2, 1, 2];
        let want = |names: bool| {
            let mut s = vec![(5, vec![1, 0, 1]), (11, kept.clone())];
            s.extend(names.then(|| custom("name", &[1, 1, 0])));
            s.push(custom("producers", &[0]));
            module(&s)
        };
        assert_eq!(strip(&wasm, false).unwrap(), want(false));
        assert_eq!(strip(&wasm, true).unwrap(), want(true));
    }
}
