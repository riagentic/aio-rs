// Its own test binary: the reporter and the panic hook are global.
use aio::prelude::*;

#[cell]
pub struct Boom {
    pub n: u32,
}

#[methods]
impl Boom {
    pub fn blow(&mut self) {
        panic!("method bug");
    }

    pub fn later(&mut self, cx: &Cx) {
        cx.run(Effect::new("fetch", |_: Handle<Boom>| async move { panic!("effect bug") }));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_panic_report_names_the_call_or_effect_it_happened_in() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join("reports");
    aio::report::init(aio::report::About { id: "boom".into(), dir: dir.clone(), ..Default::default() });
    aio::report::on_panic();
    let h = Handle::<Boom>::memory();
    assert!(h.blow().unwrap_err().why.contains("panicked: method bug"));
    h.later().unwrap();
    let titles = || -> Vec<String> {
        aio::report::list(&dir).iter().map(|(_, v)| v["body"].as_str().unwrap_or("").to_string()).collect()
    };
    for _ in 0..200 {
        if titles().len() == 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let all = titles();
    assert!(all.iter().any(|t| t.starts_with("method bug at ") && t.contains(" in boom.blow (thread ")), "{all:?}");
    assert!(all.iter().any(|t| t.starts_with("effect bug at ") && t.contains(" in effect boom.fetch")), "{all:?}");
}
