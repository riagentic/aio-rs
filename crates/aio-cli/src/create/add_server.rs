//! The `thing` server functions (`ax add server thing`): typed async code
//! that runs only on the server; the launcher serves them (`http`). A
//! client calls one with `remote.run(LIST, ()).await`, a person with
//! `ax call <app> --fn thing.list`.
use aio::fns::{Access, ServerFn};

/// Each one declared once, typed at both ends: (arguments), answer.
pub const LIST: ServerFn<(), Vec<String>> = ServerFn::new("thing.list");

/// `http` serving them.
pub fn http(http: aio::http::Http) -> aio::http::Http {
    http.server_fn(LIST, Access::Open, |_req, ()| async move { Ok(Vec::new()) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_answers() {
        aio::rt().block_on(async {
            let h = aio::Handle::<Probe>::memory();
            let server = aio::testing::TestServer::start(h, http(aio::http::Http::default())).await.unwrap();
            let client = server.client("ana").await.unwrap();
            assert_eq!(client.run(LIST, ()).await, Ok(vec![]));
        });
    }

    /// A cell to serve them beside.
    #[aio::cell(shared)]
    pub struct Probe {}

    #[aio::methods]
    impl Probe {}
}
