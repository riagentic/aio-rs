//! The USB devices this machine lets aio::hw see, then each as it comes and
//! goes; with an id, that device opened (nothing claimed, nothing sent):
//!
//! ```text
//! cargo run -p aio-rs --features usb --example usb
//! cargo run -p aio-rs --features usb --example usb -- 2c97:0001
//! ```
use aio::hw::{self, Event, Filter};

fn main() -> std::io::Result<()> {
    aio::rt().block_on(async {
        for d in hw::list().await? {
            println!(
                "{d}  {}  serial {}",
                d.manufacturer.as_deref().unwrap_or("-"),
                d.serial.as_deref().unwrap_or("-")
            );
        }
        if let Some(id) = std::env::args().nth(1) {
            let (v, p) = id.split_once(':').ok_or_else(|| std::io::Error::other("an id is vvvv:pppp (hex)"))?;
            let hex = |h: &str| u16::from_str_radix(h, 16).map_err(std::io::Error::other);
            let d = hw::open(&Filter::device(hex(v)?, hex(p)?)).await?;
            println!("opened {}: interfaces {:?}", d.info(), d.info().interfaces);
            return Ok(());
        }
        let mut w = hw::watch()?;
        loop {
            match w.next().await? {
                Event::Plugged(d) => println!("+ {d}"),
                Event::Unplugged(d) => println!("- {d}"),
            }
        }
    })
}
