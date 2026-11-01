//! Written by `ax timeline` and `ax record` for one run, then removed:
//! the app's cells replayed from a copy of its store, by its own code.
#[test]
fn aio_states() {
    // Set by ax; without them there is nothing to replay.
    let (Ok(store), Ok(ask)) = (std::env::var("AIO_STATES_STORE"), std::env::var("AIO_STATES_ASK")) else {
        return;
    };
    println!("\n--- ax states ---\n{}\n--- ax states end ---", states(&store, &ask));
}

type Map = aio::serde_json::Map<String, aio::serde_json::Value>;

/// Cell `T`'s whole state right after each event `ask` names for it.
fn at<T: aio::Cell>(store: &str, ask: &aio::serde_json::Value, out: &mut Map) {
    let Some(seqs) = ask[T::ID].as_array() else { return };
    let s = match aio::Store::<T>::open(store) {
        Ok(s) => s,
        Err(e) => {
            out.insert(T::ID.into(), aio::serde_json::json!({ "error": e.0 }));
            return;
        }
    };
    let mut m = Map::new();
    for q in seqs.iter().filter_map(|q| q.as_u64()) {
        let v = match s.state_at(q) {
            Ok(st) => aio::serde_json::to_value(&st).unwrap_or_default(),
            Err(e) => aio::serde_json::json!({ "error": e.0 }),
        };
        m.insert(q.to_string(), v);
    }
    out.insert(T::ID.into(), m.into());
}

/// Each cell's states `ask` names (`{"cell": [seq, …]}`), as JSON.
pub fn states(store: &str, ask: &str) -> String {
    let ask: aio::serde_json::Value = aio::serde_json::from_str(ask).unwrap_or_default();
    let mut out = Map::new();
    at::<crate::replayed::tests::Tally>(store, &ask, &mut out);
    aio::serde_json::Value::Object(out).to_string()
}
