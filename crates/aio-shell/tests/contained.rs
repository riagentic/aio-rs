// Its own test binary: the test world (`aio::paths::sandbox`) is process-wide.

#[test]
fn a_test_in_its_sandbox_opens_no_link_or_file() {
    let world = aio::paths::sandbox().unwrap();
    let url = aio::rt().block_on(aio_shell::os::open_url("https://example.com/")).unwrap_err();
    assert_eq!(url.to_string(), "not opened (AIORT_NO_OPEN, a contained run): https://example.com/");
    let file = aio::rt().block_on(aio_shell::os::open_path(world.root())).unwrap_err();
    assert!(file.to_string().starts_with("not opened (AIORT_NO_OPEN"), "{file}");
}
