//! The `thing` cell (`ax add cell thing`): its state, saved, and the
//! only ways to change it.
//! Its views: `thing::use_live()` in the root view, then `part(|t| &t.count)`
//! and `act!(add(1))` in this module.

#[aio::cell(persist, id = "thing")]
pub struct Thing {
    pub count: i64,
}

#[aio::methods]
impl Thing {
    /// Count `by` more (or less).
    pub fn add(&mut self, by: i64) {
        self.count += by;
    }
}

aio::client!(Thing, thing);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_counts() {
        let h = aio::Handle::<Thing>::memory();
        h.add(2).unwrap();
        h.add(-1).unwrap();
        assert_eq!(h.state().count, 1);
    }
}
