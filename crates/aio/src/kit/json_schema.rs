//! A form's schema read from JSON Schema (aio's `useForm` with a Zod,
//! Valibot or ArkType object): each of those exports one
//! (`z.toJSONSchema`, `@valibot/to-json-schema`, ArkType's
//! `toJsonSchema()`), so one file serves the TypeScript client and the Rust
//! form. Read once (`include_str!` at build time, or a file at run time);
//! a keyword it can't check is refused there, never skipped.
use super::pattern::Pattern;
use super::schema::{Parse, Schema};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::sync::Mutex;

/// What only describes (no rule to check).
const NOTES: &[&str] = &[
    "title",
    "description",
    "default",
    "examples",
    "$comment",
    "readOnly",
    "writeOnly",
    "deprecated",
    "$schema",
    "$id",
];

#[derive(Clone, Copy, Debug, PartialEq)]
enum Ty {
    Text,
    Number,
    Integer,
    Bool,
    List,
    Null,
}

#[derive(Debug, Default)]
struct Check {
    types: Vec<Ty>,
    min_len: Option<u64>,
    max_len: Option<u64>,
    pattern: Option<Pattern>,
    format: Option<&'static str>,
    min: Option<Value>,
    max: Option<Value>,
    xmin: Option<Value>,
    xmax: Option<Value>,
    multiple_of: Option<f64>,
    one_of: Option<Vec<Value>>,
    items: Option<Box<Check>>,
    min_items: Option<u64>,
    max_items: Option<u64>,
    unique: bool,
    /// `errorMessage` (ajv-errors): one for all, or one per keyword.
    says: Option<Value>,
}

/// A field's name as a form problem names it: each name kept once.
fn named(name: &str) -> &'static str {
    static NAMES: Mutex<Option<HashSet<&'static str>>> = Mutex::new(None);
    let mut names = NAMES.lock().unwrap_or_else(|e| e.into_inner());
    let names = names.get_or_insert_with(HashSet::new);
    match names.get(name) {
        Some(n) => n,
        None => {
            let n: &'static str = Box::leak(name.into());
            names.insert(n);
            n
        }
    }
}

fn num(v: &Value, k: &str) -> Result<Value, String> {
    v.is_number().then(|| v.clone()).ok_or_else(|| format!("{k}: a number"))
}

fn count(v: &Value, k: &str) -> Result<u64, String> {
    v.as_u64().ok_or_else(|| format!("{k}: a count"))
}

fn ty(t: &str) -> Result<Ty, String> {
    Ok(match t {
        "string" => Ty::Text,
        "number" => Ty::Number,
        "integer" => Ty::Integer,
        "boolean" => Ty::Bool,
        "array" => Ty::List,
        "null" => Ty::Null,
        t => return Err(format!("type {t}: a form field is a string, number, integer, boolean, array or null")),
    })
}

/// One field's rules (`at`: its name, for what is refused).
fn check(s: &Value, at: &str) -> Result<Check, String> {
    let o = s.as_object().ok_or_else(|| format!("{at}: not a schema"))?;
    let mut c = Check::default();
    for (k, v) in o {
        let k = k.as_str();
        let at = format!("{at}: {k}");
        match k {
            _ if NOTES.contains(&k) => {}
            "type" => {
                c.types = match v {
                    Value::Array(ts) => {
                        ts.iter().map(|t| ty(t.as_str().unwrap_or_default())).collect::<Result<_, _>>()?
                    }
                    t => vec![ty(t.as_str().unwrap_or_default())?],
                }
            }
            "minLength" => c.min_len = Some(count(v, &at)?),
            "maxLength" => c.max_len = Some(count(v, &at)?),
            "pattern" => {
                c.pattern = Some(Pattern::new(v.as_str().unwrap_or_default()).map_err(|e| format!("{at}: {e}"))?)
            }
            "format" => {
                const KNOWN: &[&str] = &["email", "uri", "url", "uuid", "date", "date-time", "time", "ipv4", "ipv6"];
                let f = v.as_str().unwrap_or_default();
                c.format =
                    Some(KNOWN.iter().find(|k| **k == f).ok_or_else(|| format!("{at} {f}: not one aio-rs checks"))?);
            }
            "minimum" => c.min = Some(num(v, &at)?),
            "maximum" => c.max = Some(num(v, &at)?),
            "exclusiveMinimum" => c.xmin = Some(num(v, &at)?),
            "exclusiveMaximum" => c.xmax = Some(num(v, &at)?),
            "multipleOf" => {
                c.multiple_of = Some(v.as_f64().filter(|m| *m > 0.0).ok_or_else(|| format!("{at}: more than 0"))?)
            }
            "enum" => c.one_of = Some(v.as_array().ok_or_else(|| format!("{at}: a list"))?.clone()),
            "const" => c.one_of = Some(vec![v.clone()]),
            "items" => c.items = Some(Box::new(check(v, &at)?)),
            "minItems" => c.min_items = Some(count(v, &at)?),
            "maxItems" => c.max_items = Some(count(v, &at)?),
            "uniqueItems" => c.unique = v.as_bool().unwrap_or_default(),
            "errorMessage" => c.says = Some(v.clone()),
            // `X | null` (Zod's `.nullable()`): X's rules, and null.
            "anyOf" | "oneOf" => {
                let alts = v.as_array().ok_or_else(|| format!("{at}: a list"))?;
                let null = |s: &Value| {
                    s.get("type").and_then(Value::as_str) == Some("null") && s.as_object().is_some_and(|o| o.len() == 1)
                };
                let [one] = alts.iter().filter(|s| !null(s)).collect::<Vec<_>>()[..] else {
                    return Err(format!("{at}: only `one schema or null` (a nullable field)"));
                };
                let inner = check(one, &at)?;
                let types = std::mem::take(&mut c.types);
                c = Check { types: inner.types.iter().copied().chain(types).chain([Ty::Null]).collect(), ..inner };
            }
            k => return Err(format!("{at}: `{k}` is not a keyword aio-rs checks: no field rule may be skipped")),
        }
    }
    Ok(c)
}

/// The message for a broken `keyword`: the schema's own (`errorMessage`), else `plain`.
fn said(c: &Check, keyword: &str, plain: String) -> String {
    match &c.says {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Object(m)) => m.get(keyword).and_then(Value::as_str).map_or(plain, str::to_string),
        _ => plain,
    }
}

fn show(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        v => v.to_string(),
    }
}

/// `v` as the field's type: a form holds text, so `"36"` is 36 for a
/// number, `""` is no value unless the field is text (pure).
fn coerce(c: &Check, v: Value) -> Result<Option<Value>, String> {
    let wants = |t| c.types.is_empty() || c.types.contains(&t);
    Ok(match v {
        Value::Null => None,
        // An empty input of a nullable field (Zod's `.nullable()`) is null.
        Value::String(s) if s.trim().is_empty() && c.types.contains(&Ty::Null) => None,
        Value::String(s) if wants(Ty::Text) => Some(Value::String(s)),
        Value::String(s) if s.trim().is_empty() => None,
        Value::String(s) if wants(Ty::Integer) || wants(Ty::Number) => {
            let n: f64 = s.trim().parse().map_err(|_| said(c, "type", "A number".into()))?;
            let n = match n.fract() == 0.0 && n.abs() < 9e15 {
                true => Value::from(n as i64),
                false => serde_json::Number::from_f64(n)
                    .map(Value::Number)
                    .ok_or_else(|| said(c, "type", "A number".into()))?,
            };
            Some(n)
        }
        Value::String(s) if wants(Ty::Bool) => match s.trim() {
            "true" => Some(Value::Bool(true)),
            "false" => Some(Value::Bool(false)),
            _ => return Err(said(c, "type", "true or false".into())),
        },
        v => Some(v),
    })
}

fn date(s: &str) -> bool {
    let b = s.as_bytes();
    let n = |r: std::ops::Range<usize>| {
        s.get(r).filter(|d| d.bytes().all(|c| c.is_ascii_digit())).and_then(|d| d.parse::<u32>().ok())
    };
    let (Some(y), Some(m), Some(d)) = (n(0..4), n(5..7), n(8..10)) else { return false };
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let days = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    b.len() == 10 && b[4] == b'-' && b[7] == b'-' && (1..=12).contains(&m) && d >= 1 && d <= days[m as usize - 1]
}

/// `HH:MM:SS(.f)` and, with `zone`, `Z` or `±HH:MM` after it.
fn time(s: &str, zone: bool) -> bool {
    let two = |t: &str, max: u32| {
        t.len() == 2 && t.bytes().all(|c| c.is_ascii_digit()) && t.parse::<u32>().is_ok_and(|n| n <= max)
    };
    let (clock, rest) = s.split_at(s.find(['Z', 'z', '+', '-']).unwrap_or(s.len()));
    let (hms, frac) = clock.split_once('.').unwrap_or((clock, "1"));
    let p: Vec<&str> = hms.split(':').collect();
    let off = match rest {
        "" => !zone,
        "Z" | "z" => true,
        r => r.get(1..).and_then(|o| o.split_once(':')).is_some_and(|(h, m)| two(h, 23) && two(m, 59)),
    };
    p.len() == 3
        && two(p[0], 23)
        && two(p[1], 59)
        && two(p[2], 60)
        && !frac.is_empty()
        && frac.bytes().all(|c| c.is_ascii_digit())
        && off
}

fn formatted(f: &str, s: &str) -> bool {
    let hex = |p: &str, n: usize| p.len() == n && p.bytes().all(|c| c.is_ascii_hexdigit());
    match f {
        "email" => super::schema::email(s),
        "uri" | "url" => s.split_once(':').is_some_and(|(scheme, rest)| {
            scheme.starts_with(|c: char| c.is_ascii_alphabetic())
                && scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
                && !rest.is_empty()
                && !s.contains(char::is_whitespace)
        }),
        "uuid" => {
            let p: Vec<&str> = s.split('-').collect();
            p.len() == 5 && [8, 4, 4, 4, 12].iter().zip(&p).all(|(n, p)| hex(p, *n))
        }
        "date" => date(s),
        "date-time" => {
            s.len() > 11 && matches!(s.as_bytes()[10], b'T' | b't' | b' ') && date(&s[..10]) && time(&s[11..], true)
        }
        "time" => time(s, false),
        "ipv4" => s.parse::<std::net::Ipv4Addr>().is_ok(),
        _ => s.parse::<std::net::Ipv6Addr>().is_ok(),
    }
}

const FORMATS: &[(&str, &str)] = &[
    ("email", "email address"),
    ("uri", "URL"),
    ("url", "URL"),
    ("uuid", "UUID"),
    ("date", "date"),
    ("date-time", "date and time"),
    ("time", "time"),
    ("ipv4", "IPv4 address"),
    ("ipv6", "IPv6 address"),
];

/// The problems of value `v` under `c` (pure).
fn problems(c: &Check, v: &Value) -> Vec<String> {
    let mut out = vec![];
    let mut no = |k: &str, plain: String| out.push(said(c, k, plain));
    let is = |t: Ty| match (t, v) {
        (Ty::Text, Value::String(_))
        | (Ty::Bool, Value::Bool(_))
        | (Ty::List, Value::Array(_))
        | (Ty::Null, Value::Null) => true,
        (Ty::Number, Value::Number(_)) => true,
        (Ty::Integer, Value::Number(n)) => n.as_f64().is_some_and(|f| f.fract() == 0.0),
        _ => false,
    };
    if !c.types.is_empty() && !c.types.iter().any(|t| is(*t)) {
        let plain = match c.types[0] {
            Ty::Integer if v.is_number() => "A whole number",
            Ty::Integer | Ty::Number => "A number",
            Ty::Bool => "true or false",
            Ty::List => "A list",
            Ty::Text => "Text",
            Ty::Null => "Nothing",
        };
        no("type", plain.into());
        return out;
    }
    if let Some(e) = c.one_of.as_ref().filter(|e| !e.contains(v)) {
        let k = if e.len() == 1 { "const" } else { "enum" };
        no(k, format!("One of: {}", e.iter().map(show).collect::<Vec<_>>().join(", ")));
    }
    match v {
        Value::String(s) => {
            let n = s.chars().count() as u64;
            let chars = |m: u64| if m == 1 { "character" } else { "characters" };
            if let Some(m) = c.min_len.filter(|m| n < *m) {
                no("minLength", format!("At least {m} {}", chars(m)));
            }
            if let Some(m) = c.max_len.filter(|m| n > *m) {
                no("maxLength", format!("At most {m} {}", chars(m)));
            }
            if let Some(f) = c.format.filter(|f| !formatted(f, s)) {
                let name = FORMATS.iter().find(|(k, _)| *k == f).map_or(f, |(_, n)| n);
                no("format", format!("Not a valid {name}"));
            }
            if c.pattern.as_ref().is_some_and(|p| !p.found(s)) {
                no("pattern", "Not in the expected form".into());
            }
        }
        Value::Number(n) => {
            let f = n.as_f64().unwrap_or_default();
            let bounds = [
                (&c.min, "minimum", "At least"),
                (&c.max, "maximum", "At most"),
                (&c.xmin, "exclusiveMinimum", "More than"),
                (&c.xmax, "exclusiveMaximum", "Less than"),
            ];
            for (b, k, says) in bounds {
                let Some(bv) = b else { continue };
                let m = bv.as_f64().unwrap_or_default();
                let broken = match k {
                    "minimum" => f < m,
                    "maximum" => f > m,
                    "exclusiveMinimum" => f <= m,
                    _ => f >= m,
                };
                if broken {
                    no(k, format!("{says} {}", show(bv)));
                }
            }
            if let Some(m) = c.multiple_of.filter(|m| ((f / m) - (f / m).round()).abs() > 1e-9) {
                no("multipleOf", format!("A multiple of {m}"));
            }
        }
        Value::Array(a) => {
            let n = a.len() as u64;
            if let Some(m) = c.min_items.filter(|m| n < *m) {
                no("minItems", format!("At least {m} items"));
            }
            if let Some(m) = c.max_items.filter(|m| n > *m) {
                no("maxItems", format!("At most {m} items"));
            }
            if c.unique && a.iter().enumerate().any(|(i, x)| a[..i].contains(x)) {
                no("uniqueItems", "No item twice".into());
            }
            if let Some(p) = c.items.as_ref().and_then(|it| a.iter().flat_map(|x| problems(it, x)).next()) {
                no("items", p);
            }
        }
        _ => {}
    }
    out
}

/// A JSON Schema of an object: its fields' rules, which are required.
struct Fields {
    fields: Vec<(&'static str, Check, bool)>,
    /// `errorMessage.required` (ajv-errors): a field's message when it is missing.
    missing: Map<String, Value>,
}

fn fields(text: &str) -> Result<Fields, String> {
    let s: Value = serde_json::from_str(text).map_err(|e| format!("JSON Schema: {e}"))?;
    let o = s.as_object().ok_or("JSON Schema: not an object")?;
    let mut props = &Map::new();
    let mut required = vec![];
    let mut missing = Map::new();
    for (k, v) in o {
        match k.as_str() {
            k if NOTES.contains(&k) => {}
            "additionalProperties" => {}
            "type" if v == "object" => {}
            "properties" => props = v.as_object().ok_or("JSON Schema: properties is not an object")?,
            "required" => required = v.as_array().into_iter().flatten().filter_map(Value::as_str).collect(),
            "errorMessage" => missing = v.get("required").and_then(Value::as_object).cloned().unwrap_or_default(),
            k => {
                return Err(format!(
                    "JSON Schema: `{k}` is not a keyword aio-rs checks on a form (one object, its fields)"
                ));
            }
        }
    }
    let fields = props
        .iter()
        .map(|(name, s)| Ok((named(name), check(s, name)?, required.contains(&name.as_str()))))
        .collect::<Result<_, String>>()?;
    Ok(Fields { fields, missing })
}

impl Fields {
    /// `values` (the form's, as JSON) checked: the app's value as JSON, or every problem (pure).
    fn parse(&self, values: &Value) -> Result<Value, Vec<(&'static str, String)>> {
        let mut out = Map::new();
        let mut bad = vec![];
        for (name, c, required) in &self.fields {
            match coerce(c, values.get(*name).cloned().unwrap_or(Value::Null)) {
                Err(m) => bad.push((*name, m)),
                Ok(None) if *required => {
                    let m = self.missing.get(*name).and_then(Value::as_str).map(str::to_string);
                    bad.push((*name, m.unwrap_or_else(|| said(c, "required", "Required".into()))));
                }
                Ok(None) => {}
                Ok(Some(v)) => {
                    let p = problems(c, &v);
                    bad.extend(p.into_iter().take(1).map(|m| (*name, m)));
                    out.insert(name.to_string(), v);
                }
            }
        }
        if bad.is_empty() { Ok(Value::Object(out)) } else { Err(bad) }
    }
}

impl<T: Clone + PartialEq + Serialize + 'static, O: DeserializeOwned + 'static> Schema<T, O> {
    /// The schema a JSON Schema says (Zod's `z.toJSONSchema(s)`,
    /// Valibot's `toJsonSchema(s)`, ArkType's `t.toJsonSchema()`): an
    /// object whose `properties` are the form's fields by name. Each field's
    /// value is read as its type (a text input's `"36"` is 36), checked
    /// (`type`, `minLength`, `maxLength`, `pattern`, `format`, `minimum`,
    /// `maximum`, the exclusive ones, `multipleOf`, `enum`, `const`,
    /// `items`, `minItems`, `maxItems`, `uniqueItems`, `required`, `X |
    /// null`), and what passes is deserialized into `O`. Messages are the
    /// schema's `errorMessage` (ajv-errors), else plain English. A keyword
    /// it can't check is an error here, not a rule skipped.
    pub fn json(text: &str) -> Result<Self, String> {
        let f = fields(text)?;
        Ok(Self::new(move |p: &mut Parse<T>| {
            let values = serde_json::to_value(p.values()).unwrap_or_default();
            match f.parse(&values) {
                Err(bad) => {
                    bad.into_iter().for_each(|(n, m)| p.problem(n, m));
                    None
                }
                Ok(v) => serde_json::from_value(v).map_err(|e| p.problem("", e.to_string())).ok(),
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// What Zod 4 exports for its signup schema (`z.toJSONSchema`).
    const SIGNUP: &str = r#"{
      "$schema": "https://json-schema.org/draft/2020-12/schema",
      "type": "object",
      "properties": {
        "name": { "type": "string", "minLength": 1, "maxLength": 40 },
        "age": { "type": "integer", "minimum": 18, "maximum": 130 },
        "email": { "type": "string", "format": "email",
          "pattern": "^(?!\\.)(?!.*\\.\\.)([A-Za-z0-9_'+\\-\\.]*)[A-Za-z0-9_+-]@([A-Za-z0-9][A-Za-z0-9\\-]*\\.)+[A-Za-z]{2,}$" },
        "plan": { "type": "string", "enum": ["free", "pro"] },
        "site": { "anyOf": [{ "type": "string", "format": "uri" }, { "type": "null" }] },
        "tags": { "type": "array", "items": { "type": "string", "minLength": 2 }, "maxItems": 2 },
        "code": { "type": "string", "pattern": "^[A-Z]{3}-\\d{2}$", "errorMessage": "Like ABC-12" }
      },
      "required": ["name", "age", "email", "plan"],
      "additionalProperties": false
    }"#;

    #[test]
    fn a_json_schema_checks_each_field_and_reads_text_as_its_type() {
        let f = fields(SIGNUP).unwrap();
        let ok = json!({ "name": "Ada", "age": "36", "email": "ada@x.io", "plan": "pro", "site": "", "tags": ["ab"], "code": "" });
        assert!(f.parse(&ok).is_err(), "code is text: \"\" is a value, and it doesn't match");
        let ok = json!({ "name": "Ada", "age": "36", "email": "ada@x.io", "plan": "pro", "site": "", "tags": ["ab"], "code": "ABC-12" });
        assert_eq!(
            f.parse(&ok),
            Ok(
                json!({ "name": "Ada", "age": 36, "email": "ada@x.io", "plan": "pro", "tags": ["ab"], "code": "ABC-12" })
            ),
            "a nullable field's empty input is null"
        );
        let bad = json!({ "name": "", "age": "12.5", "email": "ada..x@y.io", "plan": "gold", "site": "no", "tags": ["a"], "code": "x" });
        assert_eq!(
            f.parse(&bad).unwrap_err(),
            vec![
                ("age", "A whole number".to_string()),
                ("code", "Like ABC-12".into()),
                ("email", "Not in the expected form".into()),
                ("name", "At least 1 character".into()),
                ("plan", "One of: free, pro".into()),
                ("site", "Not a valid URL".into()),
                ("tags", "At least 2 characters".into()),
            ]
        );
        let missing = f
            .parse(&json!({ "age": "", "name": "A", "email": "a@b.co", "plan": "free", "code": "ABC-12" }))
            .unwrap_err();
        assert_eq!(missing, vec![("age", "Required".to_string())]);
        assert_eq!(f.parse(&json!({ "name": "A", "age": "x" })).unwrap_err()[0], ("age", "A number".into()), "by name");
    }

    #[derive(Clone, PartialEq, Default, Serialize)]
    struct Signup {
        name: String,
        age: String,
        email: String,
        plan: String,
        site: String,
        code: String,
    }

    #[derive(Debug, PartialEq, serde::Deserialize)]
    struct Person {
        name: String,
        age: u8,
        plan: String,
        site: Option<String>,
    }

    #[test]
    fn a_json_schema_is_a_forms_schema_parsed_into_the_apps_type() {
        let s = Schema::<Signup, Person>::json(SIGNUP).unwrap();
        let v = Signup {
            name: "Ada".into(),
            age: "36".into(),
            email: "ada@x.io".into(),
            plan: "free".into(),
            code: "ABC-12".into(),
            ..Default::default()
        };
        assert_eq!(s.parse(&v), Ok(Person { name: "Ada".into(), age: 36, plan: "free".into(), site: None }));
        let old = Signup { age: "200".into(), ..v.clone() };
        assert_eq!(s.parse(&old).unwrap_err(), vec![("age", "At most 130".to_string())]);
        assert!(Schema::<Signup, Person>::json("{").is_err());
    }

    #[test]
    fn what_it_cant_check_is_refused_when_read() {
        let e = |s: &str| fields(s).err().unwrap();
        assert!(
            e(r#"{"type":"object","properties":{"a":{"type":"string","contentMediaType":"x"}}}"#)
                .contains("a: contentMediaType")
        );
        assert!(
            e(r#"{"type":"object","properties":{"a":{"format":"hostname"}}}"#)
                .contains("hostname: not one aio-rs checks")
        );
        assert!(e(r#"{"type":"object","properties":{"a":{"pattern":"(?<=x)y"}}}"#).contains("lookbehind"));
        assert!(e(r#"{"type":"object","properties":{"a":{"type":"object"}}}"#).contains("type object"));
        assert!(e(r#"{"type":"object","$defs":{}}"#).contains("`$defs`"));
        assert!(
            e(r#"{"type":"object","properties":{"a":{"anyOf":[{"type":"string"},{"type":"number"}]}}}"#)
                .contains("nullable")
        );
    }

    #[test]
    fn formats_are_checked_as_their_rfcs_say() {
        let ok = |f, s| formatted(f, s);
        assert!(ok("date", "2024-02-29") && !ok("date", "2023-02-29") && !ok("date", "2024-13-01"));
        assert!(ok("date-time", "2024-02-29T12:30:00Z") && ok("date-time", "2024-02-29T12:30:00.5+02:00"));
        assert!(!ok("date-time", "2024-02-29T12:30:00") && !ok("date-time", "2024-02-29T24:00:00Z"));
        assert!(ok("time", "08:15:00") && !ok("time", "8:15:00"));
        assert!(ok("uuid", "123e4567-e89b-12d3-a456-426614174000") && !ok("uuid", "123e4567"));
        assert!(ok("uri", "https://x.io/a?b") && ok("uri", "mailto:a@b.c") && !ok("uri", "x y:z"));
        assert!(ok("ipv4", "192.0.2.1") && ok("ipv6", "::1") && !ok("ipv4", "1.2.3"));
    }
}
