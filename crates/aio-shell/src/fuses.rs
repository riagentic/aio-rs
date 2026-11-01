//! Electron's fuses: switches baked into the runtime binary. aio turns off
//! the ones that let anyone run it as plain Node or attach an inspector
//! (aio's `electron-fuses.ts`). The wire is found, never guessed.

pub const SENTINEL: &[u8] = b"dL7pKGdnNz796PbbjQWNKmHXBZaB9tsX";

/// FuseV1 index → name, for each fuse aio turns off.
pub const OFF: [(usize, &str); 3] =
    [(0, "RunAsNode"), (2, "EnableNodeOptionsEnvironmentVariable"), (3, "EnableNodeCliInspectArguments")];

fn find(b: &[u8], from: usize) -> Option<usize> {
    b.get(from..)?.windows(SENTINEL.len()).position(|w| w == SENTINEL).map(|i| i + from)
}

/// Where the first fuse byte is. Refuses a missing, doubled, other-version
/// or too-short wire.
pub fn offset(b: &[u8]) -> Result<usize, String> {
    let at = find(b, 0).ok_or("no Electron fuse wire")?;
    if find(b, at + 1).is_some() {
        return Err("two Electron fuse wires: refusing to guess".into());
    }
    let (version, count) = (b.get(at + SENTINEL.len()), b.get(at + SENTINEL.len() + 1));
    if version != Some(&1) {
        return Err(format!("fuse wire version {version:?}, not 1"));
    }
    let need = OFF.iter().map(|(i, _)| i + 1).max().unwrap_or(0);
    if (*count.unwrap_or(&0) as usize) < need {
        return Err(format!("{count:?} fuses, aio-rs needs {need}"));
    }
    Ok(at + SENTINEL.len() + 2)
}

pub fn are_off(b: &[u8]) -> Result<bool, String> {
    let at = offset(b)?;
    Ok(OFF.iter().all(|(i, _)| b[at + i] == b'0'))
}

/// Turn aio's fuses off in `b`.
pub fn turn_off(b: &mut [u8]) -> Result<(), String> {
    let at = offset(b)?;
    for (i, _) in OFF {
        b[at + i] = b'0';
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wire(version: u8, fuses: &[u8]) -> Vec<u8> {
        [b"head".as_slice(), SENTINEL, &[version, fuses.len() as u8], fuses, b"tail"].concat()
    }

    #[test]
    fn turns_off_only_aios_fuses() {
        let mut b = wire(1, b"11111111");
        assert!(!are_off(&b).unwrap());
        turn_off(&mut b).unwrap();
        assert!(are_off(&b).unwrap());
        let at = offset(&b).unwrap();
        assert_eq!(&b[at..at + 8], b"01001111");
    }

    #[test]
    fn refuses_to_guess() {
        assert!(offset(b"no wire here").is_err());
        assert!(offset(&wire(2, b"1111")).unwrap_err().contains("version"));
        assert!(offset(&wire(1, b"11")).unwrap_err().contains("aio-rs needs 4"));
        assert!(offset(&[wire(1, b"1111"), wire(1, b"1111")].concat()).unwrap_err().contains("two"));
    }
}
