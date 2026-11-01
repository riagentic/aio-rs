//! Surfaces, layout and data (aio's `Card`, `Stack`, `Row`, `Spinner`, `Avatar`,
//! `Pagination`, `Table`).
use super::{flag, join};
use dioxus::prelude::*;

/// A bordered surface with an optional title and footer.
#[component]
pub fn Card(
    title: Option<Element>,
    footer: Option<Element>,
    #[props(default, into)] class: String,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    rsx! {
        div { class: join("aio-card", &class), ..attributes,
            if let Some(t) = title {
                div { class: "aio-card__title", {t} }
            }
            div { class: "aio-card__body", {children} }
            if let Some(f) = footer {
                div { class: "aio-card__footer", {f} }
            }
        }
    }
}

/// The inline style of a [`Stack`] or [`Row`].
fn flex(row: bool, gap: &str, align: &str) -> String {
    let wrap = if row { "row;flex-wrap:wrap" } else { "column;flex-wrap:nowrap" };
    let align = if align.is_empty() { String::new() } else { format!(";align-items:{align}") };
    format!("display:flex;flex-direction:{wrap};gap:{gap}{align}")
}

/// Children in a column, `gap` apart (any CSS length; 12px by default).
#[component]
pub fn Stack(
    #[props(default = String::from("12px"), into)] gap: String,
    #[props(default, into)] align: String,
    #[props(default, into)] class: String,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    rsx! {
        div { class: join("aio-stack", &class), style: flex(false, &gap, &align), ..attributes, {children} }
    }
}

/// Children in a row that wraps, `gap` apart.
#[component]
pub fn Row(
    #[props(default = String::from("12px"), into)] gap: String,
    #[props(default, into)] align: String,
    #[props(default, into)] class: String,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    rsx! {
        div { class: join("aio-stack", &class), style: flex(true, &gap, &align), ..attributes, {children} }
    }
}

/// A loading spinner, announced as `label`.
#[component]
pub fn Spinner(
    #[props(default = String::from("Loading"), into)] label: String,
    #[props(default, into)] class: String,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
) -> Element {
    rsx! {
        span { class: join("aio-spinner", &class), role: "status", "aria-label": label, ..attributes }
    }
}

/// Up to two initials: the first character of the first and last word
/// (whole characters, so an emoji or 𠮷 is never cut in half).
pub fn initials(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().collect();
    let lead = |w: &str| w.chars().next().map(String::from).unwrap_or_default();
    match words.as_slice() {
        [] => "?".into(),
        [one] => lead(one).to_uppercase(),
        [first, .., last] => (lead(first) + &lead(last)).to_uppercase(),
    }
}

/// A stable hue for a name (aio's, so a person keeps their colour).
pub fn avatar_hue(name: &str) -> u16 {
    const HUES: [u16; 8] = [210, 12, 145, 275, 32, 190, 330, 95];
    let h = name.encode_utf16().fold(0u32, |h, c| h.wrapping_mul(31).wrapping_add(u32::from(c)));
    HUES[h as usize % HUES.len()]
}

/// A round avatar: the image at `src`, or initials on a colour from the name.
#[component]
pub fn Avatar(
    #[props(into)] name: String,
    #[props(into)] src: Option<String>,
    #[props(default = 32)] size: u32,
    /// Any CSS colour instead of the one from the name.
    #[props(into)]
    color: Option<String>,
    #[props(default, into)] class: String,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
) -> Element {
    let bg = color.unwrap_or_else(|| format!("hsl({}, 55%, 45%)", avatar_hue(&name)));
    let font = (f64::from(size) * 0.4).round();
    rsx! {
        span {
            class: join("aio-avatar", &class),
            style: "width:{size}px;height:{size}px;font-size:{font}px;background:{bg}",
            role: "img",
            "aria-label": "{name}",
            title: "{name}",
            ..attributes,
            if let Some(src) = src {
                img { class: "aio-avatar__img", src, alt: "{name}", width: "{size}", height: "{size}" }
            } else {
                {initials(&name)}
            }
        }
    }
}

/// The page numbers a pager shows: `win` of them around `page`, clamped to 1..=pages.
pub fn page_window(page: u32, pages: u32, win: u32) -> Vec<u32> {
    if pages <= win {
        return (1..=pages).collect();
    }
    let lo = page.saturating_sub(win / 2).max(1);
    let hi = (lo + win - 1).min(pages);
    (hi + 1 - win..=hi).collect()
}

/// Prev, the numbered pages around the current one, next. Every enabled
/// button goes somewhere: targets are clamped, and a button whose target is
/// the page shown is disabled (a stale `page` past the end still leads back).
#[component]
pub fn Pagination(
    /// The current page, from 1.
    page: u32,
    pages: u32,
    onpage: EventHandler<u32>,
    #[props(default = 5)] window: u32,
    #[props(default = String::from("Pagination"), into)] label: String,
    #[props(default, into)] class: String,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
) -> Element {
    let clamp = |p: u32| p.clamp(1, pages.max(1));
    let (prev, next) = (clamp(page.saturating_sub(1)), clamp(page + 1));
    let go = move |p: u32| {
        if p != page {
            onpage.call(p)
        }
    };
    rsx! {
        nav { class: join("aio-page", &class), "aria-label": label, ..attributes,
            button { r#type: "button", class: "aio-page__btn", disabled: flag(prev == page), "aria-label": "Previous page", "data-t": "pagePrev",
                onclick: move |_| go(prev), "‹"
            }
            for p in page_window(clamp(page), pages, window) {
                button {
                    key: "{p}",
                    r#type: "button",
                    class: if p == page { "aio-page__btn aio-page__btn--current" } else { "aio-page__btn" },
                    "aria-label": "Page {p}",
                    "data-t": "page{p}",
                    "aria-current": (p == page).then_some("page"),
                    onclick: move |_| go(p),
                    "{p}"
                }
            }
            button { r#type: "button", class: "aio-page__btn", disabled: flag(next == page), "aria-label": "Next page", "data-t": "pageNext",
                onclick: move |_| go(next), "›"
            }
        }
    }
}

/// Where a column's cells sit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    #[default]
    Start,
    Center,
    End,
}

/// One column of a [`Table`]: its header and how a row shows in it.
#[derive(Clone, PartialEq)]
pub struct Column<R: 'static> {
    pub header: String,
    pub cell: Callback<(R, usize), Element>,
    pub align: Align,
    /// Any CSS width.
    pub width: Option<String>,
}

impl<R: 'static> Column<R> {
    /// A column of text.
    pub fn text(header: impl Into<String>, f: impl Fn(&R) -> String + 'static) -> Self {
        Self::view(header, move |r, _| rsx! { {f(r)} })
    }

    /// A column of anything (`f` gets the row and its index).
    pub fn view(header: impl Into<String>, f: impl Fn(&R, usize) -> Element + 'static) -> Self {
        Self { header: header.into(), cell: Callback::new(move |(r, i)| f(&r, i)), align: Align::Start, width: None }
    }

    pub fn align(self, align: Align) -> Self {
        Self { align, ..self }
    }

    pub fn width(self, width: impl Into<String>) -> Self {
        Self { width: Some(width.into()), ..self }
    }

    fn style(&self) -> String {
        let align = ["start", "center", "end"][self.align as usize];
        let width = self.width.as_ref().map(|w| format!(";width:{w}")).unwrap_or_default();
        format!("text-align:{align}{width}")
    }
}

/// Enter or Space: what presses a button.
fn press(e: &KeyboardEvent) -> bool {
    e.key() == Key::Enter || e.key() == Key::Character(" ".into())
}

/// A table of `rows` (each row's handle is `row-{key}`). With `onrow`, a row
/// is a button too: in the tab order, Enter or Space on it (not on a control
/// inside it) opens it.
#[component]
pub fn Table<R: Clone + PartialEq + 'static>(
    columns: Vec<Column<R>>,
    rows: Vec<R>,
    /// A stable key per row (the index by default).
    key_of: Option<Callback<R, String>>,
    /// Shown when there are no rows.
    #[props(default = rsx! { "No rows" })]
    empty: Element,
    onrow: Option<EventHandler<(R, usize)>>,
    #[props(default, into)] class: String,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
) -> Element {
    let span = columns.len().max(1);
    rsx! {
        table { class: join("aio-table", &class), ..attributes,
            thead {
                tr {
                    for c in columns.iter() {
                        th { class: "aio-th", style: c.style(), "{c.header}" }
                    }
                }
            }
            tbody {
                if rows.is_empty() {
                    tr {
                        td { class: "aio-td aio-table__empty", colspan: "{span}", {empty} }
                    }
                }
                for (i, row) in rows.into_iter().enumerate() {
                    {
                        let key = key_of.map_or(i.to_string(), |k| k.call(row.clone()));
                        let cells = rsx! {
                            for c in columns.iter() {
                                td {
                                    class: "aio-td",
                                    style: c.style(),
                                    // Enter or Space on a control in a cell is the control's, not the row's.
                                    onkeydown: move |e| if onrow.is_some() && press(&e) { e.stop_propagation() },
                                    {c.cell.call((row.clone(), i))}
                                }
                            }
                        };
                        match onrow {
                            None => rsx! {
                                tr { key: "{key}", "data-t": "row-{key}", class: "aio-tr", {cells} }
                            },
                            Some(f) => {
                                let (r1, r2) = (row.clone(), row.clone());
                                rsx! {
                                    tr {
                                        key: "{key}",
                                        "data-t": "row-{key}",
                                        class: "aio-tr aio-tr--click",
                                        tabindex: "0",
                                        onclick: move |e| {
                                            e.stop_propagation();
                                            f.call((r1.clone(), i))
                                        },
                                        onkeydown: move |e: KeyboardEvent| if press(&e) {
                                            e.prevent_default();
                                            e.stop_propagation();
                                            f.call((r2.clone(), i));
                                        },
                                        {cells}
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initials_are_whole_characters_of_the_first_and_last_word() {
        assert_eq!(initials("Ada Lovelace"), "AL");
        assert_eq!(initials("root"), "R");
        assert_eq!(initials("  Grace  Brewster  Hopper "), "GH");
        assert_eq!(initials("   "), "?");
        assert_eq!(initials("𠮷野 家"), "𠮷家");
        assert_eq!(initials("🦀 crab"), "🦀C");
    }

    #[test]
    fn a_name_keeps_its_colour() {
        assert_eq!(avatar_hue("Ada Lovelace"), avatar_hue("Ada Lovelace"));
        assert_ne!(avatar_hue("Ada Lovelace"), avatar_hue("Alan Turing"));
    }

    #[test]
    fn the_page_window_centres_and_clamps_at_the_ends() {
        assert_eq!(page_window(1, 3, 5), [1, 2, 3]);
        assert_eq!(page_window(5, 10, 5), [3, 4, 5, 6, 7]);
        assert_eq!(page_window(1, 10, 5), [1, 2, 3, 4, 5]);
        assert_eq!(page_window(10, 10, 5), [6, 7, 8, 9, 10]);
    }
}
