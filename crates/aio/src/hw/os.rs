//! The machine's devices through nusb: listed and watched by the OS
//! (desktop) or by Android's USB host API (the app's host hands it in),
//! opened, claimed, and their transfers.
use super::{Changes, Control, ControlKind, DeviceInfo, Recipient, endpoint, err, packets};
#[cfg(not(target_os = "android"))]
use super::{Id, Ids, InterfaceInfo};
use nusb::MaybeFuture;
use nusb::transfer::{Buffer, Bulk, BulkOrInterrupt, ControlIn, ControlOut, In, Interrupt, Out, TransferError};
use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// `f`, a blocking OS call, off the async threads.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> io::Result<T> + Send + 'static) -> io::Result<T> {
    tokio::task::spawn_blocking(f).await.map_err(io::Error::other)?
}

#[cfg(not(target_os = "android"))]
pub(super) async fn list() -> io::Result<Vec<DeviceInfo>> {
    let all = blocking(|| nusb::list_devices().wait().map_err(|e| opening(e, None))).await?;
    Ok(all.map(|d| info_of(&d)).collect())
}

#[cfg(not(target_os = "android"))]
fn info_of(d: &nusb::DeviceInfo) -> DeviceInfo {
    let s = |s: Option<&str>| s.map(str::to_string);
    DeviceInfo {
        id: Id(Ids::Os(d.id())),
        vendor: d.vendor_id(),
        product: d.product_id(),
        manufacturer: s(d.manufacturer_string()),
        product_name: s(d.product_string()),
        serial: s(d.serial_number()),
        class: d.class(),
        interfaces: d
            .interfaces()
            .map(|i| InterfaceInfo {
                number: i.interface_number(),
                class: i.class(),
                subclass: i.subclass(),
                protocol: i.protocol(),
            })
            .collect(),
    }
}

#[cfg(not(target_os = "android"))]
pub(super) fn changes() -> io::Result<Changes> {
    nusb::watch_devices().map(Changes::Os).map_err(|e| opening(e, None))
}

#[cfg(not(target_os = "android"))]
pub(super) async fn open(info: &DeviceInfo) -> io::Result<nusb::Device> {
    let (id, info) = (info.id, info.clone());
    blocking(move || {
        let found = nusb::list_devices().wait().map_err(|e| opening(e, None))?.find(|d| Id(Ids::Os(d.id())) == id);
        let d = found.ok_or_else(|| err(io::ErrorKind::NotConnected, format!("USB {info} was unplugged")))?;
        d.open().wait().map_err(|e| opening(e, Some(&info)))
    })
    .await
}

#[cfg(target_os = "android")]
pub(super) async fn list() -> io::Result<Vec<DeviceInfo>> {
    blocking(super::android::list).await
}

#[cfg(target_os = "android")]
pub(super) fn changes() -> io::Result<Changes> {
    Ok(Changes::Signal(super::android::changes()))
}

#[cfg(target_os = "android")]
pub(super) async fn open(info: &DeviceInfo) -> io::Result<nusb::Device> {
    let info = info.clone();
    blocking(move || {
        let fd = super::android::open(&info)?;
        nusb::Device::from_fd(fd).wait().map_err(|e| opening(e, Some(&info)))
    })
    .await
}

/// An OS's refusal to list, open or claim, saying what to do.
fn opening(e: nusb::Error, info: Option<&DeviceInfo>) -> io::Error {
    use nusb::ErrorKind as K;
    let what = info.map_or_else(|| "USB".to_string(), |d| format!("USB {d}"));
    let kind = match e.kind() {
        K::Disconnected => return err(io::ErrorKind::NotConnected, format!("{what} was unplugged")),
        K::PermissionDenied => return denied(&what, info),
        K::Busy => io::ErrorKind::ResourceBusy,
        K::NotFound => io::ErrorKind::NotFound,
        K::Unsupported => io::ErrorKind::Unsupported,
        _ => io::ErrorKind::Other,
    };
    let windows = if cfg!(windows) && matches!(e.kind(), K::Unsupported | K::NotFound) {
        " (Windows opens a device through WinUSB only: one whose maker declares it, or one Zadig set up; \
         a HID device's interfaces are the OS's)"
    } else {
        ""
    };
    err(kind, format!("{what}: {e}{windows}"))
}

/// The OS said no: on Linux, the udev rule that lets this user open it.
fn denied(what: &str, info: Option<&DeviceInfo>) -> io::Error {
    let how = match info {
        Some(d) if cfg!(target_os = "linux") => {
            let rule = aio_config::capabilities::udev_rule(d.vendor, Some(d.product));
            format!(
                ": Linux lets a user open a USB device only through a udev rule. Put this line in \
                 /etc/udev/rules.d/70-usb-{:04x}.rules: {rule}  then run `sudo udevadm control --reload-rules`, \
                 and plug the device in again (an app's .deb from `ax build --deb` installs the rules of its \
                 [capabilities] usb)",
                d.vendor
            )
        }
        _ => String::new(),
    };
    err(io::ErrorKind::PermissionDenied, format!("{what}: the OS refused access{how}"))
}

/// Claim interface `number` of `d`, a kernel driver let go of first (Linux).
pub(super) async fn claim(d: nusb::Device, number: u8) -> io::Result<Iface> {
    blocking(move || {
        let i = d.detach_and_claim_interface(number).wait().map_err(|e| match e.kind() {
            nusb::ErrorKind::NotFound => {
                let has: Vec<String> = d
                    .active_configuration()
                    .map(|c| c.interfaces().map(|i| i.interface_number().to_string()).collect())
                    .unwrap_or_default();
                err(
                    io::ErrorKind::NotFound,
                    format!("the device has no interface {number} (it has: {})", has.join(", ")),
                )
            }
            nusb::ErrorKind::Busy => err(
                io::ErrorKind::ResourceBusy,
                format!(
                    "interface {number} is held by another program, a driver the OS keeps (macOS: HID, mass \
                     storage), or another Interface of this app"
                ),
            ),
            _ => opening(e, None),
        })?;
        let endpoints = i
            .descriptor()
            .map(|d| d.endpoints().map(|e| (e.address(), e.max_packet_size(), e.transfer_type())).collect())
            .unwrap_or_default();
        Ok(Iface(Arc::new(Claimed { i, endpoints, open: Mutex::new(HashMap::new()) })))
    })
    .await
}

/// A claimed interface: its endpoints, each opened once and kept.
#[derive(Clone)]
pub(super) struct Iface(Arc<Claimed>);

pub(super) struct Claimed {
    i: nusb::Interface,
    /// Address, packet size, type: the descriptor's.
    endpoints: Vec<(u8, usize, nusb::descriptors::TransferType)>,
    open: Mutex<HashMap<u8, Arc<tokio::sync::Mutex<Ep>>>>,
}

enum Ep {
    BulkIn(nusb::Endpoint<Bulk, In>),
    IntIn(nusb::Endpoint<Interrupt, In>),
    BulkOut(nusb::Endpoint<Bulk, Out>),
    IntOut(nusb::Endpoint<Interrupt, Out>),
}

impl Iface {
    /// Endpoint `address`, of direction `input`, opened on first use.
    fn ep(&self, address: u8, input: bool) -> io::Result<Arc<tokio::sync::Mutex<Ep>>> {
        use nusb::descriptors::TransferType as T;
        let c = &self.0;
        let sizes: Vec<(u8, usize)> = c.endpoints.iter().map(|(a, s, _)| (*a, *s)).collect();
        endpoint(&sizes, c.i.interface_number(), address, input)?;
        let mut open = c.open.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(ep) = open.get(&address) {
            return Ok(ep.clone());
        }
        let kind = c.endpoints.iter().find(|(a, ..)| *a == address).map(|(.., t)| *t);
        let ep = match (kind, input) {
            (Some(T::Bulk), true) => c.i.endpoint(address).map(Ep::BulkIn),
            (Some(T::Interrupt), true) => c.i.endpoint(address).map(Ep::IntIn),
            (Some(T::Bulk), false) => c.i.endpoint(address).map(Ep::BulkOut),
            (Some(T::Interrupt), false) => c.i.endpoint(address).map(Ep::IntOut),
            _ => {
                return Err(err(
                    io::ErrorKind::Unsupported,
                    format!("endpoint {address:#04x} is isochronous: aio::hw moves bulk and interrupt transfers"),
                ));
            }
        }
        .map_err(|e| err(io::ErrorKind::ResourceBusy, format!("endpoint {address:#04x}: {e}")))?;
        let ep = Arc::new(tokio::sync::Mutex::new(ep));
        open.insert(address, ep.clone());
        Ok(ep)
    }

    pub(super) async fn read(self, address: u8, max: usize, timeout: Duration) -> io::Result<Vec<u8>> {
        let ep = self.ep(address, true)?;
        let mut ep = ep.lock().await;
        match &mut *ep {
            Ep::BulkIn(e) => read(e, max, timeout).await,
            Ep::IntIn(e) => read(e, max, timeout).await,
            _ => unreachable!("an in endpoint by its address"),
        }
    }

    pub(super) async fn write(self, address: u8, data: Vec<u8>, timeout: Duration) -> io::Result<()> {
        let ep = self.ep(address, false)?;
        let mut ep = ep.lock().await;
        match &mut *ep {
            Ep::BulkOut(e) => write(e, data, timeout).await,
            Ep::IntOut(e) => write(e, data, timeout).await,
            _ => unreachable!("an out endpoint by its address"),
        }
    }

    pub(super) async fn control_in(self, c: Control, len: u16, timeout: Duration) -> io::Result<Vec<u8>> {
        let (control_type, recipient) = setup(c);
        let asked =
            ControlIn { control_type, recipient, request: c.request, value: c.value, index: c.index, length: len };
        self.0.i.control_in(asked, timeout).await.map_err(|e| transfer(e, "control", timeout))
    }

    pub(super) async fn control_out(self, c: Control, data: Vec<u8>, timeout: Duration) -> io::Result<()> {
        let (control_type, recipient) = setup(c);
        let asked =
            ControlOut { control_type, recipient, request: c.request, value: c.value, index: c.index, data: &data };
        self.0.i.control_out(asked, timeout).await.map_err(|e| transfer(e, "control", timeout))
    }
}

fn setup(c: Control) -> (nusb::transfer::ControlType, nusb::transfer::Recipient) {
    use nusb::transfer::{ControlType as T, Recipient as R};
    let kind = match c.kind {
        ControlKind::Standard => T::Standard,
        ControlKind::Class => T::Class,
        ControlKind::Vendor => T::Vendor,
    };
    let to = match c.recipient {
        Recipient::Device => R::Device,
        Recipient::Interface => R::Interface,
        Recipient::Endpoint => R::Endpoint,
        Recipient::Other => R::Other,
    };
    (kind, to)
}

/// One transfer in. A transfer a dropped or timed-out read left queued is
/// this one's: its bytes are never lost.
async fn read<T: BulkOrInterrupt>(e: &mut nusb::Endpoint<T, In>, max: usize, timeout: Duration) -> io::Result<Vec<u8>> {
    if e.pending() == 0 {
        e.submit(Buffer::new(packets(max, e.max_packet_size())));
    }
    let what = |e: &nusb::Endpoint<T, In>| format!("endpoint {:#04x}", e.endpoint_address());
    let done = match tokio::time::timeout(timeout, e.next_complete()).await {
        Ok(d) => d,
        Err(_) => return Err(timed_out(&what(e), timeout)),
    };
    done.status.map_err(|s| transfer(s, &what(e), timeout))?;
    Ok(done.buffer.into_vec())
}

/// `data` out, whole; cancelled when dropped or timed out.
async fn write<T: BulkOrInterrupt>(e: &mut nusb::Endpoint<T, Out>, data: Vec<u8>, timeout: Duration) -> io::Result<()> {
    /// Cancels what is still going when dropped: the write gave up.
    struct Going<'a, T: BulkOrInterrupt>(&'a mut nusb::Endpoint<T, Out>);
    impl<T: BulkOrInterrupt> Drop for Going<'_, T> {
        fn drop(&mut self) {
            self.0.cancel_all();
        }
    }
    // A dropped write's cancelled transfer is reaped first.
    while e.pending() > 0 {
        _ = e.next_complete().await;
    }
    let what = format!("endpoint {:#04x}", e.endpoint_address());
    let going = Going(e);
    going.0.submit(Buffer::from(data));
    let done = match tokio::time::timeout(timeout, going.0.next_complete()).await {
        Ok(d) => d,
        Err(_) => {
            going.0.cancel_all();
            _ = going.0.next_complete().await;
            return Err(timed_out(&what, timeout));
        }
    };
    done.status.map_err(|s| transfer(s, &what, timeout))
}

fn timed_out(what: &str, timeout: Duration) -> io::Error {
    err(io::ErrorKind::TimedOut, format!("{what}: no answer in {timeout:?}"))
}

/// A transfer's failure on `what`, by its kind.
fn transfer(e: TransferError, what: &str, timeout: Duration) -> io::Error {
    match e {
        TransferError::Cancelled => timed_out(what, timeout),
        TransferError::Stall => err(io::ErrorKind::BrokenPipe, format!("{what}: the device refused (a stall)")),
        TransferError::Disconnected => err(io::ErrorKind::NotConnected, format!("{what}: the device was unplugged")),
        TransferError::InvalidArgument => err(io::ErrorKind::InvalidInput, format!("{what}: {e}")),
        TransferError::Fault | TransferError::Unknown(_) => io::Error::other(format!("{what}: {e}")),
    }
}
