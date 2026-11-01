// Its own test binary: the updates runtime and cell are process-wide.
use aio::update::{Found, Offer, Phase, UpdatesCalls};

#[tokio::test(flavor = "multi_thread")]
async fn a_stubbed_update_offer_reaches_the_cell_a_ui_reads() {
    assert!(aio_shell::update::cell().is_none(), "no updates unless set up");
    let offer = Offer { version: "1.1.0".into(), notes: "faster boot".into(), ..Offer::default() };
    let h = aio_shell::update::stub("1.0.0", Found::Offer(offer));
    let ui = aio_shell::update::cell().expect("the UI's cell is the stub's");
    ui.check().unwrap();
    h.done("check").await;
    let st = ui.state();
    assert_eq!((st.phase, st.current.as_str()), (Phase::Available, "1.0.0"));
    assert_eq!(st.available.as_ref().map(|o| o.notes.as_str()), Some("faster boot"));
    ui.apply().unwrap();
    h.done("apply").await;
    assert_eq!((ui.state().phase, ui.state().progress), (Phase::Restarting, 100), "installed, nothing touched");
    // "Not now": the offer goes, and is not offered again.
    let s = aio_shell::update::stub("1.0.0", Found::Offer(Offer { version: "1.2.0".into(), ..Offer::default() }));
    s.check().unwrap();
    s.done("check").await;
    s.dismiss().unwrap();
    s.check().unwrap();
    s.done("check").await;
    assert_eq!((s.state().phase, s.state().dismissed.as_deref()), (Phase::Idle, Some("1.2.0")));
}
