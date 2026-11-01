//! A `#[server]` field of a shared cell: the app holds its value, a synced
//! copy never does. Reading it on a copy fails loudly (aio throws when client
//! code reads a field its view hides) instead of reading a default that
//! looks real: a predicted call that reads it is refused there, so mark that
//! method `#[client(predict = false)]`; a view that reads it panics.
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::ops::{Deref, DerefMut};

/// `#[server] pub api_key: Hidden<String>`: reads and writes as its value
/// (`self.api_key.is_empty()`, `*self.api_key = k`) in the app; on a synced
/// copy, where it isn't sent, any read panics, naming the field. Code that
/// runs on both sides asks [`Hidden::get`].
#[derive(Clone, PartialEq, Eq)]
pub struct Hidden<T> {
    value: Option<T>,
    /// Set on a copy: the field it is (`Mailbox.imap_password`).
    gone: Option<&'static str>,
}

impl<T> Hidden<T> {
    pub fn new(value: T) -> Self {
        Self { value: Some(value), gone: None }
    }

    /// Its value, or `None` on a synced copy (never sent there).
    pub fn get(&self) -> Option<&T> {
        self.value.as_ref()
    }

    /// Its value, taken (`None` on a copy).
    pub fn into_inner(self) -> Option<T> {
        self.value
    }

    /// On a copy: the field `name` is not there.
    #[doc(hidden)]
    pub fn conceal(&mut self, name: &'static str) {
        *self = Self { value: None, gone: Some(name) };
    }

    #[track_caller]
    fn absent(&self) -> ! {
        panic!(
            "{}: a #[server] field, never sent to a synced copy: read it in the app (a \
             #[client(predict = false)] method runs there only), or publish a fact field beside it",
            self.gone.unwrap_or("a Hidden field")
        )
    }
}

/// Its value's default: a new cell holds it (only a copy lacks it).
impl<T: Default> Default for Hidden<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T> From<T> for Hidden<T> {
    fn from(value: T) -> Self {
        Self::new(value)
    }
}

impl<T> Deref for Hidden<T> {
    type Target = T;

    #[track_caller]
    fn deref(&self) -> &T {
        match &self.value {
            Some(v) => v,
            None => self.absent(),
        }
    }
}

impl<T> DerefMut for Hidden<T> {
    #[track_caller]
    fn deref_mut(&mut self) -> &mut T {
        if self.value.is_none() {
            self.absent();
        }
        self.value.as_mut().expect("present")
    }
}

/// Its value never shows in `{:?}` (a log, a failed assert): it is a secret.
impl<T> std::fmt::Debug for Hidden<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.value {
            Some(_) => f.write_str("Hidden(..)"),
            None => f.write_str("Hidden(not on this copy)"),
        }
    }
}

/// Saved as its value (aio's file has the plain field); a copy's, never sent, as `null`.
impl<T: Serialize> Serialize for Hidden<T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.value.serialize(s)
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Hidden<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        T::deserialize(d).map(Self::new)
    }
}

#[cfg(test)]
mod tests {
    use super::Hidden;

    #[test]
    fn it_reads_as_its_value_and_fails_loudly_on_a_copy() {
        assert_eq!(Hidden::<u32>::default().get(), Some(&0), "a new cell holds its default");
        let mut h = Hidden::new("k1".to_string());
        assert!(!h.is_empty());
        h.push('!');
        assert_eq!(*h, "k1!");
        assert_eq!(serde_json::to_value(&h).unwrap(), "k1!", "saved as its value");
        assert_eq!(serde_json::from_value::<Hidden<String>>("k2".into()).unwrap(), Hidden::new("k2".into()));
        assert_eq!(format!("{h:?}"), "Hidden(..)", "never in a log");
        h.conceal("Keys.api_key");
        assert_eq!(h.get(), None);
        let read = std::panic::catch_unwind(|| h.len()).unwrap_err();
        let why = read.downcast_ref::<String>().unwrap();
        assert!(why.starts_with("Keys.api_key: a #[server] field, never sent"), "{why}");
    }
}
