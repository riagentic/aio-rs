# Tables, SQL and blobs

A cell is the reactive working set: what the app shows and changes, kept
whole in memory and copied by each call. What is bigger than that goes
in one of three places beside it, all inside the app's `data/` (so
backups carry them):

| Data | Where | Why |
|---|---|---|
| rows a cell owns, wanted as SQL | `#[table]` fields | queryable with `ax sql`, written by row |
| what no cell holds (caches, imports, logs, millions of rows) | `aio::db::Db` | raw SQL, async, live queries |
| bytes (images, uploads, documents) | `aio::blobs::Blobs` | content-addressed files, served with Range |

How to choose, and how big a cell may be, is at the end:
[big data](#big-data-the-four-tiers).

## A cell's rows as a table

`#[table("name")]` on a field keeps it as the rows of SQL table `name` in
the store's own file, written on each commit.

```rust
use aio::db::{Db, Opts};
use aio::prelude::*;

#[data]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Contact {
    pub id: i64,
    pub name: String,
}

#[cell(persist, id = "book")]
pub struct Book {
    pub title: String,
    #[table("contacts", columns = "id INTEGER PRIMARY KEY, name TEXT NOT NULL")]
    pub contacts: Vec<Contact>,
}

#[methods]
impl Book {
    pub fn add(&mut self, id: i64, name: String) {
        self.contacts.push(Contact { id, name });
    }
}

# fn main() {
let dir = tempfile::tempdir().unwrap();
let file = dir.path().join("state.db");
{
    let h = Handle::<Book>::open(&file).unwrap();
    h.add(1, "Ada".into()).unwrap();
    h.add(2, "Grace".into()).unwrap();
}
assert_eq!(Handle::<Book>::open(&file).unwrap().state().contacts.len(), 2, "the rows came back");

let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
rt.block_on(async {
    let db = Db::open(&file, Opts::default()).unwrap();
    let rows = db.query("SELECT name FROM contacts ORDER BY id", vec![]).await.unwrap();
    #[derive(serde::Deserialize)]
    struct Name {
        name: String,
    }
    let names: Vec<String> = rows.to::<Name>().unwrap().into_iter().map(|n| n.name).collect();
    assert_eq!(names, ["Ada", "Grace"]);
    db.close().await;
});
# }
```

| Form | Field | Rows |
|---|---|---|
| `#[table("contacts")]` | `Vec<Row>` | one per item |
| `#[table("contacts", key = "id")]` | `BTreeMap<String, Row>` (or `HashMap`) | one per entry; each key must equal its row's `id` |
| `…, columns = "id INTEGER PRIMARY KEY, name TEXT NOT NULL"` | either | the table's SQL columns, made at boot |

- **The field stays the source of truth.** It is still in the state, in
  memory and on the wire; only its storage is the table instead of the
  snapshot. The log still holds every call.
- **Writes are by row.** With a primary key (or `key = "col"`), a commit
  writes only the rows that changed and deletes the ones gone; without
  one it rewrites the table. All in the call's own transaction.
- **What a row must look like.** The element type is a flat struct whose
  field names are the columns. Each value is a number, a string or null
  (`Option`); a `bool`, a list or a nested struct refuses the commit, as
  does a field the table has no column for. Store a flag as `i64` (0/1)
  or a string.
- **Without `columns`** the field is a table only if the file already has
  it (a ported aio app's `db:` table); otherwise it stays in the
  snapshot like any field.
- **Changing the columns.** At boot a stored table is brought to the
  declared shape where no data is lost: a column renamed only in case
  takes the declared case; a missing column is added (or the whole
  table made again when it holds no rows); a missing column that rows
  can't fill (`NOT NULL` without a `DEFAULT`) refuses the boot, as does
  a stored `NOT NULL` column without a default that is no longer
  declared. A changed type or nullability, and a stored column no longer
  declared, are said in the log and left as SQLite keeps them: aio-rs
  never drops a column on its own.
- **Rows written from outside.** The rows are read at boot; if they
  differ from the state (another tool wrote the table), the difference
  comes in as one new event. Write through methods, not SQL, so every
  change is in the log.
- A table name the store uses itself (`log`, `snap`, `floor`, `sched`,
  `ops`) refuses the build's boot. `#[table]` needs `#[cell(persist)]`.
- `ax sql <app> "select …"` queries the store read-only (at most 10 000
  rows as JSON); `ax tables <app>` lists its tables.

## Raw SQL

`aio::db::Db::open(path, Opts::default())` opens an app's own SQLite file
(aio's `createDB`): one writer thread runs writes in order, and with
`Opts::readers` set, that many read threads answer reads beside it on
the WAL, so a long read never waits for a write.

```rust
use aio::db::{Db, Opts, Sql};

# fn main() {
let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
rt.block_on(async {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path().join("cache.db"), Opts { readers: 2, ..Opts::default() }).unwrap();
    db.execute("CREATE TABLE seen (url TEXT PRIMARY KEY, hits INTEGER NOT NULL)", vec![]).await.unwrap();

    // A live query: its rows now, then again after each write that changes them.
    let mut live = db.live("SELECT count(*) AS n FROM seen", vec![]).await.unwrap();
    assert_eq!(live.rows().rows[0][0], Sql::Integer(0));
    assert!(live.tables().contains("seen"));

    let row = |url: &str| ("INSERT INTO seen VALUES (?1, 1)".to_string(), vec![Sql::from(url.to_string())]);
    db.batch(vec![row("/a"), row("/b")]).await.unwrap(); // one transaction: both or neither
    assert_eq!(live.changed().await.unwrap().rows[0][0], Sql::Integer(2));

    let bumped = db
        .transaction(|tx| {
            tx.execute("UPDATE seen SET hits = hits + 1 WHERE url = ?1", &[Sql::from("/a".to_string())])?;
            Ok(tx.query("SELECT hits FROM seen WHERE url = '/a'", &[])?.rows[0][0].clone())
        })
        .await
        .unwrap();
    assert_eq!(bumped, Sql::Integer(2));

    // Refused before anything runs: transactions are the API's, one statement a call.
    assert!(db.execute("BEGIN", vec![]).await.is_err());
    assert!(db.execute("DELETE FROM seen; DROP TABLE seen", vec![]).await.is_err());

    let big = Sql::Integer(i64::MAX); // every 64-bit integer, exactly
    db.execute("INSERT INTO seen VALUES ('/max', ?1)", vec![big.clone()]).await.unwrap();
    let got = db.query("SELECT hits FROM seen WHERE url = '/max'", vec![]).await.unwrap();
    assert_eq!(got.rows[0][0], big);
    db.close().await;
});
# }
```

### Opening

| `Opts` field | Default | Does |
|---|---|---|
| `readers` | `0` | read threads beside the writer; with none, the writer answers reads too (and always for `:memory:`) |
| `pragmas` | none | pragmas over `aio::db::PRAGMAS`, by name: `"PRAGMA synchronous = FULL"` replaces only `synchronous` |
| `timeout` | `Some(120 s)` (`aio::db::TIMEOUT`) | how long one request may run; `None`: as long as it takes |
| `guard` | `false` | checked at open: a damaged file is set aside and its daily good copy (made at open) put in its place; `db.recovered()` says so |

- `aio::db::PRAGMAS`: WAL, `synchronous = NORMAL`, a 64 MB cache, a 5 s
  busy timeout, foreign keys on. Each pragma is read back after it is
  set: one SQLite ignored or didn't take (a typo, an unknown value)
  refuses the open, naming it.
- `Db::open(":memory:", …)` is a database in memory: for tests, nothing
  on disk.
- Put the file in the app's data, so backups hold it:
  `aio::paths::app_dirs()?.data.join("cache.db")`. Regenerable bulk goes
  in `app_dirs()?.cache` instead, which no backup carries.

### Requests

| Method | Does |
|---|---|
| `query(sql, params)` | one statement's rows (`Rows { columns, rows }`); a reader answers unless it writes |
| `execute(sql, params)` | one write statement: `Done { changes, last_id }` |
| `batch(vec![(sql, params), …])` | the statements in one transaction: all, or none on the first error |
| `transaction(\|tx\| …)` | a closure in one transaction on the writer: `Ok` commits, `Err` or a panic rolls back; `tx.query`, `tx.execute` inside |
| `live(sql, params)` | a live query ([below](#live-queries)) |
| `snapshot(to)` | a consistent compacted copy (`VACUUM INTO`), synced; an existing `to` is refused |
| `check()` | SQLite's `quick_check`: what it found, empty when sound |
| `close()` | finishes what was asked, then lets go of the file (dropping does the same) |

- Every request is async and bounded by `timeout`: a statement that runs
  longer is interrupted and rolled back, never left running, and the
  error says so.
- Each call runs exactly one statement. `BEGIN`, `COMMIT`, `END` and
  `ROLLBACK` are refused (a `SAVEPOINT` inside a transaction is fine), so
  a stray `BEGIN` can't swallow another caller's writes; several
  statements in one string are refused instead of running the first.
- `Sql` is SQLite's value: `Null`, `Integer(i64)`, `Real(f64)`,
  `Text(String)`, `Blob(Vec<u8>)`, with `From` for the usual types
  (`5.into()`, `"a".to_string().into()`, `None::<i64>.into()`). Integers
  are exact over all of `i64`.
- `rows.to::<T>()` turns each row into `T` by column name (a blob as a
  list of bytes).
- A request that panics fails its caller only; the thread goes on.

### Live queries

`db.live(sql, params)` (aio's `reactiveDB().select`) runs the query now
and again whenever a write changes a table it reads; `live.changed()`
waits for the next new rows.

- The tables it reads come from SQLite itself as it prepares the
  statement (views, triggers, `WITH`, quoted names included): no SQL is
  parsed. `live.tables()` lists them; a query that reads no table is said
  in the log, since nothing will refresh it.
- A write that leaves its rows as they were wakes nobody; writes it fell
  behind on run it once.
- Only writes through the same `Db` wake it, after they commit; a write
  by another process or another `Db` on the file doesn't.

## Blobs, files and big data

Bytes never go in a cell, the log or the sync wire. `aio::blobs::Blobs`
(aio's `app.blobs`) is a content-addressed store in `data/files/blobs`;
the cell keeps the blob's id.

```rust
use aio::blobs::Blobs;

# fn main() {
let dir = tempfile::tempdir().unwrap();
let blobs = Blobs::at(dir.path().join("blobs"));
let photo = blobs.put(&b"not really a JPEG"[..], Some("cat.jpg")).unwrap();
assert_eq!(photo.id.len(), 64, "the SHA-256 of the bytes is the id");
assert_eq!(photo.size, 17);
assert_eq!(blobs.put(&b"not really a JPEG"[..], Some("copy.jpg")).unwrap().id, photo.id, "same bytes, one blob");
assert_eq!(blobs.read(&photo.id, 0, 3).unwrap(), b"not");
assert_eq!(blobs.info(&photo.id).unwrap().unwrap().name.as_deref(), Some("cat.jpg"), "the first name stays");
assert_eq!(aio::blobs::url(&photo.id), format!("/__aio/blobs/{}", photo.id));
assert_eq!(blobs.list().unwrap().len(), 1);
assert!(blobs.delete(&photo.id).unwrap());
assert!(blobs.info(&photo.id).unwrap().is_none());
# }
```

| Method | Does |
|---|---|
| `Blobs::of(&dirs)` | the app's store, `data/files/blobs` |
| `Blobs::at(dir)` | a store in `dir` (made on the first put) |
| `put(reader, name)` | stores the bytes as they are read: `Blob { id, size, name }` |
| `info(id)` | `Some(Blob)`, or `None` when it isn't here |
| `read(id, start, end)` | bytes `start..end` (end exclusive, clamped to the size) |
| `path(id)` | the file holding it, to stream or hand to another tool |
| `list()` | every blob |
| `delete(id)` | removes it: `false` when it wasn't here |

- `put` streams: the bytes are hashed and written to a temp file as they
  come, synced, then renamed to their hash, so a blob is whole under its
  id or absent, and a multi-gigabyte upload costs a few chunks of memory.
  A crash's leftover temp file is swept by a later put once it is an hour
  old.
- Same bytes, same id, one file: the name never changes the id, and only
  the first put's name is kept. So `delete(id)` removes the bytes for
  every record that stored them: delete only when no cell names it.
- A blob that isn't here is `None` or `false`; a store that can't be
  read is an error, never an empty answer. An id that isn't 64 hex
  characters is refused, so no crafted id becomes a path.
- A peer app reads and puts another app's blobs directly (`Peer::blobs`,
  [peers](../state/plugins.md#use-a-running-app-peers)).

### Uploads and serving

A served app gives the bytes at `/__aio/blobs/<id>` (`Http::blobs`):
behind the app's key or a session, with Range (206, 416 past the end),
cached for good (the id is the content). A route streams an upload
straight into `put`:

```rust
use aio::blobs::Blobs;
use aio::http::{Http, Reply, Route};
use aio::prelude::*;
use aio::testing::TestServer;
use std::collections::BTreeMap;

#[data]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct File {
    pub name: String,
    pub size: u64,
}

#[cell(persist, shared)]
pub struct Files {
    pub by_id: BTreeMap<String, File>,
}

#[methods]
impl Files {
    pub fn record(&mut self, id: String, name: String, size: u64) {
        self.by_id.insert(id, File { name, size });
    }
}

fn http(h: Handle<Files>, blobs: Blobs) -> Http {
    let store = blobs.clone();
    let upload = Route::post("/upload/:name", move |req| {
        let (h, blobs) = (h.clone(), store.clone());
        async move {
            let name = req.param("name").unwrap_or("file").to_string();
            let Some(body) = req.body_reader() else { return Reply::text("no body").status(400) };
            // The body is a blocking reader: off the async threads, into the store.
            let n = name.clone();
            let put = tokio::task::spawn_blocking(move || blobs.put(body, Some(&n))).await;
            let Ok(Ok(blob)) = put else { return Reply::text("upload failed").status(500) };
            if blob.size == 0 {
                return Reply::text("empty body").status(400);
            }
            // The cell keeps what it is; the bytes stay on disk.
            if let Err(e) = h.record(blob.id.clone(), name, blob.size) {
                return Reply::text(e.why).status(400);
            }
            Reply::json(&serde_json::json!({ "id": blob.id, "url": aio::blobs::url(&blob.id) }))
        }
    });
    Http::default().blobs(blobs).route(upload.streamed().max_body(64 << 20))
}

# fn main() {
let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
rt.block_on(async {
    let dir = tempfile::tempdir().unwrap();
    let blobs = Blobs::at(dir.path().join("blobs"));
    let h = Handle::<Files>::memory();
    let server = TestServer::start(h.clone(), http(h.clone(), blobs)).await.unwrap();

    let up = server.fetch("POST", "/upload/notes.txt", "hello, world").await.unwrap();
    assert_eq!(up.status, 200);
    let answer: serde_json::Value = serde_json::from_str(&up.body).unwrap();
    let id = answer["id"].as_str().unwrap();
    assert_eq!(h.state().by_id[id].size, 12);

    let got = server.fetch("GET", answer["url"].as_str().unwrap(), "").await.unwrap();
    assert_eq!((got.status, got.body.as_str()), (200, "hello, world"));
    let stranger = server.fetch_with("GET", answer["url"].as_str().unwrap(), &[], "").await.unwrap();
    assert_eq!(stranger.status, 401, "blobs need the key, as the app does");
});
# }
```

- `Route::streamed()` hands the handler its body as it arrives
  (`req.body_reader()`, a blocking `std::io::Read`); a body said to be
  bigger than `max_body` is refused (413) before the handler runs.
- A file the app keeps elsewhere: `aio::http::serve_file(&req, path,
  content_type)` serves it the same way (GET and HEAD, Range, an `ETag`
  with 304, an inert type unless you name one). The path is yours to
  choose: never join a raw request path without taking its file name.
- A folder of `data/files` served whole: `[server.dirs]` in `aio.toml`
  ([sync](../state/sync.md#server-functions-and-routes)).

## Big data: the four tiers

Pick the place by what the data is:

| Data | Tier | How |
|---|---|---|
| what the UI works with now: the visible page, the selection, the form | 1. cell state | `#[cell]` fields: synced, replayed, copied by each call |
| structured bulk: thousands to millions of rows | 2. tables | `#[table]` fields (rows that are also the working set), or a `Db` table no cell holds (the bulk) |
| bytes: images, media, dumps | 3. blobs | `Blobs`, the cell keeps ids |
| heavy work over the data | 4. off the cell | an effect or async method does the work, the cell holds its progress ([effects](../state/effects.md), [scheduling](../state/scheduling.md)) |

A `#[table]` field is still state: it rides every connect and every
copy of the cell. Rows no view shows at once belong in a `Db` table, with
the window the UI shows in the cell.

### Recipe: a million rows, fifty on screen

```rust
use aio::db::{Db, Opts, Sql};
use aio::prelude::*;

#[cell]
pub struct Inbox {
    pub offset: i64,
    pub total: i64,
    pub page: Vec<String>,
}

#[methods]
impl Inbox {
    /// What the query found for the page at `offset`: the cell holds only the window.
    pub fn show(&mut self, offset: i64, total: i64, page: Vec<String>) {
        (self.offset, self.total, self.page) = (offset, total, page);
    }
}

const PAGE: i64 = 50;

/// The window at `offset` (the I/O stays out of the method).
async fn window(db: &Db, offset: i64) -> (i64, Vec<String>) {
    let total = db.query("SELECT count(*) FROM mail", vec![]).await.unwrap().rows[0][0].clone();
    let rows = db
        .query("SELECT subject FROM mail ORDER BY id LIMIT ?1 OFFSET ?2", vec![PAGE.into(), offset.into()])
        .await
        .unwrap();
    let page = rows.rows.into_iter().filter_map(|r| match &r[0] { Sql::Text(s) => Some(s.clone()), _ => None });
    (if let Sql::Integer(n) = total { n } else { 0 }, page.collect())
}

# fn main() {
let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
rt.block_on(async {
    let db = Db::open(":memory:", Opts::default()).unwrap();
    db.execute("CREATE TABLE mail (id INTEGER PRIMARY KEY, subject TEXT NOT NULL)", vec![]).await.unwrap();
    db.execute(
        "INSERT INTO mail (subject) WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 100000)
         SELECT 'mail ' || i FROM n",
        vec![],
    )
    .await
    .unwrap();

    let h = Handle::<Inbox>::memory();
    let (total, page) = window(&db, 200).await;
    h.show(200, total, page).unwrap();
    let s = h.state();
    assert_eq!((s.total, s.page.len(), s.page[0].as_str()), (100_000, 50, "mail 201"));
    db.close().await;
});
# }
```

In an app the query runs where I/O belongs: an effect or an async method
after the call that moved the page, or the view's own task. The kit's
windowed list draws only the rows on screen
([performance](../performance.md)). Order by an indexed column and page
by key (`WHERE id > ?1 LIMIT 50`) when `OFFSET` grows large.

### How big a cell may be

Every call copies the cell, every snapshot writes it whole, and every
client's first message holds it. Keep a cell to what the UI works with:
under about a megabyte.

- **Declare it**: `[budgets] state = "1MB"` (one cell's saved state),
  `payload = "500KB"` (one push to a client) and `broadcast_rate =
  "20/s"` (pushes to one client) are sizes the app commits to. A breach
  is logged once with its worst reading, sent to `on_problem` as
  `budget:<name>`, and turns `/__aio/health` degraded, so a test or CI
  can fail on it ([aio.toml](../reference/aio-toml.md#budgets)).
- Without a `payload` or `broadcast_rate` budget, a push to one client
  over 500 KB, more than 30 pushes a second to one client, or over 1 MB a
  second to one client is said in the log as pressure, naming the likely
  cause.
- `ax heap <app>` shows each cell's saved size and log, `ax cost <app>`
  what each cell sends its clients and its biggest keys.
- **Big state made cheap**: keep large parts as `Arc<…>` fields, or
  `aio::imbl` collections (feature `imbl`: `OrdMap`, `OrdSet`, `Vector`),
  so a call's copy shares what it didn't change; give a growing
  collection a map's shape so a patch sends one key
  ([offline](offline.md#shaping-state-for-the-wire)).

## Limits, compared with aio

- **No typed schema builder.** aio's column helpers and `ColumnOpts`
  are `columns = "…"` in SQL here; `aio::db` has no query builder.
- **No size guard on state by default.** aio warns at 1 MB and errors at
  16 MB on its own; aio-rs judges a cell's size only against a declared
  `[budgets] state`.
- **Bound rows are flat.** No `bool`, list or nested value in a
  `#[table]` row (aio stores its JSON); use numbers or strings.
- **Live queries see one `Db`'s writes**, as aio's `reactiveDB` sees its
  own: no cross-process invalidation.
- **Blob reads are by range into memory** (`read`) or by file (`path`);
  there is no async stream type.

## Common mistakes

- A `Vec<u8>` of file contents in a cell: every call copies it and every
  snapshot writes it. Put it in `Blobs` and keep the id.
- A `bool` in a `#[table]` row: the commit is refused. Use `i64` or a
  string.
- `BEGIN … COMMIT` as statements: refused; use `batch` or `transaction`.
- Writing to the store's own file with `Db`: the store owns its tables,
  and bound rows written from outside come in as an event at the next
  boot. Use methods and `#[table]` fields, or a file of your own.
- Expecting a live query to see writes made by another process or
  connection: only this `Db`'s writes wake it.
- Running a query inside a method: methods do no I/O. Query outside and
  pass the rows in.
- Deleting a blob another record still names: same bytes, same id.

## API

`cargo doc -p aio-rs --no-deps --open`: `aio::db` (`Db`, `Opts`, `Rows`,
`Done`, `Live`, `Tx`, `Sql`, `PRAGMAS`, `TIMEOUT`), `aio::blobs` (`Blobs`,
`Blob`, `url`, `id_ok`), `aio::http` (`Route::streamed`, `Req::body_reader`,
`Http::blobs`, `serve_file`), `aio::budgets`, `aio::imbl`.
