// A source `ax tsgen` reads (aio-rs-cli's `tsgen` test): its output is
// `fixture.ts`, which `use.ts` type-checks against (`tsc -p test`).
use aio::prelude::*;
use std::collections::BTreeMap;

#[aio::data]
#[serde(rename_all = "camelCase")]
pub struct Note {
    pub text: String,
    pub due_at: Option<i64>,
    #[serde(skip)]
    pub cache: u32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub enum Filter {
    All,
    Tag(String),
    Range(u32, u32),
    Due { before: i64 },
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Shape {
    Circle { r: f64 },
    Dot,
}

#[aio::data]
pub struct Id(pub Uuid);

#[cell(persist, shared, version = 2)]
pub struct Board {
    pub notes: BTreeMap<String, Note>,
    pub filter: Filter,
    pub shapes: Vec<Shape>,
    pub owner: Option<Id>,
    #[server]
    pub secret: Hidden<String>,
    pub seen: Hidden<u32>,
}

#[methods]
impl Board {
    #[client]
    pub fn add(&mut self, cx: &Cx, text: String, due_at: Option<i64>) -> Result<Uuid> {
        let id = cx.id();
        self.notes.insert(id.to_string(), Note { text, due_at, cache: 0, tags: vec![] });
        Ok(id)
    }

    #[client(role = "admin")]
    pub fn clear(&mut self) -> Result {
        self.notes.clear();
        Ok(())
    }

    #[client]
    pub fn pick(&mut self, filter: Filter) {
        self.filter = filter;
    }

    /// Not `#[client]`: no client may call it, so no type.
    pub fn wipe(&mut self) {}
}

/// Not shared: never on the wire.
#[cell]
pub struct Local {
    pub n: u32,
}
