//! The app's icon in each package's format: a Windows `.ico` and the exe
//! resource that carries it, a macOS `.icns`, and what is said about an
//! icon the build can't use. The monogram itself is `aio::icon`'s.
use aio_config::icon::{icon_png, png, png_size};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The sizes Windows asks an exe's icon for: taskbar, Explorer, title bar.
pub const ICO_SIZES: [u32; 4] = [256, 48, 32, 16];

/// A Windows `.ico` of PNG images (valid since Vista): each square, at most
/// 256 px (`size` of each given).
pub fn ico(pngs: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let head = 6 + 16 * pngs.len();
    let mut out = [0u16, 1, pngs.len() as u16].iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>();
    let mut at = head as u32;
    for (size, p) in pngs {
        out.extend([(*size % 256) as u8, (*size % 256) as u8, 0, 0]);
        out.extend(1u16.to_le_bytes()); // planes
        out.extend(32u16.to_le_bytes()); // bits per pixel
        out.extend((p.len() as u32).to_le_bytes());
        out.extend(at.to_le_bytes());
        at += p.len() as u32;
    }
    pngs.iter().for_each(|(_, p)| out.extend_from_slice(p));
    out
}

/// The app's icon as a Windows `.ico`, drawn at each of [`ICO_SIZES`].
pub fn icon_ico(name: &str, hue: u16) -> Vec<u8> {
    ico(&ICO_SIZES.map(|s| (s, icon_png(name, hue, s))))
}

/// The app's own `icon.png` as a Windows `.ico`, scaled to each of
/// [`ICO_SIZES`] (an `.ico` image is at most 256 px).
pub fn ico_from_png(png_bytes: &[u8]) -> Result<Vec<u8>, String> {
    let img = image::load_from_memory_with_format(png_bytes, image::ImageFormat::Png)
        .map_err(|e| format!("icon.png: {e}"))?
        .to_rgba8();
    Ok(ico(&ICO_SIZES.map(|s| {
        let scaled = image::imageops::resize(&img, s, s, image::imageops::FilterType::Lanczos3);
        (s, png(scaled.as_raw(), s, s))
    })))
}

/// The `windres` that makes an object for a Windows exe on CPU `arch`
/// (`std::env::consts::ARCH`'s word): MinGW's, or for ARM llvm-mingw's
/// (the one that links it); on Windows, the one on `PATH`. Pure.
pub fn windres_for(arch: &str, windows_host: bool) -> String {
    match (windows_host, arch) {
        (true, "x86_64") => "windres".into(),
        (_, arch) => format!("{arch}-w64-mingw32-windres"),
    }
}

/// An object file holding `ico` as an exe's icon (resource 1, the one
/// Explorer and the taskbar show) for CPU `arch`, made in `dir` by
/// [`windres_for`]; the exe's link takes it.
pub fn exe_icon_object(ico: &[u8], dir: &Path, arch: &str) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("app.ico"), ico).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("app.rc"), "1 ICON \"app.ico\"\n").map_err(|e| e.to_string())?;
    let windres = windres_for(arch, cfg!(windows));
    let out = Command::new(&windres)
        .args(["app.rc", "-O", "coff", "-o", "app-icon.o"])
        .current_dir(dir)
        .output()
        .map_err(|e| format!("{windres} (MinGW's, for the exe's icon): {e}"))?;
    if !out.status.success() {
        return Err(format!("{windres}: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(dir.join("app-icon.o"))
}

/// The `.icns` slots made of PNGs, each type and its pixel size (aio's
/// `ICNS_PNG_TYPES`): `ic11`/`ic12` are 16/32 pt at 2x, `ic13`/`ic14`
/// 128/256 pt at 2x, `ic10` 512 pt at 2x (soft on a Retina Dock without).
pub const ICNS_SLOTS: [(&[u8; 4], u32); 9] = [
    (b"ic11", 32),
    (b"ic12", 64),
    (b"ic07", 128),
    (b"ic08", 256),
    (b"ic13", 256),
    (b"ic09", 512),
    (b"ic14", 512),
    (b"ic10", 1024),
    (b"icp4", 16),
];

/// An `.icns` of `png_for(size)`'s PNG in each [`ICNS_SLOTS`] slot; a slot
/// it has no image for (`None`) is left out.
pub fn icns(png_for: impl Fn(u32) -> Option<Vec<u8>>) -> Vec<u8> {
    let mut made: Vec<(u32, Vec<u8>)> = vec![];
    let mut body = vec![];
    for (kind, size) in ICNS_SLOTS {
        let png = match made.iter().find(|(s, _)| *s == size) {
            Some((_, p)) => p.clone(),
            None => match png_for(size) {
                Some(p) => {
                    made.push((size, p.clone()));
                    p
                }
                None => continue,
            },
        };
        body.extend_from_slice(kind);
        body.extend((8 + png.len() as u32).to_be_bytes());
        body.extend(png);
    }
    [b"icns".to_vec(), (8 + body.len() as u32).to_be_bytes().to_vec(), body].concat()
}

/// The monogram as an `.icns`, drawn at each slot's own size.
pub fn icon_icns(name: &str, hue: u16) -> Vec<u8> {
    icns(|s| Some(icon_png(name, hue, s)))
}

/// The app's own `icon.png` as an `.icns`: scaled to each slot it is at
/// least as large as (a slot it can't fill is left to macOS to scale).
pub fn icns_from_png(png_bytes: &[u8]) -> Result<Vec<u8>, String> {
    let img = image::load_from_memory_with_format(png_bytes, image::ImageFormat::Png)
        .map_err(|e| format!("icon.png: {e}"))?
        .to_rgba8();
    let edge = img.width().min(img.height());
    Ok(icns(|s| {
        (s <= edge).then(|| {
            let scaled = image::imageops::resize(&img, s, s, image::imageops::FilterType::Lanczos3);
            png(scaled.as_raw(), s, s)
        })
    }))
}

/// The iPhone's 1024 px icon from `png_bytes` (the app's own, else None:
/// the monogram drawn full-bleed): opaque RGB, as the App Store takes it
/// (iOS rounds the corners itself); a transparent pixel goes onto white.
pub fn ios_icon(png_bytes: Option<&[u8]>, name: &str, hue: u16) -> Result<Vec<u8>, String> {
    const S: u32 = 1024;
    let img = match png_bytes {
        Some(b) => {
            let img = image::load_from_memory_with_format(b, image::ImageFormat::Png)
                .map_err(|e| format!("icon.png: {e}"))?
                .to_rgba8();
            image::imageops::resize(&img, S, S, image::imageops::FilterType::Lanczos3)
        }
        None => {
            let mut px = aio_config::icon::pixels(name, hue, S);
            px.chunks_mut(4).for_each(|p| p[3] = 255);
            image::RgbaImage::from_raw(S, S, px).ok_or("the monogram's size")?
        }
    };
    let rgb = image::RgbImage::from_fn(S, S, |x, y| {
        let p = img.get_pixel(x, y).0;
        let a = u16::from(p[3]);
        image::Rgb(std::array::from_fn(|i| ((u16::from(p[i]) * a + 255 * (255 - a)) / 255) as u8))
    });
    let mut out = std::io::Cursor::new(Vec::new());
    rgb.write_to(&mut out, image::ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(out.into_inner())
}

/// What to say about the icons in app folder `dir` (aio's hints): the one
/// the build reads is `assets/icon.png`, a square PNG (512 px or more for
/// every target); an `assets/icon.svg` is not read; an `icon.png` elsewhere
/// is not either. Nothing: all is well (none at all is the monogram).
pub fn hints(dir: &Path) -> Vec<String> {
    let want = dir.join("assets/icon.png");
    let mut said = vec![];
    match std::fs::read(&want) {
        Ok(png) => match png_size(&png) {
            None => said.push(format!("{} is not a PNG: the app does not build", want.display())),
            Some((w, h)) if w != h => said.push(format!(
                "{} is {w}×{h}: an icon is square; each package draws it squeezed to one. Ship it 512×512",
                want.display()
            )),
            Some(_) => {}
        },
        Err(_) => {
            let svg = dir.join("assets/icon.svg");
            if svg.is_file() {
                said.push(format!(
                    "{} is not read (windows, taskbars and Android draw PNGs): export it as {}, 512×512",
                    svg.display(),
                    want.display()
                ));
            }
            let near = ["icon.png", "src/icon.png", "assets/icons/icon.png", "app/assets/icon.png"];
            if let Some(p) = near.iter().map(|n| dir.join(n)).find(|p| p.is_file()) {
                said.push(format!(
                    "found {}, which the build does not read: the app's icon is {}. Move it there to use it",
                    p.display(),
                    want.display()
                ));
            }
        }
    }
    said
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ico_is_well_formed() {
        let i = ico(&[(256, vec![1, 2, 3]), (16, vec![4])]);
        assert_eq!(&i[..6], [0, 0, 1, 0, 2, 0]);
        assert_eq!((i[6], i[22]), (0, 16), "256 is written as 0");
        assert_eq!(u32::from_le_bytes(i[18..22].try_into().unwrap()), 38, "the first image follows the directory");
        assert_eq!(u32::from_le_bytes(i[34..38].try_into().unwrap()), 41);
        assert_eq!(&i[38..], [1, 2, 3, 4]);
        assert_eq!(icon_ico("Notes", 61)[4], 4);
    }

    #[test]
    fn the_iphone_icon_is_1024_px_and_opaque() {
        let rgb = |b: Vec<u8>| image::load_from_memory(&b).unwrap();
        let mono = rgb(ios_icon(None, "Notes", 61).unwrap());
        assert_eq!((mono.width(), mono.height(), mono.color()), (1024, 1024, image::ColorType::Rgb8));
        let corner = mono.to_rgb8().get_pixel(0, 0).0;
        assert_ne!(corner, [255, 255, 255], "full-bleed: iOS rounds the corners");
        // An own icon with a transparent pixel: scaled, that pixel white.
        let mut own = image::RgbaImage::from_pixel(8, 8, image::Rgba([10, 20, 30, 255]));
        own.put_pixel(0, 0, image::Rgba([0, 0, 0, 0]));
        let own = rgb(ios_icon(Some(&aio_config::icon::png(own.as_raw(), 8, 8)), "x", 0).unwrap());
        assert_eq!((own.width(), own.color()), (1024, image::ColorType::Rgb8));
        assert_eq!(own.to_rgb8().get_pixel(1023, 1023).0, [10, 20, 30]);
    }

    #[test]
    fn an_apps_own_icon_is_scaled_to_each_ico_size() {
        let i = ico_from_png(&icon_png("Notes", 61, 512)).unwrap();
        let dir: Vec<u8> = (0..4).map(|k| i[6 + 16 * k]).collect();
        assert_eq!(dir, [0, 48, 32, 16]);
        assert!(ico_from_png(b"not a png").unwrap_err().starts_with("icon.png: "));
    }

    /// Each `.icns` element: its type and its PNG's size.
    fn icns_slots(icns: &[u8]) -> Vec<(String, (u32, u32))> {
        assert_eq!(&icns[..4], b"icns");
        assert_eq!(u32::from_be_bytes(icns[4..8].try_into().unwrap()) as usize, icns.len());
        let (mut at, mut out) = (8, vec![]);
        while at < icns.len() {
            let len = u32::from_be_bytes(icns[at + 4..at + 8].try_into().unwrap()) as usize;
            let kind = String::from_utf8(icns[at..at + 4].to_vec()).unwrap();
            out.push((kind, png_size(&icns[at + 8..at + len]).unwrap()));
            at += len;
        }
        out
    }

    #[test]
    fn the_icns_has_every_slot_16_to_1024_each_at_its_own_size() {
        let got = icns_slots(&icns(|_| Some(icon_png("N", 61, 8))));
        assert_eq!(got.len(), ICNS_SLOTS.len());
        let kinds: Vec<&str> = got.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(kinds, ["ic11", "ic12", "ic07", "ic08", "ic13", "ic09", "ic14", "ic10", "icp4"]);
        // An own icon of 128 fills the slots up to 128, each at its size; macOS scales the rest.
        let own = icns_slots(&icns_from_png(&icon_png("Notes", 61, 128)).unwrap());
        assert_eq!(
            own,
            [("ic11", 32), ("ic12", 64), ("ic07", 128), ("icp4", 16)].map(|(k, s)| (k.to_string(), (s, s)))
        );
        assert!(icns_from_png(b"nope").is_err());
    }

    #[test]
    fn an_icon_the_build_cant_use_is_said() {
        let d = tempfile::tempdir().unwrap();
        assert!(hints(d.path()).is_empty(), "none: the monogram, nothing to say");
        std::fs::write(d.path().join("icon.png"), icon_png("x", 1, 8)).unwrap();
        std::fs::create_dir(d.path().join("assets")).unwrap();
        std::fs::write(d.path().join("assets/icon.svg"), "<svg/>").unwrap();
        let said = hints(d.path());
        assert_eq!(said.len(), 2, "{said:?}");
        assert!(
            said[0].contains("icon.svg is not read") && said[1].starts_with("found ") && said[1].contains("Move it")
        );
        std::fs::write(d.path().join("assets/icon.png"), png(&[0; 4 * 8 * 4], 8, 4)).unwrap();
        let said = hints(d.path());
        assert!(said.len() == 1 && said[0].contains("is 8×4: an icon is square"), "{said:?}");
        std::fs::write(d.path().join("assets/icon.png"), icon_png("x", 1, 8)).unwrap();
        assert!(hints(d.path()).is_empty(), "the root one is a README logo once assets/ has the icon");
    }

    /// whether an `RT_GROUP_ICON` names them.
    #[cfg(target_os = "linux")]
    fn exe_icons(exe: &[u8]) -> (Vec<Vec<u8>>, bool) {
        use object::LittleEndian as LE;
        use object::read::pe::{PeFile64, ResourceNameOrId};
        let pe = PeFile64::parse(exe).unwrap();
        let rsrc = pe.data_directories().resource_directory(exe, &pe.section_table()).unwrap().unwrap();
        let of_type = |t: u16| {
            rsrc.root().unwrap().entries.iter().find(|e| matches!(e.name_or_id(), ResourceNameOrId::Id(id) if id == t))
        };
        let icons = of_type(3).unwrap().data(rsrc).unwrap().table().unwrap();
        let images = icons
            .entries
            .iter()
            .map(|e| {
                let lang = e.data(rsrc).unwrap().table().unwrap();
                let d = lang.entries[0].data(rsrc).unwrap().data().unwrap();
                let at = pe.section_table().pe_data_at(exe, d.offset_to_data.get(LE)).unwrap();
                at[..d.size.get(LE) as usize].to_vec()
            })
            .collect();
        (images, of_type(14).is_some())
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_exe_wears_the_icon_every_image_byte_for_byte() {
        let d = tempfile::tempdir().unwrap();
        let pngs = [256, 48, 32, 16].map(|s| (s, icon_png("Notes", 61, s)));
        let obj = exe_icon_object(&ico(&pngs), d.path(), "x86_64").unwrap();
        std::fs::write(d.path().join("main.c"), "int main(void) { return 0; }\n").unwrap();
        let st = Command::new("x86_64-w64-mingw32-gcc")
            .arg("main.c")
            .arg(&obj)
            .args(["-o", "t.exe"])
            .current_dir(d.path())
            .status()
            .unwrap();
        assert!(st.success());
        let (images, group) = exe_icons(&std::fs::read(d.path().join("t.exe")).unwrap());
        assert!(group, "one icon group names the images");
        assert_eq!(images, pngs.map(|(_, p)| p).to_vec());
    }

    #[test]
    fn an_arm_exes_icon_is_made_for_arm() {
        assert_eq!(windres_for("x86_64", false), "x86_64-w64-mingw32-windres");
        assert_eq!(windres_for("aarch64", false), "aarch64-w64-mingw32-windres");
        assert_eq!(windres_for("x86_64", true), "windres");
        assert_eq!(windres_for("aarch64", true), "aarch64-w64-mingw32-windres");
    }
}
