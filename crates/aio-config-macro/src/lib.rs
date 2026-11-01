//! `aio::config!()`'s reader: the app's `aio.toml`, parsed and checked as
//! the app builds (a mistake is a compile error), written into it as the
//! Rust that makes it, so the app holds no TOML parser. The file is
//! `include_str!`'d too: cargo rebuilds when it changes.
use proc_macro::TokenStream;
use std::path::{Path, PathBuf};

/// `aio::config!()`: the nearest `aio.toml` from the crate's folder up;
/// `aio::config!("../aio.toml")`: that one, from the crate's folder. Use it
/// through `aio::config!`, which names `aio`.
#[proc_macro]
pub fn config(input: TokenStream) -> TokenStream {
    let input = proc_macro2::TokenStream::from(input);
    let asked = match input.is_empty() {
        true => None,
        false => match syn::parse2::<syn::LitStr>(input) {
            Ok(s) => Some(s.value()),
            Err(e) => return e.to_compile_error().into(),
        },
    };
    let dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_default());
    match read(&dir, asked.as_deref()) {
        Ok((file, rust, splash)) => {
            let file = file.to_string_lossy().into_owned();
            let value: proc_macro2::TokenStream = match rust.parse() {
                Ok(v) => v,
                Err(e) => return syn::Error::new(proc_macro2::Span::call_site(), e).to_compile_error().into(),
            };
            // The splash's PNG is built in: the binary carries it, not its path.
            let value = match splash {
                None => value,
                Some(png) => {
                    let png = png.to_string_lossy().into_owned();
                    quote::quote!({
                        let mut c = #value;
                        if let Some(s) = c.build.splash.as_mut() {
                            s.png = include_bytes!(#png);
                        }
                        c
                    })
                }
            };
            quote::quote!({
                const _: &str = include_str!(#file);
                #value
            })
            .into()
        }
        Err(e) => syn::Error::new(proc_macro2::Span::call_site(), e).to_compile_error().into(),
    }
}

/// The config file for a crate in `dir` (`asked`: from there), its config
/// as Rust and its splash image's path; else what is wrong, as
/// `aio::config::build` said it.
fn read(dir: &Path, asked: Option<&str>) -> Result<(PathBuf, String, Option<PathBuf>), String> {
    let file = match asked {
        Some(p) => dir.join(p),
        None => aio_config::config::find(dir)
            .ok_or_else(|| format!("aio::config!(): no aio.toml in {} or above it", dir.display()))?,
    };
    let shown = asked.map_or_else(|| file.display().to_string(), String::from);
    let c = std::fs::read_to_string(&file)
        .map_err(|e| e.to_string())
        .and_then(|t| aio_config::config::Config::parse(&t))
        .map_err(|e| format!("{shown}: {e}"))?;
    let splash = c.build.splash.as_ref().map(|s| file.parent().unwrap_or(Path::new(".")).join(&s.image));
    if let Some(p) = splash.as_ref().filter(|p| !p.is_file()) {
        return Err(format!("{shown}: [build.splash] image {} is not there", p.display()));
    }
    Ok((file, c.rust(), splash))
}

#[cfg(test)]
mod tests {
    use super::read;

    #[test]
    fn the_nearest_aio_toml_is_read_and_a_mistake_is_said_as_the_build_said_it() {
        let d = std::env::temp_dir().join(format!("aio-config-macro-{}", std::process::id()));
        std::fs::create_dir_all(d.join("app/src")).unwrap();
        std::fs::write(d.join("aio.toml"), "[app]\nid = \"t\"\n").unwrap();
        let (file, rust, _) = read(&d.join("app"), None).unwrap();
        assert_eq!(file, d.join("aio.toml"));
        assert!(rust.starts_with("aio::config::Config{app: aio::config::App{id: "), "{rust}");
        assert_eq!(read(&d.join("app"), Some("../aio.toml")).unwrap().1, rust);
        std::fs::write(d.join("aio.toml"), "[app]\nid = \"t\"\n[ui]\nhue = 360\n").unwrap();
        assert_eq!(
            read(&d.join("app"), Some("../aio.toml")).unwrap_err(),
            "../aio.toml: aio.toml: [ui] hue is degrees, 0 to 359"
        );
        std::fs::write(d.join("aio.toml"), "[app]\nid = \"t\"\nnmae = \"x\"\n").unwrap();
        assert!(read(&d.join("app"), None).unwrap_err().contains("unknown field `nmae`"));
        assert!(read(&d.join("app"), Some("nope.toml")).unwrap_err().starts_with("nope.toml: "));
        // The splash's image is built in: it must be there.
        std::fs::write(d.join("aio.toml"), "[app]\nid = \"t\"\n[build.splash]\nimage = \"s.png\"\n").unwrap();
        assert!(read(&d.join("app"), None).unwrap_err().contains("[build.splash] image"));
        std::fs::write(d.join("s.png"), b"png").unwrap();
        assert_eq!(read(&d.join("app"), None).unwrap().2, Some(d.join("s.png")));
        std::fs::remove_dir_all(&d).unwrap();
        let none = std::env::temp_dir().join("aio-config-macro-none");
        if !none.ancestors().any(|a| a.join("aio.toml").is_file()) {
            assert!(read(&none, None).unwrap_err().contains("no aio.toml in"));
        }
    }
}
