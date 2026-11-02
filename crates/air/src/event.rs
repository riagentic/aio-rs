//! Events from the page and air's own payload types, W3C-shaped with the
//! names and methods the views were written against (`e.key()`,
//! `e.modifiers()`, `e.value()`, `e.client_coordinates()`, …). The page
//! sends one frame per event (`page.js`, [`Raw::encode`]): the nodes that
//! listen for it, innermost first, and what it carries. Handlers run in
//! Rust in that order, so it bubbles as in the DOM until one calls
//! [`Event::stop_propagation`].
use std::cell::Cell;
use std::rc::Rc;
use std::str::FromStr;

pub use keyboard_types::{Code, Key, Location, Modifiers};

/// What every event carries, as the page sends it (pure codec).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Raw {
    /// The listening nodes it reached, innermost first.
    pub ids: Vec<u32>,
    pub name: String,
    pub key: String,
    pub code: String,
    pub location: u8,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
    pub repeat: bool,
    pub composing: bool,
    pub checked: bool,
    /// `MouseEvent.button`; `None`: not a mouse event.
    pub button: Option<u8>,
    pub buttons: u16,
    /// client, page, offset (element), screen: x, y each.
    pub points: [f64; 8],
    /// deltaX, deltaY, deltaZ, and `deltaMode`.
    pub delta: [f64; 3],
    pub delta_mode: u8,
    pub value: String,
    /// The target's scrollTop, scrollLeft, scrollWidth, scrollHeight, clientWidth, clientHeight.
    pub scroll: [f64; 6],
    pub pointer_id: i32,
    pub pointer_type: String,
    pub primary: bool,
    /// A form's fields (`submit`), name and value.
    pub values: Vec<(String, String)>,
}

struct W(Vec<u8>);

impl W {
    fn u8(&mut self, v: u8) {
        self.0.push(v)
    }
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes())
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes())
    }
    fn f64(&mut self, v: f64) {
        self.0.extend_from_slice(&v.to_le_bytes())
    }
    fn str(&mut self, s: &str) {
        self.u32(s.len() as u32);
        self.0.extend_from_slice(s.as_bytes())
    }
}

pub(crate) struct R<'a> {
    pub(crate) b: &'a [u8],
    pub(crate) p: usize,
}

impl R<'_> {
    pub(crate) fn take(&mut self, n: usize) -> Option<&[u8]> {
        let s = self.b.get(self.p..self.p.checked_add(n)?)?;
        self.p += n;
        Some(s)
    }
    pub(crate) fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }
    pub(crate) fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn f64(&mut self) -> Option<f64> {
        Some(f64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    pub(crate) fn str(&mut self) -> Option<String> {
        let n = self.u32()? as usize;
        String::from_utf8(self.take(n)?.to_vec()).ok()
    }
}

/// The frame kinds the page sends.
pub(crate) const EVENT: u8 = 1;
pub(crate) const EVAL: u8 = 2;

impl Raw {
    /// The frame the page sends (`page.js` writes the same).
    pub fn encode(&self) -> Vec<u8> {
        let mut w = W(vec![EVENT]);
        w.u16(self.ids.len() as u16);
        self.ids.iter().for_each(|i| w.u32(*i));
        w.str(&self.name);
        w.str(&self.key);
        w.str(&self.code);
        w.u8(self.location);
        let flags =
            [self.shift, self.ctrl, self.alt, self.meta, self.repeat, self.composing, self.checked, self.primary];
        w.u8(flags.iter().enumerate().fold(0, |f, (i, b)| f | (*b as u8) << i));
        w.u8(self.button.unwrap_or(255));
        w.u16(self.buttons);
        self.points.iter().chain(&self.delta).for_each(|v| w.f64(*v));
        w.u8(self.delta_mode);
        w.str(&self.value);
        self.scroll.iter().for_each(|v| w.f64(*v));
        w.u32(self.pointer_id as u32);
        w.str(&self.pointer_type);
        w.u16(self.values.len() as u16);
        for (k, v) in &self.values {
            w.str(k);
            w.str(v);
        }
        w.0
    }

    /// An event frame, or `None` if malformed.
    pub fn decode(b: &[u8]) -> Option<Raw> {
        let mut r = R { b, p: 0 };
        if r.u8()? != EVENT {
            return None;
        }
        let n = r.u16()?;
        let ids = (0..n).map(|_| r.u32()).collect::<Option<Vec<_>>>()?;
        let (name, key, code) = (r.str()?, r.str()?, r.str()?);
        let location = r.u8()?;
        let f = r.u8()?;
        let bit = |i: u8| f & (1 << i) != 0;
        let button = Some(r.u8()?).filter(|b| *b != 255);
        let buttons = r.u16()?;
        let mut points = [0.0; 8];
        for p in &mut points {
            *p = r.f64()?;
        }
        let mut delta = [0.0; 3];
        for d in &mut delta {
            *d = r.f64()?;
        }
        let delta_mode = r.u8()?;
        let value = r.str()?;
        let mut scroll = [0.0; 6];
        for s in &mut scroll {
            *s = r.f64()?;
        }
        let pointer_id = r.u32()? as i32;
        let pointer_type = r.str()?;
        let n = r.u16()?;
        let values = (0..n).map(|_| Some((r.str()?, r.str()?))).collect::<Option<Vec<_>>>()?;
        (r.p == b.len()).then_some(Raw {
            ids,
            name,
            key,
            code,
            location,
            shift: bit(0),
            ctrl: bit(1),
            alt: bit(2),
            meta: bit(3),
            repeat: bit(4),
            composing: bit(5),
            checked: bit(6),
            primary: bit(7),
            button,
            buttons,
            points,
            delta,
            delta_mode,
            value,
            scroll,
            pointer_id,
            pointer_type,
            values,
        })
    }

    fn modifiers(&self) -> Modifiers {
        let mut m = Modifiers::empty();
        for (on, b) in [
            (self.shift, Modifiers::SHIFT),
            (self.ctrl, Modifiers::CONTROL),
            (self.alt, Modifiers::ALT),
            (self.meta, Modifiers::META),
        ] {
            m.set(b, on);
        }
        m
    }
}

/// An event a handler gets: its data (`Deref`: `e.key()`), and what it
/// tells the page and the handlers above.
pub struct Event<T> {
    data: Rc<T>,
    flow: Rc<Flow>,
}

/// Where an event stands: stopped (no handler above runs), its default prevented.
#[doc(hidden)]
#[derive(Default)]
pub struct Flow {
    pub(crate) stopped: Cell<bool>,
    pub(crate) prevented: Cell<bool>,
}

impl<T> Clone for Event<T> {
    fn clone(&self) -> Self {
        Self { data: self.data.clone(), flow: self.flow.clone() }
    }
}

impl<T> std::ops::Deref for Event<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.data
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for Event<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.data.fmt(f)
    }
}

impl<T> Event<T> {
    pub fn new(data: T) -> Self {
        Self { data: Rc::new(data), flow: Rc::default() }
    }

    pub(crate) fn with_flow(data: T, flow: Rc<Flow>) -> Self {
        Self { data: Rc::new(data), flow }
    }

    pub fn data(&self) -> Rc<T> {
        self.data.clone()
    }

    /// The handlers on the nodes above don't get it.
    pub fn stop_propagation(&self) {
        self.flow.stopped.set(true)
    }

    /// Asked of the page too late to matter (it has moved on when the views
    /// hear it): a handled `submit` never navigates, a handled click on a
    /// link never follows it, and keys are kept from the page with
    /// `data-aio-prevent`. Recorded for [`default_prevented`](Self::default_prevented).
    pub fn prevent_default(&self) {
        self.flow.prevented.set(true)
    }

    pub fn default_prevented(&self) -> bool {
        self.flow.prevented.get()
    }

    /// The same event, its data mapped.
    pub fn map<U>(&self, f: impl FnOnce(&T) -> U) -> Event<U> {
        Event { data: Rc::new(f(&self.data)), flow: self.flow.clone() }
    }
}

/// A payload made from what the page sent.
pub trait FromRaw: Sized + 'static {
    fn from_raw(r: &Raw) -> Self;
}

/// Points in the window, the page, the element, the screen (CSS px).
pub mod geometry {
    macro_rules! point {
        ($($n:ident),*) => {$(
            #[derive(Clone, Copy, Debug, Default, PartialEq)]
            pub struct $n {
                pub x: f64,
                pub y: f64,
            }
            impl $n {
                pub fn new(x: f64, y: f64) -> Self {
                    Self { x, y }
                }
            }
        )*};
    }
    point!(ClientPoint, PagePoint, ElementPoint, ScreenPoint);

    /// How far a wheel turned, in its unit.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub enum WheelDelta {
        Pixels(Vector),
        Lines(Vector),
        Pages(Vector),
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq)]
    pub struct Vector {
        pub x: f64,
        pub y: f64,
        pub z: f64,
    }

    impl WheelDelta {
        /// The amounts, whatever the unit.
        pub fn strip_units(&self) -> Vector {
            match self {
                Self::Pixels(v) | Self::Lines(v) | Self::Pages(v) => *v,
            }
        }
    }

    /// A size in CSS px.
    #[derive(Clone, Copy, Debug, Default, PartialEq)]
    pub struct PixelsSize {
        pub width: f64,
        pub height: f64,
    }

    impl PixelsSize {
        pub fn new(width: f64, height: f64) -> Self {
            Self { width, height }
        }
    }

    /// A point in CSS px.
    pub type PixelsPoint = ClientPoint;

    /// An offset in CSS px (`MountedData::scroll`).
    #[derive(Clone, Copy, Debug, Default, PartialEq)]
    pub struct PixelsVector2D {
        pub x: f64,
        pub y: f64,
    }

    impl PixelsVector2D {
        pub fn new(x: f64, y: f64) -> Self {
            Self { x, y }
        }
    }

    /// An element's box (`MountedData::get_client_rect`): its corner and size.
    #[derive(Clone, Copy, Debug, Default, PartialEq)]
    pub struct Rect {
        pub origin: PixelsPoint,
        pub size: PixelsSize,
    }

    impl Rect {
        pub fn new(origin: PixelsPoint, size: PixelsSize) -> Self {
            Self { origin, size }
        }
        pub fn width(&self) -> f64 {
            self.size.width
        }
        pub fn height(&self) -> f64 {
            self.size.height
        }
        pub fn min_x(&self) -> f64 {
            self.origin.x
        }
        pub fn min_y(&self) -> f64 {
            self.origin.y
        }
        pub fn max_x(&self) -> f64 {
            self.origin.x + self.size.width
        }
        pub fn max_y(&self) -> f64 {
            self.origin.y + self.size.height
        }
    }
}

/// How a scroll moves.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScrollBehavior {
    #[default]
    Instant,
    Smooth,
}
use geometry::*;

/// A mouse button (`MouseEvent.button`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MouseButton {
    Primary,
    Auxiliary,
    Secondary,
    Fourth,
    Fifth,
    Unknown,
}

impl MouseButton {
    fn from_button(b: u8) -> Self {
        match b {
            0 => Self::Primary,
            1 => Self::Auxiliary,
            2 => Self::Secondary,
            3 => Self::Fourth,
            4 => Self::Fifth,
            _ => Self::Unknown,
        }
    }

    fn bit(self) -> u16 {
        match self {
            Self::Primary => 1,
            Self::Secondary => 2,
            Self::Auxiliary => 4,
            Self::Fourth => 8,
            Self::Fifth => 16,
            Self::Unknown => 0,
        }
    }
}

/// The buttons held (`MouseEvent.buttons`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MouseButtons(u16);

impl MouseButtons {
    pub fn contains(&self, b: MouseButton) -> bool {
        b.bit() != 0 && self.0 & b.bit() != 0
    }
    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct KeyboardData {
    key: String,
    code: String,
    location: u8,
    modifiers: Modifiers,
    repeat: bool,
    composing: bool,
}

impl KeyboardData {
    /// A key press as the views get one (`key_event`).
    pub fn new(key: Key, code: Code, location: Location, modifiers: Modifiers, repeat: bool) -> Self {
        let location = match location {
            Location::Standard => 0,
            Location::Left => 1,
            Location::Right => 2,
            Location::Numpad => 3,
        };
        Self { key: key.to_string(), code: code.to_string(), location, modifiers, repeat, composing: false }
    }
    pub fn key(&self) -> Key {
        Key::from_str(&self.key).unwrap_or(Key::Unidentified)
    }
    pub fn code(&self) -> Code {
        Code::from_str(&self.code).unwrap_or(Code::Unidentified)
    }
    pub fn location(&self) -> Location {
        match self.location {
            1 => Location::Left,
            2 => Location::Right,
            3 => Location::Numpad,
            _ => Location::Standard,
        }
    }
    pub fn modifiers(&self) -> Modifiers {
        self.modifiers
    }
    pub fn is_auto_repeating(&self) -> bool {
        self.repeat
    }
    pub fn is_composing(&self) -> bool {
        self.composing
    }
}

impl FromRaw for KeyboardData {
    fn from_raw(r: &Raw) -> Self {
        Self {
            key: r.key.clone(),
            code: r.code.clone(),
            location: r.location,
            modifiers: r.modifiers(),
            repeat: r.repeat,
            composing: r.composing,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MouseData {
    points: [f64; 8],
    button: Option<u8>,
    buttons: u16,
    modifiers: Modifiers,
}

impl MouseData {
    pub fn client_coordinates(&self) -> ClientPoint {
        ClientPoint::new(self.points[0], self.points[1])
    }
    pub fn page_coordinates(&self) -> PagePoint {
        PagePoint::new(self.points[2], self.points[3])
    }
    pub fn element_coordinates(&self) -> ElementPoint {
        ElementPoint::new(self.points[4], self.points[5])
    }
    pub fn screen_coordinates(&self) -> ScreenPoint {
        ScreenPoint::new(self.points[6], self.points[7])
    }
    /// The button that made this event.
    pub fn trigger_button(&self) -> Option<MouseButton> {
        self.button.map(MouseButton::from_button)
    }
    pub fn held_buttons(&self) -> MouseButtons {
        MouseButtons(self.buttons)
    }
    pub fn modifiers(&self) -> Modifiers {
        self.modifiers
    }
}

impl FromRaw for MouseData {
    fn from_raw(r: &Raw) -> Self {
        Self { points: r.points, button: r.button, buttons: r.buttons, modifiers: r.modifiers() }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PointerData {
    mouse: MouseData,
    pointer_id: i32,
    pointer_type: String,
    primary: bool,
}

impl std::ops::Deref for PointerData {
    type Target = MouseData;
    fn deref(&self) -> &MouseData {
        &self.mouse
    }
}

impl PointerData {
    pub fn pointer_id(&self) -> i32 {
        self.pointer_id
    }
    pub fn pointer_type(&self) -> String {
        self.pointer_type.clone()
    }
    pub fn is_primary(&self) -> bool {
        self.primary
    }
}

impl FromRaw for PointerData {
    fn from_raw(r: &Raw) -> Self {
        Self {
            mouse: MouseData::from_raw(r),
            pointer_id: r.pointer_id,
            pointer_type: r.pointer_type.clone(),
            primary: r.primary,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WheelData {
    mouse: MouseData,
    delta: [f64; 3],
    mode: u8,
}

impl std::ops::Deref for WheelData {
    type Target = MouseData;
    fn deref(&self) -> &MouseData {
        &self.mouse
    }
}

impl WheelData {
    pub fn delta(&self) -> WheelDelta {
        let v = Vector { x: self.delta[0], y: self.delta[1], z: self.delta[2] };
        match self.mode {
            1 => WheelDelta::Lines(v),
            2 => WheelDelta::Pages(v),
            _ => WheelDelta::Pixels(v),
        }
    }
}

impl FromRaw for WheelData {
    fn from_raw(r: &Raw) -> Self {
        Self { mouse: MouseData::from_raw(r), delta: r.delta, mode: r.delta_mode }
    }
}

/// A field's or a form's event (`input`, `change`, `submit`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FormData {
    value: String,
    checked: bool,
    values: Vec<(String, String)>,
}

impl FormData {
    /// The field's value (a checkbox's: its `value`; see [`checked`](Self::checked)).
    pub fn value(&self) -> String {
        self.value.clone()
    }
    pub fn checked(&self) -> bool {
        self.checked
    }
    /// A submitted form's fields, in order (a name may come more than once).
    pub fn values(&self) -> Vec<(String, String)> {
        self.values.clone()
    }
}

impl FromRaw for FormData {
    fn from_raw(r: &Raw) -> Self {
        Self { value: r.value.clone(), checked: r.checked, values: r.values.clone() }
    }
}

/// How far the scrolled element is.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScrollData {
    s: [f64; 6],
}

impl ScrollData {
    pub fn scroll_top(&self) -> f64 {
        self.s[0]
    }
    pub fn scroll_left(&self) -> f64 {
        self.s[1]
    }
    pub fn scroll_width(&self) -> f64 {
        self.s[2]
    }
    pub fn scroll_height(&self) -> f64 {
        self.s[3]
    }
    pub fn client_width(&self) -> f64 {
        self.s[4]
    }
    pub fn client_height(&self) -> f64 {
        self.s[5]
    }
}

impl FromRaw for ScrollData {
    fn from_raw(r: &Raw) -> Self {
        Self { s: r.scroll }
    }
}

/// An element's new size (`onresize`: the page watches the element).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ResizeData {
    size: geometry::PixelsSize,
}

impl ResizeData {
    /// Its border box.
    pub fn get_border_box_size(&self) -> Result<geometry::PixelsSize, String> {
        Ok(self.size)
    }

    /// Its content box (the border box here: the kit's boxes have none).
    pub fn get_content_box_size(&self) -> Result<geometry::PixelsSize, String> {
        Ok(self.size)
    }
}

impl FromRaw for ResizeData {
    /// The page puts the box in the event's client point.
    fn from_raw(r: &Raw) -> Self {
        Self { size: geometry::PixelsSize::new(r.points[0], r.points[1]) }
    }
}

/// Whether an element came into view or left it (`onvisible`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VisibleData {
    on: bool,
}

impl VisibleData {
    pub fn is_intersecting(&self) -> Result<bool, String> {
        Ok(self.on)
    }
}

impl FromRaw for VisibleData {
    /// The page puts 1 (in view) or 0 in the event's client x.
    fn from_raw(r: &Raw) -> Self {
        Self { on: r.points[0] != 0.0 }
    }
}

/// An event that carries nothing the views read (focus, clipboard, media…).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlainData {}

impl FromRaw for PlainData {
    fn from_raw(_: &Raw) -> Self {
        Self {}
    }
}

impl FromRaw for () {
    fn from_raw(_: &Raw) -> Self {}
}

pub type KeyboardEvent = Event<KeyboardData>;
pub type MouseEvent = Event<MouseData>;
pub type PointerEvent = Event<PointerData>;
pub type WheelEvent = Event<WheelData>;
pub type FormEvent = Event<FormData>;
pub type ScrollEvent = Event<ScrollData>;
pub type DragEvent = Event<MouseData>;
pub type DragData = MouseData;
pub type FocusData = PlainData;
pub type FocusEvent = Event<PlainData>;
pub type ClipboardEvent = Event<PlainData>;
pub type CompositionEvent = Event<PlainData>;
pub type AnimationEvent = Event<PlainData>;
pub type TransitionEvent = Event<PlainData>;
pub type ToggleEvent = Event<PlainData>;
pub type MediaEvent = Event<PlainData>;
pub type ImageEvent = Event<PlainData>;
pub type SelectionEvent = Event<PlainData>;
pub type TouchEvent = Event<PointerData>;
pub type MountedEvent = Event<crate::dom::MountedData>;
pub type ResizeEvent = Event<ResizeData>;
pub type VisibleEvent = Event<VisibleData>;

/// A key press as the views get one, for a key heard elsewhere (a guest
/// page, the host) and handed to the views' handlers.
pub fn key_event(key: Key, modifiers: Modifiers, repeat: bool) -> KeyboardEvent {
    Event::new(KeyboardData::new(key, Code::Unidentified, Location::Standard, modifiers, repeat))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_event_frame_reads_back_as_written() {
        let e = Raw {
            ids: vec![42, 7],
            name: "keydown".into(),
            key: "ArrowDown".into(),
            code: "ArrowDown".into(),
            value: "é".into(),
            shift: true,
            checked: true,
            button: Some(2),
            points: [1.5, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0],
            values: vec![("name".into(), "Ann".into())],
            ..Raw::default()
        };
        assert_eq!(Raw::decode(&e.encode()), Some(e));
        assert_eq!(Raw::decode(&[1, 2]), None, "cut short");
        assert_eq!(Raw::decode(&[9]), None, "not an event");
    }

    #[test]
    fn payloads_read_as_the_dom_says() {
        let r = Raw { key: "a".into(), code: "KeyA".into(), ctrl: true, button: Some(0), buttons: 1, ..Raw::default() };
        let k = KeyboardData::from_raw(&r);
        assert_eq!((k.key(), k.code(), k.modifiers()), (Key::Character("a".into()), Code::KeyA, Modifiers::CONTROL));
        let m = MouseData::from_raw(&r);
        assert_eq!(m.trigger_button(), Some(MouseButton::Primary));
        assert!(m.held_buttons().contains(MouseButton::Primary) && !m.held_buttons().contains(MouseButton::Secondary));
        let e = key_event(Key::ArrowDown, Modifiers::SHIFT, true);
        assert_eq!((e.key(), e.modifiers(), e.is_auto_repeating()), (Key::ArrowDown, Modifiers::SHIFT, true));
    }
}
