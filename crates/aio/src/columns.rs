//! A table's columns, typed (aio's `table({ id: pk(), name: text() })` and
//! its `ColumnOpts`), for `#[table("contacts", columns = CONTACTS)]` and for
//! [`crate::db`]'s own tables ([`create`]). Each helper is a `const fn`, so a
//! table's columns are a `const`:
//!
//! ```
//! use aio::columns::{Col, boolean, integer, json, pk, real, refs, text};
//! const ORDERS: &[Col] = &[
//!     pk("id"),
//!     text("customer"),
//!     real("total").default_real(0.0),
//!     text("status").default_text("pending"),
//!     refs("user_id", "users"),
//!     boolean("paid").default_int(0),
//!     json("lines").nullable(),
//! ];
//! assert_eq!(
//!     aio::columns::sql(ORDERS).unwrap(),
//!     "\"id\" INTEGER PRIMARY KEY, \"customer\" TEXT NOT NULL, \"total\" REAL NOT NULL DEFAULT 0, \
//!      \"status\" TEXT NOT NULL DEFAULT 'pending', \"user_id\" INTEGER NOT NULL REFERENCES \"users\", \
//!      \"paid\" BOOLEAN NOT NULL DEFAULT 0, \"lines\" JSON"
//! );
//! ```
//!
//! `boolean` and `json` are aio-rs's: a bound field's `bool` is stored as
//! 0/1 in a `BOOLEAN` column, and a list or an object as JSON text in a `JSON`
//! column, both read back as they were (aio itself refuses them in a row).

/// A column's `DEFAULT`: text, or a finite number.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fill {
    Text(&'static str),
    Int(i64),
    Real(f64),
}

/// One column: `NOT NULL` unless [`Col::nullable`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Col {
    pub name: &'static str,
    /// Its SQL type: `INTEGER`, `TEXT`, `REAL`, `BOOLEAN` or `JSON`.
    pub ty: &'static str,
    pub pk: bool,
    pub nullable: bool,
    pub unique: bool,
    pub default: Option<Fill>,
    /// The table whose primary key it references.
    pub refs: Option<&'static str>,
}

const fn col(name: &'static str, ty: &'static str) -> Col {
    Col { name, ty, pk: false, nullable: false, unique: false, default: None, refs: None }
}

/// `INTEGER PRIMARY KEY` (one per table): rows are updated and deleted by it.
pub const fn pk(name: &'static str) -> Col {
    Col { pk: true, ..col(name, "INTEGER") }
}

/// `TEXT NOT NULL`.
pub const fn text(name: &'static str) -> Col {
    col(name, "TEXT")
}

/// `INTEGER NOT NULL`.
pub const fn integer(name: &'static str) -> Col {
    col(name, "INTEGER")
}

/// `REAL NOT NULL`.
pub const fn real(name: &'static str) -> Col {
    col(name, "REAL")
}

/// `BOOLEAN NOT NULL`: a `bool`, stored as 0/1.
pub const fn boolean(name: &'static str) -> Col {
    col(name, "BOOLEAN")
}

/// `JSON NOT NULL`: a list or an object, stored as JSON text.
pub const fn json(name: &'static str) -> Col {
    col(name, "JSON")
}

/// `INTEGER NOT NULL REFERENCES <table>`: the primary key of `table`'s row,
/// whatever its key column is named (aio's `ref`).
pub const fn refs(name: &'static str, table: &'static str) -> Col {
    Col { refs: Some(table), ..col(name, "INTEGER") }
}

impl Col {
    /// Without `NOT NULL`: it takes SQL `NULL` (`None`).
    pub const fn nullable(self) -> Self {
        Self { nullable: true, ..self }
    }

    /// `UNIQUE`.
    pub const fn unique(self) -> Self {
        Self { unique: true, ..self }
    }

    pub const fn default_text(self, v: &'static str) -> Self {
        Self { default: Some(Fill::Text(v)), ..self }
    }

    pub const fn default_int(self, v: i64) -> Self {
        Self { default: Some(Fill::Int(v)), ..self }
    }

    pub const fn default_real(self, v: f64) -> Self {
        Self { default: Some(Fill::Real(v)), ..self }
    }

    /// Its definition, as `CREATE TABLE` takes it (aio's `columnToSQL`).
    fn sql(&self) -> Result<String, String> {
        if self.name.is_empty() {
            return Err("a column without a name".into());
        }
        let mut s = format!("{} {}", quoted(self.name), self.ty);
        if self.pk {
            return Ok(s + " PRIMARY KEY");
        }
        if !self.nullable {
            s += " NOT NULL";
        }
        if self.unique {
            s += " UNIQUE";
        }
        match self.default {
            Some(Fill::Text(t)) => s += &format!(" DEFAULT '{}'", t.replace('\'', "''")),
            Some(Fill::Int(n)) => s += &format!(" DEFAULT {n}"),
            Some(Fill::Real(f)) if f.is_finite() => s += &format!(" DEFAULT {f}"),
            Some(Fill::Real(f)) => return Err(format!("column {}: DEFAULT {f} is not a finite number", self.name)),
            None => {}
        }
        if let Some(t) = self.refs {
            s += &format!(" REFERENCES {}", quoted(t));
        }
        Ok(s)
    }
}

fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// `cols` as a `CREATE TABLE` column list; two primary keys, or two columns
/// of one name, are refused (SQLite would refuse the table at boot).
pub fn sql(cols: &[Col]) -> Result<String, String> {
    let pks: Vec<&str> = cols.iter().filter(|c| c.pk).map(|c| c.name).collect();
    if pks.len() > 1 {
        return Err(format!(
            "{} primary keys declared ({}): SQLite takes one; keep one pk() and make the others unique()",
            pks.len(),
            pks.join(", ")
        ));
    }
    if let Some((i, c)) = cols.iter().enumerate().find(|(i, c)| cols[..*i].iter().any(|d| d.name.eq_ignore_ascii_case(c.name))) {
        return Err(format!("column {} is declared twice (the {}th)", c.name, i + 1));
    }
    Ok(cols.iter().map(Col::sql).collect::<Result<Vec<_>, _>>()?.join(", "))
}

/// `CREATE TABLE IF NOT EXISTS <table> (<cols>)`, for [`crate::db::Db`].
pub fn create(table: &str, cols: &[Col]) -> Result<String, String> {
    Ok(format!("CREATE TABLE IF NOT EXISTS {} ({})", quoted(table), sql(cols)?))
}

/// A bound table's declared columns: SQL text (`columns = "id INTEGER
/// PRIMARY KEY, …"`) or typed (`columns = CONTACTS`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Columns {
    Sql(&'static str),
    Typed(&'static [Col]),
}

impl Columns {
    /// The column list `CREATE TABLE` takes.
    pub fn sql(&self) -> Result<String, String> {
        match self {
            Self::Sql(s) => Ok((*s).into()),
            Self::Typed(cols) => sql(cols),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_render_as_aios_column_helpers() {
        assert_eq!(sql(&[pk("id"), text("email").unique().nullable()]).unwrap(), "\"id\" INTEGER PRIMARY KEY, \"email\" TEXT UNIQUE");
        assert_eq!(text("t").default_text("it's").sql().unwrap(), "\"t\" TEXT NOT NULL DEFAULT 'it''s'");
        assert_eq!(integer("n").default_int(-3).sql().unwrap(), "\"n\" INTEGER NOT NULL DEFAULT -3");
        assert!(real("x").default_real(f64::NAN).sql().unwrap_err().contains("not a finite number"));
        assert!(sql(&[pk("a"), pk("b")]).unwrap_err().contains("2 primary keys declared (a, b)"));
        assert!(sql(&[text("a"), integer("A")]).unwrap_err().contains("declared twice"));
        assert_eq!(create("my \"t\"", &[pk("id")]).unwrap(), "CREATE TABLE IF NOT EXISTS \"my \"\"t\"\"\" (\"id\" INTEGER PRIMARY KEY)");
        assert_eq!(Columns::Sql("id INTEGER").sql().unwrap(), "id INTEGER");
    }
}
