//! `ax sql`: one read-only query against the app's store, answered as JSON
//! rows. SQLite judges what is read-only, not a scan of the text: the store
//! is opened read-only and the statement must be one that writes nothing,
//! so no quote or comment can smuggle a write past it (aio's lexer bug). An
//! answer too big is refused, never cut: a cut list reads as the whole table.
use aio::serde_json::{Map, Value};
use base64::Engine;
use rusqlite::Connection;
use rusqlite::types::ValueRef;

type Out = Result<String, String>;

/// Most rows an answer holds.
pub const MAX_ROWS: usize = 10_000;
/// Most bytes an answer holds (as UTF-8, its real size).
pub const MAX_BYTES: usize = 10_000_000;

/// The store's tables.
pub const TABLES: &str = "SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name";

fn json(v: ValueRef) -> Value {
    match v {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => i.into(),
        ValueRef::Real(f) => f.into(),
        ValueRef::Text(t) => String::from_utf8_lossy(t).into(),
        ValueRef::Blob(b) => base64::engine::general_purpose::STANDARD.encode(b).into(),
    }
}

/// `sql`'s rows as a pretty JSON list of `{column: value}` (blobs as base64).
pub fn query(db: &Connection, sql: &str) -> Out {
    let mut st = db.prepare(sql).map_err(|e| e.to_string())?;
    if !st.readonly() {
        return Err("ax sql is read-only: SELECT (or WITH … SELECT) only".into());
    }
    let cols: Vec<String> = st.column_names().map(String::from).collect();
    let mut rows = st.query([]).map_err(|e| e.to_string())?;
    let (mut out, mut bytes) = (Vec::new(), 0);
    while let Some(r) = rows.next().map_err(|e| e.to_string())? {
        if out.len() == MAX_ROWS {
            return Err(format!(
                "more than {MAX_ROWS} rows: add your own LIMIT (or a WHERE) so the answer is the one you asked for"
            ));
        }
        let row: Map<String, Value> =
            cols.iter().enumerate().map(|(i, c)| (c.clone(), r.get_ref(i).map_or(Value::Null, json))).collect();
        let row = Value::Object(row);
        bytes += row.to_string().len();
        if bytes > MAX_BYTES {
            return Err(format!("the answer is over {MAX_BYTES} bytes: add a tighter LIMIT or fewer columns"));
        }
        out.push(row);
    }
    aio::serde_json::to_string_pretty(&out).map_err(|e| e.to_string())
}

/// The table names, one a line.
pub fn tables(db: &Connection) -> Out {
    let mut st = db.prepare(TABLES).map_err(|e| e.to_string())?;
    let names: Vec<String> = st
        .query_map([], |r| r.get(0))
        .map_err(|e| e.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    Ok(if names.is_empty() { "no tables".into() } else { names.join("\n") })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::OpenFlags;

    fn store() -> (tempfile::TempDir, Connection) {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("state.db");
        Connection::open(&p)
            .unwrap()
            .execute_batch(
                "CREATE TABLE notes (id INTEGER PRIMARY KEY, body TEXT, pic BLOB, w REAL);
                 INSERT INTO notes VALUES (1, 'keep me', x'0102', 0.5);",
            )
            .unwrap();
        let db = Connection::open_with_flags(&p, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        (d, db)
    }

    fn rows(db: &Connection, q: &str) -> Value {
        aio::serde_json::from_str(&query(db, q).unwrap()).unwrap()
    }

    #[test]
    fn rows_come_back_as_json_and_tables_are_listed() {
        let (_d, db) = store();
        assert_eq!(
            rows(&db, "SELECT * FROM notes"),
            aio::serde_json::json!([
                { "id": 1, "body": "keep me", "pic": "AQI=", "w": 0.5 }
            ])
        );
        assert_eq!(tables(&db).unwrap(), "notes");
    }

    #[test]
    fn no_write_gets_past_whatever_the_quotes_and_comments_hide() {
        let (_d, db) = store();
        for q in [
            "WITH x AS (SELECT 1 LIMIT 1), y AS (SELECT '--')\nDELETE FROM notes WHERE 'b' = 'b'",
            "WITH x AS (SELECT 1 LIMIT 1), y AS (SELECT '/*')\nDELETE FROM notes WHERE '*/' = '*/'",
            "DELETE FROM notes",
            "PRAGMA user_version = 9",
        ] {
            assert!(query(&db, q).unwrap_err().contains("read-only"), "{q}");
        }
        // Two statements: SQLite runs one, and refuses the pair.
        assert!(query(&db, "SELECT 1; DELETE FROM notes").is_err());
        assert_eq!(rows(&db, "SELECT id FROM notes"), aio::serde_json::json!([{ "id": 1 }]), "no row was deleted");
        // Write words, `;` and comment marks inside quotes are only text.
        assert_eq!(rows(&db, "SELECT '--' AS a, 'x;y' AS b FROM notes")[0]["b"], "x;y");
        assert_eq!(rows(&db, "SELECT 'DROP TABLE x' AS note -- ; DELETE\nFROM notes")[0]["note"], "DROP TABLE x");
    }

    #[test]
    fn too_big_an_answer_is_refused_not_cut() {
        let (_d, db) = store();
        let many = format!(
            "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n LIMIT {}) SELECT i FROM n",
            MAX_ROWS + 1
        );
        assert!(query(&db, &many).unwrap_err().contains("more than 10000 rows"));
        assert_eq!(query(&db, &many.replace("LIMIT 10001", "LIMIT 10000")).map(|_| ()), Ok(()));
        // Bytes, not chars: 4 M of `日` is 12 MB.
        assert!(
            query(&db, "SELECT replace(printf('%.*c', 4000000, 'x'), 'x', '日') AS t").unwrap_err().contains("bytes")
        );
    }
}
