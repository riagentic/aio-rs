// Its own test binary: the sandbox is process-wide.
use aio::prelude::*;
use aio::testing::TestCells;

#[cell]
pub struct Probe {
    pub n: u32,
}

#[methods]
impl Probe {}

#[test]
fn a_test_runs_in_a_world_of_its_own_and_real_data_is_refused() {
    let real = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(std::path::PathBuf::from);
    let t = TestCells::new();
    let root = aio::paths::sandbox().unwrap().root().to_path_buf();
    // Every dir aio would use is in the test's world.
    let (data, logs, cache) = aio::paths::locate("probe").unwrap();
    for d in [&data, &logs, &cache, &aio::paths::tool_home()] {
        assert!(d.starts_with(&root), "{} is outside {}", d.display(), root.display());
    }
    let dirs = aio::paths::app("probe").unwrap();
    assert!(dirs.data.starts_with(&root));
    drop(dirs);
    // The user's data is refused, before anything is made; a temp dir is fine.
    if let Some(real) = real.filter(|h| !h.starts_with(std::env::temp_dir())) {
        let theirs = real.join(".aio-sandbox-probe-never-made");
        let e = aio::paths::AppDirs::at(&theirs, "probe").unwrap_err();
        assert!(e.to_string().starts_with("a test may not touch real app data: "), "{e}");
        let e = Store::<Probe>::open(theirs.join("state.db")).err().unwrap();
        assert!(e.to_string().contains("a test may not touch real app data"), "{e}");
        assert!(!theirs.exists(), "nothing was made");
    }
    let tmp = tempfile::tempdir().unwrap();
    Store::<Probe>::open(tmp.path().join("state.db")).unwrap();
    // The last harness gone, the world goes with it, and paths are real again.
    drop(t);
    assert!(!root.exists(), "{} left behind", root.display());
    assert!(!aio::paths::tool_home().starts_with(&root));
}
