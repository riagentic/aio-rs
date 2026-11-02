// An app's aio.toml, parsed when it is built: the literal `aio::config!()`
// writes compiles to the very config the parser makes.
use aio::config::Config;

/// Every section, every kind of value (enums with data, maps, spans, paths, a rate).
const EVERY: &str = include_str!("fixtures/every.toml");

#[test]
fn the_built_literal_is_the_parsed_config() {
    let c = Config::parse(EVERY).unwrap();
    assert_eq!(c.rust(), include_str!("fixtures/config.lit.rs").trim_end());
    let built: Config = include!("fixtures/config.lit.rs");
    assert_eq!(built, c);
    let mut made: Config = aio::config!("tests/fixtures/every.toml");
    let splash = made.build.splash.as_mut().unwrap();
    assert_eq!(splash.png, include_bytes!("fixtures/splash.png"), "the splash's image is built in");
    splash.png = &[];
    // `[build] assets`: each file at its URL path, a dotfile left out.
    let files: &[(&str, &[u8])] = &[("/built/a%20b/c.txt", b"deep\n"), ("/built/dot.txt", b"dot\n")];
    assert_eq!(made.build.asset_files, files);
    assert_eq!(aio::config::asset_path("/built/a%20b/c.txt"), std::path::Path::new("built/a b/c.txt"));
    made.build.asset_files = &[];
    assert_eq!(made, c, "aio::config!() builds the same config in, no Result");
    let bad = |toml: &str| Config::parse(&format!("[app]\nid = \"lit\"\n[build]\n{toml}")).unwrap_err();
    assert!(bad("assets = { \"/ws\" = \"m\" }").contains("[build] assets \"/ws\" is not a prefix"));
    assert!(bad("assets = { \"/m\" = \"../m\" }").contains("beside aio.toml"));
    assert!(bad("assets = { \"/m\" = \"/srv/m\" }").contains("beside aio.toml"));
    let both =
        Config::parse("[app]\nid = \"lit\"\n[build]\nassets = { \"/m\" = \"m\" }\n[server.dirs]\n\"/m\" = \"m\"");
    assert!(both.unwrap_err().contains("both in [build] assets and [server] dirs"));
    let bad = |toml: &str| Config::parse(&format!("[app]\nid = \"lit\"\n[build.splash]\n{toml}")).unwrap_err();
    assert!(bad("image = \"../s.png\"").contains("in the app's folder"));
    assert!(bad("image = \"s.jpg\"").contains("is a .png"));
    assert!(bad("image = \"s.png\"\nbackground = [\"#fff\", \"red\"]").contains("#rgb or #rrggbb"));
    assert!(bad("image = \"s.png\"\nmin_ms = 2000\nmax_ms = 1000").contains("min_ms is more than max_ms"));
    let plain = Config::parse("[app]\nid = \"lit\"").unwrap();
    assert!(bad("image = \"s.png\"\nbar_height = 0").contains("bar_height is px"));
    assert!(bad("image = \"s.png\"\nposition = \"left\"").contains("unknown variant"));
    assert!(plain.rust().starts_with("aio::config::Config{app: aio::config::App{id: "), "{}", plain.rust());
}

#[test]
fn the_one_aio_toml_is_the_nearest_from_the_crate_up() {
    let d = tempfile::tempdir().unwrap();
    let app = d.path().join("notes/app/src");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::write(d.path().join("notes/aio.toml"), "[app]\nid = \"notes\"").unwrap();
    let find = |p: &std::path::Path| aio::config::find(p);
    assert_eq!(find(&d.path().join("notes/app")), Some(d.path().join("notes/aio.toml")));
    std::fs::write(d.path().join("notes/app/aio.toml"), "[app]\nid = \"inner\"").unwrap();
    assert_eq!(find(&d.path().join("notes/app")), Some(d.path().join("notes/app/aio.toml")), "the nearest wins");
}
