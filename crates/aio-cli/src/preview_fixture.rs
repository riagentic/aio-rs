//! Written by `ax preview` for one run, then removed.
#[test]
fn aio_preview() {
    let ui = aio::testing::TestUi::with(crate::preview::tests::Greeting, crate::preview::tests::GreetingProps { name: "Ada".into(), times: 2 });
    println!("\n--- ax preview ---\n{}\n--- ax preview end ---", ui.surface());
}
