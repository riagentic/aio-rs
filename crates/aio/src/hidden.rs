//! A `#[server]` field of a shared cell: the app holds its value, a synced
//! copy never does. Reading it on a copy fails loudly (aio throws when client
//! code reads a field its view hides) instead of reading a default that
//! looks real: a predicted call that reads it is refused there, so mark that
//! method `#[client(predict = false)]`; a view that reads it panics.
//!
//! A `Hidden<T>` field that isn't `#[server]` is for `visible`: the view
//! of a viewer who may not see it holds [`Hidden::withheld`], and reading it
//! on their copy panics the same way (aio throws on reading a field the
//! view hides), where a plain field would read as a default that looks real.
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
    /// Hidden by the view (`visible`), not a `#[server]` field.
    hid: bool,
}

impl<T> Hidden<T> {
    pub fn new(value: T) -> Self {
        Self { value: Some(value), gone: None, hid: false }
    }

    /// Not shown to this viewer: in a `visible` view, for a field they may
    /// not see. On their copy any read panics, naming the field.
    pub fn withheld() -> Self {
        Self { value: None, gone: None, hid: true }
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
        *self = Self { value: None, gone: Some(name), hid: false };
    }

    /// On a copy: if the view hid it, it is the field `name`.
    #[doc(hidden)]
    pub fn name(&mut self, name: &'static str) {
        if self.value.is_none() {
            self.gone = Some(name);
        }
        self.hid = true;
    }

    #[track_caller]
    fn absent(&self) -> ! {
        let field = self.gone.unwrap_or("a Hidden field");
        match self.hid {
            true => panic!(
                "{field}: hidden from this viewer by the cell's `visible`: check `.get()` (None where hidden), or \
                 show it to them"
            ),
            false => panic!(
                "{field}: a #[server] field, never sent to a synced copy: read it in the app (a \
                 #[client(predict = false)] method runs there only), or publish a fact field beside it"
            ),
        }
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

/// What a view sends for a field it hides: never a value of the field's.
const HID: &str = "$hidden";

/// Saved as its value (aio's file has the plain field); one this copy or
/// view doesn't have as `{"$hidden": true}`.
impl<T: Serialize> Serialize for Hidden<T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match &self.value {
            Some(v) => v.serialize(s),
            None => {
                use serde::ser::SerializeMap;
                let mut m = s.serialize_map(Some(1))?;
                m.serialize_entry(HID, &true)?;
                m.end()
            }
        }
    }
}

impl<'de, T: serde::de::DeserializeOwned> Deserialize<'de> for Hidden<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = serde_json::Value::deserialize(d)?;
        if v.as_object().is_some_and(|o| o.len() == 1 && o.get(HID) == Some(&serde_json::Value::Bool(true))) {
            return Ok(Self::withheld());
        }
        T::deserialize(v).map(Self::new).map_err(serde::de::Error::custom)
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
        // Hidden by a view: sent as such, and a read on the copy says so.
        let sent = serde_json::to_value(Hidden::<String>::withheld()).unwrap();
        assert_eq!(sent, serde_json::json!({ "$hidden": true }));
        let mut got: Hidden<String> = serde_json::from_value(sent).unwrap();
        got.name("Staff.salary");
        assert_eq!(got.get(), None);
        let why = std::panic::catch_unwind(|| got.len()).unwrap_err().downcast_ref::<String>().unwrap().clone();
        assert!(why.starts_with("Staff.salary: hidden from this viewer by the cell's `visible`"), "{why}");
        // An Option inside keeps its None: no view hid it.
        let none: Hidden<Option<u8>> =
            serde_json::from_value(serde_json::to_value(Hidden::new(None::<u8>)).unwrap()).unwrap();
        assert_eq!(none.get(), Some(&None));
    }
}
