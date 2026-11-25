# USB devices

`aio::hw` (feature `usb`) lets the app's own code talk to a USB device: a
hardware key, a lab instrument, a board to flash, a data logger. It lists
the devices plugged in, watches them come and go, opens one, claims an
interface and moves bytes (bulk, interrupt and control transfers). It
runs where the app's cells, effects and handlers run, natively; the
protocol on top (a vendor's commands, a key's framing) stays in the app.

```toml
# the app's Cargo.toml
aio = { package = "aio-rs", version = "0.4", features = ["kit", "usb"] }

# aio.toml: the devices the app may see (lsusb shows the ids)
[capabilities]
usb = ["1209:5342", "2c97"]
```

| Call | What it does |
|---|---|
| `hw::list()` | the devices plugged in that the app may see: ids, strings, interfaces |
| `hw::watch()` | each as it comes and goes, those there first (`Event::Plugged`, `Event::Unplugged`) |
| `hw::open(&Filter)` | the first one plugged in that matches, opened (`DeviceInfo::open`: that one) |
| `device.claim(n)` | interface `n`, for this app alone |
| `iface.read(ep, max, timeout)` | one transfer in from a bulk or interrupt endpoint (`0x81`) |
| `iface.write(ep, &bytes, timeout)` | the bytes out to one (`0x01`) |
| `iface.control_in(setup, len, timeout)` / `control_out(setup, &bytes, timeout)` | a control transfer |

A `Filter` is `Filter::any()`, `Filter::vendor(0x2c97)` or
`Filter::device(0x1209, 0x5342)`, each with `.serial("…")` to pick one of
several alike. Every call is `async` and runs on aio's own runtime, so
any executor may await it: an effect, an async method's future, a
handler's task, `main`. Never a method's body (a method has no I/O,
[methods](../state/methods.md)).

## Talking to a device

```rust
use aio::hw::{self, Control, ControlKind, Filter, Interface, Recipient};
use std::io;
use std::time::Duration;

const PATIENCE: Duration = Duration::from_secs(2);

/// The device's firmware version: its vendor request 0x01, three bytes.
async fn firmware(i: &Interface) -> io::Result<String> {
    let setup = Control { kind: ControlKind::Vendor, recipient: Recipient::Device, request: 0x01, value: 0, index: 0 };
    match &i.control_in(setup, 3, PATIENCE).await?[..] {
        [major, minor, patch] => Ok(format!("{major}.{minor}.{patch}")),
        short => Err(io::Error::new(io::ErrorKind::InvalidData, format!("a version of {} bytes", short.len()))),
    }
}

/// One command out on bulk endpoint 0x01, its answer in on 0x81.
async fn ask(i: &Interface, command: &[u8]) -> io::Result<Vec<u8>> {
    i.write(0x01, command, PATIENCE).await?;
    i.read(0x81, 512, PATIENCE).await
}

async fn status() -> io::Result<String> {
    let dev = hw::open(&Filter::device(0x1209, 0x5342)).await?;
    let i = dev.claim(0).await?;
    let version = firmware(&i).await?;
    let answer = ask(&i, b"STATUS").await?;
    Ok(format!("v{version}: {}", String::from_utf8_lossy(&answer)))
}
# fn main() {
# use aio::testing::usb::{self, Reply, Transfer};
# let iface = hw::InterfaceInfo { number: 0, class: 0xff, subclass: 0, protocol: 0 };
# let _plugged = usb::plug(usb::Device::new(0x1209, 0x5342).interface(iface, &[(0x81, 512), (0x01, 512)]).respond(|t, io| match t {
#     Transfer::ControlIn { setup, .. } if setup.request == 1 => Reply::Data(vec![1, 4, 2]),
#     Transfer::Write { .. } => { io.send(0x81, b"ready".to_vec()); Reply::Done }
#     _ => Reply::Wait,
# }));
# assert_eq!(aio::rt().block_on(status()).unwrap(), "v1.4.2: ready");
# }
```

A device's packets are its own: `read` asks for `max` bytes rounded up to
whole packets of the endpoint and returns what the device sent (a short
packet ends it early). Endpoint types come from the interface's
descriptor: bulk and interrupt endpoints read and write the same way.

## Timeouts and cancelling

Each transfer has its own timeout, and every call can be dropped at any
await (a `select!`, a view that went away, `#[mode(newest)]` replacing
an effect):

- a **read** that times out or is dropped leaves its transfer queued: the
  next read on that endpoint gets those bytes, so nothing the device sent
  is lost;
- a **write** that times out or is dropped is cancelled (part of it may
  have gone out);
- a control transfer is cancelled with it.

## A device coming and going

`hw::watch()` hears every device the app may see: first the ones plugged
in already, then each change. Dropping the `Watch` stops it.

```rust
use aio::hw::{self, Event};

/// Wait for the first device of vendor 0x1209 to be plugged in.
async fn plugged_in() -> std::io::Result<hw::DeviceInfo> {
    let mut watch = hw::watch()?;
    loop {
        if let Event::Plugged(d) = watch.next().await?
            && d.vendor == 0x1209
        {
            return Ok(d);
        }
    }
}
# fn main() {
# let _key = aio::testing::usb::plug(aio::testing::usb::Device::new(0x1209, 0x0001));
# assert_eq!(aio::rt().block_on(plugged_in()).unwrap().product, 1);
# }
```

A device unplugged fails what is under way on it, and everything after,
with `NotConnected`: watch for it and open it again (a new plug is a new
`Id`).

## Who may open what

Three gates, each saying what to do when it says no:

1. **The app**: `[capabilities] usb` in `aio.toml` lists the devices the
   app may see. An entry is a vendor (`"2c97"`, 4 hex digits as `lsusb`
   shows them), a product (`"1209:5342"`) or `"*"` (any). Every other
   device is invisible: not listed, not watched; opening one (by its ids,
   or by a filter only it matches) is `PermissionDenied`, naming the entry
   to add. A process that never
   booted an app (a tool, a test) is not held.
2. **The OS**:
   - Linux opens a USB device for the person at the machine only when a
     udev rule says so. Opening one without it is `PermissionDenied`, and
     the message names the rule and the file to put it in
     (`SUBSYSTEM=="usb", ATTRS{idVendor}=="2c97", TAG+="uaccess"`). The
     app's `.deb` (`ax build --deb`) installs the rules of its
     `[capabilities] usb` (`/usr/lib/udev/rules.d/70-<app>.rules`; `"*"`
     installs none: too wide); an AppImage's user adds the rule once.
     Claiming an interface a kernel driver holds (`usbhid`, `cdc_acm`) lets
     the driver go for the claim's life.
   - Windows opens a device through WinUSB only: one whose firmware
     declares it (Microsoft OS descriptors, as most vendor-class devices
     do), or one set up with Zadig. A HID interface is the OS's HID
     driver's: not through `aio::hw`.
   - macOS opens any device, but its HID and mass storage interfaces stay
     the kernel's: claiming one is `ResourceBusy`.
3. **The user (Android)**: opening a device asks the user once ("Allow
   the app to access the USB device?"); no is `PermissionDenied`. The APK
   declares USB host (optional: it still installs without) and, for the
   vendors and products `[capabilities] usb` names, offers itself when one
   is plugged in; a user who ticks "always" isn't asked again. A device's
   serial number is seen once the user has let the app open it (a
   `Filter` by serial finds it only then: open by vendor and product first).

## Where devices come from

| Platform | Devices |
|---|---|
| Linux, Windows, macOS | the OS's USB stack (usbfs, WinUSB, IOKit) through nusb, pure Rust: no libusb |
| Android | the USB host API (Android 10 or newer); transfers through the device's usbfs descriptor, as on Linux |
| the web | none: every call is `Unsupported` |
| a test | fake devices only (below): never the machine's |

The web build has the same API, so one codebase compiles everywhere, and
says `Unsupported` at run time. A page reaches USB only through the
browser's own WebUSB (`navigator.usb`): Chromium only, a device picked by
the user in the browser's dialog after a click, and its API unstable in
Rust's bindings. Use it from the page's JS when a web build must; the
desktop and Android apps have `aio::hw`.

## When it says no

The kind of the `io::Error` says what happened:

| Kind | When | What the app does |
|---|---|---|
| `NotFound` | no device matches (`hw::open`), no such interface | ask the user to plug it in; watch for it |
| `PermissionDenied` | not in `[capabilities] usb`; the OS refused (the udev rule on Linux); the user said no (Android) | the message names the fix |
| `NotConnected` | the device was unplugged | watch for it, open it again |
| `TimedOut` | no answer in time | try again; a read's bytes stay queued |
| `BrokenPipe` | the device refused the request (a stall) | it's the device's answer: check the request |
| `ResourceBusy` | another program, or another `Interface` of this app, holds the interface | close the other one |
| `InvalidInput` | no such endpoint on the interface, or one of the other direction | fix the address (`0x81` is in, `0x01` out) |
| `Unsupported` | the web; an isochronous endpoint | use the desktop or Android app |

## In tests

`aio::testing::usb` plugs in fake devices; the app's code under test uses
`aio::hw` unchanged. In the test world (`TestCells`, `TestUi`, a
sandbox, or a plugged fake, which holds one) `aio::hw` sees the fakes only,
never the machine's devices.

```rust
use aio::hw::{self, Filter, InterfaceInfo};
use aio::testing::usb::{self, Reply, Transfer};
use std::time::Duration;

# fn main() {
let _world = aio::testing::exclusive();
let iface = InterfaceInfo { number: 0, class: 0x03, subclass: 0, protocol: 0 };
let key = usb::plug(
    usb::Device::new(0x1209, 0x0001)
        .strings("Acme", "Key", "A1")
        .interface(iface, &[(0x81, 64), (0x01, 64)])
        .respond(|t, io| match t {
            // Each command is answered on 0x81.
            Transfer::Write { data, .. } if data == b"PING" => {
                io.send(0x81, b"PONG".to_vec());
                Reply::Done
            }
            Transfer::Write { .. } => Reply::Stall,
            _ => Reply::Wait,
        }),
);
aio::rt().block_on(async {
    let i = hw::open(&Filter::vendor(0x1209)).await.unwrap().claim(0).await.unwrap();
    let t = Duration::from_secs(1);
    i.write(0x01, b"PING", t).await.unwrap();
    assert_eq!(i.read(0x81, 64, t).await.unwrap(), b"PONG");
    // A button pressed on the device: it sends by itself.
    key.send(0x81, b"PRESSED".to_vec());
    assert_eq!(i.read(0x81, 64, t).await.unwrap(), b"PRESSED");
    // Pulled out mid-use.
    let again = key.unplug();
    assert_eq!(i.read(0x81, 64, t).await.unwrap_err().kind(), std::io::ErrorKind::NotConnected);
    let _back = usb::plug(again);
});
# }
```

- `Device::new(vendor, product)`, `.strings(..)`, `.interface(info,
  &[(address, packet size)])` declare its descriptors.
- `.respond(|transfer, io| reply)` scripts it: each write, control
  transfer and read it has nothing queued for is asked; `io.send(ep,
  bytes)` queues bytes for the app's reads. A reply is `Done`,
  `Data(bytes)`, `Stall` (the app gets `BrokenPipe`) or `Wait` (no answer:
  a read waits for bytes sent to it, anything else times out). Unscripted,
  a write is taken, a read waits, a control transfer stalls.
- `Plugged::send(ep, bytes)`: the device sends by itself;
  `Plugged::transfers()`: what the app did, in order; `unplug()` (or
  dropping it) pulls it out and hands the declaration back to plug in
  again.
- A read of more than it asked (in whole packets) fails, as an overflow
  does on a real bus; a second claim of a claimed interface is
  `ResourceBusy`.

The fake devices are the process's: a test that lists or watches every
device takes the world alone (`aio::testing::exclusive()`), as above.

## What it doesn't do

- Isochronous transfers (audio, video), alternate settings and
  configurations: aio-rs's USB is for devices that talk in commands.
- A HID API: HID devices are reached as USB here (Linux, Android, and
  macOS for vendor-defined ones); on Windows a HID interface is not.
- A protocol: the app frames its own commands.

## API

`cargo doc -p aio-rs --no-deps --features usb,testing --open`: `aio::hw`
(`list`, `watch`, `open`, `Filter`, `DeviceInfo`, `Device`, `Interface`,
`Control`) and `aio::testing::usb` (`Device`, `plug`, `Plugged`,
`Transfer`, `Reply`, `Io`).
