//! `ax theme adopt <app>`: aio-rs's default theme (`aio::kit::theme::sheet`:
//! the tokens for the app's hue and the whole look, as its `[ui]` hue and
//! layout say) written into the app as
//! `assets/aio-theme.css`, a file it owns from then on: in its git history,
//! edited like any stylesheet, and no aio-rs upgrade can move its look.
use aio::config::{Look, Ui};
use std::path::Path;

type Out = Result<String, String>;

/// Where the adopted theme goes, beside the app's `aio.toml`.
pub const FILE: &str = "assets/aio-theme.css";

/// The adopted file for app `id` with `[ui]` `ui`: what it is, then the
/// whole theme (pure).
pub fn css(id: &str, ui: &Ui) -> String {
    format!(
        "/* aio-rs's default theme, adopted by this app.
 *
 * Written by `ax theme adopt` from aio-rs {} for \"{id}\".
 * This file is yours: aio-rs does not read it and never updates it, so no
 * upgrade changes what it does. Edit it like any other stylesheet.
 *
 * It stays in `@layer aio` on purpose: an unlayered rule of yours beats
 * everything here without !important, so a piece can be overridden
 * without deleting it. The accent comes from the app's id: change
 * --aio-hue (or --aio-accent) and every tone made from it follows.
 */
{}",
        env!("CARGO_PKG_VERSION"),
        aio::kit::theme::sheet(&Ui { theme: Look::Full, ..*ui }, id, false)
    )
}

/// Write the theme of app `id` into `dir` (its `aio.toml`'s); an adopted
/// file is the app's: kept unless `force`.
pub fn adopt(dir: &Path, id: &str, ui: &Ui, force: bool) -> Out {
    let file = dir.join(FILE);
    if file.exists() && !force {
        return Err(format!(
            "{} exists: it is the app's now, and writing over it would lose its edits. \
             Keep it, or --force to replace it with aio {}'s theme (then look at git diff)",
            file.display(),
            env!("CARGO_PKG_VERSION")
        ));
    }
    std::fs::create_dir_all(dir.join("assets")).map_err(|e| e.to_string())?;
    aio::paths::write_atomic(&file, css(id, ui).as_bytes(), false).map_err(|e| e.to_string())?;
    Ok(format!(
        "adopted aio {}'s theme as {}\nthe app's page takes it first: \
         aio::ui::page_css(&[include_str!(\"../../{FILE}\"), include_str!(\"../../assets/style.css\")], dark)",
        env!("CARGO_PKG_VERSION"),
        file.display()
    ))
}

/// `ax theme adopt <app>`: the app's dir and id from this workspace.
pub fn run(name: &str, force: bool) -> Out {
    let meta = crate::dev::metadata()?;
    let pkg = crate::dev::locate(&meta, name)?;
    let dir = crate::build::app_dir(&pkg)?;
    let (app, cfg) = crate::build::app_of(&pkg)?;
    adopt(&dir, &app.id, &cfg.ui, force)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adopt_writes_the_exact_theme_the_kit_gives() {
        let d = tempfile::tempdir().unwrap();
        let said = adopt(d.path(), "notes", &Ui::default(), false).unwrap();
        assert!(said.contains("page_css"), "{said}");
        let text = std::fs::read_to_string(d.path().join(FILE)).unwrap();
        let theme = aio::kit::theme_css(aio::kit::theme::hue("notes"));
        assert!(text.ends_with(&theme) && text.starts_with("/* aio-rs's default theme"));
        assert!(text.contains(&format!("--aio-hue:{};", aio::kit::theme::hue("notes"))));
        assert_ne!(css("notes", &Ui::default()), css("todo", &Ui::default()), "the hue is the app's");
        let knobs = Ui { theme: Look::Tokens, hue: Some(210), layout: false, ..Ui::default() };
        let own = css("notes", &knobs);
        assert!(
            own.contains("--aio-hue:210;") && own.contains("body{"),
            "[ui] hue, and the whole look whatever its theme"
        );
        assert!(!own.contains(":where(main)"), "[ui] layout");
    }

    #[test]
    fn adopt_refuses_to_overwrite_the_file_it_gave_unless_forced() {
        let d = tempfile::tempdir().unwrap();
        adopt(d.path(), "notes", &Ui::default(), false).unwrap();
        std::fs::write(d.path().join(FILE), "/* mine */").unwrap();
        assert!(adopt(d.path(), "notes", &Ui::default(), false).unwrap_err().contains("--force"));
        assert_eq!(std::fs::read_to_string(d.path().join(FILE)).unwrap(), "/* mine */");
        adopt(d.path(), "notes", &Ui::default(), true).unwrap();
        assert!(std::fs::read_to_string(d.path().join(FILE)).unwrap().contains("@layer aio"));
    }
}
