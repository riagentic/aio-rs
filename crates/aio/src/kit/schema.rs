//! A form's schema (aio's Standard Schema path: Zod, Valibot): each field's
//! rule written once, declaratively (`text().trim().required("Your name")`,
//! `text().parse::<u8>("A number").range(18..=130, "18 or more")`), the
//! form's values parsed into the type the app wants (`Form::parsed`), each
//! problem on its field, cross-field rules on the field they name, and
//! checks that wait (a lookup) with [`Schema::wait`]. Pure but for `wait`:
//! [`Schema::parse`] runs without a page.
use super::forms::{FieldOf, Form, Problems, use_async_rule};
use std::future::Future;
use std::ops::RangeInclusive;
use std::rc::Rc;
use std::str::FromStr;
use std::time::Duration;

/// One field's rule: from what the form holds (`I`) to what the app wants
/// (`O`), or the message why not. Built up step by step; `Clone`.
pub struct Rule<I, O>(Rc<dyn Fn(I) -> Result<O, String>>);

impl<I, O> Clone for Rule<I, O> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

/// The rule that takes a text field as it is.
pub fn text() -> Rule<String, String> {
    rule()
}

/// The rule that takes a field of any type as it is (`rule::<bool>()`).
pub fn rule<T: 'static>() -> Rule<T, T> {
    Rule::new(Ok)
}

/// `r`, but an empty (or blank) text is no value rather than a problem
/// (Zod's `.optional()` over a text input).
pub fn optional<O: 'static>(r: Rule<String, O>) -> Rule<String, Option<O>> {
    Rule::new(move |s: String| if s.trim().is_empty() { Ok(None) } else { r.run(s).map(Some) })
}

impl<I: 'static, O: 'static> Rule<I, O> {
    pub fn new(f: impl Fn(I) -> Result<O, String> + 'static) -> Self {
        Self(Rc::new(f))
    }

    /// The value, or why not.
    pub fn run(&self, i: I) -> Result<O, String> {
        (self.0)(i)
    }

    /// Then a step that may refuse (`|s| s.parse().map_err(..)`).
    pub fn then<P: 'static>(self, f: impl Fn(O) -> Result<P, String> + 'static) -> Rule<I, P> {
        Rule::new(move |i| self.run(i).and_then(&f))
    }

    /// Then a step that can't refuse.
    pub fn map<P: 'static>(self, f: impl Fn(O) -> P + 'static) -> Rule<I, P> {
        Rule::new(move |i| self.run(i).map(&f))
    }

    /// Refused with `msg` unless `ok`.
    pub fn check(self, ok: impl Fn(&O) -> bool + 'static, msg: impl Into<String>) -> Self {
        let msg = msg.into();
        self.then(move |o| if ok(&o) { Ok(o) } else { Err(msg.clone()) })
    }
}

impl<I: 'static> Rule<I, String> {
    /// Without the spaces around it.
    pub fn trim(self) -> Self {
        self.map(|s| s.trim().to_string())
    }

    /// Refused with `msg` when empty.
    pub fn required(self, msg: impl Into<String>) -> Self {
        self.check(|s| !s.is_empty(), msg)
    }

    /// Refused with `msg` when shorter than `n` characters.
    pub fn min_len(self, n: usize, msg: impl Into<String>) -> Self {
        self.check(move |s| s.chars().count() >= n, msg)
    }

    /// Refused with `msg` when longer than `n` characters.
    pub fn max_len(self, n: usize, msg: impl Into<String>) -> Self {
        self.check(move |s| s.chars().count() <= n, msg)
    }

    /// Refused with `msg` unless it reads as an email (`a@b.c`, no spaces).
    pub fn email(self, msg: impl Into<String>) -> Self {
        self.check(|s| email(s), msg)
    }

    /// Parsed as an `N` (a number, a date type of the app's), refused with `msg` if it doesn't.
    pub fn parse<N: FromStr + 'static>(self, msg: impl Into<String>) -> Rule<I, N> {
        let msg = msg.into();
        self.then(move |s| s.trim().parse().map_err(|_| msg.clone()))
    }
}

impl<I: 'static, N: PartialOrd + 'static> Rule<I, N> {
    /// Refused with `msg` outside `r`.
    pub fn range(self, r: RangeInclusive<N>, msg: impl Into<String>) -> Self {
        self.check(move |n| r.contains(n), msg)
    }
}

/// `s` reads as an email: `a@b.c`, no spaces.
pub(super) fn email(s: &str) -> bool {
    let (user, host) = s.split_once('@').unwrap_or_default();
    !user.is_empty()
        && !s.contains(char::is_whitespace)
        && host.split('.').count() > 1
        && host.split('.').all(|p| !p.is_empty())
}

/// What a schema's parse sees: the values, and the problems so far.
pub struct Parse<'a, T> {
    values: &'a T,
    problems: Problems,
}

impl<T> Parse<'_, T> {
    /// The form's values as they are (for a cross-field rule).
    pub fn values(&self) -> &T {
        self.values
    }

    /// Field `f` through rule `r`: its value, or none and the problem said on `f`.
    pub fn field<V: 'static, O: 'static>(&mut self, f: FieldOf<T, V>, r: &Rule<V, O>) -> Option<O> {
        r.run((f.get)(self.values)).map_err(|m| self.problem(f.name, m)).ok()
    }

    /// A problem on `field` (`""`: the form as a whole): a cross-field rule's.
    pub fn problem(&mut self, field: &'static str, msg: impl Into<String>) {
        self.problems.push((field, msg.into()));
    }
}

type Waits<T, O> = Vec<Rc<dyn Fn(Form<T, O>)>>;
type ParseFn<T, O> = Rc<dyn Fn(&mut Parse<T>) -> Option<O>>;

/// A form's schema: its values (`T`, what the inputs hold) parsed into
/// `O`, what the app wants ([`use_form_schema`]).
pub struct Schema<T: 'static, O: 'static> {
    parse: ParseFn<T, O>,
    waits: Waits<T, O>,
}

impl<T: Clone + PartialEq + 'static, O: 'static> Schema<T, O> {
    /// `parse` (`|p: &mut Parse<Values>| ..`) reads each field through its rule (`p.field(field!(age), &age)`)
    /// and builds `O` from what passed: all fields first, then `Some(O { age: age?, .. })`,
    /// so every field's problem is said at once.
    pub fn new(parse: impl Fn(&mut Parse<T>) -> Option<O> + 'static) -> Self {
        Self { parse: Rc::new(parse), waits: vec![] }
    }

    /// A check that waits (a uniqueness lookup) on field `f`: run `debounce`
    /// after its last edit, unless its rule already refuses; its answer
    /// (`Some(msg)`: refused) holds the form until it comes ([`use_async_rule`]).
    pub fn wait<V, F>(mut self, f: FieldOf<T, V>, debounce: Duration, check: impl Fn(V) -> F + 'static) -> Self
    where
        V: Clone + PartialEq + 'static,
        F: Future<Output = Option<String>> + 'static,
    {
        let check = Rc::new(check);
        self.waits.push(Rc::new(move |form| {
            let check = check.clone();
            use_async_rule(form, f.name, f.get, debounce, move |v| check(v))
        }));
        self
    }

    /// `values` parsed: `O`, or every problem found (pure).
    pub fn parse(&self, values: &T) -> Result<O, Problems> {
        parse_with(&*self.parse, values)
    }
}

fn parse_with<T, O>(parse: &dyn Fn(&mut Parse<T>) -> Option<O>, values: &T) -> Result<O, Problems> {
    let mut p = Parse { values, problems: vec![] };
    match parse(&mut p) {
        Some(o) if p.problems.is_empty() => Ok(o),
        _ => Err(p.problems),
    }
}

/// A form starting at `initial`, checked and parsed by `schema` (a hook;
/// aio's `useForm` with a `schema`): [`Form::parsed`] is the typed value.
pub fn use_form_schema<T: Clone + PartialEq + 'static, O: 'static>(
    initial: impl FnOnce() -> T,
    schema: Schema<T, O>,
) -> Form<T, O> {
    let Schema { parse, waits } = schema;
    let form = Form::made(initial, Rc::new(move |v: &T| parse_with(&*parse, v)));
    waits.iter().for_each(|w| w(form));
    form
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, PartialEq, Default)]
    struct Signup {
        name: String,
        age: String,
        email: String,
        site: String,
        pw: String,
        pw2: String,
    }

    #[derive(Debug, PartialEq)]
    struct Person {
        name: String,
        age: u8,
        email: String,
        site: Option<String>,
    }

    fn schema() -> Schema<Signup, Person> {
        let name = text().trim().required("Your name, please");
        let age = text().parse::<u8>("A number").range(18..=130, "18 or more");
        let email = text().trim().email("An email address");
        let site = optional(text().trim().check(|s| s.starts_with("https://"), "An https:// address"));
        Schema::new(move |p: &mut Parse<Signup>| {
            let (name, age) = (p.field(crate::field!(name), &name), p.field(crate::field!(age), &age));
            let (email, site) = (p.field(crate::field!(email), &email), p.field(crate::field!(site), &site));
            if p.values().pw != p.values().pw2 {
                p.problem("pw2", "The passwords differ");
            }
            Some(Person { name: name?, age: age?, email: email?, site: site? })
        })
    }

    #[test]
    fn a_schema_parses_into_the_apps_type_or_says_each_fields_problem() {
        let ok = Signup { name: " Ada ".into(), age: "36".into(), email: "ada@x.io".into(), ..Default::default() };
        assert_eq!(
            schema().parse(&ok),
            Ok(Person { name: "Ada".into(), age: 36, email: "ada@x.io".into(), site: None })
        );
        let bad = Signup {
            age: "12".into(),
            email: "ada@x".into(),
            site: "http://x".into(),
            pw: "a".into(),
            ..Default::default()
        };
        assert_eq!(
            schema().parse(&bad).unwrap_err(),
            vec![
                ("name", "Your name, please".to_string()),
                ("age", "18 or more".into()),
                ("email", "An email address".into()),
                ("site", "An https:// address".into()),
                ("pw2", "The passwords differ".into()),
            ]
        );
        assert_eq!(text().parse::<u8>("A number").run("x".into()), Err("A number".into()));
        assert_eq!(text().min_len(2, "short").max_len(3, "long").run("abcd".into()), Err("long".into()));
    }
}
