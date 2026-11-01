# A checkout workflow

A shop's checkout as a whole app: a cart, a stock of goods, and a payment
that takes several steps and can fail at each. It shows how aio-rs keeps
a multi-step flow correct when people click twice, change their mind
halfway, or a provider says no:

- a **state machine** decides which calls may run now, so a second
  "Pay" or an edit during payment is refused, not raced;
- an **effect** does the slow work (reserve, charge) after the call
  commits, and reports back with calls of its own;
- **refusals** are the error path: a refused call changes nothing, and a
  late answer for an order that was cancelled is refused too;
- **two cells** work together without calling each other from a method.

Every Rust block on this page compiles, and the logic runs, as a test of
aio-rs. Prerequisites: [cells](../state/cells.md) and
[effects](../state/effects.md).

## What we build

| Piece | Kind | Job |
|---|---|---|
| `Stock` | a cell | what is on hand, and what each order holds |
| `Checkout` | a cell with a machine | the cart, the phase, the receipt or the error |
| `pay` | an effect of `Checkout` | reserve the stock, charge the card, report back |
| the window | Dioxus views | the cart, a Pay button, the outcome |

The phases of a checkout, and the calls that move between them:

```text
Shopping --checkout--> Paying --paid--> Paid --start_over--> Shopping
   ^  (add, remove)      |  \--failed--> Failed --checkout--> Paying
   |                     |                  \--edit--> Shopping
   +-------cancel--------+
```

Start from an app (`ax create shop`) and put the code below in
`src/lib.rs`.

## 1. The stock

The stock is a plain cell. `reserve` holds every line of an order or
none: it changes the copy it runs on line by line, and a refusal halfway
throws the whole copy away, so nothing is ever half reserved.

```rust
use aio::prelude::*;
use std::collections::BTreeMap;

#[data]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Line {
    pub item: String,
    pub qty: u32,
    pub cents: u64,
}

/// What is on hand, and what each order holds until it is paid or let go.
#[cell(persist, id = "stock")]
pub struct Stock {
    pub on_hand: BTreeMap<String, u32>,
    pub held: BTreeMap<Uuid, Vec<Line>>,
}

#[methods]
impl Stock {
    pub fn restock(&mut self, item: String, qty: u32) {
        *self.on_hand.entry(item).or_default() += qty;
    }

    /// Hold every line for `order`, or none.
    pub fn reserve(&mut self, order: Uuid, lines: Vec<Line>) -> Result {
        ensure!(!self.held.contains_key(&order), "order {order} holds stock already");
        for l in &lines {
            let have = self.on_hand.entry(l.item.clone()).or_default();
            ensure!(*have >= l.qty, "only {have} {} left", l.item);
            *have -= l.qty;
        }
        self.held.insert(order, lines);
        Ok(())
    }

    /// Put what `order` holds back. Holding nothing is fine: safe to repeat.
    pub fn release(&mut self, order: Uuid) {
        for l in self.held.remove(&order).unwrap_or_default() {
            *self.on_hand.entry(l.item).or_default() += l.qty;
        }
    }

    /// The order is paid: what it held is sold.
    pub fn spend(&mut self, order: Uuid) {
        self.held.remove(&order);
    }
}

# fn main() {
let s = Handle::<Stock>::memory();
s.restock("tea".into(), 3).unwrap();
s.restock("cups".into(), 1).unwrap();
let order = Uuid::nil();
let lines = vec![
    Line { item: "tea".into(), qty: 2, cents: 450 },
    Line { item: "cups".into(), qty: 2, cents: 900 },
];
assert_eq!(s.reserve(order, lines).unwrap_err().why, "only 1 cups left");
assert_eq!(s.state().on_hand["tea"], 3, "the tea taken before the refusal is back: nothing changed");
s.release(order).unwrap(); // nothing held: nothing to do
# }
```

`release` takes the order, not the lines: whoever calls it (a cancel, a
failed charge, both) can't put back more than was held.

## 2. The checkout and its machine

`aio::machine!` declares the phases and, per phase, the calls that leave
it. On the cell, `machine = phase` makes it a guard: a call the machine
names runs only from a phase that lists it, and the field moves to the
target before the method runs. A call from any other phase is refused
with a reason, and nothing changes.

```rust
# use aio::prelude::*;
# use std::collections::BTreeMap;
# #[data]
# #[derive(Clone, Debug, Default, PartialEq)]
# pub struct Line { pub item: String, pub qty: u32, pub cents: u64 }
aio::machine! {
    pub enum Phase {
        Shopping { add => Shopping, remove => Shopping, checkout => Paying },
        Paying { paid => Paid, failed => Failed, cancel => Shopping },
        Failed { checkout => Paying, edit => Shopping },
        Paid { start_over => Shopping },
    }
}

#[cell(persist, machine = phase, id = "checkout")]
pub struct Checkout {
    pub phase: Phase,
    pub lines: Vec<Line>,
    /// The order being paid (or last paid), so a late answer for another is refused.
    pub order: Option<Uuid>,
    pub receipt: Option<String>,
    pub error: Option<String>,
}

#[methods]
impl Checkout {
    pub fn add(&mut self, item: String, qty: u32, cents: u64) -> Result {
        ensure!(qty > 0, "how many?");
        match self.lines.iter_mut().find(|l| l.item == item) {
            Some(l) => l.qty += qty,
            None => self.lines.push(Line { item, qty, cents }),
        }
        Ok(())
    }

    pub fn remove(&mut self, item: String) {
        self.lines.retain(|l| l.item != item);
    }

    /// Back to the cart from a failure, keeping the lines to change them.
    pub fn edit(&mut self) {}

    /// A new cart after a paid order.
    pub fn start_over(&mut self) {
        self.lines.clear();
        (self.order, self.receipt, self.error) = (None, None, None);
    }

    #[derived]
    pub fn total(&self) -> u64 {
        self.lines.iter().map(|l| u64::from(l.qty) * l.cents).sum()
    }
#     pub fn checkout(&mut self) {}
#     pub fn paid(&mut self) {}
#     pub fn failed(&mut self) {}
#     pub fn cancel(&mut self) {}
}

# fn main() {
let c = Handle::<Checkout>::memory();
assert_eq!(c.state().phase, Phase::Shopping, "the first phase is where it starts");
c.add("tea".into(), 2, 450).unwrap();
c.add("tea".into(), 1, 450).unwrap();
assert_eq!(c.state().total(), 1350);
assert_eq!(c.edit().unwrap_err().why, "`edit` can't run while Shopping: only from Failed");
# }
```

- The first phase listed is the default. The field is saved as the
  phase's lowercase name (`"paying"`), so `ax state shop checkout` reads
  well.
- `add => Shopping` is a move to itself: `add` runs only while shopping,
  so a line can't change under a payment.
- A machine that names a method the cell lacks, or has a phase no path
  reaches, refuses to open: a typo fails the first test, not a customer.
- A method the machine never names (none here) runs in any phase.

## 3. Paying: the effect

`checkout` commits the move to `Paying` and asks for the effect `pay`.
The effect runs after the commit, off the UI thread. It reserves the
stock, charges the card and ends with exactly one call back: `paid` or
`failed`. Each of those is an ordinary call, checked by the machine and
by the method, and logged.

```rust
# use aio::prelude::*;
# use std::collections::BTreeMap;
# #[data]
# #[derive(Clone, Debug, Default, PartialEq)]
# pub struct Line { pub item: String, pub qty: u32, pub cents: u64 }
# #[cell(persist, id = "stock")]
# pub struct Stock { pub on_hand: BTreeMap<String, u32>, pub held: BTreeMap<Uuid, Vec<Line>> }
# #[methods]
# impl Stock {
#     pub fn restock(&mut self, item: String, qty: u32) { *self.on_hand.entry(item).or_default() += qty; }
#     pub fn reserve(&mut self, order: Uuid, lines: Vec<Line>) -> Result {
#         ensure!(!self.held.contains_key(&order), "order {order} holds stock already");
#         for l in &lines {
#             let have = self.on_hand.entry(l.item.clone()).or_default();
#             ensure!(*have >= l.qty, "only {have} {} left", l.item);
#             *have -= l.qty;
#         }
#         self.held.insert(order, lines);
#         Ok(())
#     }
#     pub fn release(&mut self, order: Uuid) {
#         for l in self.held.remove(&order).unwrap_or_default() { *self.on_hand.entry(l.item).or_default() += l.qty; }
#     }
#     pub fn spend(&mut self, order: Uuid) { self.held.remove(&order); }
# }
# aio::machine! {
#     pub enum Phase {
#         Shopping { add => Shopping, remove => Shopping, checkout => Paying },
#         Paying { paid => Paid, failed => Failed, cancel => Shopping },
#         Failed { checkout => Paying, edit => Shopping },
#         Paid { start_over => Shopping },
#     }
# }
# #[cell(persist, machine = phase, id = "checkout")]
# pub struct Checkout {
#     pub phase: Phase,
#     pub lines: Vec<Line>,
#     pub order: Option<Uuid>,
#     pub receipt: Option<String>,
#     pub error: Option<String>,
# }
use std::sync::OnceLock;
use std::time::Duration;

/// The stock cell, set once at boot (and by tests): effects reach it here.
pub static STOCK: OnceLock<Handle<Stock>> = OnceLock::new();

/// Stands in for a payment provider: 0.5 s, and it declines over 100.00.
async fn charge(order: Uuid, cents: u64) -> std::result::Result<String, String> {
    aio::time::sleep(Duration::from_millis(500)).await;
    match cents {
        0..=10_000 => Ok(format!("ch_{}", order.simple())),
        _ => Err("card declined".into()),
    }
}

/// Reserve, charge, report: one answer per order, and the stock never leaks.
async fn pay(h: Handle<Checkout>, order: Uuid, lines: Vec<Line>, total: u64) {
    let stock = STOCK.get().expect("STOCK is set at boot");
    if let Err(e) = stock.reserve(order, lines) {
        _ = h.failed(order, e.why);
        return;
    }
    match charge(order, total).await {
        Ok(receipt) => match h.paid(order, receipt) {
            Ok(()) => _ = stock.spend(order),
            Err(_) => _ = stock.release(order), // cancelled meanwhile: a real app refunds here
        },
        Err(why) => {
            _ = stock.release(order);
            _ = h.failed(order, why);
        }
    }
}

#[methods]
impl Checkout {
#     pub fn add(&mut self, item: String, qty: u32, cents: u64) -> Result {
#         ensure!(qty > 0, "how many?");
#         match self.lines.iter_mut().find(|l| l.item == item) {
#             Some(l) => l.qty += qty,
#             None => self.lines.push(Line { item, qty, cents }),
#         }
#         Ok(())
#     }
#     pub fn remove(&mut self, item: String) { self.lines.retain(|l| l.item != item); }
#     pub fn edit(&mut self) {}
#     pub fn start_over(&mut self) {
#         self.lines.clear();
#         (self.order, self.receipt, self.error) = (None, None, None);
#     }
#     #[derived]
#     pub fn total(&self) -> u64 { self.lines.iter().map(|l| u64::from(l.qty) * l.cents).sum() }
    /// Shopping or Failed -> Paying: a new order, paid by the effect.
    pub fn checkout(&mut self, cx: &Cx) -> Result {
        ensure!(!self.lines.is_empty(), "the cart is empty");
        let order = cx.id();
        (self.order, self.receipt, self.error) = (Some(order), None, None);
        let (lines, total) = (self.lines.clone(), self.total());
        cx.run(
            Effect::new("pay", move |h: Handle<Checkout>| pay(h, order, lines, total))
                .timeout(Duration::from_secs(30)),
        );
        Ok(())
    }

    /// Paying -> Paid, for the order being paid only.
    pub fn paid(&mut self, order: Uuid, receipt: String) -> Result {
        ensure!(self.order == Some(order), "a late answer for another order");
        self.receipt = Some(receipt);
        Ok(())
    }

    /// Paying -> Failed, with the reason to show.
    pub fn failed(&mut self, order: Uuid, why: String) -> Result {
        ensure!(self.order == Some(order), "a late answer for another order");
        self.error = Some(why);
        Ok(())
    }

    /// Paying -> Shopping: stop paying and give the stock back.
    pub fn cancel(&mut self, cx: &Cx) {
        cx.abort("pay"); // a running `pay` stops at its next await: the charge
        if let Some(order) = self.order.take() {
            cx.run(Effect::new("release", move |_: Handle<Checkout>| async move {
                _ = STOCK.get().expect("STOCK is set at boot").release(order);
            }));
        }
    }
}

# fn main() {
use aio::testing::TestCells;
let mut t = TestCells::new(); // in-process cells on a virtual clock
let stock = t.cell::<Stock>();
STOCK.set(stock.clone()).ok();
stock.restock("tea".into(), 3).unwrap();
let c = t.cell::<Checkout>();

// The happy path.
c.add("tea".into(), 2, 450).unwrap();
c.checkout().unwrap();
assert_eq!(c.state().phase, Phase::Paying, "committed; the effect waits");
assert_eq!(c.checkout().unwrap_err().why, "`checkout` can't run while Paying: only from Shopping, Failed");
assert!(c.add("cups".into(), 1, 900).is_err(), "no edits while paying");
t.settle(); // runs effects to their end, the 0.5 s at once
assert_eq!(c.state().phase, Phase::Paid);
assert!(c.state().receipt.as_deref().unwrap().starts_with("ch_"));
assert_eq!((stock.state().on_hand["tea"], stock.state().held.len()), (1, 0), "sold, nothing held");

// Out of stock: refused by Stock, said by Checkout.
c.start_over().unwrap();
c.add("tea".into(), 2, 450).unwrap();
c.checkout().unwrap();
t.settle();
assert_eq!((c.state().phase, c.state().error.as_deref()), (Phase::Failed, Some("only 1 tea left")));

// Declined: the stock it held goes back.
c.edit().unwrap();
c.remove("tea".into()).unwrap();
stock.restock("teapot".into(), 1).unwrap();
c.add("teapot".into(), 1, 20_000).unwrap();
c.checkout().unwrap();
t.settle();
assert_eq!(c.state().error.as_deref(), Some("card declined"));
assert_eq!((stock.state().on_hand["teapot"], stock.state().held.len()), (1, 0));

// Cancelled before `pay` started: it never starts, so nothing is held
// and no answer comes late.
c.edit().unwrap();
c.remove("teapot".into()).unwrap();
c.add("tea".into(), 1, 450).unwrap();
c.checkout().unwrap();
assert_eq!(t.effects(), ["Checkout.pay"], "asked for, not started yet");
c.cancel().unwrap();
t.settle();
assert_eq!(c.state().phase, Phase::Shopping);
assert!(c.state().receipt.is_none());
assert_eq!((stock.state().on_hand["tea"], stock.state().held.len()), (1, 0));
assert!(t.failures().is_empty(), "pay never ran: nothing was refused");
# }
```

In the last case `cancel` came before the test let the effect start, so
`abort` dropped it: `pay` never ran and nothing was reserved. A `pay`
already charging stops at its next await; had its `paid` still arrived,
it would find the order gone and the machine in `Shopping`, and be
refused. That is the race the order check and the machine exist for. A
refused call an effect made is kept as a failure: `TestCells` fails the
test at its end unless the test takes it with `failures()`.

What each part guarantees:

| Risk | What stops it |
|---|---|
| Pay clicked twice | the machine: `checkout` can't run while `Paying` |
| the cart edited during payment | the machine: `add` and `remove` only while `Shopping` |
| half an order reserved | `reserve` is one call: a refusal keeps none of its changes |
| a slow answer for a cancelled order | `paid` / `failed` check `order`; while `Shopping` the machine refuses them anyway |
| stock held by a dead payment | `cancel` aborts `pay` and releases; `release` of nothing is a no-op |
| a provider that hangs | `.timeout(30 s)` drops the effect; the order stays `Paying` until `cancel` |

The effect does the I/O, the methods decide. A method never waits and
never calls another cell: a method does no I/O, so a replay of the log
gives the same state without charging anyone again
([effects](../state/effects.md)).

### Why not an async method

An async method (`pub async fn pay(..)` with `let x: T = io.await;`)
reads top to bottom like aio's, and is the right choice for a plain
chain of steps. Here each outcome must pass the machine as a call of its
own (`paid`, `failed`), and the effect calls another cell: an explicit
`Effect` says that directly. The steps of an async method (`pay.1`…) are
not named by a machine, so they run in any phase.

## 4. The window

The views read the phase and the outcome, and offer only the buttons
the phase allows. The server-side guard is the machine; the disabled
buttons are a courtesy.

```rust,no_run
# #[aio::data]
# #[derive(Clone, Debug, Default, PartialEq)]
# pub struct Line { pub item: String, pub qty: u32, pub cents: u64 }
# aio::machine! {
#     pub enum Phase {
#         Shopping { add => Shopping, remove => Shopping, checkout => Paying },
#         Paying { paid => Paid, failed => Failed, cancel => Shopping },
#         Failed { checkout => Paying, edit => Shopping },
#         Paid { start_over => Shopping },
#     }
# }
# #[aio::cell(persist, machine = phase, id = "checkout")]
# pub struct Checkout {
#     pub phase: Phase,
#     pub lines: Vec<Line>,
#     pub order: Option<aio::Uuid>,
#     pub receipt: Option<String>,
#     pub error: Option<String>,
# }
# #[aio::methods]
# impl Checkout {
#     pub fn add(&mut self, item: String, qty: u32, cents: u64) -> aio::Result {
#         match self.lines.iter_mut().find(|l| l.item == item) {
#             Some(l) => l.qty += qty,
#             None => self.lines.push(Line { item, qty, cents }),
#         }
#         Ok(())
#     }
#     pub fn remove(&mut self, item: String) { self.lines.retain(|l| l.item != item); }
#     pub fn edit(&mut self) {}
#     pub fn start_over(&mut self) { self.lines.clear(); }
#     pub fn checkout(&mut self) -> aio::Result { aio::ensure!(!self.lines.is_empty(), "the cart is empty"); Ok(()) }
#     pub fn paid(&mut self, receipt: String) { self.receipt = Some(receipt); }
#     pub fn failed(&mut self, why: String) { self.error = Some(why); }
#     pub fn cancel(&mut self) {}
#     #[derived]
#     pub fn total(&self) -> u64 { self.lines.iter().map(|l| u64::from(l.qty) * l.cents).sum() }
# }
use aio::act;
use dioxus::prelude::*;

aio::client!(Checkout, checkout);

fn money(cents: u64) -> String {
    format!("{}.{:02}", cents / 100, cents % 100)
}

#[component]
pub fn App() -> Element {
    use_live();
    rsx! { main { Cart {} Pay {} } }
}

#[component]
fn Cart() -> Element {
    let lines = part(|c| &c.lines);
    let shopping = *part(|c| &c.phase) == Phase::Shopping;
    rsx! {
        button { "data-t": "add-tea", disabled: !shopping, onclick: move |_| _ = act!(add("tea".into(), 1, 450)), "Add tea" }
        ul { "data-t": "lines",
            for l in lines.iter() {
                li { key: "{l.item}", "{l.qty} × {l.item} ",
                    button { disabled: !shopping, onclick: { let item = l.item.clone(); move |_| _ = act!(remove(item.clone())) }, "Remove" }
                }
            }
        }
    }
}

#[component]
fn Pay() -> Element {
    let phase = *part(|c| &c.phase);
    let total = checkout().total(); // a derived value, cached per state
    let why = part(|c| &c.error).as_deref().unwrap_or_default().to_string();
    rsx! {
        p { "data-t": "phase", "{phase:?}" }
        match phase {
            Phase::Shopping => rsx! {
                button { "data-t": "pay", class: aio::ui::btn("primary"), disabled: total == 0,
                    onclick: move |_| _ = act!(checkout()), "Pay {money(total)}" }
            },
            Phase::Paying => rsx! { button { "data-t": "cancel", onclick: move |_| _ = act!(cancel()), "Cancel" } },
            Phase::Failed => rsx! {
                p { "data-t": "error", role: "alert", "{why}" }
                button { "data-t": "retry", onclick: move |_| _ = act!(checkout()), "Try again" }
                button { "data-t": "edit", onclick: move |_| _ = act!(edit()), "Change the cart" }
            },
            Phase::Paid => rsx! { button { "data-t": "again", onclick: move |_| _ = act!(start_over()), "New order" } },
        }
    }
}

# fn main() {
let mut ui = aio::testing::TestUi::new(App);
ui.click("add-tea");
ui.click("pay");
# }
```

`checkout()` reads the whole state, so `Pay` re-renders on every commit
of the cell; that is fine for a small view, and `part(..)` per field is
the way when it isn't ([views](../ui/views.md)).

## 5. Boot

The window's cell comes from the template's launcher (`app::SOURCE`,
[project](../basics/project.md#the-launcher)). The stock opens on the same
store, before the window, so the effects find it:

```rust,ignore
// ax create's launcher (app/src/main.rs), compiled by ax's own tests:
let cfg = aio::config!();
let Start::Run(dirs) = aio_shell::start(&cfg, aio::app_version!())? else { return Ok(()) };
_ = app::SOURCE.set(aio::Source::for_app(&cfg, &dirs)?);
aio_shell::run(app::App, aio_shell::Window { width: 900, height: 640, ..aio_shell::Window::of(&cfg) }, &dirs)
```

Add one line before `aio_shell::run`:
`_ = app::STOCK.set(aio::Handle::open(&dirs.store).map_err(|e| std::io::Error::other(e.0))?);`.
Then run it and look inside from another terminal:

```sh
ax dev shop                         # the window
ax call shop stock restock '"tea"' 5
ax state shop checkout              # the phase, the lines, the outcome
ax events shop --last 10 --diff     # every call, each with what it changed
ax timetravel shop checkout undo    # step the running cell back one event
```

## 6. Reading what happened

Every committed call is an event in the log, the effect's own calls
included (made `by` the effect, `By::Effect("pay")`). A paid order reads:

| `checkout` events | `stock` events |
|---|---|
| `add` | `restock` |
| `checkout` (to `Paying`) | `reserve` |
| `paid` (to `Paid`) | `spend` |

A declined one ends in `failed` and `release`; a cancelled one in
`cancel` and `release`. A refusal is not an event: the call never
happened, and the caller got its reason. `ax events shop --diff` shows
each event with the fields it changed, and the time-travel panel
(`aio::kit::TimeTravelPanel`, or `ax timetravel`) steps the live cell
through them ([debugging](../debugging/README.md)).

## What maps to what

| aio's checkout | Here |
|---|---|
| a guard line, `if (s.status === "processing") return` | `machine = phase`: a refusal with a reason, not a silent return |
| `cancelOn: { process: [cart.clear] }` + `s.$signal` | `cx.abort("pay")` in `cancel`: the effect is dropped at its next await |
| `await inventory.reserve(items)` from a method | the effect calls `STOCK`; a method never calls another cell |
| `call({ timeoutMs, retries })` | `Effect::timeout`, `aio::effect::retry` |
| time-travel of `payment:__setProcess` batches | one event per call: `checkout`, `paid` or `failed`, and the stock's own |

## Common mistakes

- Waiting for the charge inside `checkout`: a method does no I/O. Ask
  for an effect and let it call back.
- A result call that doesn't check which order it is for: a slow answer
  for a cancelled order would land on the next one. Keep the id and
  compare.
- Releasing by lines instead of by order: a cancel and a failure both
  releasing would put the stock back twice.
- Checking the phase in the view only: another client or `ax call` can
  still call. The machine is the check that holds.
- Expecting an aborted effect to stop at once: it stops at its next
  await. Code between two awaits always runs to the next one.

## API

`cargo doc -p aio-rs --no-deps --open`: `aio::machine!`,
`aio::machine` (`Machine`, `step`, `dead_ends`), `aio::Effect`,
`aio::Mode`, `aio::Cx` (`run`, `abort`, `id`), `aio::effect::retry`,
`aio::testing::TestCells` (`settle`, `advance`, `effects`).
