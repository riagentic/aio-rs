//! `aio::hw` end to end on fake devices (`aio::testing::usb`): what an app's
//! hardware path does, from listing to transfers, unplugs and refusals. The
//! fake bus is the process's: each test takes the world alone.
use aio::hw::{self, Control, ControlKind, Event, Filter, InterfaceInfo, Recipient};
use aio::testing::exclusive;
use aio::testing::usb::{self, Reply, Transfer};
use std::io::ErrorKind;
use std::time::Duration;

const HID: InterfaceInfo = InterfaceInfo { number: 0, class: 0x03, subclass: 0, protocol: 0 };
const VENDOR: InterfaceInfo = InterfaceInfo { number: 1, class: 0xff, subclass: 0, protocol: 0 };
const SHORT: Duration = Duration::from_millis(50);
const LONG: Duration = Duration::from_secs(5);

/// A key: HID-like on interface 0 (0x81 in, 0x01 out, 64-byte packets), the
/// vendor's on 1 (bulk 0x82, 0x02, 512); each packet written is answered
/// on 0x81 with its bytes reversed, a vendor control request 0x30 with its
/// value's bytes.
fn key(serial: &str) -> usb::Device {
    usb::Device::new(0x1209, 0x0001)
        .strings("Acme", "Key", serial)
        .interface(HID, &[(0x81, 64), (0x01, 64)])
        .interface(VENDOR, &[(0x82, 512), (0x02, 512)])
        .respond(|t, io| match t {
            Transfer::Write { endpoint: 0x01, data } => {
                io.send(0x81, data.iter().rev().copied().collect::<Vec<u8>>());
                Reply::Done
            }
            Transfer::Write { .. } => Reply::Done,
            Transfer::ControlIn { setup, .. } if setup.request == 0x30 => Reply::Data(setup.value.to_le_bytes().into()),
            Transfer::ControlOut { setup, .. } if setup.request == 0x31 => Reply::Done,
            Transfer::Read { .. } => Reply::Wait,
            _ => Reply::Stall,
        })
}

fn vendor_request(request: u8, value: u16) -> Control {
    Control { kind: ControlKind::Vendor, recipient: Recipient::Device, request, value, index: 0 }
}

/// Run `f` on a runtime of the test's own: aio::hw works from any.
fn run<T>(f: impl Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(f)
}

#[test]
fn a_device_is_listed_opened_claimed_and_talked_to() {
    let _world = exclusive();
    let plugged = usb::plug(key("A1"));
    run(async {
        let all = hw::list().await.unwrap();
        assert_eq!(all.len(), 1);
        let d = &all[0];
        assert_eq!(
            (d.vendor, d.product, d.serial.as_deref(), d.product_name.as_deref()),
            (0x1209, 1, Some("A1"), Some("Key"))
        );
        assert_eq!(d.interfaces, [HID, VENDOR]);
        assert_eq!(d.id, plugged.info().id);

        let dev = hw::open(&Filter::device(0x1209, 0x0001).serial("A1")).await.unwrap();
        assert_eq!(dev.info().manufacturer.as_deref(), Some("Acme"));
        let i = dev.claim(0).await.unwrap();
        assert_eq!(i.number(), 0);
        i.write(0x01, b"ping", LONG).await.unwrap();
        assert_eq!(i.read(0x81, 64, LONG).await.unwrap(), b"gnip");
        assert_eq!(i.control_in(vendor_request(0x30, 0x0102), 2, LONG).await.unwrap(), [2, 1]);
        i.control_out(vendor_request(0x31, 7), b"x", LONG).await.unwrap();

        let bulk = dev.claim(1).await.unwrap();
        bulk.write(0x02, &[0; 1000], LONG).await.unwrap();
        plugged.send(0x82, vec![9; 600]);
        assert_eq!(bulk.read(0x82, 513, LONG).await.unwrap().len(), 600, "a read asks whole packets: 513 → 1024");
    });
    let seen = plugged.transfers();
    assert_eq!(seen[0], Transfer::Write { endpoint: 0x01, data: b"ping".to_vec() });
    assert_eq!(seen[1], Transfer::ControlIn { setup: vendor_request(0x30, 0x0102), len: 2 });
    assert_eq!(seen[2], Transfer::ControlOut { setup: vendor_request(0x31, 7), data: b"x".to_vec() });
    assert_eq!(seen.len(), 4, "reads served from the queue are no question: {seen:?}");
}

#[test]
fn a_watch_hears_devices_there_coming_and_going() {
    let _world = exclusive();
    let first = usb::plug(key("A1"));
    run(async {
        let mut w = hw::watch().unwrap();
        assert_eq!(w.next().await.unwrap(), Event::Plugged(first.info()), "the ones there first");
        let second = usb::plug(key("B2"));
        assert_eq!(w.next().await.unwrap(), Event::Plugged(second.info()));
        let info = first.info();
        let again = first.unplug();
        assert_eq!(w.next().await.unwrap(), Event::Unplugged(info.clone()));
        let back = usb::plug(again);
        assert_ne!(back.info().id, info.id, "another plug, another id");
        assert_eq!(w.next().await.unwrap(), Event::Plugged(back.info()));
        drop(second);
        assert!(matches!(w.next().await.unwrap(), Event::Unplugged(d) if d.serial.as_deref() == Some("B2")));
    });
}

#[test]
fn an_unplug_fails_what_is_under_way_and_what_comes_after() {
    let _world = exclusive();
    let plugged = usb::plug(key("A1"));
    run(async {
        let dev = hw::open(&Filter::vendor(0x1209)).await.unwrap();
        let i = dev.claim(0).await.unwrap();
        let reading = tokio::spawn({
            let i = i.clone();
            async move { i.read(0x81, 64, LONG).await }
        });
        tokio::time::sleep(SHORT).await;
        drop(plugged);
        let e = reading.await.unwrap().unwrap_err();
        assert_eq!(e.kind(), ErrorKind::NotConnected, "{e}");
        assert_eq!(i.write(0x01, b"x", LONG).await.unwrap_err().kind(), ErrorKind::NotConnected);
        assert_eq!(dev.claim(1).await.unwrap_err().kind(), ErrorKind::NotConnected);
        let e = hw::open(&Filter::vendor(0x1209)).await.unwrap_err();
        assert_eq!((e.kind(), e.to_string()), (ErrorKind::NotFound, "no USB device 1209:* is plugged in".into()));
    });
}

#[test]
fn a_read_dropped_or_timed_out_loses_nothing() {
    let _world = exclusive();
    let plugged = usb::plug(key("A1"));
    run(async {
        let i = hw::open(&Filter::any()).await.unwrap().claim(0).await.unwrap();
        let e = i.read(0x81, 64, SHORT).await.unwrap_err();
        assert_eq!(e.kind(), ErrorKind::TimedOut, "{e}");
        assert!(e.to_string().starts_with("endpoint 0x81: no answer in"), "{e}");
        // Given up on in a select (another branch won): the bytes stay for the next read.
        tokio::select! {
            _ = i.read(0x81, 64, LONG) => panic!("nothing was sent"),
            _ = tokio::time::sleep(SHORT) => {}
        }
        plugged.send(0x81, b"late".to_vec());
        assert_eq!(i.read(0x81, 64, LONG).await.unwrap(), b"late");
    });
}

#[test]
fn a_device_s_refusals_reach_the_app_by_kind() {
    let _world = exclusive();
    let _plugged = usb::plug(key("A1"));
    run(async {
        let dev = hw::open(&Filter::any()).await.unwrap();
        let i = dev.claim(0).await.unwrap();
        let e = i.control_in(vendor_request(0x99, 0), 8, LONG).await.unwrap_err();
        assert_eq!((e.kind(), e.to_string()), (ErrorKind::BrokenPipe, "control: the device refused (a stall)".into()));
        let e = i.write(0x81, b"x", LONG).await.unwrap_err();
        assert_eq!(e.kind(), ErrorKind::InvalidInput);
        assert!(e.to_string().ends_with("read from it"), "{e}");
        let e = i.read(0x82, 64, LONG).await.unwrap_err();
        assert_eq!(e.to_string(), "interface 0 has no endpoint 0x82 (it has: 0x81 in, 0x01 out)");
        let e = dev.claim(0).await.unwrap_err();
        assert_eq!(e.kind(), ErrorKind::ResourceBusy, "held by the Interface above: {e}");
        drop(i);
        dev.claim(0).await.unwrap();
        let e = dev.claim(5).await.unwrap_err();
        assert_eq!(
            (e.kind(), e.to_string()),
            (ErrorKind::NotFound, "the device has no interface 5 (it has: 0, 1)".into())
        );
    });
}

#[test]
fn an_unscripted_device_takes_writes_stalls_controls_and_overflows_are_said() {
    let _world = exclusive();
    let plugged = usb::plug(usb::Device::new(0x16c0, 0x05dc).interface(VENDOR, &[(0x81, 8), (0x01, 8)]));
    run(async {
        let i = hw::open(&Filter::any()).await.unwrap().claim(1).await.unwrap();
        i.write(0x01, b"anything", LONG).await.unwrap();
        assert_eq!(i.control_out(vendor_request(1, 0), &[], LONG).await.unwrap_err().kind(), ErrorKind::BrokenPipe);
        plugged.send(0x81, vec![0; 9]);
        let e = i.read(0x81, 8, LONG).await.unwrap_err();
        assert!(e.to_string().contains("sent 9 bytes into a read of 8 (an overflow)"), "{e}");
    });
}

#[test]
fn only_the_devices_the_capability_names_are_seen() {
    let _world = exclusive();
    let _key = usb::plug(key("A1"));
    let _other = usb::plug(usb::Device::new(0x2c97, 0x0001));
    aio::capabilities::set_usb(&["1209:0001".into()]);
    run(async {
        let seen: Vec<u16> = hw::list().await.unwrap().iter().map(|d| d.vendor).collect();
        assert_eq!(seen, [0x1209]);
        let e = hw::open(&Filter::device(0x2c97, 1)).await.unwrap_err();
        assert_eq!(e.kind(), ErrorKind::PermissionDenied);
        assert!(e.to_string().contains("add \"2c97:0001\" (or the vendor's \"2c97\") to usb in aio.toml"), "{e}");
        let e = hw::open(&Filter::vendor(0x2c97)).await.unwrap_err();
        assert_eq!(e.kind(), ErrorKind::PermissionDenied, "plugged in but left out, said so: {e}");
        assert!(e.to_string().contains("add \"2c97:0001\""), "{e}");
        let e = hw::open(&Filter::vendor(0x0bda)).await.unwrap_err();
        assert_eq!(e.kind(), ErrorKind::NotFound, "none plugged in: {e}");
        let mut w = hw::watch().unwrap();
        assert!(matches!(w.next().await.unwrap(), Event::Plugged(d) if d.vendor == 0x1209));
        let third = usb::plug(usb::Device::new(0x2c97, 0x0002));
        let fourth = usb::plug(usb::Device::new(0x1209, 0x0001));
        assert_eq!(w.next().await.unwrap(), Event::Plugged(fourth.info()), "the 2c97 between: unheard");
        drop(third);
    });
    aio::capabilities::set_usb(&["*".into()]);
}

#[test]
fn a_test_world_never_reaches_the_machine_s_devices() {
    let _world = exclusive();
    let _sandbox = aio::paths::sandbox().unwrap();
    assert!(run(hw::list()).unwrap().is_empty(), "no fake plugged: none, whatever the machine has");
}
