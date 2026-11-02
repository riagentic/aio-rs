//! `aio::config!()`'s reader: the app's `aio.toml`, parsed and checked as
//! the app builds (a mistake is a compile error), written into it as the
//! Rust that makes it, so the app holds no TOML parser. The file is
//! `include_str!`'d too: cargo rebuilds when it changes.
use aio_config::config::{Config, Mode};
use proc_macro::TokenStream;
use proc_macro2::{Literal, Span};
use quote::ToTokens;
use std::path::{Path, PathBuf};

/// `aio::config!()`: the nearest `aio.toml` from the crate's folder up;
/// `aio::config!("../aio.toml")`: that one, from the crate's folder. Use it
/// through `aio::config!`, which names `aio`.
#[proc_macro]
pub fn config(input: TokenStream) -> TokenStream {
    let made = asked(input).and_then(|asked| {
        let dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_default());
        let Read { file, config, splash, icon, assets } = read(&dir, asked.as_deref()).map_err(error)?;
        let value: proc_macro2::TokenStream = config.rust().parse().map_err(error)?;
        let file = file.to_string_lossy().into_owned();
        // The splash's PNG and the icon are built in: the binary carries them, not their paths.
        let splash = splash.map(|png| {
            let png = png.to_string_lossy().into_owned();
            quote::quote!(if let Some(s) = c.build.splash.as_mut() {
                s.png = include_bytes!(#png);
            })
        });
        let icon = icon.map(|p| {
            let p = p.to_string_lossy().into_owned();
            quote::quote!(c.app.icon = include_bytes!(#p);)
        });
        // `[build] assets`: each file built in at its URL path. A file added
        // later is built in once ax builds again (`AIORT_ASSETS` changes).
        let assets = (!assets.is_empty()).then(|| {
            let (urls, files): (Vec<_>, Vec<_>) =
                assets.into_iter().map(|(u, p)| (u, p.to_string_lossy().into_owned())).unzip();
            quote::quote!(
                const _: Option<&str> = option_env!("AIORT_ASSETS");
                c.build.asset_files = &[#((#urls, include_bytes!(#files) as &[u8])),*];
            )
        });
        Ok(quote::quote!({
            const _: &str = include_str!(#file);
            #[allow(unused_mut)]
            let mut c = #value;
            #splash
            #icon
            #assets
            c
        }))
    });
    made.unwrap_or_else(|e| e.to_compile_error()).into()
}

/// `aio::hue!()`: the app's one hue as a `u16` (`Config::hue`: `[ui] hue`,
/// else its id's), from the nearest `aio.toml`, as `config!` finds it; a
/// crate with none (a library of views) takes its own name's.
#[proc_macro]
pub fn hue(input: TokenStream) -> TokenStream {
    let made = asked(input).and_then(|asked| {
        let dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_default());
        if asked.is_none() && aio_config::config::find(&dir).is_none() {
            let name = std::env::var("CARGO_PKG_NAME").unwrap_or_default();
            return Ok(Literal::u16_suffixed(aio_config::icon::hue(&name)).into_token_stream());
        }
        let (file, c) = parsed(&dir, asked.as_deref()).map_err(error)?;
        let (file, h) = (file.to_string_lossy().into_owned(), Literal::u16_suffixed(c.hue()));
        Ok(quote::quote!({
            const _: &str = include_str!(#file);
            #h
        }))
    });
    made.unwrap_or_else(|e| e.to_compile_error()).into()
}

fn error(e: impl std::fmt::Display) -> syn::Error {
    syn::Error::new(Span::call_site(), e)
}

/// The macro's one optional argument: a path to the `aio.toml`.
fn asked(input: TokenStream) -> syn::Result<Option<String>> {
    let input = proc_macro2::TokenStream::from(input);
    match input.is_empty() {
        true => Ok(None),
        false => syn::parse2::<syn::LitStr>(input).map(|s| Some(s.value())),
    }
}

/// What `config!` builds in.
struct Read {
    file: PathBuf,
    config: Config,
    splash: Option<PathBuf>,
    /// The window's icon, `assets/icon.png` (none: a CLI, a server, or the
    /// monogram the shell draws).
    icon: Option<PathBuf>,
    /// `[build] assets`' files: URL path, file.
    assets: Vec<(String, PathBuf)>,
}

/// The config file for a crate in `dir` (`asked`: from there), parsed and
/// checked; else what is wrong, as `aio::config::build` said it.
fn parsed(dir: &Path, asked: Option<&str>) -> Result<(PathBuf, Config), String> {
    let file = match asked {
        Some(p) => dir.join(p),
        None => aio_config::config::find(dir)
            .ok_or_else(|| format!("aio::config!(): no aio.toml in {} or above it", dir.display()))?,
    };
    let shown = asked.map_or_else(|| file.display().to_string(), String::from);
    let c = std::fs::read_to_string(&file)
        .map_err(|e| e.to_string())
        .and_then(|t| Config::parse(&t))
        .map_err(|e| format!("{shown}: {e}"))?;
    Ok((file, c))
}

/// [`parsed`], with the splash image's path and the window's icon.
fn read(dir: &Path, asked: Option<&str>) -> Result<Read, String> {
    let (file, c) = parsed(dir, asked)?;
    let shown = asked.map_or_else(|| file.display().to_string(), String::from);
    let at = file.parent().unwrap_or(Path::new(".")).to_path_buf();
    let splash = c.build.splash.as_ref().map(|s| at.join(&s.image));
    if let Some(p) = splash.as_ref().filter(|p| !p.is_file()) {
        return Err(format!("{shown}: [build.splash] image {} is not there", p.display()));
    }
    let own = at.join("assets/icon.png");
    let icon = match c.app.mode {
        Mode::Cli | Mode::Server => None,
        _ if own.is_file() => {
            let png = std::fs::read(&own).map_err(|e| format!("{}: {e}", own.display()))?;
            if aio_config::icon::png_size(&png).is_none() {
                return Err(format!("{}: not a PNG (the window's icon is a PNG)", own.display()));
            }
            Some(own)
        }
        _ => None,
    };
    let assets = aio_config::config::asset_files(&at, &c.build.assets).map_err(|e| format!("{shown}: {e}"))?;
    Ok(Read { file, config: c, splash, icon, assets })
}

#[cfg(test)]
mod tests {
    use super::read;

    #[test]
    fn the_nearest_aio_toml_is_read_and_a_mistake_is_said_as_the_build_said_it() {
        let d = std::env::temp_dir().join(format!("aio-config-macro-{}", std::process::id()));
        std::fs::create_dir_all(d.join("app/src")).unwrap();
        std::fs::write(d.join("aio.toml"), "[app]\nid = \"t\"\n").unwrap();
        let r = read(&d.join("app"), None).unwrap();
        assert_eq!(r.file, d.join("aio.toml"));
        let rust = r.config.rust();
        assert!(rust.starts_with("aio::config::Config{app: aio::config::App{id: "), "{rust}");
        assert_eq!(read(&d.join("app"), Some("../aio.toml")).unwrap().config.rust(), rust);
        std::fs::write(d.join("aio.toml"), "[app]\nid = \"t\"\n[ui]\nhue = 360\n").unwrap();
        assert_eq!(
            read(&d.join("app"), Some("../aio.toml")).err().unwrap(),
            "../aio.toml: aio.toml: [ui] hue is degrees, 0 to 359"
        );
        std::fs::write(d.join("aio.toml"), "[app]\nid = \"t\"\nnmae = \"x\"\n").unwrap();
        assert!(read(&d.join("app"), None).err().unwrap().contains("unknown field `nmae`"));
        assert!(read(&d.join("app"), Some("nope.toml")).err().unwrap().starts_with("nope.toml: "));
        // The splash's image is built in: it must be there.
        std::fs::write(d.join("aio.toml"), "[app]\nid = \"t\"\n[build.splash]\nimage = \"s.png\"\n").unwrap();
        assert!(read(&d.join("app"), None).err().unwrap().contains("[build.splash] image"));
        std::fs::write(d.join("s.png"), b"png").unwrap();
        assert_eq!(read(&d.join("app"), None).unwrap().splash, Some(d.join("s.png")));
        std::fs::remove_dir_all(&d).unwrap();
        let none = std::env::temp_dir().join("aio-config-macro-none");
        if !none.ancestors().any(|a| a.join("aio.toml").is_file()) {
            assert!(read(&none, None).err().unwrap().contains("no aio.toml in"));
        }
    }

    #[test]
    fn a_window_apps_own_icon_is_built_in_by_path() {
        let d = std::env::temp_dir().join(format!("aio-config-macro-icon-{}", std::process::id()));
        std::fs::create_dir_all(d.join("assets")).unwrap();
        let icon = |toml: &str| {
            std::fs::write(d.join("aio.toml"), toml).unwrap();
            read(&d, None).map(|r| r.icon)
        };
        // None: the shell draws the monogram.
        assert_eq!(icon("[app]\nid = \"notes\"\n").unwrap(), None);
        // Its own, by path (cargo then rebuilds when it changes); a non-PNG is refused.
        let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x08\0\0\0\x08";
        std::fs::write(d.join("assets/icon.png"), png).unwrap();
        assert_eq!(icon("[app]\nid = \"notes\"\n").unwrap(), Some(d.join("assets/icon.png")));
        // No window, no icon.
        assert_eq!(icon("[app]\nid = \"notes\"\nmode = \"cli\"\n").unwrap(), None);
        std::fs::write(d.join("assets/icon.png"), b"<svg/>").unwrap();
        assert!(icon("[app]\nid = \"notes\"\n").unwrap_err().ends_with("not a PNG (the window's icon is a PNG)"));
        std::fs::remove_dir_all(&d).unwrap();
    }
}
