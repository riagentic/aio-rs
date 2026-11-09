//! aio's `where` objects over [`super::Db`]: a filter, an order and a page,
//! made into one parameterized `SELECT`. Every value is a parameter (`?`),
//! never text in the statement; every name (table, column) must be a plain
//! identifier, so neither can change what the statement does.
use super::{Rows, Sql};
use crate::Error;
use serde_json::Value;

/// SQLite's most parameters in one statement: a filter can't be split.
pub const MAX_PARAMS: usize = 32766;

/// How a column compares with a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Eq,
    Ne,
    Lt,
    Lte,
    Gt,
    Gte,
    Like,
}

/// A filter (aio's `where` object):
/// `Where::eq("status", "open".to_string()).and(Where::gt("total", 10))`.
#[derive(Clone, Debug, PartialEq)]
pub enum Where {
    /// `column op value`; `Eq` and `Ne` with `Sql::Null` are `IS NULL`, `IS NOT NULL`.
    Cmp(String, Op, Sql),
    /// `column IN (values)`; none matches nothing.
    In(String, Vec<Sql>),
    /// All of them (none: every row).
    And(Vec<Where>),
    /// Any of them (none: no row).
    Or(Vec<Where>),
}

impl Where {
    fn cmp(col: &str, op: Op, v: impl Into<Sql>) -> Self {
        Self::Cmp(col.into(), op, v.into())
    }
    pub fn eq(col: &str, v: impl Into<Sql>) -> Self {
        Self::cmp(col, Op::Eq, v)
    }
    pub fn ne(col: &str, v: impl Into<Sql>) -> Self {
        Self::cmp(col, Op::Ne, v)
    }
    pub fn lt(col: &str, v: impl Into<Sql>) -> Self {
        Self::cmp(col, Op::Lt, v)
    }
    pub fn lte(col: &str, v: impl Into<Sql>) -> Self {
        Self::cmp(col, Op::Lte, v)
    }
    pub fn gt(col: &str, v: impl Into<Sql>) -> Self {
        Self::cmp(col, Op::Gt, v)
    }
    pub fn gte(col: &str, v: impl Into<Sql>) -> Self {
        Self::cmp(col, Op::Gte, v)
    }
    /// SQL's `LIKE` (`%` any run, `_` one character).
    pub fn like(col: &str, pattern: &str) -> Self {
        Self::cmp(col, Op::Like, pattern.to_string())
    }
    pub fn is_in<V: Into<Sql>>(col: &str, vs: impl IntoIterator<Item = V>) -> Self {
        Self::In(col.into(), vs.into_iter().map(Into::into).collect())
    }

    pub fn and(self, other: Where) -> Self {
        match self {
            Self::And(mut all) => {
                all.push(other);
                Self::And(all)
            }
            one => Self::And(vec![one, other]),
        }
    }

    pub fn or(self, other: Where) -> Self {
        match self {
            Self::Or(mut any) => {
                any.push(other);
                Self::Or(any)
            }
            one => Self::Or(vec![one, other]),
        }
    }

    /// aio's `where` object as JSON: `{"status": "open", "total": {"gt": 10},
    /// "tag": {"in": ["a", "b"]}, "note": null}` (each field: all of them),
    /// or a list of such objects (any of them, aio's `whereOr`). Operators:
    /// `gt`, `gte`, `lt`, `lte`, `ne`, `like`, `in`. A number, a string, a
    /// bool (0/1) or null is a value; anything else is refused.
    pub fn object(w: &Value) -> Result<Self, Error> {
        let value = |v: &Value| match v {
            Value::Null => Ok(Sql::Null),
            Value::Bool(b) => Ok(Sql::Integer(i64::from(*b))),
            Value::Number(n) => Ok(n.as_i64().map_or_else(|| Sql::Real(n.as_f64().unwrap_or(f64::NAN)), Sql::Integer)),
            Value::String(s) => Ok(Sql::Text(s.clone())),
            v => Err(Error(format!("db: where: {v} is not a value (a number, a string, a bool or null)"))),
        };
        let field = |col: &String, v: &Value| -> Result<Vec<Where>, Error> {
            let Some(ops) = v.as_object().filter(|o| !o.is_empty()) else { return Ok(vec![Where::eq(col, value(v)?)]) };
            ops.iter()
                .map(|(op, v)| {
                    let op = match op.as_str() {
                        "in" => {
                            let vs = v.as_array().ok_or_else(|| Error(format!("db: where {col}: `in` takes a list")))?;
                            return Ok(Where::In(col.clone(), vs.iter().map(value).collect::<Result<_, _>>()?));
                        }
                        "gt" => Op::Gt,
                        "gte" => Op::Gte,
                        "lt" => Op::Lt,
                        "lte" => Op::Lte,
                        "ne" => Op::Ne,
                        "like" if v.is_string() => Op::Like,
                        "like" => return Err(Error(format!("db: where {col}: `like` takes a string"))),
                        o => return Err(Error(format!("db: where {col}: unknown operator {o:?}"))),
                    };
                    Ok(Where::Cmp(col.clone(), op, value(v)?))
                })
                .collect()
        };
        match w {
            Value::Object(m) => {
                let all: Vec<Vec<Where>> = m.iter().map(|(c, v)| field(c, v)).collect::<Result<_, _>>()?;
                Ok(Self::And(all.into_iter().flatten().collect()))
            }
            Value::Array(any) => Ok(Self::Or(any.iter().map(Self::object).collect::<Result<_, _>>()?)),
            v => Err(Error(format!("db: where: {v} is neither an object nor a list of them"))),
        }
    }

    /// Its SQL, its values added to `params` (pure).
    fn sql(&self, params: &mut Vec<Sql>) -> Result<String, Error> {
        Ok(match self {
            Self::Cmp(c, Op::Eq, Sql::Null) => format!("{} IS NULL", ident(c)?),
            Self::Cmp(c, Op::Ne, Sql::Null) => format!("{} IS NOT NULL", ident(c)?),
            Self::Cmp(c, op, v) => {
                let op = match op {
                    Op::Eq => "=",
                    Op::Ne => "!=",
                    Op::Lt => "<",
                    Op::Lte => "<=",
                    Op::Gt => ">",
                    Op::Gte => ">=",
                    Op::Like => "LIKE",
                };
                params.push(v.clone());
                format!("{} {op} ?", ident(c)?)
            }
            Self::In(c, vs) if vs.is_empty() => {
                ident(c)?;
                "0 = 1".into()
            }
            Self::In(c, vs) => {
                params.extend(vs.iter().cloned());
                format!("{} IN ({})", ident(c)?, vec!["?"; vs.len()].join(", "))
            }
            Self::And(ws) if ws.is_empty() => "1 = 1".into(),
            Self::Or(ws) if ws.is_empty() => "0 = 1".into(),
            Self::And(ws) | Self::Or(ws) => {
                let join = if matches!(self, Self::And(_)) { " AND " } else { " OR " };
                let parts: Vec<String> = ws.iter().map(|w| w.sql(params).map(|s| format!("({s})"))).collect::<Result<_, _>>()?;
                parts.join(join)
            }
        })
    }
}

/// `name` if it is a plain identifier (letters, digits, `_`; not first a
/// digit), else refused: it goes into the statement as it is.
fn ident(name: &str) -> Result<&str, Error> {
    let mut cs = name.chars();
    let ok = cs.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && cs.all(|c| c.is_ascii_alphanumeric() || c == '_');
    ok.then_some(name).ok_or_else(|| Error(format!("db: {name:?} is not a plain table or column name")))
}

/// Ascending or descending.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    Asc,
    Desc,
}

/// A `SELECT * FROM table` with aio's `where`, `orderBy`, `limit` and
/// `offset`: `Select::from("orders").filter(w).order_by("total", Order::Desc).limit(10)`.
#[derive(Clone, Debug, PartialEq)]
pub struct Select {
    table: String,
    filter: Option<Where>,
    order: Vec<(String, Order)>,
    limit: Option<u64>,
    offset: Option<u64>,
}

impl Select {
    pub fn from(table: &str) -> Self {
        Self { table: table.into(), filter: None, order: vec![], limit: None, offset: None }
    }

    /// The rows `w` lets through (another `filter` adds to it: both).
    pub fn filter(self, w: Where) -> Self {
        let filter = Some(match self.filter {
            Some(f) => f.and(w),
            None => w,
        });
        Self { filter, ..self }
    }

    /// Sorted by `col` (after the orders before it).
    pub fn order_by(mut self, col: &str, o: Order) -> Self {
        self.order.push((col.into(), o));
        self
    }

    pub fn limit(self, n: u64) -> Self {
        Self { limit: Some(n), ..self }
    }

    pub fn offset(self, n: u64) -> Self {
        Self { offset: Some(n), ..self }
    }

    /// The statement and its parameters (pure), for [`super::Db::query`]
    /// or [`super::Db::live`].
    pub fn sql(&self) -> Result<(String, Vec<Sql>), Error> {
        let mut params = vec![];
        let mut sql = format!("SELECT * FROM {}", ident(&self.table)?);
        if let Some(w) = &self.filter {
            sql += &format!(" WHERE {}", w.sql(&mut params)?);
        }
        if params.len() > MAX_PARAMS {
            return Err(Error(format!(
                "db: this filter has {} values, and SQLite takes at most {MAX_PARAMS} in one statement: \
                 query in batches, or join a temporary table of them",
                params.len()
            )));
        }
        let order: Vec<String> = self
            .order
            .iter()
            .map(|(c, o)| Ok(format!("{} {}", ident(c)?, if *o == Order::Desc { "DESC" } else { "ASC" })))
            .collect::<Result<_, Error>>()?;
        if !order.is_empty() {
            sql += &format!(" ORDER BY {}", order.join(", "));
        }
        // SQLite needs a LIMIT before an OFFSET: -1 is none.
        match (self.limit, self.offset) {
            (None, None) => {}
            (l, o) => {
                sql += &format!(" LIMIT {}", l.map_or(-1, |l| i64::try_from(l).unwrap_or(i64::MAX)));
                if let Some(o) = o {
                    sql += &format!(" OFFSET {}", i64::try_from(o).unwrap_or(i64::MAX));
                }
            }
        }
        Ok((sql, params))
    }
}

impl super::Db {
    /// The rows `q` selects (aio's `table.where(..)`), as [`Self::query`].
    pub async fn select(&self, q: &Select) -> Result<Rows, Error> {
        let (sql, params) = q.sql()?;
        self.query(&sql, params).await
    }
}
