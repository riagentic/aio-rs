//! The app: its cell (the list, saved, and the only ways to change it)
//! and its window. `ax dev` hot-patches all of this library.
use aio::ui::prelude::*;

/// One thing to do. `#[aio::data]`: saved and sent (aio brings serde).
#[aio::data]
#[derive(Clone, Debug, PartialEq)]
pub struct Todo {
    pub id: u64,
    pub text: String,
    pub done: bool,
}

/// What the app keeps: each change shown at once and saved within 100 ms
/// (`coalesce`), its history kept (`ax events`).
#[aio::cell(persist, coalesce, id = "todo")]
pub struct Todos {
    pub items: Vec<Todo>,
    pub next_id: u64,
}

#[aio::methods]
impl Todos {
    /// A new item; an empty one is refused.
    pub fn add(&mut self, text: String) -> aio::Result {
        let text = text.trim();
        aio::ensure!(!text.is_empty(), "nothing to do");
        self.next_id += 1;
        self.items.push(Todo { id: self.next_id, text: text.into(), done: false });
        Ok(())
    }

    pub fn toggle(&mut self, id: u64) {
        if let Some(t) = self.items.iter_mut().find(|t| t.id == id) {
            t.done = !t.done;
        }
    }

    pub fn remove(&mut self, id: u64) {
        self.items.retain(|t| t.id != id);
    }

    pub fn clear_done(&mut self) {
        self.items.retain(|t| !t.done);
    }
}

aio::client!(Todos, todos);

/// Which items the window lists.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Filter {
    All,
    Active,
    Done,
}

impl Filter {
    pub const ALL: [Filter; 3] = [Filter::All, Filter::Active, Filter::Done];

    pub fn name(self) -> &'static str {
        match self {
            Filter::All => "all",
            Filter::Active => "active",
            Filter::Done => "done",
        }
    }

    /// Does the window list `t` (pure)?
    pub fn shows(self, t: &Todo) -> bool {
        match self {
            Filter::All => true,
            Filter::Active => !t.done,
            Filter::Done => t.done,
        }
    }
}

/// This window's filter: not in the cell, so never saved or sent.
static FILTER: Global<Filter> = Global::new(|| Filter::All);

/// The window: a field to add, the list (tick, remove), what is left, the
/// filters and Clear done. `part(|t| &t.items)` re-renders only on the list.
#[component]
pub fn App() -> View {
    use_live();
    let items = part(|t| &t.items);
    let text = signal(String::new);
    let theme = aio::kit::theme_css(aio::hue!());
    let add = move |e: FormEvent| {
        e.prevent_default();
        if act!(add(text.get())).is_some() {
            text.set(String::new());
        }
    };
    let left = move || items.with(|i| i.iter().filter(|t| !t.done).count());
    rsx! {
        style { {aio::use_css!(aio::ui::scheme(), &theme; "assets/style.css")} }
        main { style: "max-width:36rem",
            h1 { "todos" }
            form { class: "row", onsubmit: add,
                input { "data-t": "new", value: "{text}", placeholder: "What needs to be done?",
                    "aria-label": "What needs to be done?", style: "flex:1",
                    oninput: move |e| text.set(e.value()) }
                button { r#type: "submit", class: aio::ui::btn("primary"), "Add" }
            }
            ul { "data-t": "items", style: "list-style:none;padding:0",
                for t in items.get().into_iter().filter(|t| FILTER.get().shows(t)) {
                    li { key: "{t.id}", class: "row", "data-t": "item-{t.id}",
                        input { r#type: "checkbox", "data-t": "done-{t.id}", checked: t.done,
                            "aria-label": "Toggle {t.text}", onchange: move |_| _ = act!(toggle(t.id)) }
                        span { style: if t.done { "flex:1;text-decoration:line-through" } else { "flex:1" }, "{t.text}" }
                        button { r#type: "button", class: aio::ui::btn("ghost"), "data-t": "remove-{t.id}",
                            "aria-label": "Remove {t.text}", onclick: move |_| _ = act!(remove(t.id)), "×" }
                    }
                }
            }
            if !items.with(Vec::is_empty) {
                div { class: "row", style: "justify-content:space-between",
                    span { "data-t": "left", {format!("{} item{} left", left(), if left() == 1 { "" } else { "s" })} }
                    div { class: "row",
                        for f in Filter::ALL {
                            button { r#type: "button", key: "{f.name()}", "data-t": "filter-{f.name()}",
                                class: aio::ui::btn(if FILTER.get() == f { "primary" } else { "ghost" }),
                                onclick: move |_| *FILTER.write() = f, "{f.name()}" }
                        }
                    }
                    button { r#type: "button", class: aio::ui::btn("ghost"), "data-t": "clear",
                        onclick: move |_| _ = act!(clear_done()), "Clear done" }
                }
            }
        }
    }
}

/// The app in a browser (`ax dev --web`, `ax build --web`): the same
/// window, its cell kept in the browser's storage under the app's id.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn web() {
    let id = option_env!("AIORT_APP_ID").unwrap_or(env!("CARGO_PKG_NAME"));
    _ = SOURCE.set(aio::Source::Local(aio::Handle::saved(id)));
    aio::web::launch(App);
}

#[cfg(test)]
mod tests {
    use super::*;
    use aio::testing::TestUi;

    #[test]
    fn items_are_added_ticked_removed_and_cleared() {
        let h = aio::Handle::<Todos>::memory();
        assert!(h.add("  ".into()).is_err(), "nothing to do");
        h.add(" write a test ".into()).unwrap();
        h.add("ship it".into()).unwrap();
        h.toggle(1).unwrap();
        assert_eq!(h.state().items[0], Todo { id: 1, text: "write a test".into(), done: true });
        h.clear_done().unwrap();
        h.remove(2).unwrap();
        assert!(h.state().items.is_empty());
    }

    #[test]
    fn the_window_adds_ticks_filters_and_clears() {
        let mut ui = TestUi::new(App);
        assert!(ui.absent("left"), "no items, no footer");
        ui.set_value("new", "milk");
        ui.click("AddButton");
        ui.set_value("new", "bread");
        ui.press("new", "Enter");
        assert_eq!(ui.value("new"), "", "the field empties once added");
        assert_eq!(ui.text("left"), "2 items left");
        ui.check("done-1");
        assert_eq!(ui.text("left"), "1 item left");
        ui.click("filter-active");
        assert_eq!(ui.text("items"), "bread×");
        ui.click("filter-done");
        assert_eq!(ui.text("items"), "milk×");
        ui.click("filter-all");
        ui.click("clear");
        assert_eq!(ui.text("items"), "bread×");
        ui.click("remove-2");
        assert!(ui.absent("left"));
    }
}
