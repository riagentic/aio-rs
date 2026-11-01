//! Encoded frames into a file (aio's `media/webm`, `media/mp4`): a WebM of VP8
//! or an MP4 of H.264, each frame at its own time for as long as it was on
//! screen. What `ax shot --video` writes when the window encoded it (no
//! ffmpeg here). Pure: bytes in, bytes out.

/// One encoded frame: a key frame or not, when it shows and for how long
/// (ms), its bytes (VP8; H.264 as length-prefixed NAL units).
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub key: bool,
    pub at: u64,
    pub dur: u64,
    pub data: Vec<u8>,
}

/// An EBML element: its id (marker bits included), its size, its body.
fn el(id: u32, body: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = id.to_be_bytes().into_iter().skip_while(|b| *b == 0).collect();
    // The size in 8 bytes: 0x01 then 7 (always valid, never ambiguous).
    out.push(1);
    out.extend_from_slice(&(body.len() as u64).to_be_bytes()[1..]);
    out.extend_from_slice(body);
    out
}

fn uint(id: u32, v: u64) -> Vec<u8> {
    el(id, &v.to_be_bytes())
}

/// A WebM of VP8 `frames` (`w`×`h`), lasting until `end` ms.
pub fn webm(frames: &[Frame], w: u32, h: u32, end: u64) -> Vec<u8> {
    let head = [
        uint(0x4286, 1),
        uint(0x42F7, 1),
        uint(0x42F2, 4),
        uint(0x42F3, 8),
        el(0x4282, b"webm"),
        uint(0x4287, 2),
        uint(0x4285, 2),
    ]
    .concat();
    let info = [
        uint(0x2AD7B1, 1_000_000), // a tick is 1 ms
        el(0x4D80, b"aio"),
        el(0x5741, b"aio"),
        el(0x4489, &(end as f64).to_be_bytes()),
    ]
    .concat();
    let video = [uint(0xB0, w.into()), uint(0xBA, h.into())].concat();
    let track =
        [uint(0xD7, 1), uint(0x73C5, 1), uint(0x83, 1), el(0x86, b"V_VP8"), uint(0x9C, 0), el(0xE0, &video)].concat();
    let head_els = [el(0x1549A966, &info), el(0x1654AE6B, &el(0xAE, &track))].concat();
    // A cluster from each key frame 5 s on (or as a block's 16-bit time needs):
    // its time, whether a key frame starts it, its body.
    let mut clusters: Vec<(u64, bool, Vec<u8>)> = vec![];
    for f in frames {
        let start = match clusters.last() {
            Some((t, _, _)) => (f.key && f.at - t >= 5_000) || f.at - t > 30_000,
            None => true,
        };
        if start {
            clusters.push((f.at, f.key, uint(0xE7, f.at)));
        }
        let (t, _, c) = clusters.last_mut().expect("a cluster was started");
        let mut block = vec![0x81];
        block.extend_from_slice(&((f.at - *t) as i16).to_be_bytes());
        block.push(if f.key { 0x80 } else { 0 });
        block.extend_from_slice(&f.data);
        c.extend(el(0xA3, &block));
    }
    let clusters: Vec<_> = clusters.into_iter().map(|(t, key, c)| (t, key, el(0x1F43B675, &c))).collect();
    // The cues before the clusters (aio webm: it seeks while it loads), one per
    // cluster a key frame starts. Every number is 8 bytes, so their size, and
    // with it each cluster's place, is known before the places are.
    let cues = |places: &[u64]| {
        let starts = clusters.iter().filter(|(_, key, _)| *key);
        let points = starts.zip(places).map(|((t, _, _), place)| {
            el(0xBB, &[uint(0xB3, *t), el(0xB7, &[uint(0xF7, 1), uint(0xF1, *place)].concat())].concat())
        });
        el(0x1C53BB6B, &points.collect::<Vec<_>>().concat())
    };
    let mut place = (head_els.len() + cues(&vec![0; clusters.len()]).len()) as u64;
    let mut places = vec![];
    for (_, key, c) in &clusters {
        if *key {
            places.push(place);
        }
        place += c.len() as u64;
    }
    let body = [head_els, cues(&places), clusters.into_iter().flat_map(|(_, _, c)| c).collect()].concat();
    [el(0x1A45DFA3, &head), el(0x18538067, &body)].concat()
}

/// An ISO box: its size, its type, its body.
fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    [&(8 + body.len() as u32).to_be_bytes()[..], kind, body].concat()
}

/// A full box: version 0 and `flags` first.
fn full(kind: &[u8; 4], flags: u32, body: &[u8]) -> Vec<u8> {
    bx(kind, &[&flags.to_be_bytes()[..], body].concat())
}

fn u32s(v: &[u32]) -> Vec<u8> {
    v.iter().flat_map(|n| n.to_be_bytes()).collect()
}

/// The `moov` of `frames` (`w`×`h`, `avcc`: the decoder's configuration),
/// their bytes starting at `at` in the file.
fn moov(frames: &[Frame], w: u32, h: u32, avcc: &[u8], at: u32) -> Vec<u8> {
    let dur: u32 = frames.iter().map(|f| f.dur as u32).sum();
    let matrix = u32s(&[0x10000, 0, 0, 0, 0x10000, 0, 0, 0, 0x4000_0000]);
    let mvhd = [u32s(&[0, 0, 1000, dur, 0x10000]), vec![1, 0], vec![0; 10], matrix.clone(), vec![0; 24], u32s(&[2])];
    let tkhd = [u32s(&[0, 0, 1, 0, dur, 0, 0, 0]), vec![0; 4], matrix, u32s(&[w << 16, h << 16])];
    let mdhd = [u32s(&[0, 0, 1000, dur]), vec![0x55, 0xC4, 0, 0]];
    let hdlr = [u32s(&[0]), b"vide".to_vec(), vec![0; 12], b"VideoHandler\0".to_vec()];
    let avc1 = [
        vec![0; 6],
        vec![0, 1],
        vec![0; 16],
        (w as u16).to_be_bytes().to_vec(),
        (h as u16).to_be_bytes().to_vec(),
        u32s(&[0x48_0000, 0x48_0000, 0]),
        vec![0, 1],
        vec![0; 32],
        vec![0, 0x18, 0xFF, 0xFF],
        bx(b"avcC", avcc),
    ];
    // Each duration run once (stts), the key frames (stss), one chunk.
    let mut runs: Vec<(u32, u32)> = vec![];
    for f in frames {
        match runs.last_mut() {
            Some((n, d)) if *d == f.dur as u32 => *n += 1,
            _ => runs.push((1, f.dur as u32)),
        }
    }
    let keys: Vec<u32> = (1..).zip(frames).filter(|(_, f)| f.key).map(|(i, _)| i).collect();
    let n = frames.len() as u32;
    let stbl = [
        full(b"stsd", 0, &[u32s(&[1]), bx(b"avc1", &avc1.concat())].concat()),
        full(
            b"stts",
            0,
            &[u32s(&[runs.len() as u32]), runs.iter().flat_map(|(n, d)| u32s(&[*n, *d])).collect()].concat(),
        ),
        full(b"stss", 0, &[u32s(&[keys.len() as u32]), u32s(&keys)].concat()),
        full(b"stsc", 0, &u32s(&[1, 1, n, 1])),
        full(
            b"stsz",
            0,
            &[u32s(&[0, n]), frames.iter().flat_map(|f| (f.data.len() as u32).to_be_bytes()).collect()].concat(),
        ),
        full(b"stco", 0, &u32s(&[1, at])),
    ];
    let minf = [
        full(b"vmhd", 1, &[0; 8]),
        bx(b"dinf", &full(b"dref", 0, &[u32s(&[1]), full(b"url ", 1, &[])].concat())),
        bx(b"stbl", &stbl.concat()),
    ];
    let mdia = [full(b"mdhd", 0, &mdhd.concat()), full(b"hdlr", 0, &hdlr.concat()), bx(b"minf", &minf.concat())];
    let trak = [full(b"tkhd", 3, &tkhd.concat()), bx(b"mdia", &mdia.concat())];
    bx(b"moov", &[full(b"mvhd", 0, &mvhd.concat()), bx(b"trak", &trak.concat())].concat())
}

/// An MP4 of H.264 `frames` (`w`×`h`; `avcc`: the encoder's `avcC`), its
/// index before its data (it plays while it loads). An `avcc` no player
/// could decode with is refused, never written (aio mp4).
pub fn mp4(frames: &[Frame], w: u32, h: u32, avcc: &[u8]) -> Result<Vec<u8>, String> {
    if avcc.len() < 7 || avcc[0] != 1 {
        return Err("the encoder gave no usable avcC (H.264 decoder configuration)".into());
    }
    let ftyp = bx(b"ftyp", b"isom\0\0\x02\0isomiso2avc1mp41");
    // The index's size doesn't depend on where the data starts.
    let head = ftyp.len() + moov(frames, w, h, avcc, 0).len();
    let data: Vec<u8> = frames.iter().flat_map(|f| f.data.iter().copied()).collect();
    Ok([ftyp, moov(frames, w, h, avcc, head as u32 + 8), bx(b"mdat", &data)].concat())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The boxes of an ISO file's level, by type: their bodies.
    pub(crate) fn boxes(b: &[u8]) -> Vec<(String, &[u8])> {
        let mut out = vec![];
        let mut i = 0;
        while i + 8 <= b.len() {
            let n = u32::from_be_bytes(b[i..i + 4].try_into().unwrap()) as usize;
            out.push((String::from_utf8_lossy(&b[i + 4..i + 8]).into_owned(), &b[i + 8..i + n]));
            i += n;
        }
        assert_eq!(i, b.len(), "the boxes fill their parent exactly");
        out
    }

    fn child<'a>(b: &'a [u8], path: &[&str]) -> &'a [u8] {
        path.iter().fold(b, |b, k| boxes(b).into_iter().find(|(t, _)| t == k).unwrap_or_else(|| panic!("no {k}")).1)
    }

    fn frame(key: bool, at: u64, dur: u64, data: &[u8]) -> Frame {
        Frame { key, at, dur, data: data.to_vec() }
    }

    #[test]
    fn an_mp4_says_each_frames_time_size_and_key_and_its_data_is_where_it_says() {
        let fs = [frame(true, 0, 40, b"aaaa"), frame(false, 40, 40, b"bb"), frame(true, 80, 1000, b"ccc")];
        let f = mp4(&fs, 640, 360, b"\x01avc1.4d").unwrap();
        let top: Vec<String> = boxes(&f).into_iter().map(|(t, _)| t).collect();
        assert_eq!(top, ["ftyp", "moov", "mdat"], "its index first");
        let stbl = child(&f, &["moov", "trak", "mdia", "minf", "stbl"]);
        assert_eq!(child(stbl, &["stts"]), &u32s(&[0, 2, 2, 40, 1, 1000])[..], "two of 40 ms, one of 1 s");
        assert_eq!(child(stbl, &["stss"]), &u32s(&[0, 2, 1, 3])[..], "frames 1 and 3 are key frames");
        assert_eq!(child(stbl, &["stsz"]), &u32s(&[0, 0, 3, 4, 2, 3])[..]);
        let at = u32::from_be_bytes(child(stbl, &["stco"])[8..12].try_into().unwrap()) as usize;
        assert_eq!(&f[at..at + 9], b"aaaabbccc", "stco points at the frames");
        let mvhd = child(&f, &["moov", "mvhd"]);
        assert_eq!(&mvhd[12..20], &u32s(&[1000, 1080])[..], "ms, 1.08 s");
        let tkhd = child(&f, &["moov", "trak", "tkhd"]);
        assert_eq!(&tkhd[tkhd.len() - 8..], &u32s(&[640 << 16, 360 << 16])[..]);
        let avc1 = child(child(stbl, &["stsd"]), &[]);
        assert!(avc1.windows(12).any(|w| w == b"avcC\x01avc1.4d"), "the decoder's configuration");
        assert!(mp4(&fs, 2, 2, b"\x00\x01").unwrap_err().contains("avcC"), "an unusable avcC is refused");
    }

    /// An EBML element at the start of `b`: its id, its body, the rest.
    fn ebml(b: &[u8]) -> (u32, &[u8], &[u8]) {
        let len = |first: u8| first.leading_zeros() as usize + 1;
        let n = len(b[0]);
        let id = b[..n].iter().fold(0, |a, x| a << 8 | u32::from(*x));
        let m = len(b[n]);
        let size = b[n..n + m].iter().fold(0u64, |a, x| a << 8 | u64::from(*x)) & !(1u64 << (7 * m));
        let body = &b[n + m..n + m + size as usize];
        (id, body, &b[n + m + size as usize..])
    }

    fn ebmls(mut b: &[u8]) -> Vec<(u32, &[u8])> {
        let mut out = vec![];
        while !b.is_empty() {
            let (id, body, rest) = ebml(b);
            out.push((id, body));
            b = rest;
        }
        out
    }

    #[test]
    fn a_webm_holds_vp8_blocks_at_their_times_in_clusters_from_key_frames() {
        let fs = [
            frame(true, 0, 40, b"k0"),
            frame(false, 40, 6000, b"d1"),
            frame(true, 6040, 100, b"k2"),
            frame(false, 6140, 50, b"d3"),
        ];
        let f = webm(&fs, 320, 240, 6190);
        let top = ebmls(&f);
        assert_eq!(top.iter().map(|(id, _)| *id).collect::<Vec<_>>(), [0x1A45DFA3, 0x18538067]);
        assert!(ebmls(top[0].1).contains(&(0x4282, &b"webm"[..])));
        let seg = ebmls(top[1].1);
        let info = ebmls(seg[0].1);
        assert!(info.contains(&(0x4489, &6190f64.to_be_bytes()[..])), "it lasts until the end");
        let track = ebmls(ebmls(seg[1].1)[0].1);
        assert!(track.contains(&(0x86, &b"V_VP8"[..])));
        assert_eq!(seg[2].0, 0x1C53BB6B, "the cues before the clusters");
        let clusters: Vec<_> = seg[3..].iter().map(|(id, b)| (*id, ebmls(b))).collect();
        assert_eq!(clusters.len(), 2, "a new one from the key frame 6 s on");
        assert!(clusters.iter().all(|(id, _)| *id == 0x1F43B675));
        // Each cue: a key frame's time, and its cluster's place from the
        // segment's data start (the cluster's id, not its body).
        let n = |b: &[u8]| b.iter().fold(0u64, |a, x| a << 8 | u64::from(*x));
        let cues: Vec<(u64, u64)> = ebmls(seg[2].1)
            .iter()
            .map(|(id, p)| {
                let p = ebmls(p);
                let pos = ebmls(p[1].1);
                assert_eq!((*id, p[0].0, p[1].0, pos[0].0, n(pos[0].1), pos[1].0), (0xBB, 0xB3, 0xB7, 0xF7, 1, 0xF1));
                (n(p[0].1), n(pos[1].1))
            })
            .collect();
        assert_eq!(cues.iter().map(|c| c.0).collect::<Vec<_>>(), [0, 6040]);
        let data_start = f.len() - top[1].1.len();
        for (_, place) in cues {
            let at = data_start + place as usize;
            assert_eq!(f[at..at + 4], [0x1F, 0x43, 0xB6, 0x75], "a cue lands on a cluster");
        }
        let block = |c: &[(u32, &[u8])], i: usize| c[i + 1].1.to_vec();
        assert_eq!(clusters[0].1[0], (0xE7, &0u64.to_be_bytes()[..]));
        assert_eq!(block(&clusters[0].1, 1), [0x81, 0, 40, 0, b'd', b'1'], "track 1, +40 ms, not a key frame");
        assert_eq!(clusters[1].1[0], (0xE7, &6040u64.to_be_bytes()[..]));
        assert_eq!(block(&clusters[1].1, 0), [0x81, 0, 0, 0x80, b'k', b'2'], "a key frame");
        assert_eq!(block(&clusters[1].1, 1), [0x81, 0, 100, 0, b'd', b'3']);
    }
}
