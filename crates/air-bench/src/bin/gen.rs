//! `gen <out> <aio-rs checkout>`: the measured apps, Dioxus and air, the
//! same views in each: `dx-1`, `dx-50`, `air-1`, `air-50`, each a crate of
//! its own (lib: the views; bin: mounts them), 204 rsx lines a view: 40
//! blocks of a static label, a class and a text that follow a signal and a
//! click handler, plus a keyed loop and a branch. `bench.sh` builds them.
use std::fmt::Write;
use std::path::Path;

const BLOCKS: usize = 40;

fn view(dx: bool, k: usize) -> String {
    let mut s = String::new();
    let (imports, ret, read, list_expr, key, gt) = if dx {
        ("use dioxus::prelude::*;", "Element", "let n = count();\n", "items.iter()", "\"{i}\"", "n")
    } else {
        ("use air::{Signal, View, component, rsx};", "View", "", "items.clone()", "*i", "count.get()")
    };
    let txt = if dx { "{n}" } else { "{count}" };
    let mutc = if dx { "mut " } else { "" };
    let _ = write!(
        s,
        "{imports}\n\n#[component]\npub fn View{k}(title: String, {mutc}count: Signal<i64>) -> {ret} {{\n    {read}    let items: Vec<(u32, String)> = (0..5u32).map(|i| (i, format!(\"item {{i}}\"))).collect();\n    rsx! {{\n        section {{ class: \"view v{k}\",\n            h2 {{ class: \"title\", \"{{title}}\" }}\n            for (i, s) in {list_expr} {{ p {{ key: {key}, \"{{s}}\" }} }}\n            if {gt} > 3 {{ p {{ \"big\" }} }} else {{ p {{ \"small\" }} }}\n"
    );
    for b in 0..BLOCKS {
        let _ = write!(
            s,
            "            div {{ class: \"row r{b}\",\n                span {{ class: \"label\", \"Label {b}\" }}\n                span {{ class: if {gt} > {b} {{ \"pos\" }} else {{ \"neg\" }}, \"{txt}\" }}\n                button {{ class: \"btn\", onclick: move |_| count += 1, \"Add {b}\" }}\n            }}\n"
        );
    }
    s.push_str("        }\n    }\n}\n");
    s
}

fn krate(out: &Path, aio: &Path, dx: bool, n: usize) -> std::io::Result<()> {
    let dir = out.join(format!("{}-{n}", if dx { "dx" } else { "air" }));
    std::fs::create_dir_all(dir.join("src/views"))?;
    let dep = if dx {
        r#"dioxus = { version = "=0.7.10", default-features = false, features = ["macro", "html", "signals", "hooks"] }
dioxus-core = "=0.7.10""#
            .to_string()
    } else {
        format!("air = {{ package = \"aio-rs-air\", path = \"{}\" }}", aio.join("crates/air").display())
    };
    let name = format!("bench-{}-{n}", if dx { "dx" } else { "air" });
    std::fs::write(
        dir.join("Cargo.toml"),
        format!(
            "[package]\nname = \"{name}\"\nedition = \"2024\"\nversion = \"0.0.0\"\npublish = false\n\n[lib]\nname = \"views\"\n\n[dependencies]\n{dep}\n\n[workspace]\n\n# As aio-rs's own (its workspace Cargo.toml).\n[profile.dev]\ndebug = \"line-tables-only\"\n\n[profile.dev.package.\"*\"]\ndebug = false\nopt-level = 1\n"
        ),
    )?;
    std::fs::copy(aio.join("Cargo.lock"), dir.join("Cargo.lock"))?;
    let mut lib = String::from("pub mod views;\n");
    let mut mods = String::new();
    for k in 0..n {
        std::fs::write(dir.join(format!("src/views/v{k}.rs")), view(dx, k))?;
        let _ = writeln!(mods, "mod v{k};\npub use v{k}::*;");
    }
    std::fs::write(dir.join("src/views.rs"), mods)?;
    let calls: String = (0..n).map(|k| format!("            View{k} {{ title: \"t{k}\", count }}\n")).collect();
    if dx {
        let _ = write!(
            lib,
            "use dioxus::prelude::*;\npub use views::*;\n\npub fn app() -> Element {{\n    let count = use_signal(|| 0i64);\n    rsx! {{\n        main {{\n{calls}        }}\n    }}\n}}\n"
        );
        std::fs::write(
            dir.join("src/main.rs"),
            "fn main() {\n    let mut dom = dioxus_core::VirtualDom::new(views::app);\n    let m = dom.rebuild_to_vec();\n    println!(\"{} edits\", m.edits.len());\n}\n",
        )?;
    } else {
        let _ = write!(
            lib,
            "use air::{{Signal, View, rsx}};\npub use views::*;\n\npub fn app() -> View {{\n    let count = Signal::new(0i64);\n    rsx! {{\n        main {{\n{calls}        }}\n    }}\n}}\n"
        );
        std::fs::write(
            dir.join("src/main.rs"),
            "fn main() {\n    let _root = air::Root::mount(views::app);\n    println!(\"{} bytes of edits\", air::take_edits().len());\n}\n",
        )?;
    }
    std::fs::write(dir.join("src/lib.rs"), lib)
}

fn main() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let [_, out, aio] = &args[..] else {
        eprintln!("usage: gen <out dir> <aio-rs checkout>");
        std::process::exit(2);
    };
    for dx in [true, false] {
        for n in [1, 50] {
            krate(Path::new(out), Path::new(aio), dx, n)?;
        }
    }
    Ok(())
}
