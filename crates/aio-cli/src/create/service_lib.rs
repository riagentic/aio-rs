//! The service: its cell, served to clients where `aio.toml` `[server]
//! listen` says, with no window. Each `#[client]` method is what a client
//! may call; a client gets the state and every change.
/// What the service keeps: saved on every change, its history kept.
#[aio::cell(persist, shared, id = "board")]
pub struct Board {
    pub posts: Vec<Post>,
}

/// A post: a type the cell keeps is `#[aio::data]` (saved and sent; aio brings serde).
#[aio::data]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Post {
    pub by: aio::By,
    pub text: String,
}

#[aio::methods]
impl Board {
    /// Post `text` as the caller.
    #[client]
    pub fn post(&mut self, cx: &aio::Cx, text: String) -> aio::Result {
        aio::ensure!(!text.trim().is_empty(), "a post needs some text");
        aio::ensure!(text.chars().count() <= 500, "a post is 500 characters at most");
        self.posts.push(Post { by: cx.by().clone(), text });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_post_is_kept_with_who_wrote_it() {
        let h = aio::Handle::<Board>::memory();
        let ann = h.by(aio::By::User("ann".into()));
        ann.post("hello".into()).unwrap();
        assert!(ann.post(" ".into()).is_err());
        assert!(ann.post("x".repeat(501)).is_err());
        assert_eq!(h.state().posts, [Post { by: aio::By::User("ann".into()), text: "hello".into() }]);
    }
}
