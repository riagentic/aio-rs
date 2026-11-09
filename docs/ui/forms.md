# Forms

A form in aio-rs is three things: the values a person is typing, the
rules they must meet, and the call that sends them to a
[cell](../state/cells.md). `aio::kit::use_form` (feature `kit`; aio's
`useForm`) holds the first two; `act!` or the client's door does the
third. The kit's `Field`, `Input`, `Select`, `Checkbox` and `RadioGroup`
show the label, hint and error, and wire `for`, `aria-describedby`,
`aria-invalid` and `aria-required` for you ([kit](kit.md#buttons-and-fields)).

- **Use `use_form`** when a form has rules a person should see before
  sending: required fields, formats, a check across fields, a lookup that
  waits (is this name taken?).
- **Use plain signals** for one field with no rules (a search box), or a
  cell field when the draft must survive a restart (see
  [Drafts in a cell](#drafts-in-a-cell)).
- **The cell has the last word.** A method still checks what it is sent
  (`aio::ensure!`); the form only makes sure a person rarely meets that
  refusal. Show the refusal in the form when they do
  ([Errors from the cell](#errors-from-the-cell)).

## A whole form

```rust,no_run
use aio::kit::{Button, Field, Input, Problems, Variant, use_form};
use aio::ui::prelude::*;

#[aio::cell(persist, id = "contacts")]
pub struct Contacts {
    pub names: Vec<String>,
}

#[aio::methods]
impl Contacts {
    pub fn add(&mut self, name: String, email: String) -> aio::Result {
        aio::ensure!(!self.names.contains(&name), "{name} is already a contact");
        aio::ensure!(email.contains('@'), "that is not an email");
        self.names.push(name);
        Ok(())
    }
}

aio::client!(Contacts, contacts);

/// What the form edits: plain data, one field per input.
#[derive(Clone, Default, PartialEq)]
struct NewContact {
    name: String,
    email: String,
    age: Option<u32>,
}

/// Every rule, in one function: each problem names its field.
fn check(c: &NewContact) -> Problems {
    let mut p = Problems::new();
    if c.name.trim().is_empty() {
        p.push(("name", "A name, please".into()));
    }
    if !c.email.contains('@') {
        p.push(("email", "That is not an email".into()));
    }
    if c.age.is_some_and(|a| a > 150) {
        p.push(("age", "That age is too high".into()));
    }
    p
}

#[component]
pub fn App() -> View {
    use_live();
    let f = use_form(NewContact::default, check);
    let refused = signal(|| None::<String>);
    let send = move |e: FormEvent| {
        e.prevent_default();
        // Every error shows; the values only when they pass.
        let Some(c) = f.submit() else { return };
        match Ui.add(c.name, c.email) {
            Ok(()) => {
                refused.set(None);
                f.reset();
            }
            Err(e) => refused.set(Some(e.why)),
        }
    };
    rsx! {
        form { onsubmit: send,
            Field { label: "Name", required: true, error: f.error("name"),
                Input { "data-t": "name", value: f.values().name,
                    oninput: move |s| f.edit("name", |c| c.name = s),
                    onchange: move |_| f.touch("name") }
            }
            Field { label: "Email", required: true, hint: "We never share it", error: f.error("email"),
                Input { "data-t": "email", r#type: "email", value: f.values().email,
                    oninput: move |s| f.edit("email", |c| c.email = s),
                    onchange: move |_| f.touch("email") }
            }
            Field { label: "Age", error: f.error("age"),
                Input { "data-t": "age", r#type: "number",
                    oninput: move |s: String| f.edit("age", |c| c.age = s.parse().ok()),
                    onchange: move |_| f.touch("age") }
            }
            {refused.get().map(|why| rsx! { p { class: "aio-field__error", role: "alert", "data-t": "refused", "{why}" } })}
            Button { r#type: "submit", variant: Variant::Primary, "data-t": "save", "Save" }
            Button { disabled: !f.dirty(), onclick: move |_| f.reset(), "data-t": "clear", "Clear" }
        }
        ul { "data-t": "names",
            for n in part(|c| &c.names).get() {
                li { key: "{n}", "{n}" }
            }
        }
    }
}

# fn main() {
let mut ui = aio::testing::TestUi::new(App);
ui.click("save");
assert!(ui.html().contains("A name, please"), "a submit shows every error");
ui.type_text("name", "Ana");
ui.type_text("email", "ana@example.org");
ui.click("save");
assert_eq!(ui.text("names"), "Ana");
ui.type_text("name", "Ana");
ui.type_text("email", "ana@example.org");
ui.click("save");
assert_eq!(ui.text("refused"), "Ana is already a contact");
# }
```

What happens, step by step:

1. `use_form(initial, check)` makes the form's state once per component:
   the values (`NewContact::default()`), which fields were left, which
   were edited, whether a submit was tried.
2. Each input's `oninput` calls `f.edit(field, |values| ..)`: the values
   change, and anything a waiting rule or the form said about that field
   is stale.
3. `onchange` (the browser fires it when a person leaves a field they
   changed) calls `f.touch(field)`: from now on that field's error shows.
4. `f.submit()` marks the form tried, so every error shows, and gives
   `Some(values)` only when `check` finds nothing and no waiting rule is
   running or refusing.
5. The values go to the cell. A refusal comes back as `Rejected`, whose
   `why` the form shows.

## The form handle

`use_form` returns `Form<T>` (`use_form_schema`: `Form<T, O>`, `O` what
the values parse into), which is `Copy`: move it into every handler.

| Call | Gives | Does |
|---|---|---|
| `use_form(initial, check)` | `Form<T>` | a hook: the form, starting at `initial()`, checked by `check` (a `fn(&T) -> Problems`) |
| `f.values()` | `T` | the values now; a hole reading them follows each edit |
| `f.edit(field, \|v\| ..)` | | changes the values; `field` is edited, a waiting rule's answer for it and the form's own error are dropped |
| `f.touch(field)` | | the field was left: its error may show |
| `f.error(field)` | `Option<String>` | the field's first problem, once it was left or a submit was tried |
| `f.form_error()` | `Option<String>` | a problem about no single field (field `""`), once a submit was tried |
| `f.valid()` | `bool` | no problem, no waiting rule running or refusing |
| `f.checking(field)` | `bool` | a waiting rule on the field is running |
| `f.dirty()` | `bool` | the values differ from where they started |
| `f.edited(field)` | `bool` | the field was edited |
| `f.submit()` | `Option<T>` | every error shows; the values when valid |
| `f.parsed()`, `f.submit_parsed()` | `Option<O>` | the values parsed by the [schema](#a-schema) (`use_form`'s: the values) when valid; the second tries a submit first |
| `f.reset()` | | back to the initial values; nothing left or tried |

`T` needs `Clone + PartialEq + 'static`. `Problems` is
`Vec<(&'static str, String)>`: the field's name and the message.

## Rules

One function holds every rule. It runs on the values each time a hole
that shows an error asks, so keep it cheap (string checks, comparisons, a
regex built once).

- **The first problem per field wins.** Push a field's own rules before
  rules across fields, so "A name, please" shows before "Names differ".
- **A rule across fields names the field it lands on.** "Passwords
  differ" goes on `"confirm"`, not on both.
- **A rule about no single field uses `""`.** It shows through
  `f.form_error()`, once a submit was tried, and an edit to any field hides
  it until the next check.
- **A check is a function.** Parse into the type you send and push a
  problem per failure; the same function can run in the method too, so
  the rule is written once. Or declare it as a [schema](#a-schema):

```rust
use aio::kit::Problems;

#[derive(Clone, Default, PartialEq)]
pub struct Signup {
    pub password: String,
    pub confirm: String,
    pub tags: Vec<String>,
}

pub fn check(s: &Signup) -> Problems {
    let mut p = Problems::new();
    if s.password.chars().count() < 8 {
        p.push(("password", "At least 8 characters".into()));
    }
    if s.password != s.confirm {
        p.push(("confirm", "Passwords differ".into()));
    }
    if s.tags.len() > 2 {
        p.push(("", "At most two tags".into()));
    }
    p
}

# fn main() {
let s = Signup { password: "secret".into(), confirm: "secreT".into(), tags: vec![] };
assert_eq!(
    check(&s),
    [("password", "At least 8 characters".to_string()), ("confirm", "Passwords differ".to_string())]
);
assert!(check(&Signup { password: "long enough".into(), confirm: "long enough".into(), tags: vec![] }).is_empty());
# }
```

A check you can call without a view is also a check you can unit-test
without one.

## A schema

Where aio's `useForm` took a Standard Schema (Zod, Valibot),
`use_form_schema(initial, schema)` takes a `Schema`: each field's rule
declared once, the values parsed into the type the app sends, each
problem on its field, and checks that wait. `f.parsed()` is that typed
value (aio's `form.parsed()`, typed where aio's was `unknown`);
`f.submit_parsed()` tries a submit and gives it.

```rust
use aio::kit::{Parse, Schema, optional, text};
use std::time::Duration;

/// What the inputs hold: text.
#[derive(Clone, Default, PartialEq)]
pub struct Join {
    pub name: String,
    pub age: String,
    pub site: String,
}

/// What the app sends.
#[derive(Debug, PartialEq)]
pub struct Member {
    pub name: String,
    pub age: u8,
    pub site: Option<String>,
}

pub fn schema() -> Schema<Join, Member> {
    let name = text().trim().required("Your name, please");
    let age = text().parse::<u8>("A number").range(18..=130, "18 or more");
    let site = optional(text().trim().check(|s| s.starts_with("https://"), "An https:// address"));
    Schema::new(move |p: &mut Parse<Join>| {
        // Every field first, so each one's problem is said at once.
        let name = p.field(aio::field!(name), &name);
        let age = p.field(aio::field!(age), &age);
        let site = p.field(aio::field!(site), &site);
        Some(Member { name: name?, age: age?, site: site? })
    })
    .wait(aio::field!(name), Duration::from_millis(300), |name: String| async move {
        (name == "admin").then(|| "That name is taken".to_string())
    })
}

# fn main() {
let ok = Join { name: " Ada ".into(), age: "36".into(), site: String::new() };
assert_eq!(schema().parse(&ok), Ok(Member { name: "Ada".into(), age: 36, site: None }));
let bad = Join { age: "12".into(), ..ok };
assert_eq!(schema().parse(&bad).unwrap_err(), [("age", "18 or more".to_string())]);
# }
```

In a view: `let f = use_form_schema(Join::default, schema());`, then
`Input { bind: f.bind(aio::field!(age)) }` in a `Field { error:
f.error("age"), .. }`, and `f.submit_parsed()` on submit: a `Member`.

| Rule | Does |
|---|---|
| `text()`, `rule::<T>()` | a field as it is (text; any type: `rule::<bool>()`) |
| `.trim()` | without the spaces around |
| `.required(msg)`, `.min_len(n, msg)`, `.max_len(n, msg)`, `.email(msg)` | refuses an empty, short, long or non-email text |
| `.parse::<N>(msg)` | text into any `FromStr` type (a number, the app's own), refused if it doesn't parse |
| `.range(a..=b, msg)` | refuses a value outside |
| `.check(\|v\| ok, msg)` | any rule of the app's |
| `.map(f)`, `.then(\|v\| Ok(..) / Err(msg))` | a step that changes the value, or may refuse |
| `optional(rule)` | an empty text is `None`, else the rule's `Some` |
| `p.problem(field, msg)` | a rule across fields, on the field it names (`""`: the form) |
| `.wait(field, debounce, check)` | a check that waits ([below](#rules-that-wait)) |

`Schema::parse(&values)` runs without a view, so a schema is
unit-tested as above and can run in the cell's method too.

### One schema with a TypeScript client: JSON Schema

aio's `useForm` took the Zod, Valibot or ArkType object as it is. Each
of them exports it as JSON Schema (`z.toJSONSchema(s)`,
`toJsonSchema(s)` of `@valibot/to-json-schema`, ArkType's
`t.toJsonSchema()`), and `Schema::json` reads that file: the same rules
check the TypeScript client and the Rust form.

```rust
use aio::kit::Schema;

/// What the inputs hold.
#[derive(Clone, Default, PartialEq, aio::serde::Serialize)]
#[serde(crate = "aio::serde")]
pub struct Join {
    pub name: String,
    pub age: String,
    pub plan: String,
}

/// What the app sends.
#[derive(Debug, PartialEq, aio::serde::Deserialize)]
#[serde(crate = "aio::serde")]
pub struct Member {
    pub name: String,
    pub age: u8,
    pub plan: String,
}

/// In the app: `Schema::json(include_str!("../shared/join.schema.json"))`.
const JOIN: &str = r#"{
  "type": "object",
  "properties": {
    "name": { "type": "string", "minLength": 1, "errorMessage": "Your name, please" },
    "age": { "type": "integer", "minimum": 18, "maximum": 130 },
    "plan": { "enum": ["free", "pro"] }
  },
  "required": ["name", "age", "plan"]
}"#;

# fn main() {
let schema = Schema::<Join, Member>::json(JOIN).expect("a schema aio-rs can check");
let ok = Join { name: "Ada".into(), age: "36".into(), plan: "pro".into() };
assert_eq!(schema.parse(&ok), Ok(Member { name: "Ada".into(), age: 36, plan: "pro".into() }));
let bad = Join { name: String::new(), age: "12".into(), ..ok };
assert_eq!(
    schema.parse(&bad).unwrap_err(),
    [("age", "At least 18".to_string()), ("name", "Your name, please".into())]
);
# }
```

- Each property is a field of the form's values, by name; what passes
  is deserialized into the app's type, and a field that won't
  deserialize is a problem on the form (`""`).
- A text input's value is read as its field's type: `"36"` is 36 for an
  `integer`, `""` is no value for a number, a boolean or a nullable field
  (`X | null`). A missing `required` field says `Required`.
- Checked: `type`, `minLength`, `maxLength`, `pattern` (ECMAScript, as
  Zod writes it: classes, groups, lazy and greedy repeats, lookahead),
  `format` (`email`, `uri`, `url`, `uuid`, `date`, `date-time`, `time`,
  `ipv4`, `ipv6`), `minimum`, `maximum`, `exclusiveMinimum`,
  `exclusiveMaximum`, `multipleOf`, `enum`, `const`, `items`,
  `minItems`, `maxItems`, `uniqueItems`, `required`, `anyOf` of one
  schema and `null`. Notes (`title`, `description`, `default`, ...) are
  read past.
- Any other keyword, a lookbehind or a backreference in a `pattern`, a
  nested object or `$ref` is an error from `Schema::json`, naming it:
  never a rule skipped. Read it once (`include_str!` builds it in; a
  test that calls `Schema::json` fails the build's tests on such a file).
- Messages: the schema's own `errorMessage` (ajv-errors: one text, or
  one per keyword; `errorMessage.required` at the top, by field), else
  plain English (`At least 18`, `Not a valid email address`). Problems
  come in the fields' name order.
- A rule across fields or a check that waits: write that schema in Rust
  (above); `.wait(..)` adds a waiting check to a `Schema::json` too.

## Rules that wait

`use_async_rule(form, field, part, debounce, rule)` adds a rule that has to
ask something (is this name taken?):

```rust,no_run
use aio::kit::{Field, Input, Problems, use_async_rule, use_form};
use aio::ui::prelude::*;
use std::time::Duration;

#[derive(Clone, Default, PartialEq)]
struct Account {
    name: String,
}

fn check(a: &Account) -> Problems {
    if a.name.is_empty() { vec![("name", "Required".into())] } else { vec![] }
}

/// Stands in for a lookup (a server function, an HTTP call).
async fn taken(name: String) -> bool {
    name == "admin"
}

#[component]
fn NewAccount() -> View {
    let f = use_form(Account::default, check);
    use_async_rule(f, "name", |a| a.name.clone(), Duration::from_millis(300), |name: String| async move {
        taken(name).await.then(|| "That name is taken".to_string())
    });
    rsx! {
        Field { label: "Name", error: f.error("name"), "data-t": "field",
            Input { "data-t": "name", value: f.values().name,
                oninput: move |s| f.edit("name", |a| a.name = s),
                onchange: move |_| f.touch("name") }
        }
        if f.checking("name") {
            span { "data-t": "checking", "Checking…" }
        }
    }
}

# fn main() {
let mut ui = aio::testing::TestUi::new(NewAccount);
ui.type_text("name", "admin");
ui.blur("name");
ui.until_contains("field", "That name is taken");
# }
```

- It runs `debounce` after the last edit of `part` of the values, not on
  every key, and not for the starting value.
- It doesn't run while the field's own check fails: "Required" is said
  before anything is asked.
- An answer for a value since changed is dropped, so a slow lookup can't
  mark a newer name as taken.
- `Some(message)` refuses, `None` passes. While it runs or refuses, the
  form isn't `valid()` and `submit()` gives `None`; `f.checking(field)`
  shows a spinner.

## Inputs

| Control | Bind it with | Note |
|---|---|---|
| `Input` | `value: v.name, oninput: move \|s\| f.edit("name", ..)` | `oninput` and `onchange` get the text; `r#type` is any input type |
| `Input { r#type: "number" }` | `oninput: move \|s: String\| f.edit("age", \|v\| v.age = s.parse().ok())` | the text is parsed by you; an empty field is `None` |
| `Textarea` | `value`, `oninput` | `rows` (default 3) |
| `Select` | `options: vec!["a".into(), ..]`, `value`, `onchange` | an `Opt { value, label, disabled, view }` per choice |
| `Checkbox` | `checked: v.agree, onchange: move \|on\| f.edit("agree", ..)` | `onchange` gets the new `bool`; `label` puts the text beside it |
| `RadioGroup` | `options`, `value`, `onchange` | one value for the group; a `Field` around it names it |
| `Switch` | `checked`, `onchange` | a checkbox drawn as a switch |

- **Controlled or not.** `Input` with `value: Some(..)` is controlled:
  each change of the value writes it, so what a person types must reach the values
  through `oninput`. Without `value` the input keeps what was typed,
  as aio's `defaultValue` does; read it from `oninput`.
- **A list of rows** (aio's `useFieldArray`) is a `Vec` field: push and
  remove in `f.edit`, and reorder with `aio::kit::move_item(&mut rows,
  from, to)` (out of range moves nothing).
- **Radio buttons** need no binding per button: `RadioGroup` keeps the
  one value, the arrow keys move it, and the group is one tab stop.
- **Files**: a plain `input { r#type: "file" }` opens the system's
  picker; its `onchange` (and its form's `onsubmit`) gets the picked
  files whole, read by the page: `e.files()`, each with `name()`,
  `size()`, `content_type()`, `field()` (its form field on a submit) and
  `read_bytes().await` / `read_string().await`. `e.values()` names each
  file by its file name. A file of many MB travels in the event, so send
  big uploads through an HTTP route instead
  ([cookbook](../basics/cookbook.md#15-an-upload-that-keeps-only-metadata-in-state)).

```rust
use aio::ui::prelude::*;

#[component]
fn Pick() -> View {
    let names = signal(String::new);
    rsx! {
        input { r#type: "file", multiple: true,
            onchange: move |e: FormEvent| names.set(e.files().iter().map(|f| f.name()).collect::<Vec<_>>().join(", ")) }
        p { "{names}" }
    }
}
```

## Submitting

- Put the inputs in a `form` with `onsubmit`, and a `Button { r#type:
  "submit" }`: Enter in any field submits. `Button` is `type="button"`
  unless said, so other buttons in the form never submit it.
- Enter in a form with no submit button submits it too (aio's rule), and
  a button the view enables a moment later (as it hears the last key) is
  submitted once it is.
- A submit never leaves the page: the window has navigation off, and the
  web build stops every submit's default. Call `e.prevent_default()`
  anyway; it says what you mean.
- Native rules (`required`, `pattern`, `min`) still work: the browser
  refuses the submit and shows its own bubble. `TestUi` fails a test whose
  submit the browser refused, naming the field. Prefer `check`: its
  messages are yours, and it runs the same in tests.
- A submit that is a long call: run it as an async method, and show
  `use_pending(Some("method"))` to disable the button while it runs
  ([views](views.md#what-client-makes)).

## Errors from the cell

`act!(add(..))` logs a refusal and gives `None`: right for calls a person
can't get wrong. A form calls the door the client made instead,
`Ui.add(..)` (with the cell's calls trait in scope, as in the module that
holds `client!`), and gets `Result<_, aio::Rejected>`. Show `e.why`
beside the button, as the first example does; `e.code` is `None` for the
method's own `ensure!`, or an `aio::Code` for aio's own refusals
(`AccessDenied`, `ActionRefused`, `ConnectionLost`).

## Drafts in a cell

`use_form`'s values live in the view: closing the window loses them. To
keep a draft across restarts, put it in a cell field with
`#[aio::cell(persist, coalesce)]` (each keystroke a coalesced write,
[performance](../performance.md#2-coalesce-frequent-writes)), and keep
only `touched`-style view state in signals. `views.md`'s first example is
such a form.

## Testing forms

`TestUi` types as a person does, so every handler above runs:

| Call | Does |
|---|---|
| `ui.type_text(t, text)` | focuses `t` and types at the end of its text |
| `ui.set_value(t, text)` | replaces its text, as a paste would |
| `ui.blur(t)` | leaves it (fires `change` when it changed) |
| `ui.press(t, "Enter")` | a key in it |
| `ui.select(t, value)`, `ui.check(t)` | a `Select`'s choice, a checkbox |
| `ui.pick_files(t, &[(name, type, bytes)])` | files picked in file input `t` (`input` and `change`) |
| `ui.until_contains(t, part)` | waits until an async rule's answer shows |

A field left as it was found fires no `change` (the browser's rule): to
test an error that shows on leaving, type something first.

## Bound fields

aio's `bind(name)` spread `value`, `onInput` and `onBlur` in one go;
`form.bind(aio::field!(name))` does the same, and a bound input is touched
on blur, as aio's: `Input { bind: form.bind(aio::field!(name)) }` in a
`Field { error: form.error("name"), … }`; a `Select` or `RadioGroup`
takes `bind` too. A bool field binds a `Checkbox` or `Switch` with
`form.bind_check(aio::field!(agree))` (touched as it is clicked), and a
number (any type text parses into) an `Input` with
`form.bind_parsed(aio::field!(count))`: an edit that doesn't parse leaves
the field as it was (aio's `bindInput`). `aio::field!(name)` names a
field of the values (it reads and writes `v.name`); `set_field` and
`reset_field` change one field (aio's `field.set`, `field.reset`),
`touched(name)` says whether it was left, `use_field_array(|| rows)` is
aio's `useFieldArray` (`push`, `remove`, `move_item`, `set`, `reset`),
and `use_on_change(selector, f)` is aio's `onChange` (only the selector
is followed; `f` may return its cleanup). An input with its own two
handlers (`oninput`, `onchange: f.touch(..)`) is touched on `change`
(leaving a field that changed), not on blur.

## Common mistakes

- Reading `f.error(..)` with no `onchange: f.touch(..)`: the error only
  shows after a submit. Touch each field as it is left.
- A rule that names a field no input edits: it never shows, and the form
  is never valid. Use `""` for the form as a whole.
- `Input { value }` with no `oninput`: the input is controlled, so the
  next write of its value puts the old text back.
- Calling the method from `oninput`: every keystroke becomes an event in
  the log. Edit the form's values; call the method on submit (or keep a
  coalesced draft, as above).
- A second `Button` in the form without `r#type`: it is a plain button,
  as you want. A raw `button {}` inside a `form` submits it (`ax lint`
  warns).

## Gaps

- aio's `useForm` ran the Zod, Valibot or ArkType object itself, so a
  `.refine` or `.transform` written in TypeScript ran too; JSON Schema
  can't carry code, so aio-rs reads what they export (`Schema::json`)
  and a rule written as code is written again in Rust (above).

## API

`cargo doc -p aio-rs --no-deps --all-features --open`: `aio::kit::use_form`,
`use_form_schema`, `Schema`, `Schema::json`, `Rule`, `Parse`, `text`, `rule`, `optional`,
`Form`, `Problems`, `use_async_rule`, `move_item`, `Field`, `Input`,
`Textarea`, `Select`, `Opt`, `Checkbox`, `RadioGroup`, `Switch`, `Button`;
`aio::Rejected`, `aio::Code`.
