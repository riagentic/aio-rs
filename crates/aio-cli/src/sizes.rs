//! Which crates take a binary's space (aio's `bundle-analyze.ts`): its code
//! symbols, demangled and summed per crate, biggest first. The framework
//! counts per area (`aio::kit`), so "how much of this is aio" has an
//! answer; code that is not Rust (SQLite) by its name's prefix (`[C] sqlite3`).
//! Read from a build that kept its symbols (`ax build <app> --sizes`).
use object::{Object, ObjectSection, ObjectSymbol, SymbolKind};

/// One row: a crate (or framework area), its bytes, share 0..1, symbols.
#[derive(Debug, PartialEq)]
pub struct Row {
    pub name: String,
    pub bytes: u64,
    pub share: f64,
    pub symbols: usize,
}

/// The rows (biggest first, at most the limit) and the rest, summed.
#[derive(Debug, PartialEq)]
pub struct Analysis {
    pub total: u64,
    pub rows: Vec<Row>,
    pub rest_bytes: u64,
    pub rest_symbols: usize,
}

/// The code symbols of a binary (ELF, Mach-O, PE): name and size. Where the
/// format gives no size, a symbol runs to the next one in its section.
pub fn symbols(bin: &[u8]) -> Result<Vec<(String, u64)>, String> {
    let file = object::File::parse(bin).map_err(|e| format!("not a binary: {e}"))?;
    let mut syms: Vec<(u64, u64, u64, String)> = file
        .symbols()
        .filter(|s| s.kind() == SymbolKind::Text && s.is_definition())
        .filter_map(|s| {
            let sec = file.section_by_index(s.section_index()?).ok()?;
            Some((s.address(), s.size(), sec.address() + sec.size(), s.name().ok()?.to_string()))
        })
        .collect();
    if syms.is_empty() {
        return Err("the binary has no symbols (stripped)".into());
    }
    syms.sort_by_key(|s| s.0);
    syms.dedup_by_key(|s| s.0);
    let next: Vec<u64> = syms.iter().skip(1).map(|s| s.0).chain([u64::MAX]).collect();
    Ok(syms
        .into_iter()
        .zip(next)
        .map(|((at, size, end, name), next)| (name, if size > 0 { size } else { next.min(end).saturating_sub(at) }))
        .collect())
}

/// The bytes of a binary that ship: its sections less symbols and debug
/// info (what a stripped release keeps, the budget's measure).
pub fn shipped(bin: &[u8]) -> Result<u64, String> {
    use object::{SectionFlags, SectionKind as K};
    let file = object::File::parse(bin).map_err(|e| format!("not a binary: {e}"))?;
    let ships = |s: &object::Section| match s.flags() {
        // ELF says it: what is loaded (symbols and debug info are not).
        SectionFlags::Elf { sh_flags } => sh_flags & u64::from(object::elf::SHF_ALLOC) != 0,
        _ => !matches!(s.kind(), K::Debug | K::DebugString | K::Metadata | K::Linker),
    };
    let in_file = |k| !matches!(k, K::UninitializedData | K::UninitializedTls | K::Common);
    Ok(file.sections().filter(|s| ships(s) && in_file(s.kind())).map(|s| s.size()).sum())
}

/// The row a symbol counts in: its crate (in `<T as Trait>` the type's, the
/// trait's when the type is a primitive), `aio::<area>` for the framework,
/// `[C] <prefix>` for other code.
pub fn group_of(symbol: &str) -> String {
    let Ok(d) = rustc_demangle::try_demangle(symbol) else {
        let bare = symbol.trim_start_matches('_');
        let end = bare.find(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit())).unwrap_or(bare.len());
        return format!("[C] {}", if end == 0 { bare } else { &bare[..end] });
    };
    let path = format!("{d:#}");
    let (ty, tr) = match path.strip_prefix('<') {
        Some(rest) => impl_of(rest),
        None => (path.as_str(), None),
    };
    match crate_of(ty).or_else(|| tr.and_then(crate_of)) {
        Some((k @ ("aio" | "aio_shell"), Some(area))) => format!("{k}::{area}"),
        Some((k, _)) => k.to_string(),
        None => "core".into(),
    }
}

/// `T as Trait>…` (or `T>…`): the type and the trait.
fn impl_of(s: &str) -> (&str, Option<&str>) {
    let mut depth = 0;
    for (i, c) in s.char_indices() {
        match c {
            '<' => depth += 1,
            '>' if depth == 0 => return (&s[..i], None),
            '>' => depth -= 1,
            ' ' if depth == 0 && s[i..].starts_with(" as ") => return (&s[..i], Some(&s[i + 4..])),
            _ => {}
        }
    }
    (s, None)
}

/// A path's crate and, when it goes deeper, its module: None for a
/// primitive or a generic parameter (no `::`).
fn crate_of(path: &str) -> Option<(&str, Option<&str>)> {
    let p = path.trim_start_matches(['&', '*', '(', '[', ' ']);
    let p = ["mut ", "const ", "dyn "].iter().fold(p, |p, k| p.strip_prefix(k).unwrap_or(p));
    let (krate, rest) = p.split_once("::")?;
    if !krate.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    let ident = |s: &str| s.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(s.len());
    let area = rest.split_once("::").map(|(a, _)| &a[..ident(a)]);
    Some((krate, area))
}

/// Rank `symbols` by crate: the `limit` biggest rows, the tail summed.
pub fn analyze(symbols: &[(String, u64)], limit: usize) -> Analysis {
    let mut groups = std::collections::BTreeMap::<String, (u64, usize)>::new();
    for (name, bytes) in symbols {
        let g = groups.entry(group_of(name)).or_default();
        *g = (g.0 + bytes, g.1 + 1);
    }
    let total: u64 = groups.values().map(|g| g.0).sum();
    let mut rows: Vec<Row> = groups
        .into_iter()
        .map(|(name, (bytes, symbols))| Row { name, bytes, share: bytes as f64 / total.max(1) as f64, symbols })
        .collect();
    // Bytes, then name: equal rows never swap between runs.
    rows.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.name.cmp(&b.name)));
    let rest = rows.split_off(limit.max(1).min(rows.len()));
    Analysis {
        total,
        rows,
        rest_bytes: rest.iter().map(|r| r.bytes).sum(),
        rest_symbols: rest.iter().map(|r| r.symbols).sum(),
    }
}

/// The report, saying which bytes it counts (`shipped`: the binary's).
pub fn report(a: &Analysis, shipped: u64) -> String {
    if a.total == 0 {
        return "no code symbols to count".into();
    }
    let mb = |b: u64| b as f64 / 1e6;
    let mut out = vec![format!(
        "code by crate: {:.2} MB of symbols in a {:.2} MB binary (stripped; the rest is data, unwind tables, \
         relocations)",
        mb(a.total),
        mb(shipped)
    )];
    out.extend(a.rows.iter().map(|r| {
        format!("{:>8.1} KB {:>5.1}%  {} ({} symbols)", r.bytes as f64 / 1e3, 100.0 * r.share, r.name, r.symbols)
    }));
    if a.rest_symbols > 0 {
        out.push(format!(
            "{:>8.1} KB {:>5.1}%  the rest ({} symbols)",
            a.rest_bytes as f64 / 1e3,
            100.0 * a.rest_bytes as f64 / a.total as f64,
            a.rest_symbols
        ));
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn syms(list: &[(&str, u64)]) -> Vec<(String, u64)> {
        list.iter().map(|(n, b)| (n.to_string(), *b)).collect()
    }

    #[test]
    fn a_symbol_counts_for_its_crate_the_framework_per_area_and_c_by_prefix() {
        assert_eq!(group_of("_ZN5serde2de5Error6custom17h0123456789abcdefE"), "serde");
        assert_eq!(group_of("_ZN3aio3kit6button6render17h0123456789abcdefE"), "aio::kit");
        assert_eq!(group_of("_ZN3aio3run17h0123456789abcdefE"), "aio");
        // A primitive's impl counts for the trait's crate; an inherent one for core.
        assert_eq!(
            group_of("_ZN45_$LT$str$u20$as$u20$serde..ser..Serialize$GT$9serialize17h0123456789abcdefE"),
            "serde"
        );
        assert_eq!(group_of("_ZN4core5slice29_$LT$impl$u20$$u5b$T$u5d$$GT$4sort17h0123456789abcdefE"), "core");
        assert_eq!(group_of("_ZN49_$LT$$u5b$u8$u5d$$u20$as$u20$core..fmt..Debug$GT$3fmt17h0123456789abcdefE"), "core");
        // `<Vec<T> as Drop>::drop` is the Vec's: the type's crate.
        assert_eq!(
            group_of(
                "_ZN70_$LT$alloc..vec..Vec$LT$T$C$A$GT$$u20$as$u20$core..ops..drop..Drop$GT$4drop17h0123456789abcdefE"
            ),
            "alloc"
        );
        assert_eq!(group_of("sqlite3VdbeExec"), "[C] sqlite3");
        assert_eq!(group_of("_memcpy"), "[C] memcpy");
    }

    #[test]
    fn rows_sum_to_the_whole_with_counts_and_a_summed_tail() {
        let s = syms(&[
            ("_ZN5serde1a17h0123456789abcdefE", 300),
            ("_ZN5serde1b17h0123456789abcdefE", 100),
            ("_ZN4core1a17h0123456789abcdefE", 500),
            ("sqlite3Main", 50),
            ("zlibInflate", 50),
        ]);
        let a = analyze(&s, 2);
        assert_eq!(a.total, 1000);
        assert_eq!(
            a.rows,
            vec![
                Row { name: "core".into(), bytes: 500, share: 0.5, symbols: 1 },
                Row { name: "serde".into(), bytes: 400, share: 0.4, symbols: 2 },
            ]
        );
        assert_eq!((a.rest_bytes, a.rest_symbols), (100, 2), "the tail is summed, never dropped");
        let all = analyze(&s, 20);
        assert_eq!(all.rows.iter().map(|r| r.bytes).sum::<u64>(), all.total);
        // Equal rows: by name, every run.
        assert_eq!(all.rows[2..].iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["[C] sqlite3", "[C] zlib"]);
    }

    #[test]
    fn the_report_names_its_bytes_and_an_empty_one_says_so() {
        let a = analyze(&syms(&[("_ZN4core1a17h0123456789abcdefE", 2000), ("x1", 1), ("y1", 1)]), 1);
        let r = report(&a, 10_000);
        assert!(r.starts_with("code by crate: 0.00 MB of symbols in a 0.01 MB binary (stripped; "), "{r}");
        assert!(r.contains("2.0 KB  99.9%  core (1 symbols)") && r.contains("the rest (2 symbols)"), "{r}");
        assert_eq!(report(&analyze(&[], 20), 0), "no code symbols to count");
    }

    #[test]
    fn this_test_binary_reads_as_its_crates() {
        let bin = std::fs::read(std::env::current_exe().unwrap()).unwrap();
        let Ok(s) = symbols(&bin) else { return }; // a stripped test binary has nothing to read
        let a = analyze(&s, 50);
        assert!(a.rows.iter().any(|r| r.name == "core" || r.name == "std"), "{}", report(&a, bin.len() as u64));
        // What ships holds the code, never the symbols or debug info.
        let shipped = shipped(&bin).unwrap();
        assert!(a.total > 0 && a.total <= shipped && shipped < bin.len() as u64, "{} {shipped}", a.total);
    }
}
