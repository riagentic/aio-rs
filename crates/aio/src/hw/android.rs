//! Android: the USB host API, through the app's host (aio-shell's
//! `aio.rt.Host.usbList`, `usbOpen`, handed in as [`crate::USB_HOST`]); its
//! attach and detach broadcasts reach [`crate::usb_changed`].
use super::{DeviceInfo, Id, Ids, InterfaceInfo};
use std::io;

#[cfg(target_os = "android")]
fn host() -> io::Result<&'static crate::UsbHost> {
    crate::USB_HOST
        .get()
        .ok_or_else(|| io::Error::other("no USB host yet: the app's Android host hands it in at start"))
}

#[cfg(target_os = "android")]
pub(super) fn list() -> io::Result<Vec<DeviceInfo>> {
    parse(&(host()?.list)()?)
}

/// Device `info` opened (the user asked first, if the app may not yet):
/// its usbfs descriptor, now this library's.
#[cfg(target_os = "android")]
pub(super) fn open(info: &DeviceInfo) -> io::Result<std::os::fd::OwnedFd> {
    let Ids::N(id) = info.id.0;
    (host()?.open)(id as i32)
}

#[cfg(target_os = "android")]
pub(super) fn changes() -> tokio::sync::watch::Receiver<u64> {
    crate::USB_CHANGED.subscribe()
}

/// One device as `usbList` writes it.
#[derive(serde::Deserialize)]
struct Seen {
    id: u32,
    vendor: u16,
    product: u16,
    class: u8,
    manufacturer: Option<String>,
    name: Option<String>,
    serial: Option<String>,
    /// Number, class, subclass, protocol.
    interfaces: Vec<(u8, u8, u8, u8)>,
}

/// The devices of `usbList`'s JSON. Pure.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn parse(json: &str) -> io::Result<Vec<DeviceInfo>> {
    let seen: Vec<Seen> = serde_json::from_str(json).map_err(|e| io::Error::other(format!("Host.usbList: {e}")))?;
    Ok(seen
        .into_iter()
        .map(|s| DeviceInfo {
            id: Id(Ids::N(s.id.into())),
            vendor: s.vendor,
            product: s.product,
            manufacturer: s.manufacturer,
            product_name: s.name,
            serial: s.serial,
            class: s.class,
            interfaces: s
                .interfaces
                .into_iter()
                .map(|(number, class, subclass, protocol)| InterfaceInfo { number, class, subclass, protocol })
                .collect(),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hosts_list_is_read_whole() {
        let json = r#"[{"id":1002,"vendor":11415,"product":1,"class":0,"manufacturer":"Acme","name":null,
            "serial":null,"interfaces":[[0,3,0,0],[1,255,0,0]]}]"#;
        let d = &parse(json).unwrap()[0];
        assert_eq!((d.id, d.vendor, d.product, d.class), (Id(Ids::N(1002)), 0x2c97, 1, 0));
        assert_eq!(
            (d.manufacturer.as_deref(), d.product_name.as_deref(), d.serial.as_deref()),
            (Some("Acme"), None, None)
        );
        assert_eq!(d.interfaces[1], InterfaceInfo { number: 1, class: 0xff, subclass: 0, protocol: 0 });
        assert!(parse("[]").unwrap().is_empty());
        assert!(parse("{").unwrap_err().to_string().starts_with("Host.usbList: "));
    }
}
