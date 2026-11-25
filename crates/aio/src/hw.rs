//! USB devices from the app's own code (cells' effects, async methods,
//! handlers' tasks): a hardware key, a lab instrument, a board to flash.
//! List them, watch them come and go, open one, claim an interface, move
//! bytes (bulk, interrupt, control). The guide: `docs/devices/usb.md`.
//!
//! | Where | Devices come from |
//! |---|---|
//! | Linux, Windows, macOS | the OS's USB stack (usbfs, WinUSB, IOKit), through nusb: no C library |
//! | Android | the USB host API: the user is asked once per device when it is opened |
//! | the web | nowhere: `Unsupported` (the page's own `navigator.usb` is the browser's) |
//! | a test | fake devices only ([`crate::testing::usb`]), never the machine's |
//!
//! The app sees only the devices its `[capabilities] usb` names; with none
//! held (not an app: a tool, a test that never booted one), any.
//!
//! Every call is `async`, runs on aio's own runtime (any executor may await
//! it) and can be dropped at any await: a dropped read leaves its transfer
//! queued and the next read on that endpoint gets its bytes; a dropped
//! write is cancelled. Errors say what happened by their kind:
//! - `NotFound`: no device matches (plugged in?), or no such interface;
//! - `PermissionDenied`: not in `[capabilities] usb`; the OS refused (on
//!   Linux the message names the udev rule that allows it); the user said
//!   no (Android);
//! - `NotConnected`: the device was unplugged: watch for it, open it again;
//! - `TimedOut`: no answer in time (a read's transfer stays queued);
//! - `BrokenPipe`: the device refused the request (a stall);
//! - `ResourceBusy`: another program (or a kernel driver the OS won't let
//!   go of) holds the interface;
//! - `InvalidInput`: no such endpoint on the interface, or one of the
//!   other direction;
//! - `Unsupported`: no USB here (the web), or an endpoint type aio doesn't
//!   move (isochronous).
use std::fmt;
use std::io;
use std::time::Duration;

#[cfg(any(target_os = "android", all(test, not(target_arch = "wasm32"))))]
mod android;
#[cfg(not(target_arch = "wasm32"))]
mod os;

/// Which devices: all, a vendor's, one product, one serial number.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filter {
    vendor: Option<u16>,
    product: Option<u16>,
    serial: Option<String>,
}

impl Filter {
    /// Any device.
    pub fn any() -> Self {
        Self::default()
    }

    /// Vendor `vendor`'s devices (its id as `lsusb` shows it: `0x2c97`).
    pub fn vendor(vendor: u16) -> Self {
        Filter { vendor: Some(vendor), ..Self::default() }
    }

    /// Product `product` of vendor `vendor`.
    pub fn device(vendor: u16, product: u16) -> Self {
        Filter { vendor: Some(vendor), product: Some(product), serial: None }
    }

    /// Only the one with serial number `serial`.
    pub fn serial(self, serial: impl Into<String>) -> Self {
        Filter { serial: Some(serial.into()), ..self }
    }

    /// Does `d` match? Pure.
    pub fn matches(&self, d: &DeviceInfo) -> bool {
        self.vendor.is_none_or(|v| v == d.vendor)
            && self.product.is_none_or(|p| p == d.product)
            && self.serial.as_ref().is_none_or(|s| d.serial.as_ref() == Some(s))
    }
}

impl fmt::Display for Filter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.vendor, self.product) {
            (Some(v), Some(p)) => write!(f, "{v:04x}:{p:04x}")?,
            (Some(v), None) => write!(f, "{v:04x}:*")?,
            _ => f.write_str("any device")?,
        }
        match &self.serial {
            Some(s) => write!(f, " (serial {s})"),
            None => Ok(()),
        }
    }
}

/// A device plugged in: who made it, what it is, its interfaces. Its
/// strings are `None` where the device has none, or (Android) until the
/// user has let the app open it.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct DeviceInfo {
    /// This plug of it: another plug is another id.
    pub id: Id,
    pub vendor: u16,
    pub product: u16,
    pub manufacturer: Option<String>,
    pub product_name: Option<String>,
    pub serial: Option<String>,
    /// The device's class (`0`: each interface says its own).
    pub class: u8,
    pub interfaces: Vec<InterfaceInfo>,
}

impl DeviceInfo {
    /// Open it (Android asks the user first, once per device).
    pub async fn open(&self) -> io::Result<Device> {
        held(self.vendor, self.product)?;
        let (bus, info) = (bus()?, self.clone());
        on_rt(async move { bus.open(info).await }).await
    }
}

impl fmt::Display for DeviceInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04x}:{:04x}", self.vendor, self.product)?;
        if let Some(n) = &self.product_name {
            write!(f, " {n}")?;
        }
        Ok(())
    }
}

/// One interface of a device, as its descriptor says.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct InterfaceInfo {
    pub number: u8,
    /// `0x03` HID, `0x0a` CDC data, `0xff` the vendor's own, …
    pub class: u8,
    pub subclass: u8,
    pub protocol: u8,
}

/// A device's plug: its devices' ids differ while they are plugged in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Id(pub(crate) Ids);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Ids {
    #[cfg(not(any(target_os = "android", target_arch = "wasm32")))]
    Os(nusb::DeviceId),
    /// Android's device id; a fake device's.
    #[cfg_attr(not(any(target_os = "android", feature = "testing")), allow(dead_code))]
    N(u64),
}

/// A device came or went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Plugged(DeviceInfo),
    Unplugged(DeviceInfo),
}

/// The devices the app may see, plugged in now.
pub async fn list() -> io::Result<Vec<DeviceInfo>> {
    Ok(visible(all().await?))
}

/// Every device plugged in, those `[capabilities] usb` leaves out too.
async fn all() -> io::Result<Vec<DeviceInfo>> {
    let bus = bus()?;
    on_rt(async move { bus.list().await }).await
}

/// The devices of `all` the app may see. Pure but for the capability.
fn visible(all: Vec<DeviceInfo>) -> Vec<DeviceInfo> {
    all.into_iter().filter(|d| held(d.vendor, d.product).is_ok()).collect()
}

/// The first device plugged in that `filter` matches, opened (Android asks
/// the user first, once per device).
pub async fn open(filter: &Filter) -> io::Result<Device> {
    if let (Some(v), Some(p)) = (filter.vendor, filter.product) {
        held(v, p)?;
    }
    let matching: Vec<DeviceInfo> = all().await?.into_iter().filter(|d| filter.matches(d)).collect();
    match (visible(matching.clone()).into_iter().next(), matching.first()) {
        (Some(d), _) => d.open().await,
        // Plugged in, but left out: say the entry that lets the app see it.
        (None, Some(d)) => Err(refused(d.vendor, d.product)),
        (None, None) => Err(err(io::ErrorKind::NotFound, format!("no USB device {filter} is plugged in"))),
    }
}

/// Every device the app may see as it comes and goes: those plugged in
/// now first, each a [`Event::Plugged`]. Dropping it stops watching.
pub fn watch() -> io::Result<Watch> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let bus = bus()?;
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let task = crate::rt().spawn(watching(bus, tx));
        Ok(Watch { rx, task: task.abort_handle() })
    }
    #[cfg(target_arch = "wasm32")]
    Err(web())
}

/// Devices coming and going ([`watch`]).
pub struct Watch {
    #[cfg(not(target_arch = "wasm32"))]
    rx: tokio::sync::mpsc::UnboundedReceiver<io::Result<Event>>,
    #[cfg(not(target_arch = "wasm32"))]
    task: tokio::task::AbortHandle,
}

impl Watch {
    /// The next device to come or go. Cancel-safe: dropped, no event is lost.
    pub async fn next(&mut self) -> io::Result<Event> {
        #[cfg(not(target_arch = "wasm32"))]
        return self.rx.recv().await.unwrap_or_else(|| Err(io::Error::other("the USB watch ended")));
        #[cfg(target_arch = "wasm32")]
        Err(web())
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        self.task.abort();
    }
}

/// The events that turn `known` into `now` (by id), unplugs first; `known`
/// becomes `now`. Pure.
fn diff(known: &mut Vec<DeviceInfo>, now: Vec<DeviceInfo>) -> Vec<Event> {
    let gone = known.iter().filter(|k| !now.iter().any(|n| n.id == k.id)).cloned().map(Event::Unplugged);
    let came = now.iter().filter(|n| !known.iter().any(|k| k.id == n.id)).cloned().map(Event::Plugged);
    let events = gone.chain(came).collect();
    *known = now;
    events
}

#[cfg(not(target_arch = "wasm32"))]
async fn watching(bus: Bus, tx: tokio::sync::mpsc::UnboundedSender<io::Result<Event>>) {
    // Changes are heard from before the first list: none falls between.
    let mut changes = match bus.changes() {
        Ok(c) => c,
        Err(e) => return drop(tx.send(Err(e))),
    };
    let mut known = vec![];
    loop {
        let sent = match bus.list().await {
            Ok(now) => diff(&mut known, visible(now)).into_iter().all(|e| tx.send(Ok(e)).is_ok()),
            Err(e) => tx.send(Err(e)).is_ok(),
        };
        if !sent || !changes.next().await {
            return;
        }
    }
}

/// An open device.
#[derive(Clone)]
pub struct Device {
    info: DeviceInfo,
    dev: Dev,
}

impl Device {
    pub fn info(&self) -> &DeviceInfo {
        &self.info
    }

    /// Claim interface `number` for this app alone (on Linux, a kernel
    /// driver holding it is let go of first, and takes it back after).
    pub async fn claim(&self, number: u8) -> io::Result<Interface> {
        let dev = self.dev.clone();
        let iface = on_rt(async move { dev.claim(number).await }).await?;
        Ok(Interface { iface, number })
    }
}

impl fmt::Debug for Device {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Device").field("info", &self.info).finish()
    }
}

/// A claimed interface: its endpoints' transfers, and control transfers
/// (which every OS takes through a claimed interface). Clones share it; the
/// last one dropped lets it go.
#[derive(Clone)]
pub struct Interface {
    iface: Iface,
    number: u8,
}

impl fmt::Debug for Interface {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Interface").field("number", &self.number).finish()
    }
}

/// A control transfer's setup (USB 2.0, 9.3): its type, its recipient,
/// `bRequest`, `wValue` and `wIndex`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Control {
    pub kind: ControlKind,
    pub recipient: Recipient,
    pub request: u8,
    pub value: u16,
    pub index: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlKind {
    Standard,
    Class,
    Vendor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recipient {
    Device,
    Interface,
    Endpoint,
    Other,
}

impl Interface {
    pub fn number(&self) -> u8 {
        self.number
    }

    /// One transfer in from bulk or interrupt endpoint `endpoint` (`0x81`):
    /// up to `max` bytes (rounded up to whole packets), what the device
    /// sent. `TimedOut` after `timeout`: the transfer stays queued, and
    /// the next read on the endpoint gets its bytes.
    pub async fn read(&self, endpoint: u8, max: usize, timeout: Duration) -> io::Result<Vec<u8>> {
        let i = self.iface.clone();
        on_rt(async move { i.read(endpoint, max, timeout).await }).await
    }

    /// `data` out to bulk or interrupt endpoint `endpoint` (`0x01`), whole.
    /// `TimedOut` after `timeout`, cancelled (part of it may have gone).
    pub async fn write(&self, endpoint: u8, data: &[u8], timeout: Duration) -> io::Result<()> {
        let (i, data) = (self.iface.clone(), data.to_vec());
        on_rt(async move { i.write(endpoint, data, timeout).await }).await
    }

    /// A control transfer in: up to `len` bytes.
    pub async fn control_in(&self, setup: Control, len: u16, timeout: Duration) -> io::Result<Vec<u8>> {
        let i = self.iface.clone();
        on_rt(async move { i.control_in(setup, len, timeout).await }).await
    }

    /// A control transfer out, with `data` (may be empty).
    pub async fn control_out(&self, setup: Control, data: &[u8], timeout: Duration) -> io::Result<()> {
        let (i, data) = (self.iface.clone(), data.to_vec());
        on_rt(async move { i.control_out(setup, data, timeout).await }).await
    }
}

/// The devices' source: the machine's, or the test world's fakes.
#[cfg(not(target_arch = "wasm32"))]
enum Bus {
    Os,
    #[cfg(feature = "testing")]
    Fake(std::sync::Arc<crate::testing::usb::Bus>),
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone)]
enum Dev {
    Os(nusb::Device),
    #[cfg(feature = "testing")]
    Fake(crate::testing::usb::Opened),
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone)]
enum Iface {
    Os(os::Iface),
    #[cfg(feature = "testing")]
    Fake(crate::testing::usb::Claimed),
}

/// The web's bus, device and interface: each call says there is no USB.
#[cfg(target_arch = "wasm32")]
#[derive(Clone)]
struct Web;

#[cfg(target_arch = "wasm32")]
type Bus = Web;
#[cfg(target_arch = "wasm32")]
type Dev = Web;
#[cfg(target_arch = "wasm32")]
type Iface = Web;

#[cfg(target_arch = "wasm32")]
impl Web {
    async fn list(&self) -> io::Result<Vec<DeviceInfo>> {
        Err(web())
    }
    async fn open(&self, _: DeviceInfo) -> io::Result<Device> {
        Err(web())
    }
    async fn claim(self, _: u8) -> io::Result<Iface> {
        Err(web())
    }
    async fn read(self, _: u8, _: usize, _: Duration) -> io::Result<Vec<u8>> {
        Err(web())
    }
    async fn write(self, _: u8, _: Vec<u8>, _: Duration) -> io::Result<()> {
        Err(web())
    }
    async fn control_in(self, _: Control, _: u16, _: Duration) -> io::Result<Vec<u8>> {
        Err(web())
    }
    async fn control_out(self, _: Control, _: Vec<u8>, _: Duration) -> io::Result<()> {
        Err(web())
    }
}

#[cfg(target_arch = "wasm32")]
fn bus() -> io::Result<Bus> {
    Ok(Web)
}

#[cfg(target_arch = "wasm32")]
async fn on_rt<T>(f: impl Future<Output = io::Result<T>>) -> io::Result<T> {
    f.await
}

#[cfg(target_arch = "wasm32")]
fn web() -> io::Error {
    err(
        io::ErrorKind::Unsupported,
        "the web has no USB from the app's code: a page asks the browser itself (navigator.usb, Chromium, after \
         a click); the desktop and Android builds have aio::hw",
    )
}

/// What tells a watch to look again.
#[cfg(not(target_arch = "wasm32"))]
enum Changes {
    #[cfg(not(target_os = "android"))]
    Os(nusb::hotplug::HotplugWatch),
    #[cfg_attr(not(any(target_os = "android", feature = "testing")), allow(dead_code))]
    Signal(tokio::sync::watch::Receiver<u64>),
}

#[cfg(not(target_arch = "wasm32"))]
impl Changes {
    /// Wait for a change; `false`: no more will come.
    async fn next(&mut self) -> bool {
        match self {
            #[cfg(not(target_os = "android"))]
            Changes::Os(w) => futures_util::StreamExt::next(w).await.is_some(),
            Changes::Signal(rx) => rx.changed().await.is_ok(),
        }
    }
}

/// The machine's devices, or in the test world its fakes: a test never
/// reaches real hardware.
#[cfg(not(target_arch = "wasm32"))]
fn bus() -> io::Result<Bus> {
    if !crate::paths::in_sandbox() {
        return Ok(Bus::Os);
    }
    #[cfg(feature = "testing")]
    return Ok(Bus::Fake(crate::testing::usb::bus()));
    #[cfg(not(feature = "testing"))]
    Err(err(
        io::ErrorKind::Unsupported,
        "a test's world has no USB devices but fakes: aio::testing::usb (feature testing) plugs one in",
    ))
}

#[cfg(not(target_arch = "wasm32"))]
impl Bus {
    /// Every device plugged in.
    async fn list(&self) -> io::Result<Vec<DeviceInfo>> {
        match self {
            Bus::Os => os::list().await,
            #[cfg(feature = "testing")]
            Bus::Fake(b) => Ok(b.list()),
        }
    }

    fn changes(&self) -> io::Result<Changes> {
        match self {
            Bus::Os => os::changes(),
            #[cfg(feature = "testing")]
            Bus::Fake(b) => Ok(Changes::Signal(b.changes())),
        }
    }

    async fn open(&self, info: DeviceInfo) -> io::Result<Device> {
        let dev = match self {
            Bus::Os => Dev::Os(os::open(&info).await?),
            #[cfg(feature = "testing")]
            Bus::Fake(b) => Dev::Fake(b.open(&info)?),
        };
        Ok(Device { info, dev })
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Dev {
    async fn claim(self, number: u8) -> io::Result<Iface> {
        match self {
            Dev::Os(d) => os::claim(d, number).await.map(Iface::Os),
            #[cfg(feature = "testing")]
            Dev::Fake(d) => d.claim(number).map(Iface::Fake),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Iface {
    async fn read(self, endpoint: u8, max: usize, timeout: Duration) -> io::Result<Vec<u8>> {
        match self {
            Iface::Os(i) => i.read(endpoint, max, timeout).await,
            #[cfg(feature = "testing")]
            Iface::Fake(i) => i.read(endpoint, max, timeout).await,
        }
    }

    async fn write(self, endpoint: u8, data: Vec<u8>, timeout: Duration) -> io::Result<()> {
        match self {
            Iface::Os(i) => i.write(endpoint, data, timeout).await,
            #[cfg(feature = "testing")]
            Iface::Fake(i) => i.write(endpoint, data, timeout).await,
        }
    }

    async fn control_in(self, setup: Control, len: u16, timeout: Duration) -> io::Result<Vec<u8>> {
        match self {
            Iface::Os(i) => i.control_in(setup, len, timeout).await,
            #[cfg(feature = "testing")]
            Iface::Fake(i) => i.control_in(setup, len, timeout).await,
        }
    }

    async fn control_out(self, setup: Control, data: Vec<u8>, timeout: Duration) -> io::Result<()> {
        match self {
            Iface::Os(i) => i.control_out(setup, data, timeout).await,
            #[cfg(feature = "testing")]
            Iface::Fake(i) => i.control_out(setup, data, timeout).await,
        }
    }
}

/// `f` on aio's runtime, cancelled (its task aborted) when dropped: any
/// executor may await it.
#[cfg(not(target_arch = "wasm32"))]
async fn on_rt<T: Send + 'static>(f: impl Future<Output = io::Result<T>> + Send + 'static) -> io::Result<T> {
    struct Aborting<T>(tokio::task::JoinHandle<T>);
    impl<T> Drop for Aborting<T> {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let mut task = Aborting(crate::rt().spawn(f));
    (&mut task.0).await.map_err(io::Error::other)?
}

/// `Err` (`PermissionDenied`) when `[capabilities] usb` leaves `vendor:product`
/// out (the web has no devices to hold).
#[cfg_attr(target_arch = "wasm32", allow(unused_variables))]
fn held(vendor: u16, product: u16) -> io::Result<()> {
    #[cfg(not(target_arch = "wasm32"))]
    if !crate::capabilities::usb_allows(vendor, product) {
        return Err(refused(vendor, product));
    }
    Ok(())
}

/// Why device `vendor:product` is not opened, and the line that allows it. Pure.
fn refused(vendor: u16, product: u16) -> io::Error {
    err(
        io::ErrorKind::PermissionDenied,
        format!(
            "USB {vendor:04x}:{product:04x} is not in this app's [capabilities] usb, so aio does not open it. If it \
             should, add \"{vendor:04x}:{product:04x}\" (or the vendor's \"{vendor:04x}\") to usb in aio.toml"
        ),
    )
}

fn err(kind: io::ErrorKind, why: impl Into<String>) -> io::Error {
    io::Error::new(kind, why.into())
}

/// The packet size of endpoint `address` among `endpoints` (address,
/// packet size) of interface `number`, if it is one of direction `input`;
/// else why not. Pure.
#[cfg_attr(not(any(feature = "testing", not(target_arch = "wasm32"))), allow(dead_code))]
pub(crate) fn endpoint(endpoints: &[(u8, usize)], number: u8, address: u8, input: bool) -> io::Result<usize> {
    let dir = |a: u8| if a & 0x80 != 0 { "in" } else { "out" };
    if (address & 0x80 != 0) != input {
        let (want, use_) = if input { ("in", "write to") } else { ("out", "read from") };
        return Err(err(
            io::ErrorKind::InvalidInput,
            format!("endpoint {address:#04x} is not an {want} endpoint ({} by its address): {use_} it", dir(address)),
        ));
    }
    endpoints.iter().find(|(a, _)| *a == address).map(|(_, size)| *size).ok_or_else(|| {
        let has: Vec<String> = endpoints.iter().map(|(a, _)| format!("{a:#04x} {}", dir(*a))).collect();
        err(
            io::ErrorKind::InvalidInput,
            format!("interface {number} has no endpoint {address:#04x} (it has: {})", has.join(", ")),
        )
    })
}

/// `max` bytes as whole packets of `size` (at least one). Pure.
#[cfg_attr(not(any(feature = "testing", not(target_arch = "wasm32"))), allow(dead_code))]
pub(crate) fn packets(max: usize, size: usize) -> usize {
    max.div_ceil(size.max(1)).max(1) * size.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(id: u64, vendor: u16, product: u16, serial: Option<&str>) -> DeviceInfo {
        DeviceInfo {
            id: Id(Ids::N(id)),
            vendor,
            product,
            manufacturer: None,
            product_name: None,
            serial: serial.map(Into::into),
            class: 0,
            interfaces: vec![],
        }
    }

    #[test]
    fn a_filter_matches_by_vendor_product_and_serial_and_says_which() {
        let d = dev(1, 0x2c97, 0x0001, Some("A1"));
        assert!(Filter::any().matches(&d) && Filter::vendor(0x2c97).matches(&d));
        assert!(Filter::device(0x2c97, 1).matches(&d) && !Filter::device(0x2c97, 2).matches(&d));
        assert!(Filter::vendor(0x2c97).serial("A1").matches(&d) && !Filter::any().serial("B").matches(&d));
        assert!(!Filter::any().serial("A1").matches(&dev(2, 1, 1, None)), "no serial: not that one");
        assert_eq!(Filter::device(0x2c97, 1).serial("A1").to_string(), "2c97:0001 (serial A1)");
        assert_eq!(Filter::vendor(0x1209).to_string(), "1209:*");
        assert_eq!(Filter::any().to_string(), "any device");
    }

    #[test]
    fn a_watch_hears_unplugs_then_plugs_by_id() {
        let (a, b, c) = (dev(1, 1, 1, None), dev(2, 1, 1, None), dev(3, 2, 2, None));
        let mut known = vec![];
        assert_eq!(
            diff(&mut known, vec![a.clone(), b.clone()]),
            [Event::Plugged(a.clone()), Event::Plugged(b.clone())]
        );
        assert_eq!(diff(&mut known, vec![a.clone(), b.clone()]), []);
        assert_eq!(diff(&mut known, vec![c.clone(), a.clone()]), [Event::Unplugged(b), Event::Plugged(c.clone())]);
        assert_eq!(known, [c, a]);
    }

    #[test]
    fn an_endpoint_is_named_with_its_direction_and_the_ones_there_are() {
        let eps = [(0x81, 64), (0x01, 64)];
        assert_eq!(endpoint(&eps, 0, 0x81, true).unwrap(), 64);
        assert_eq!(endpoint(&eps, 0, 0x01, false).unwrap(), 64);
        let e = endpoint(&eps, 0, 0x01, true).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(e.to_string(), "endpoint 0x01 is not an in endpoint (out by its address): write to it");
        let e = endpoint(&eps, 2, 0x82, true).unwrap_err();
        assert_eq!(e.to_string(), "interface 2 has no endpoint 0x82 (it has: 0x81 in, 0x01 out)");
    }

    #[test]
    fn a_read_asks_whole_packets() {
        assert_eq!((packets(1, 64), packets(64, 64), packets(65, 64), packets(0, 64)), (64, 64, 128, 64));
        assert_eq!(packets(10, 0), 10, "a size of 0 (a broken descriptor) is no division by zero");
    }

    #[test]
    fn a_device_not_in_the_capability_is_refused_naming_the_fix() {
        let e = refused(0x2c97, 1);
        assert_eq!(e.kind(), io::ErrorKind::PermissionDenied);
        assert!(e.to_string().contains("add \"2c97:0001\" (or the vendor's \"2c97\") to usb in aio.toml"), "{e}");
    }
}
