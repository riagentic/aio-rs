//! Key-to-paint bench app: two panels (1000 and 200 rows), a window of 60
//! rows drawn around each cursor; ↓ ↑ Home End move the active cursor, Tab
//! switches panels. The same app as aio's in `../aio`.
use aio::ui::prelude::*;

const SHOWN: usize = 60;
const SIZES: [usize; 2] = [1000, 200];

#[aio::data]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Row {
    pub name: String,
    pub amount: i64,
    pub tag: String,
}

fn rows(p: usize) -> Vec<Row> {
    (0..SIZES[p])
        .map(|i| Row {
            name: format!("Account {p}-{i:04}"),
            amount: ((i * 7919) % 100_000) as i64 - 50_000,
            tag: ["cash", "card", "bank", "loan", "fund"][i % 5].into(),
        })
        .collect()
}

#[aio::cell(id = "nav", custom_default)]
pub struct Nav {
    pub left: aio::Big<Vec<Row>>,
    pub right: aio::Big<Vec<Row>>,
    pub active: usize,
    pub cursor: [usize; 2],
}

impl Default for Nav {
    fn default() -> Self {
        Self { left: aio::Big::new(rows(0)), right: aio::Big::new(rows(1)), active: 0, cursor: [0, 0] }
    }
}

#[aio::methods]
impl Nav {
    pub fn go(&mut self, to: i64) {
        let last = SIZES[self.active] as i64 - 1;
        self.cursor[self.active] = to.clamp(0, last) as usize;
    }

    pub fn step(&mut self, d: i64) {
        let at = self.cursor[self.active] as i64 + d;
        self.go(at);
    }

    pub fn tab(&mut self) {
        self.active = 1 - self.active;
    }
}

aio::client!(Nav, nav);

fn money(a: i64) -> String {
    let s = format!("{:.2}", a as f64 / 100.0);
    if a < 0 { s } else { format!("+{s}") }
}

#[component]
fn Line(p: usize, i: usize, row: Row, cur: Prop<bool>) -> View {
    rsx! {
        div { class: "row", id: "nav-p{p}-i{i}",
            "data-cursor": cur.get().then_some("1"),
            "data-aio-reveal": cur.get().then(|| i.to_string()),
            span { class: "name", "{row.name}" }
            span { class: if row.amount < 0 { "amt neg" } else { "amt pos" }, {money(row.amount)} }
            span { class: "tag", "{row.tag}" }
        }
    }
}

#[component]
fn Panel(p: usize) -> View {
    let (left, right) = (part(|n| &n.left), part(|n| &n.right));
    let (cursor, active) = (part(|n| &n.cursor), part(|n| &n.active));
    let len = SIZES[p];
    let shown = memo(move || {
        let from = cursor.get()[p].saturating_sub(SHOWN / 2).min(len.saturating_sub(SHOWN));
        from..(from + SHOWN).min(len)
    });
    let row = move |i: usize| if p == 0 { left.with(|r| r[i].clone()) } else { right.with(|r| r[i].clone()) };
    rsx! {
        div { class: "panel", id: "nav-panel-{p}",
            for i in shown.get() {
                Line { key: "{i}", p, i, row: row(i), cur: active.get() == p && cursor.get()[p] == i }
            }
        }
    }
}

#[component]
pub fn App() -> View {
    use_live();
    let keys = move |e: KeyboardEvent| {
        _ = match e.key() {
            Key::ArrowDown => act!(step(1)),
            Key::ArrowUp => act!(step(-1)),
            Key::Home => act!(go(0)),
            Key::End => act!(go(i64::MAX)),
            Key::Tab => act!(tab()),
            _ => None,
        };
    };
    rsx! {
        style { {CSS} }
        div { id: "nav", tabindex: "0", onkeydown: keys,
            "data-aio-prevent": "ArrowUp ArrowDown Home End Tab",
            Panel { p: 0 }
            Panel { p: 1 }
        }
    }
}

const CSS: &str = "body{margin:0;font:13px sans-serif}#nav{display:flex;gap:8px;height:100vh;outline:none}\
.panel{flex:1;overflow:auto}.row{display:flex;gap:8px;padding:2px 6px;transition:none}\
.row[data-cursor]{background:#36c;color:#fff}.name{flex:1}.neg{color:#c33}.pos{color:#3a3}\
.tag{width:40px;opacity:.7}";
