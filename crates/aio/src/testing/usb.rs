//! Fake USB devices (aio's tests of an app's hardware path): declare one's
//! descriptors, script its answers, plug it in and out at will. The app's
//! code under test uses [`crate::hw`] unchanged: in the test world (a
//! plugged device holds one) `aio::hw` sees these devices only, never the
//! machine's.
//!
//! ```
//! use aio::hw::{self, Filter, InterfaceInfo};
//! use aio::testing::usb::{self, Reply, Transfer};
//! use std::time::Duration;
//!
//! let hid = InterfaceInfo { number: 0, class: 0x03, subclass: 0, protocol: 0 };
//! let key = usb::plug(
//!     usb::Device::new(0x1209, 0x0001)
//!         .strings("Acme", "Key", "A1")
//!         .interface(hid, &[(0x81, 64), (0x01, 64)])
//!         // Each packet written comes back on 0x81, reversed.
//!         .respond(|t, io| match t {
//!             Transfer::Write { data, .. } => {
//!                 io.send(0x81, data.iter().rev().copied().collect::<Vec<u8>>());
//!                 Reply::Done
//!             }
//!             _ => Reply::Wait,
//!         }),
//! );
//! aio::rt().block_on(async {
//!     let dev = hw::open(&Filter::device(0x1209, 0x0001)).await.unwrap();
//!     let i = dev.claim(0).await.unwrap();
//!     i.write(0x01, b"ping", Duration::from_secs(1)).await.unwrap();
//!     assert_eq!(i.read(0x81, 64, Duration::from_secs(1)).await.unwrap(), b"gnip");
//! });
//! assert_eq!(key.transfers()[0], Transfer::Write { endpoint: 0x01, data: b"ping".to_vec() });
//! ```
use crate::hw::{Control, DeviceInfo, Id, Ids, InterfaceInfo, endpoint, packets};
use std::collections::{HashMap, VecDeque};
use std::io;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::Duration;

/// A fake device's script: it answers each transfer the app makes that
/// its queues don't (`&mut Io`: what it sends back on its in endpoints). It
/// runs with the device held: it must not call its [`Plugged`].
type Respond = Box<dyn FnMut(&Transfer, &mut Io) -> Reply + Send>;

/// A fake device, as declared: its ids, strings, interfaces and script.
pub struct Device {
    info: DeviceInfo,
    endpoints: Vec<(u8, Vec<(u8, usize)>)>,
    respond: Option<Respond>,
}

impl Device {
    /// Vendor `vendor`'s product `product`, with no strings or interfaces.
    pub fn new(vendor: u16, product: u16) -> Self {
        let info = DeviceInfo {
            id: Id(Ids::N(0)),
            vendor,
            product,
            manufacturer: None,
            product_name: None,
            serial: None,
            class: 0,
            interfaces: vec![],
        };
        Device { info, endpoints: vec![], respond: None }
    }

    /// Its manufacturer, product and serial number strings.
    pub fn strings(mut self, manufacturer: &str, product: &str, serial: &str) -> Self {
        self.info.manufacturer = Some(manufacturer.into());
        self.info.product_name = Some(product.into());
        self.info.serial = Some(serial.into());
        self
    }

    /// An interface, with its bulk or interrupt endpoints: (address, packet
    /// size), `0x81` in, `0x01` out.
    pub fn interface(mut self, info: InterfaceInfo, endpoints: &[(u8, usize)]) -> Self {
        self.info.interfaces.push(info);
        self.endpoints.push((info.number, endpoints.to_vec()));
        self
    }

    /// How it answers. Unscripted, a write is taken, a read gets what was
    /// [`Plugged::send`] (else waits), and a control transfer stalls.
    pub fn respond(mut self, f: impl FnMut(&Transfer, &mut Io) -> Reply + Send + 'static) -> Self {
        self.respond = Some(Box::new(f));
        self
    }
}

/// A transfer the app made, as the device saw it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Transfer {
    Write { endpoint: u8, data: Vec<u8> },
    /// A read the device had nothing queued for: up to `max` bytes.
    Read { endpoint: u8, max: usize },
    ControlIn { setup: Control, len: u16 },
    ControlOut { setup: Control, data: Vec<u8> },
}

/// A device's answer to a [`Transfer`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    /// Done, with no data: a write or control out taken, a read of nothing.
    Done,
    /// A read's or control in's bytes.
    Data(Vec<u8>),
    /// Refused: the app gets `BrokenPipe`.
    Stall,
    /// No answer: a read waits for bytes sent to its endpoint, anything
    /// else (and a read sent none) times out.
    Wait,
}

/// What a device sends back on its in endpoints, queued per endpoint for
/// the app's reads.
#[derive(Default)]
pub struct Io(HashMap<u8, VecDeque<Vec<u8>>>);

impl Io {
    /// `data`, one transfer's worth, for the next read of `endpoint` (`0x81`).
    pub fn send(&mut self, endpoint: u8, data: impl Into<Vec<u8>>) {
        self.0.entry(endpoint).or_default().push_back(data.into());
    }
}

/// A plugged-in fake device: unplugged when dropped (or by [`Plugged::unplug`]).
pub struct Plugged {
    fake: Arc<Fake>,
    _world: crate::paths::Sandbox,
}

/// Plug `device` in: a watch hears it, `list` and `open` find it.
pub fn plug(device: Device) -> Plugged {
    let world = crate::paths::sandbox().expect("a test sandbox");
    let b = bus();
    let id = {
        let mut n = lock(&b.next);
        *n += 1;
        *n
    };
    let info = DeviceInfo { id: Id(Ids::N(id)), ..device.info };
    let state = State {
        plugged: true,
        endpoints: device.endpoints,
        respond: device.respond,
        io: Io::default(),
        claimed: vec![],
        log: vec![],
    };
    let fake = Arc::new(Fake { info, state: Mutex::new(state), wake: tokio::sync::Notify::new() });
    lock(&b.devices).push(fake.clone());
    b.changed.send_modify(|n| *n += 1);
    Plugged { fake, _world: world }
}

impl Plugged {
    /// What the app sees of it (its id is this plug's).
    pub fn info(&self) -> DeviceInfo {
        self.fake.info.clone()
    }

    /// `data` for the app's next read of in endpoint `endpoint`: the device
    /// sends it by itself (a button pressed, a measurement).
    pub fn send(&self, endpoint: u8, data: impl Into<Vec<u8>>) {
        self.fake.state().io.send(endpoint, data);
        self.fake.wake.notify_waiters();
    }

    /// Every transfer the app made, in order.
    pub fn transfers(&self) -> Vec<Transfer> {
        self.fake.state().log.clone()
    }

    /// Pull it out: the app's transfers on it fail (`NotConnected`), a watch
    /// hears it. Its declaration comes back, to plug in again (a new id).
    pub fn unplug(self) -> Device {
        self.pull()
    }

    fn pull(&self) -> Device {
        let b = bus();
        lock(&b.devices).retain(|d| !Arc::ptr_eq(d, &self.fake));
        let declared = {
            let mut s = self.fake.state();
            s.plugged = false;
            Device {
                info: DeviceInfo { id: Id(Ids::N(0)), ..self.fake.info.clone() },
                endpoints: s.endpoints.clone(),
                respond: s.respond.take(),
            }
        };
        self.fake.wake.notify_waiters();
        b.changed.send_modify(|n| *n += 1);
        declared
    }
}

impl Drop for Plugged {
    fn drop(&mut self) {
        if self.fake.state().plugged {
            self.pull();
        }
    }
}

/// The test world's devices.
pub(crate) struct Bus {
    devices: Mutex<Vec<Arc<Fake>>>,
    next: Mutex<u64>,
    changed: tokio::sync::watch::Sender<u64>,
}

pub(crate) fn bus() -> Arc<Bus> {
    static BUS: LazyLock<Arc<Bus>> = LazyLock::new(|| {
        Arc::new(Bus {
            devices: Mutex::new(vec![]),
            next: Mutex::new(0),
            changed: tokio::sync::watch::Sender::new(0),
        })
    });
    BUS.clone()
}

impl Bus {
    pub(crate) fn list(&self) -> Vec<DeviceInfo> {
        lock(&self.devices).iter().map(|d| d.info.clone()).collect()
    }

    pub(crate) fn changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.changed.subscribe()
    }

    pub(crate) fn open(&self, info: &DeviceInfo) -> io::Result<Opened> {
        let found = lock(&self.devices).iter().find(|d| d.info.id == info.id).cloned();
        found.map(Opened).ok_or_else(|| gone(&format!("USB {info}")))
    }
}

struct Fake {
    info: DeviceInfo,
    state: Mutex<State>,
    /// Bytes sent, or the device pulled out: waiting transfers look again.
    wake: tokio::sync::Notify,
}

struct State {
    plugged: bool,
    endpoints: Vec<(u8, Vec<(u8, usize)>)>,
    respond: Option<Respond>,
    io: Io,
    claimed: Vec<u8>,
    log: Vec<Transfer>,
}

impl Fake {
    fn state(&self) -> MutexGuard<'_, State> {
        lock(&self.state)
    }
}

impl State {
    fn ask(&mut self, t: Transfer, unscripted: Reply) -> Reply {
        let reply = match self.respond.as_mut() {
            Some(f) => f(&t, &mut self.io),
            None => unscripted,
        };
        self.log.push(t);
        reply
    }
}

/// An opened fake device.
#[derive(Clone)]
pub(crate) struct Opened(Arc<Fake>);

impl Opened {
    pub(crate) fn claim(&self, number: u8) -> io::Result<Claimed> {
        let mut s = self.0.state();
        if !s.plugged {
            return Err(gone(&format!("USB {}", self.0.info)));
        }
        let Some((_, eps)) = s.endpoints.iter().find(|(n, _)| *n == number) else {
            let has: Vec<String> = s.endpoints.iter().map(|(n, _)| n.to_string()).collect();
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("the device has no interface {number} (it has: {})", has.join(", ")),
            ));
        };
        let endpoints = eps.clone();
        if s.claimed.contains(&number) {
            return Err(io::Error::new(
                io::ErrorKind::ResourceBusy,
                format!("interface {number} is held by another program, or another Interface of this app"),
            ));
        }
        s.claimed.push(number);
        Ok(Claimed(Arc::new(Held { fake: self.0.clone(), number, endpoints })))
    }
}

/// A claimed interface of a fake device: let go when the last clone drops.
#[derive(Clone)]
pub(crate) struct Claimed(Arc<Held>);

struct Held {
    fake: Arc<Fake>,
    number: u8,
    endpoints: Vec<(u8, usize)>,
}

impl Drop for Held {
    fn drop(&mut self) {
        self.fake.state().claimed.retain(|n| *n != self.number);
    }
}

impl Claimed {
    pub(crate) async fn read(self, ep: u8, max: usize, timeout: Duration) -> io::Result<Vec<u8>> {
        let size = endpoint(&self.0.endpoints, self.0.number, ep, true)?;
        let mut asked = false;
        self.until(timeout, &format!("endpoint {ep:#04x}"), |s| {
            if let Some(d) = s.io.0.get_mut(&ep).and_then(VecDeque::pop_front) {
                return Some(fits(d, packets(max, size), ep));
            }
            if std::mem::replace(&mut asked, true) {
                return None;
            }
            match s.ask(Transfer::Read { endpoint: ep, max }, Reply::Wait) {
                Reply::Done => Some(Ok(vec![])),
                Reply::Data(d) => Some(fits(d, packets(max, size), ep)),
                Reply::Stall => Some(Err(stall(&format!("endpoint {ep:#04x}")))),
                Reply::Wait => None,
            }
        })
        .await
    }

    pub(crate) async fn write(self, ep: u8, data: Vec<u8>, timeout: Duration) -> io::Result<()> {
        endpoint(&self.0.endpoints, self.0.number, ep, false)?;
        let what = format!("endpoint {ep:#04x}");
        let mut t = Some(Transfer::Write { endpoint: ep, data });
        self.until(timeout, &what, |s| match s.ask(t.take()?, Reply::Done) {
            Reply::Done => Some(Ok(())),
            Reply::Stall => Some(Err(stall(&what))),
            Reply::Wait => None,
            Reply::Data(_) => Some(Err(misreply("a write"))),
        })
        .await
    }

    pub(crate) async fn control_in(self, setup: Control, len: u16, timeout: Duration) -> io::Result<Vec<u8>> {
        let mut t = Some(Transfer::ControlIn { setup, len });
        self.until(timeout, "control", |s| match s.ask(t.take()?, Reply::Stall) {
            Reply::Done => Some(Ok(vec![])),
            Reply::Data(d) if d.len() > usize::from(len) => {
                Some(Err(io::Error::other(format!("control: the device sent {} bytes, {len} were asked", d.len()))))
            }
            Reply::Data(d) => Some(Ok(d)),
            Reply::Stall => Some(Err(stall("control"))),
            Reply::Wait => None,
        })
        .await
    }

    pub(crate) async fn control_out(self, setup: Control, data: Vec<u8>, timeout: Duration) -> io::Result<()> {
        let mut t = Some(Transfer::ControlOut { setup, data });
        self.until(timeout, "control", |s| match s.ask(t.take()?, Reply::Stall) {
            Reply::Done => Some(Ok(())),
            Reply::Stall => Some(Err(stall("control"))),
            Reply::Wait => None,
            Reply::Data(_) => Some(Err(misreply("a control transfer out"))),
        })
        .await
    }

    /// `step`'s answer, asked now and each time the device wakes, until
    /// `timeout` (`TimedOut`) or it is pulled out (`NotConnected`).
    async fn until<T>(
        &self,
        timeout: Duration,
        what: &str,
        mut step: impl FnMut(&mut State) -> Option<io::Result<T>>,
    ) -> io::Result<T> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            // Heard from before the state is read: no wake falls between.
            let woken = self.0.fake.wake.notified();
            {
                let mut s = self.0.fake.state();
                if !s.plugged {
                    return Err(gone(what));
                }
                if let Some(r) = step(&mut s) {
                    return r;
                }
            }
            if tokio::time::timeout_at(deadline, woken).await.is_err() {
                return Err(io::Error::new(io::ErrorKind::TimedOut, format!("{what}: no answer in {timeout:?}")));
            }
        }
    }
}

/// A read's bytes, if they fit the `room` it asked (whole packets).
fn fits(d: Vec<u8>, room: usize, ep: u8) -> io::Result<Vec<u8>> {
    match d.len() <= room {
        true => Ok(d),
        false => Err(io::Error::other(format!(
            "endpoint {ep:#04x}: the device sent {} bytes into a read of {room} (an overflow)",
            d.len()
        ))),
    }
}

fn stall(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, format!("{what}: the device refused (a stall)"))
}

fn gone(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::NotConnected, format!("{what}: the device was unplugged"))
}

fn misreply(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, format!("the fake device answered {what} with Data: Done, Stall or Wait"))
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}
