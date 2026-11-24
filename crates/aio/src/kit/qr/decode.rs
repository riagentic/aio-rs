//! A QR code read from a grey picture (a camera's frame, a screenshot):
//! ISO/IEC 18004 model 2, versions 1 to 40, every error-correction level,
//! numeric, alphanumeric and byte text (UTF-8; Latin-1 when its ECI says so
//! or it isn't UTF-8). Kanji is not read. Dark on light or light on dark,
//! turned, tilted, unevenly lit. Pure: the picture in, the text out.

/// The text of the QR code in `luma` (`width` × `height` grey levels, row
/// by row, 0 black), or `None`: no code found, too damaged, empty or kanji.
pub fn decode_qr(luma: &[u8], width: usize, height: usize) -> Option<String> {
    if width == 0 || height == 0 || luma.len() < width.checked_mul(height)? {
        return None;
    }
    let img = binarize(luma, width, height);
    read(&img).or_else(|| read(&Bits { dark: img.dark.iter().map(|d| !d).collect(), ..img }))
}

/// A picture in black and white.
struct Bits {
    w: usize,
    h: usize,
    dark: Vec<bool>,
}

impl Bits {
    fn get(&self, x: i64, y: i64) -> Option<bool> {
        let ok = x >= 0 && y >= 0 && (x as usize) < self.w && (y as usize) < self.h;
        ok.then(|| self.dark[y as usize * self.w + x as usize])
    }

    /// The pixel under point (`x`, `y`) (a pixel spans [x, x + 1)).
    fn at(&self, x: f64, y: f64) -> Option<bool> {
        self.get(x.floor() as i64, y.floor() as i64)
    }
}

/// Black where a pixel is darker than its neighbourhood: 8-pixel blocks,
/// each held to the mean of the 5 × 5 blocks around it, so shade and glare
/// don't drown the code. A flat block takes its neighbours' level.
fn binarize(luma: &[u8], w: usize, h: usize) -> Bits {
    const B: usize = 8;
    let (bw, bh) = (w.div_ceil(B), h.div_ceil(B));
    let mut level = vec![0u32; bw * bh];
    for by in 0..bh {
        for bx in 0..bw {
            let (mut sum, mut n, mut lo, mut hi) = (0u32, 0u32, 255u32, 0u32);
            for y in by * B..((by + 1) * B).min(h) {
                for &p in &luma[y * w + bx * B..y * w + ((bx + 1) * B).min(w)] {
                    let p = p as u32;
                    (sum, n, lo, hi) = (sum + p, n + 1, lo.min(p), hi.max(p));
                }
            }
            let mut avg = sum / n;
            if hi - lo <= 24 {
                // Flat: light, unless the blocks before it say it is dark.
                avg = lo / 2;
                if bx > 0 && by > 0 {
                    let at = |x: usize, y: usize| level[y * bw + x];
                    let near = (at(bx, by - 1) + 2 * at(bx - 1, by) + at(bx - 1, by - 1)) / 4;
                    if lo < near {
                        avg = near;
                    }
                }
            }
            level[by * bw + bx] = avg;
        }
    }
    let mut dark = vec![false; w * h];
    for by in 0..bh {
        for bx in 0..bw {
            let (x0, y0) =
                (bx.saturating_sub(2).min(bw.saturating_sub(5)), by.saturating_sub(2).min(bh.saturating_sub(5)));
            let (mut sum, mut n) = (0, 0);
            for y in y0..(y0 + 5).min(bh) {
                for x in x0..(x0 + 5).min(bw) {
                    (sum, n) = (sum + level[y * bw + x], n + 1);
                }
            }
            let t = sum / n;
            for y in by * B..((by + 1) * B).min(h) {
                for x in bx * B..((bx + 1) * B).min(w) {
                    dark[y * w + x] = luma[y * w + x] as u32 <= t;
                }
            }
        }
    }
    Bits { w, h, dark }
}

/// A point in pixels.
type Pt = (f64, f64);

fn dist(a: Pt, b: Pt) -> f64 {
    ((a.0 - b.0) * (a.0 - b.0) + (a.1 - b.1) * (a.1 - b.1)).sqrt()
}

/// `v` by `key`, least first: an insertion sort, for the few finders and
/// corners a picture has (the standard sorts are kilobytes of a web build).
fn sort_by_key<T: Copy>(v: &mut [T], key: impl Fn(&T) -> f64) {
    for i in 1..v.len() {
        let mut j = i;
        while j > 0 && key(&v[j]) < key(&v[j - 1]) {
            v.swap(j, j - 1);
            j -= 1;
        }
    }
}

/// A finder pattern: its centre, its module's size, how many rows saw it.
#[derive(Clone, Copy)]
struct Finder {
    at: Pt,
    size: f64,
    seen: u32,
}

/// Runs of dark, light, dark, light, dark as wide as 1:1:3:1:1 modules.
fn finder_runs(c: &[usize; 5]) -> bool {
    let total: usize = c.iter().sum();
    let m = total as f64 / 7.0;
    let near = |n: usize, k: f64| (k * m - n as f64).abs() < k * m / 2.0;
    total >= 7 && near(c[0], 1.0) && near(c[1], 1.0) && near(c[2], 3.0) && near(c[3], 1.0) && near(c[4], 1.0)
}

/// The finder's runs across pixel (`x`, `y`) along (`dx`, `dy`), when they
/// are a finder's about `width` wide (what the row scan saw): the centre's
/// offset from (`x`, `y`) on that line, and the runs' width.
fn cross(img: &Bits, (x, y): (i64, i64), (dx, dy): (i64, i64), width: usize) -> Option<(f64, usize)> {
    let at = |i: i64| img.get(x + i * dx, y + i * dy);
    let run = |i: &mut i64, step: i64, dark: bool| {
        let mut n = 0;
        while n <= width && at(*i) == Some(dark) {
            n += 1;
            *i += step;
        }
        n
    };
    let (mut back, mut on) = (0, 1);
    let centre = run(&mut back, -1, true) + run(&mut on, 1, true);
    let end = on;
    // A light run reaching the picture's edge is no finder's.
    let c1 = run(&mut back, -1, false);
    let mut edge = at(back).is_none();
    let c0 = run(&mut back, -1, true);
    let c3 = run(&mut on, 1, false);
    edge |= at(on).is_none();
    let c = [c0, c1, centre, c3, run(&mut on, 1, true)];
    if edge || c.iter().any(|&n| n > width) {
        return None;
    }
    let total: usize = c.iter().sum();
    let ok = 5 * total.abs_diff(width) < 2 * width && finder_runs(&c);
    ok.then(|| (end as f64 - centre as f64 / 2.0, total))
}

/// Every finder pattern: rows scanned for 1:1:3:1:1, each hit checked down
/// its column and across again, hits of one pattern merged.
fn finders(img: &Bits) -> Vec<Finder> {
    let mut found = Vec::new();
    for y in 0..img.h {
        let (mut c, mut s) = ([0usize; 5], 0);
        // Past the edge is light: it ends the last run.
        for x in 0..=img.w {
            let dark = x < img.w && img.dark[y * img.w + x];
            if s % 2 == 1 {
                if dark {
                    s += 1;
                    c[s] = 1;
                } else {
                    c[s] += 1;
                }
            } else if dark {
                c[s] += 1;
            } else if c[s] > 0 {
                if s < 4 {
                    s += 1;
                    c[s] = 1;
                    continue;
                }
                if finder_runs(&c) {
                    found_at(img, &c, x, y, &mut found);
                }
                (c, s) = ([c[2], c[3], c[4], 1, 0], 3);
            }
        }
    }
    found
}

/// Check the row's hit `c` (ending before `end`) down and across, and keep it.
fn found_at(img: &Bits, c: &[usize; 5], end: usize, y: usize, found: &mut Vec<Finder>) {
    let width = c.iter().sum();
    let x = (end - c[4] - c[3]) as f64 - c[2] as f64 / 2.0;
    let Some((dy, tall)) = cross(img, (x as i64, y as i64), (0, 1), width) else { return };
    let y = y as f64 + dy;
    let Some((dx, wide)) = cross(img, (x as i64, y as i64), (1, 0), width) else { return };
    let (at, size) = (((x as i64) as f64 + dx, y), (wide + tall) as f64 / 14.0);
    let same = |f: &&mut Finder| {
        (f.at.0 - at.0).abs() <= size && (f.at.1 - at.1).abs() <= size && (f.size - size).abs() <= f.size.max(4.0) / 4.0
    };
    match found.iter_mut().find(same) {
        Some(f) => {
            let n = f.seen as f64;
            let avg = |a: f64, b: f64| (a * n + b) / (n + 1.0);
            (f.at, f.size, f.seen) = ((avg(f.at.0, at.0), avg(f.at.1, at.1)), avg(f.size, size), f.seen + 1);
        }
        None => found.push(Finder { at, size, seen: 1 }),
    }
}

/// The likeliest codes' corners (top-left, top-right, bottom-left): every
/// three of the surest finders, nearest a square first.
fn corners(found: &[Finder]) -> Vec<[Finder; 3]> {
    let mut f: Vec<Finder> = found.iter().filter(|f| f.seen >= 2).copied().collect();
    if f.len() < 3 {
        f = found.to_vec();
    }
    sort_by_key(&mut f, |f| -(f.seen as f64));
    f.truncate(12);
    let mut all = vec![];
    for i in 0..f.len() {
        for j in i + 1..f.len() {
            for k in j + 1..f.len() {
                all.extend(shape([f[i], f[j], f[k]]));
            }
        }
    }
    sort_by_key(&mut all, |a| a.0);
    all.into_iter().take(5).map(|(_, c)| c).collect()
}

/// Three finders as a code's top-left, top-right and bottom-left, and how
/// far they are from a square's (0: a square); `None`: no code's.
fn shape(p: [Finder; 3]) -> Option<(f64, [Finder; 3])> {
    let (lo, hi) = p.iter().fold((f64::MAX, 0f64), |(lo, hi), f| (lo.min(f.size), hi.max(f.size)));
    // The corner opposite the longest side is the top-left.
    let side = |i: usize| dist(p[(i + 1) % 3].at, p[(i + 2) % 3].at);
    let tl = (0..3).max_by(|&a, &b| side(a).total_cmp(&side(b)))?;
    let (top_left, mut tr, mut bl) = (p[tl], p[(tl + 1) % 3], p[(tl + 2) % 3]);
    let (o, a, b) = (top_left.at, tr.at, bl.at);
    // Down the picture, the top-right is clockwise of the bottom-left.
    if (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0) < 0.0 {
        (tr, bl) = (bl, tr);
    }
    let (a, b, c) = (dist(o, tr.at), dist(o, bl.at), side(tl));
    let skew = (a - b).abs() / a.max(b);
    let corner = (c * c - a * a - b * b).abs() / (c * c);
    let ok = hi <= lo * 2.0 && a.min(b) >= 10.0 * lo && skew < 0.5 && corner < 0.5;
    ok.then(|| (skew + corner + hi / lo - 1.0, [top_left, tr, bl]))
}

/// The text of a code with these corners: its version guessed from their
/// distance, then its neighbours; from version 7 the code says its own.
fn read(img: &Bits) -> Option<String> {
    for c in corners(&finders(img)) {
        let size = (c[0].size + c[1].size + c[2].size) / 3.0;
        let span = (dist(c[0].at, c[1].at) + dist(c[0].at, c[2].at)) / 2.0 / size;
        let guess = ((span - 10.0) / 4.0 + 0.5).floor() as i64;
        for v in [guess, guess - 1, guess + 1].into_iter().filter(|v| (1..=40).contains(v)) {
            let Some(g) = sample(img, c, size, v as usize) else { continue };
            let g = match version(&g) {
                Some(said) if said != v as usize => match sample(img, c, size, said) {
                    Some(g) if version(&g) == Some(said) => g,
                    _ => continue,
                },
                _ => g,
            };
            if let Some(t) = decode_grid(&g) {
                return Some(t);
            }
        }
    }
    None
}

/// A code's modules, row by row (dark: `true`), `n` on a side.
struct Grid {
    n: usize,
    m: Vec<bool>,
}

impl Grid {
    fn at(&self, x: usize, y: usize) -> bool {
        self.m[y * self.n + x]
    }
}

/// Version `v`'s modules read off the picture: the finders' centres and
/// the bottom-right alignment pattern (or, with none, the corner they
/// imply) fix the perspective.
fn sample(img: &Bits, [tl, tr, bl]: [Finder; 3], size: f64, v: usize) -> Option<Grid> {
    let n = 17 + 4 * v;
    let (s, apart) = (n as f64, n as f64 - 7.0);
    let u = ((tr.at.0 - tl.at.0) / apart, (tr.at.1 - tl.at.1) / apart);
    let w = ((bl.at.0 - tl.at.0) / apart, (bl.at.1 - tl.at.1) / apart);
    let flat =
        |x: f64, y: f64| (tl.at.0 + (x - 3.5) * u.0 + (y - 3.5) * w.0, tl.at.1 + (x - 3.5) * u.1 + (y - 3.5) * w.1);
    let (corner, at) = match (v >= 2).then(|| alignment(img, flat(s - 6.5, s - 6.5), u, w, size)).flatten() {
        Some(a) => ((s - 6.5, s - 6.5), a),
        None => ((s - 3.5, s - 3.5), flat(s - 3.5, s - 3.5)),
    };
    let h = Homography::between([(3.5, 3.5), (s - 3.5, 3.5), corner, (3.5, s - 3.5)], [tl.at, tr.at, at, bl.at])?;
    let mut m = Vec::with_capacity(n * n);
    for y in 0..n {
        for x in 0..n {
            let (px, py) = h.map((x as f64 + 0.5, y as f64 + 0.5));
            // A little past the edge is the edge; further, the code is cut.
            if !(px > -size && py > -size && px < img.w as f64 + size && py < img.h as f64 + size) {
                return None;
            }
            m.push(img.at(px.clamp(0.0, img.w as f64 - 1.0), py.clamp(0.0, img.h as f64 - 1.0)) == Some(true));
        }
    }
    Some(Grid { n, m })
}

/// The alignment pattern's centre nearest `guess`: a dark module ringed by
/// light then dark ones along the code's axes `u`, `w` (a module each).
fn alignment(img: &Bits, guess: Pt, u: Pt, w: Pt, size: f64) -> Option<Pt> {
    let is = |p: Pt, i: f64, j: f64, dark: bool| img.at(p.0 + i * u.0 + j * w.0, p.1 + i * u.1 + j * w.1) == Some(dark);
    let ring = [(-1.0, -1.0), (0.0, -1.0), (1.0, -1.0), (-1.0, 0.0), (1.0, 0.0), (-1.0, 1.0), (0.0, 1.0), (1.0, 1.0)];
    let fits =
        |p: Pt| is(p, 0.0, 0.0, true) && ring.iter().all(|&(i, j)| is(p, i, j, false) && is(p, 2.0 * i, 2.0 * j, true));
    let mean = |ps: &[Pt], c: Pt| {
        let near: Vec<&Pt> = ps.iter().filter(|p| (p.0 - c.0).abs() <= size && (p.1 - c.1).abs() <= size).collect();
        let k = near.len() as f64;
        (near.iter().map(|p| p.0).sum::<f64>() / k, near.iter().map(|p| p.1).sum::<f64>() / k)
    };
    for reach in [4.0, 12.0] {
        let r = (reach * size).ceil() as i64;
        let (gx, gy) = (guess.0 as i64, guess.1 as i64);
        let mut hits = vec![];
        for y in gy - r..=gy + r {
            for x in gx - r..=gx + r {
                let p = (x as f64 + 0.5, y as f64 + 0.5);
                if fits(p) {
                    hits.push(p);
                }
            }
        }
        if let Some(&best) = hits.iter().min_by(|a, b| dist(**a, guess).total_cmp(&dist(**b, guess))) {
            // The hits around the nearest are one pattern's: their mean, twice.
            return Some(mean(&hits, mean(&hits, best)));
        }
    }
    None
}

/// A plane's perspective map: (x, y, 1) times the matrix.
#[derive(Clone, Copy)]
struct Homography([[f64; 3]; 3]);

impl Homography {
    /// The map taking the unit square's (0,0), (1,0), (1,1), (0,1) to `q`.
    fn square_to(q: [Pt; 4]) -> Option<Self> {
        let [(x0, y0), (x1, y1), (x2, y2), (x3, y3)] = q;
        let (sx, sy) = (x0 - x1 + x2 - x3, y0 - y1 + y2 - y3);
        let (dx1, dx2, dy1, dy2) = (x1 - x2, x3 - x2, y1 - y2, y3 - y2);
        let den = dx1 * dy2 - dx2 * dy1;
        if den.abs() < 1e-12 {
            return None;
        }
        let (g, h) = ((sx * dy2 - dx2 * sy) / den, (dx1 * sy - sx * dy1) / den);
        Some(Self([[x1 - x0 + g * x1, x3 - x0 + h * x3, x0], [y1 - y0 + g * y1, y3 - y0 + h * y3, y0], [g, h, 1.0]]))
    }

    /// The map taking each point of `from` to its point in `to`.
    fn between(from: [Pt; 4], to: [Pt; 4]) -> Option<Self> {
        let (a, b) = (Self::square_to(to)?.0, Self::square_to(from)?.inverse()?.0);
        Some(Self(std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum()))))
    }

    /// The inverse, up to scale (all a perspective map needs).
    fn inverse(self) -> Option<Self> {
        let [[a, b, c], [d, e, f], [g, h, i]] = self.0;
        let (ca, cb, cc) = (e * i - f * h, f * g - d * i, d * h - e * g);
        let det = a * ca + b * cb + c * cc;
        let ok = det.abs() > 1e-12;
        ok.then_some({
            Self([
                [ca, c * h - b * i, b * f - c * e],
                [cb, a * i - c * g, c * d - a * f],
                [cc, b * g - a * h, a * e - b * d],
            ])
        })
    }

    fn map(&self, (x, y): Pt) -> Pt {
        let m = &self.0;
        let z = m[2][0] * x + m[2][1] * y + m[2][2];
        ((m[0][0] * x + m[0][1] * y + m[0][2]) / z, (m[1][0] * x + m[1][1] * y + m[1][2]) / z)
    }
}

/// The text of a code read module by module.
fn decode_grid(g: &Grid) -> Option<String> {
    let v = (g.n.checked_sub(17)?) / 4;
    if g.n != 17 + 4 * v || !(1..=40).contains(&v) || (v >= 7 && version(g) != Some(v)) {
        return None;
    }
    let (level, mask) = format(g)?;
    let data = correct(&codewords(g, &functions(v), mask), v, level)?;
    text(&data, v)
}

/// The error-correction levels' 2 format bits, by level (L, M, Q, H).
const LEVEL_BITS: [u32; 4] = [1, 0, 3, 2];

/// A format's 15 bits: level and mask, BCH-coded and masked.
fn format_bits(level: usize, mask: usize) -> u32 {
    let d = LEVEL_BITS[level] << 3 | mask as u32;
    let mut r = d;
    for _ in 0..10 {
        r = (r << 1) ^ ((r >> 9) * 0x537);
    }
    ((d << 10) | r) ^ 0x5412
}

/// A version's 18 bits (from 7), BCH-coded.
fn version_bits(v: usize) -> u32 {
    let mut r = v as u32;
    for _ in 0..12 {
        r = (r << 1) ^ ((r >> 11) * 0x1F25);
    }
    (v as u32) << 12 | r
}

/// Where format bit `i` (0..15) sits, in its first and its second copy.
fn format_at(i: usize, n: usize) -> [(usize, usize); 2] {
    let first = match i {
        0..=5 => (8, i),
        6 => (8, 7),
        7 => (8, 8),
        8 => (7, 8),
        _ => (14 - i, 8),
    };
    [first, if i < 8 { (n - 1 - i, 8) } else { (8, n - 15 + i) }]
}

/// Where version bit `i` (0..18) sits, top-right and bottom-left.
fn version_at(i: usize, n: usize) -> [(usize, usize); 2] {
    let (a, b) = (n - 11 + i % 3, i / 3);
    [(a, b), (b, a)]
}

/// The level and mask the format bits say: the nearest valid format to
/// either copy, 3 wrong bits at most.
fn format(g: &Grid) -> Option<(usize, usize)> {
    let copy =
        |k: usize| (0..15).fold(0u32, |a, i| a | (g.at(format_at(i, g.n)[k].0, format_at(i, g.n)[k].1) as u32) << i);
    let (a, b) = (copy(0), copy(1));
    let off = |f: u32| (a ^ f).count_ones().min((b ^ f).count_ones());
    let best = (0..32).min_by_key(|&i| off(format_bits(i / 8, i % 8)))?;
    (off(format_bits(best / 8, best % 8)) <= 3).then_some((best / 8, best % 8))
}

/// The version a grid's version bits say (from 7), 3 wrong bits at most.
fn version(g: &Grid) -> Option<usize> {
    if g.n < 45 {
        return None;
    }
    let copy =
        |k: usize| (0..18).fold(0u32, |a, i| a | (g.at(version_at(i, g.n)[k].0, version_at(i, g.n)[k].1) as u32) << i);
    let (a, b) = (copy(0), copy(1));
    let off = |v: usize| (a ^ version_bits(v)).count_ones().min((b ^ version_bits(v)).count_ones());
    (7..=40).map(|v| (off(v), v)).filter(|(e, _)| *e <= 3).min().map(|(_, v)| v)
}

/// Version `v`'s alignment patterns' centres, on either axis.
fn alignments(v: usize) -> Vec<usize> {
    if v == 1 {
        return vec![];
    }
    let (k, n) = (v / 7 + 2, 17 + 4 * v);
    let step = (v * 8 + k * 3 + 5) / (k * 4 - 4) * 2;
    let mut at: Vec<usize> = (0..k - 1).map(|i| n - 7 - i * step).collect();
    at.push(6);
    at.reverse();
    at
}

/// Version `v`'s modules that carry no data: finders and their separators,
/// timing, alignment, format, version, the dark module.
fn functions(v: usize) -> Vec<bool> {
    let n = 17 + 4 * v;
    let mut f = vec![false; n * n];
    let mut fill = |x0: usize, y0: usize, w: usize, h: usize| {
        for y in y0..y0 + h {
            f[y * n + x0..y * n + x0 + w].fill(true);
        }
    };
    fill(0, 6, n, 1);
    fill(6, 0, 1, n);
    fill(0, 0, 9, 9);
    fill(n - 8, 0, 8, 9);
    fill(0, n - 8, 9, 8);
    let at = alignments(v);
    let last = at.len().saturating_sub(1);
    for (i, &y) in at.iter().enumerate() {
        for (j, &x) in at.iter().enumerate() {
            if !matches!((i, j), (0, 0)) && (i, j) != (0, last) && (i, j) != (last, 0) {
                fill(x - 2, y - 2, 5, 5);
            }
        }
    }
    if v >= 7 {
        fill(n - 11, 0, 3, 6);
        fill(0, n - 11, 6, 3);
    }
    f
}

/// Is data module (`x`, `y`) flipped by `mask`?
fn masked(mask: usize, x: usize, y: usize) -> bool {
    match mask {
        0 => (x + y).is_multiple_of(2),
        1 => y.is_multiple_of(2),
        2 => x.is_multiple_of(3),
        3 => (x + y).is_multiple_of(3),
        4 => (x / 3 + y / 2).is_multiple_of(2),
        5 => x * y % 2 + x * y % 3 == 0,
        6 => (x * y % 2 + x * y % 3).is_multiple_of(2),
        _ => ((x + y) % 2 + x * y % 3).is_multiple_of(2),
    }
}

/// The data modules in reading order: up and down two columns at a time,
/// from the right, around the timing column.
fn order(n: usize, func: &[bool]) -> Vec<(usize, usize)> {
    let mut out = Vec::with_capacity(n * n);
    let mut right = n - 1;
    loop {
        if right == 6 {
            right = 5;
        }
        for vert in 0..n {
            for x in [right, right - 1] {
                let y = if (right + 1) & 2 == 0 { n - 1 - vert } else { vert };
                if !func[y * n + x] {
                    out.push((x, y));
                }
            }
        }
        if right < 3 {
            return out;
        }
        right -= 2;
    }
}

/// The codewords, unmasked; bits past the last whole one dropped.
fn codewords(g: &Grid, func: &[bool], mask: usize) -> Vec<u8> {
    let bits: Vec<bool> = order(g.n, func).into_iter().map(|(x, y)| g.at(x, y) ^ masked(mask, x, y)).collect();
    bits.as_chunks::<8>().0.iter().map(|b| b.iter().fold(0u8, |a, &d| a << 1 | d as u8)).collect()
}

/// Error-correction codewords per block, by level (L, M, Q, H) and version.
const ECC: [[u8; 41]; 4] = [
    [
        0, 7, 10, 15, 20, 26, 18, 20, 24, 30, 18, 20, 24, 26, 30, 22, 24, 28, 30, 28, 28, 28, 28, 30, 30, 26, 28, 30,
        30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30,
    ],
    [
        0, 10, 16, 26, 18, 24, 16, 18, 22, 22, 26, 30, 22, 22, 24, 24, 28, 28, 26, 26, 26, 26, 28, 28, 28, 28, 28, 28,
        28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28,
    ],
    [
        0, 13, 22, 18, 26, 18, 24, 18, 22, 20, 24, 28, 26, 24, 20, 30, 24, 28, 28, 26, 30, 28, 30, 30, 30, 30, 28, 30,
        30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30,
    ],
    [
        0, 17, 28, 22, 16, 22, 28, 26, 26, 24, 28, 24, 28, 22, 24, 24, 30, 28, 28, 26, 28, 30, 24, 30, 30, 30, 30, 30,
        30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30,
    ],
];

/// Blocks, by level (L, M, Q, H) and version.
const BLOCKS: [[u8; 41]; 4] = [
    [
        0, 1, 1, 1, 1, 1, 2, 2, 2, 2, 4, 4, 4, 4, 4, 6, 6, 6, 6, 7, 8, 8, 9, 9, 10, 12, 12, 12, 13, 14, 15, 16, 17, 18,
        19, 19, 20, 21, 22, 24, 25,
    ],
    [
        0, 1, 1, 1, 2, 2, 4, 4, 4, 5, 5, 5, 8, 9, 9, 10, 10, 11, 13, 14, 16, 17, 17, 18, 20, 21, 23, 25, 26, 28, 29,
        31, 33, 35, 37, 38, 40, 43, 45, 47, 49,
    ],
    [
        0, 1, 1, 2, 2, 4, 4, 6, 6, 8, 8, 8, 10, 12, 16, 12, 17, 16, 18, 21, 20, 23, 23, 25, 27, 29, 34, 34, 35, 38, 40,
        43, 45, 48, 51, 53, 56, 59, 62, 65, 68,
    ],
    [
        0, 1, 1, 2, 4, 4, 4, 5, 6, 8, 8, 11, 11, 16, 16, 18, 16, 19, 21, 25, 25, 25, 34, 30, 32, 35, 37, 40, 42, 45,
        48, 51, 54, 57, 60, 63, 66, 70, 74, 77, 81,
    ],
];

/// The data codewords: `raw` split into its blocks (the short ones first;
/// the data interleaved, then the error correction), each corrected.
fn correct(raw: &[u8], v: usize, level: usize) -> Option<Vec<u8>> {
    let (blocks, ecc) = (BLOCKS[level][v] as usize, ECC[level][v] as usize);
    let short = blocks - raw.len() % blocks;
    let data_short = (raw.len() / blocks).checked_sub(ecc)?;
    let mut bl: Vec<Vec<u8>> = vec![Vec::with_capacity(data_short + 1 + ecc); blocks];
    let mut next = raw.iter().copied();
    for i in 0..=data_short {
        for (j, b) in bl.iter_mut().enumerate() {
            if i < data_short || j >= short {
                b.push(next.next()?);
            }
        }
    }
    for _ in 0..ecc {
        for b in &mut bl {
            b.push(next.next()?);
        }
    }
    let mut data = Vec::with_capacity(raw.len());
    for mut b in bl {
        if !reed_solomon(&mut b, ecc) {
            return None;
        }
        data.extend_from_slice(&b[..b.len() - ecc]);
    }
    Some(data)
}

/// GF(256) over x⁸ + x⁴ + x³ + x² + 1: powers of 2, and logarithms.
static GF: ([u8; 256], [u8; 256]) = {
    let (mut exp, mut log) = ([0u8; 256], [0u8; 256]);
    let (mut x, mut i) = (1u16, 0);
    while i < 255 {
        exp[i] = x as u8;
        log[x as usize] = i as u8;
        x <<= 1;
        if x & 0x100 != 0 {
            x ^= 0x11D;
        }
        i += 1;
    }
    (exp, log)
};

/// 2 to the `i`.
fn pow2(i: usize) -> u8 {
    GF.0[i % 255]
}

fn mul(a: u8, b: u8) -> u8 {
    if a == 0 || b == 0 { 0 } else { pow2(GF.1[a as usize] as usize + GF.1[b as usize] as usize) }
}

/// `a / b`, `b` not 0.
fn div(a: u8, b: u8) -> u8 {
    if a == 0 { 0 } else { pow2(GF.1[a as usize] as usize + 255 - GF.1[b as usize] as usize) }
}

/// A polynomial (lowest power first) at `x`.
fn eval(p: &[u8], x: u8) -> u8 {
    p.iter().rev().fold(0, |a, &c| mul(a, x) ^ c)
}

/// Correct block `b` (data, then `ecc` codewords; the first is the highest
/// power) in place: syndromes, Berlekamp–Massey, Chien, Forney. `false`:
/// more errors than it can mend.
fn reed_solomon(b: &mut [u8], ecc: usize) -> bool {
    let n = b.len();
    let syndromes = |b: &[u8]| -> Vec<u8> { (0..ecc).map(|i| b.iter().fold(0, |a, &c| mul(a, pow2(i)) ^ c)).collect() };
    let s = syndromes(b);
    if s.iter().all(|&x| x == 0) {
        return true;
    }
    // The error locator Λ, with Λ(0) = 1.
    let (mut lam, mut prev) = (vec![1u8], vec![1u8]);
    let (mut l, mut m, mut last) = (0, 1, 1u8);
    for i in 0..ecc {
        let d = (1..=l).fold(s[i], |d, j| d ^ mul(*lam.get(j).unwrap_or(&0), s[i - j]));
        if d == 0 {
            m += 1;
            continue;
        }
        let was = lam.clone();
        let k = div(d, last);
        if lam.len() < prev.len() + m {
            lam.resize(prev.len() + m, 0);
        }
        for (j, &p) in prev.iter().enumerate() {
            lam[j + m] ^= mul(k, p);
        }
        if 2 * l <= i {
            (l, prev, last, m) = (i + 1 - l, was, d, 1);
        } else {
            m += 1;
        }
    }
    while lam.len() > 1 && lam.last() == Some(&0) {
        lam.pop();
    }
    if lam.len() - 1 != l || 2 * l > ecc {
        return false;
    }
    // The evaluator Ω = S·Λ mod x^ecc.
    let mut om = vec![0u8; ecc];
    for (i, &si) in s.iter().enumerate() {
        for (j, &lj) in lam.iter().enumerate().take(ecc - i) {
            om[i + j] ^= mul(si, lj);
        }
    }
    let mut fixed = 0;
    for (k, c) in b.iter_mut().enumerate() {
        let p = (n - 1 - k) % 255;
        let inv = pow2(255 - p);
        if eval(&lam, inv) != 0 {
            continue;
        }
        // Λ' at X⁻¹: its odd terms.
        let slope = (1..lam.len()).step_by(2).fold(0, |a, j| a ^ mul(lam[j], pow2((255 - p) * (j - 1))));
        if slope == 0 {
            return false;
        }
        *c ^= mul(pow2(p), div(eval(&om, inv), slope));
        fixed += 1;
    }
    fixed == l && syndromes(b).iter().all(|&x| x == 0)
}

/// Bits read off the front of bytes.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn left(&self) -> usize {
        self.bytes.len() * 8 - self.at
    }

    fn take(&mut self, n: usize) -> Option<u32> {
        if n > self.left() {
            return None;
        }
        let v = (self.at..self.at + n).fold(0u32, |a, i| a << 1 | (self.bytes[i / 8] >> (7 - i % 8) & 1) as u32);
        self.at += n;
        Some(v)
    }
}

const ALNUM: &[u8; 45] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ $%*+-./:";

/// The text the data codewords carry, segment by segment.
fn text(data: &[u8], v: usize) -> Option<String> {
    let mut r = Reader { bytes: data, at: 0 };
    let (mut out, mut latin1) = (Vec::new(), false);
    let wide = usize::from(v > 9) + usize::from(v > 26);
    while r.left() >= 4 {
        match r.take(4)? {
            0 => break,
            1 => {
                let mut k = r.take([10, 12, 14][wide])?;
                for (digits, bits, max) in [(3, 10, 1000), (2, 7, 100), (1, 4, 10)] {
                    while k >= digits {
                        let d = r.take(bits)?;
                        if d >= max {
                            return None;
                        }
                        out.extend((0..digits).rev().map(|k| b'0' + (d / 10u32.pow(k) % 10) as u8));
                        k -= digits;
                    }
                }
            }
            2 => {
                let mut k = r.take([9, 11, 13][wide])?;
                while k >= 2 {
                    let p = r.take(11)? as usize;
                    out.extend([*ALNUM.get(p / 45)?, ALNUM[p % 45]]);
                    k -= 2;
                }
                if k == 1 {
                    out.push(*ALNUM.get(r.take(6)? as usize)?);
                }
            }
            4 => {
                for _ in 0..r.take([8, 16, 16][wide])? {
                    out.push(r.take(8)? as u8);
                }
            }
            7 => {
                let b = r.take(8)?;
                let eci = match b {
                    0..=0x7F => b,
                    0x80..=0xBF => (b & 0x3F) << 8 | r.take(8)?,
                    0xC0..=0xDF => (b & 0x1F) << 16 | r.take(16)?,
                    _ => return None,
                };
                // ISO-8859-1 (1, 3) or UTF-8 (26); others are read as UTF-8.
                latin1 = matches!(eci, 1 | 3);
            }
            // A structured append's header, FNC1 (GS1) and its application id.
            3 => _ = r.take(16)?,
            5 => {}
            9 => _ = r.take(8)?,
            _ => return None,
        }
    }
    let as_latin1 = |b: &[u8]| b.iter().map(|&c| c as char).collect::<String>();
    let t = match latin1 {
        true => as_latin1(&out),
        false => String::from_utf8(out).unwrap_or_else(|e| as_latin1(e.as_bytes())),
    };
    (!t.is_empty()).then_some(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A segment of a code's text.
    enum Seg<'a> {
        Num(&'a str),
        Alnum(&'a str),
        Bytes(&'a [u8]),
        Eci(u32),
    }

    /// Bits written high first.
    #[derive(Default)]
    struct Out(Vec<bool>);

    impl Out {
        fn put(&mut self, v: u32, n: usize) {
            self.0.extend((0..n).rev().map(|i| v >> i & 1 == 1));
        }
    }

    /// The error-correction codewords of `data`: the remainder by the
    /// generator with roots 2⁰..2^(ecc-1).
    fn ecc_of(data: &[u8], ecc: usize) -> Vec<u8> {
        let mut g = vec![0u8; ecc];
        g[ecc - 1] = 1;
        let mut root = 1u8;
        for _ in 0..ecc {
            for j in 0..ecc {
                g[j] = mul(g[j], root) ^ g.get(j + 1).copied().unwrap_or(0);
            }
            root = mul(root, 2);
        }
        let mut r = vec![0u8; ecc];
        for &b in data {
            let f = b ^ r.remove(0);
            r.push(0);
            r.iter_mut().zip(&g).for_each(|(x, &y)| *x ^= mul(y, f));
        }
        r
    }

    /// The code an encoder draws for `segs` (version `v`, level L, M, Q, H
    /// as 0..4, `mask`), its padding and every function pattern included.
    fn encode(segs: &[Seg], v: usize, level: usize, mask: usize) -> Grid {
        let wide = usize::from(v > 9) + usize::from(v > 26);
        let mut o = Out::default();
        let at = |b: u8| ALNUM.iter().position(|&a| a == b).unwrap() as u32;
        for s in segs {
            match s {
                Seg::Num(t) => {
                    o.put(1, 4);
                    o.put(t.len() as u32, [10, 12, 14][wide]);
                    for c in t.as_bytes().chunks(3) {
                        o.put(std::str::from_utf8(c).unwrap().parse().unwrap(), c.len() * 3 + 1);
                    }
                }
                Seg::Alnum(t) => {
                    o.put(2, 4);
                    o.put(t.len() as u32, [9, 11, 13][wide]);
                    for c in t.as_bytes().chunks(2) {
                        match c {
                            [a, b] => o.put(at(*a) * 45 + at(*b), 11),
                            [a] => o.put(at(*a), 6),
                            _ => unreachable!(),
                        }
                    }
                }
                Seg::Bytes(b) => {
                    o.put(4, 4);
                    o.put(b.len() as u32, [8, 16, 16][wide]);
                    b.iter().for_each(|&x| o.put(x as u32, 8));
                }
                Seg::Eci(e) => {
                    o.put(7, 4);
                    o.put(*e, 8);
                }
            }
        }
        let func = functions(v);
        let total = func.iter().filter(|f| !**f).count() / 8;
        let (blocks, ecc) = (BLOCKS[level][v] as usize, ECC[level][v] as usize);
        let cap = total - blocks * ecc;
        assert!(o.0.len() <= cap * 8, "{} bits: too long for version {v}", o.0.len());
        o.put(0, (cap * 8 - o.0.len()).min(4));
        o.0.resize(o.0.len().next_multiple_of(8), false);
        let mut data: Vec<u8> = o.0.chunks(8).map(|b| b.iter().fold(0, |a, &d| a << 1 | d as u8)).collect();
        data.extend([0xEC, 0x11].into_iter().cycle().take(cap - data.len()));
        let (short, data_short) = (blocks - total % blocks, total / blocks - ecc);
        let mut bl = vec![];
        let mut from = 0;
        for j in 0..blocks {
            let d = data[from..from + data_short + usize::from(j >= short)].to_vec();
            from += d.len();
            bl.push((ecc_of(&d, ecc), d));
        }
        let mut raw = vec![];
        for i in 0..=data_short {
            for (j, (_, d)) in bl.iter().enumerate() {
                if i < data_short || j >= short {
                    raw.push(d[i]);
                }
            }
        }
        for i in 0..ecc {
            raw.extend(bl.iter().map(|(e, _)| e[i]));
        }
        let n = 17 + 4 * v;
        let mut m = vec![false; n * n];
        let mut set = |x: i64, y: i64, d: bool| {
            if (0..n as i64).contains(&x) && (0..n as i64).contains(&y) {
                m[y as usize * n + x as usize] = d;
            }
        };
        for i in 0..n as i64 {
            set(6, i, i % 2 == 0);
            set(i, 6, i % 2 == 0);
        }
        let square = |set: &mut dyn FnMut(i64, i64, bool), (cx, cy): (i64, i64), r: i64, dark: &dyn Fn(i64) -> bool| {
            for dy in -r..=r {
                for dx in -r..=r {
                    set(cx + dx, cy + dy, dark(dx.abs().max(dy.abs())));
                }
            }
        };
        let k = n as i64 - 4;
        for c in [(3, 3), (k, 3), (3, k)] {
            square(&mut set, c, 4, &|d| d != 2 && d != 4);
        }
        let al = alignments(v);
        for (i, &y) in al.iter().enumerate() {
            for (j, &x) in al.iter().enumerate() {
                let last = al.len() - 1;
                if (i, j) != (0, 0) && (i, j) != (0, last) && (i, j) != (last, 0) {
                    square(&mut set, (x as i64, y as i64), 2, &|d| d != 1);
                }
            }
        }
        let f = format_bits(level, mask);
        for i in 0..15 {
            format_at(i, n).iter().for_each(|&(x, y)| set(x as i64, y as i64, f >> i & 1 == 1));
        }
        set(8, n as i64 - 8, true);
        if v >= 7 {
            let b = version_bits(v);
            for i in 0..18 {
                version_at(i, n).iter().for_each(|&(x, y)| set(x as i64, y as i64, b >> i & 1 == 1));
            }
        }
        let bits = raw.iter().flat_map(|&b| (0..8).rev().map(move |i| b >> i & 1 == 1)).chain(std::iter::repeat(false));
        for ((x, y), d) in order(n, &func).into_iter().zip(bits) {
            set(x as i64, y as i64, d ^ masked(mask, x, y));
        }
        Grid { n, m }
    }

    /// `g` as a camera sees it: its corners (with 4 modules of quiet zone)
    /// at `quad` in a `w` × `h` picture, each pixel the mean of 9 samples,
    /// dark and light levels by place (`lit`), ± `noise`.
    fn shoot(
        g: &Grid,
        (w, h): (usize, usize),
        quad: [Pt; 4],
        lit: impl Fn(usize, usize) -> (f64, f64),
        noise: u32,
    ) -> Vec<u8> {
        let q = (g.n + 8) as f64;
        let to = Homography::between(quad, [(0.0, 0.0), (q, 0.0), (q, q), (0.0, q)]).unwrap();
        let mut seed = 7u32;
        let mut out = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let mut ink = 0.0;
                for s in 0..9 {
                    let (mx, my) = to.map((
                        x as f64 + (s % 3) as f64 / 3.0 + 1.0 / 6.0,
                        y as f64 + (s / 3) as f64 / 3.0 + 1.0 / 6.0,
                    ));
                    let (i, j) = (mx.floor() as i64 - 4, my.floor() as i64 - 4);
                    let inside = (0..g.n as i64).contains(&i) && (0..g.n as i64).contains(&j);
                    ink += f64::from(u8::from(inside && g.at(i as usize, j as usize))) / 9.0;
                }
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                let r = ((seed >> 16) % (2 * noise + 1)) as f64 - noise as f64;
                let (dark, light) = lit(x, y);
                out.push((light + (dark - light) * ink + r).clamp(0.0, 255.0) as u8);
            }
        }
        out
    }

    /// `g` square and upright, `px` pixels a module, black on white.
    fn plain(g: &Grid, px: f64) -> (Vec<u8>, usize) {
        let side = ((g.n + 8) as f64 * px) as usize + 10;
        let (a, b) = (5.0, 5.0 + (g.n + 8) as f64 * px);
        (shoot(g, (side, side), [(a, a), (b, a), (b, b), (a, b)], |_, _| (0.0, 255.0), 0), side)
    }

    /// `quad` turned `deg` degrees about (`cx`, `cy`).
    fn turned(quad: [Pt; 4], deg: f64, (cx, cy): Pt) -> [Pt; 4] {
        let (s, c) = deg.to_radians().sin_cos();
        quad.map(|(x, y)| (cx + (x - cx) * c - (y - cy) * s, cy + (x - cx) * s + (y - cy) * c))
    }

    const URI: &str = "solana:7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU?amount=1.5&label=Caf%C3%A9";

    /// Version 7, level M, numeric, alphanumeric and byte segments and a
    /// mask another encoder (python-qrcode 8) chose.
    const OTHERS: [&str; 45] = [
        "#######..#.#..##..###.#.#####.###...#.#######",
        "#.....#...#....#..#.####....####.#.#..#.....#",
        "#.###.#.##.#.#.#.####.#......#.#...#..#.###.#",
        "#.###.#.#..######.#..#...#.#.......##.#.###.#",
        "#.###.#.#......##########....#.#.####.#.###.#",
        "#.....#.#...##.###.##...##.#..#..#....#.....#",
        "#######.#.#.#.#.#.#.#.#.#.#.#.#.#.#.#.#######",
        "........######.#...##...#.#...#..#.#.........",
        "#.#####..#.##.##.#.########.#####..##.#####..",
        "##...#.#.###....#.#..#.##.#.#.#.#.#.###...#.#",
        ".###..#..#.##.##.#.#....##.#.#########..##..#",
        "#.#.##...####.#..#...#.#####.#...##.#.##.###.",
        "#####.########.#.#.#....#.#.#....#..#...###..",
        "...........#..#####...#.######...#.###.###...",
        "..##.####.#..##...#..###....#.#...#..#..#..##",
        ".###....##...#..#####.#........##.#..##..#..#",
        "#.#####.#...#..#..#.##.#.#.#.#.######.##..###",
        ".####..###.##.#.#####.###....#.#..#.###...##.",
        ".##.###.#..#...#.#.#....##.#.#.#.#.#...#.##..",
        "..####.###.##.#.#......#####..#..#........##.",
        "#..#######...#.#.#.######.#.#####..########..",
        "....#...###.#.....#.#...#####.#.##..#...#..#.",
        "#.###.#.#.####..#.#.#.#.#...#####...#.#.#..#.",
        "##..#...##..#.#.###.#...#..#.#.#.##.#...####.",
        ".#########....###.#.######.##......######....",
        ".#.###.###.#..##.##..#..#.####.#....##..#.###",
        "#...###.#.###..#....##..#..#..#.....#..#.#...",
        ".#...#.#.####.#.#.............#..##..#.###...",
        "###.####...####..###.#.###.#.#####.##...#.###",
        "....#..##.##.#..##..#...#.....#..###..##..##.",
        "#..##.##.##.#####..#..#.##.#.###.####.#####..",
        ".##..#..#.#..##.########.###..####.#.#....##.",
        ".#...##########.##.##.#...#.###....##.#..##..",
        ".#.###..#.##.........#.#.####.##.#..##.#.#...",
        "....#.#..#.###....#.##.##...#..#......#....##",
        ".####..#.#..##.##.#.....#....#.#..######.#..#",
        "#..##.##.##..#.##...######.#.....##.#####.###",
        "........###..##..#..#...#....#.#..###...####.",
        "#######..###..#.....#.#.##.#..#..#.##.#.####.",
        "#.....#.##.###.#.####...#.##..#..#.##...#####",
        "#.###.#.#.#.#.##...######.########..#####.#..",
        "#.###.#.###..##.#...##.##.##..#.#...#..#.#...",
        "#.###.#.#.#####.#####...#..#######.#.#..##...",
        "#.....#...#.#..####.#.#.####.#..#......####.#",
        "#######.##.#.##.##.#...#..#.#.....##.#..###..",
    ];

    #[test]
    fn a_code_another_encoder_made_reads_from_its_modules_and_its_picture() {
        let g = Grid { n: 45, m: OTHERS.iter().flat_map(|r| r.bytes().map(|b| b == b'#')).collect() };
        let text = "HTTPS://EXAMPLE.ORG/PAY/12345678901234567890?MEMO=café ✓";
        assert_eq!(decode_grid(&g).as_deref(), Some(text));
        let shot = shoot(
            &g,
            (400, 300),
            turned([(90.0, 40.0), (310.0, 40.0), (310.0, 260.0), (90.0, 260.0)], -12.0, (200.0, 150.0)),
            |_, _| (30.0, 220.0),
            6,
        );
        assert_eq!(decode_qr(&shot, 400, 300).as_deref(), Some(text));
    }

    #[test]
    fn the_function_patterns_leave_the_standards_codeword_count_for_every_version() {
        for v in 1..=40 {
            // ISO/IEC 18004 by formula: all modules less the patterns'.
            let k = v / 7 + 2;
            let mut raw = (16 * v + 128) * v + 64;
            if v >= 2 {
                raw -= (25 * k - 10) * k - 55;
            }
            if v >= 7 {
                raw -= 36;
            }
            assert_eq!(functions(v).iter().filter(|f| !**f).count(), raw, "version {v}");
        }
        // Data codewords, from the standard's table.
        let data = |v: usize, l: usize| {
            (functions(v).iter().filter(|f| !**f).count() / 8) - BLOCKS[l][v] as usize * ECC[l][v] as usize
        };
        let want =
            [(1, [19, 16, 13, 9]), (7, [156, 124, 88, 66]), (10, [274, 216, 154, 122]), (40, [2956, 2334, 1666, 1276])];
        for (v, d) in want {
            assert_eq!([0, 1, 2, 3].map(|l| data(v, l)), d, "version {v}");
        }
        assert_eq!(alignments(32), [6, 34, 60, 86, 112, 138]);
        assert_eq!(alignments(36), [6, 24, 50, 76, 102, 128, 154]);
    }

    #[test]
    fn every_version_level_and_mask_reads_back_and_errors_up_to_half_the_ecc_are_mended() {
        for v in 1..=40 {
            for level in 0..4 {
                let cap = functions(v).iter().filter(|f| !**f).count() / 8
                    - BLOCKS[level][v] as usize * ECC[level][v] as usize;
                // Bytes filling the code: its count and mode take 3 bytes.
                let text: String = (0..cap - 3).map(|i| (b'a' + ((i * 7 + v + level) % 26) as u8) as char).collect();
                let g = encode(&[Seg::Bytes(text.as_bytes())], v, level, (v + level) % 8);
                assert_eq!(decode_grid(&g).as_deref(), Some(text.as_str()), "version {v} level {level}");
                // Half the ecc wrong in every block (whole codewords): mended.
                let mut bad = Grid { n: g.n, m: g.m.clone() };
                let (blocks, ecc) = (BLOCKS[level][v] as usize, ECC[level][v] as usize);
                let order = order(g.n, &functions(v));
                for word in 0..blocks * (ecc / 2) {
                    // Interleaved: codeword `word` of the ecc part is block `word % blocks`'s.
                    let first = (order.len() / 8 - blocks * ecc + word) * 8;
                    for &(x, y) in &order[first..first + 8] {
                        bad.m[y * g.n + x] ^= true;
                    }
                }
                assert_eq!(decode_grid(&bad).as_deref(), Some(text.as_str()), "version {v} level {level}, damaged");
            }
        }
    }

    #[test]
    fn numeric_alphanumeric_bytes_and_eci_segments_read_as_one_text() {
        let segs = [Seg::Num("0123456789"), Seg::Alnum("HELLO WORLD $%*+-./:"), Seg::Bytes(", ünï ✓".as_bytes())];
        for v in [1, 10, 27] {
            let g = encode(&segs[1..2], v, 0, 0);
            assert_eq!(decode_grid(&g).as_deref(), Some("HELLO WORLD $%*+-./:"), "version {v}");
        }
        let g = encode(&segs, 4, 1, 3);
        assert_eq!(decode_grid(&g).as_deref(), Some("0123456789HELLO WORLD $%*+-./:, ünï ✓"));
        for (num, v) in [("7", 1), ("42", 2), ("12345678901234", 11)] {
            assert_eq!(decode_grid(&encode(&[Seg::Num(num)], v, 0, 1)).as_deref(), Some(num));
        }
        // Latin-1 by its ECI, and bytes that aren't UTF-8 read as Latin-1.
        assert_eq!(decode_grid(&encode(&[Seg::Eci(3), Seg::Bytes(b"caf\xe9")], 1, 1, 2)).as_deref(), Some("café"));
        assert_eq!(decode_grid(&encode(&[Seg::Bytes(b"caf\xe9")], 1, 1, 2)).as_deref(), Some("café"));
        assert_eq!(
            decode_grid(&encode(&[Seg::Eci(26), Seg::Bytes("café".as_bytes())], 1, 1, 2)).as_deref(),
            Some("café")
        );
        assert_eq!(decode_grid(&encode(&[], 1, 0, 0)), None, "an empty code says nothing");
    }

    #[test]
    fn a_code_reads_off_a_picture_upright_turned_tilted_shaded_noisy_or_inverted() {
        let g = encode(&[Seg::Bytes(URI.as_bytes())], 6, 1, 5);
        let (pic, side) = plain(&g, 4.0);
        assert_eq!(decode_qr(&pic, side, side).as_deref(), Some(URI), "upright");
        let (w, h) = (480, 360);
        let sq = |x: f64, y: f64, s: f64| [(x, y), (x + s, y), (x + s, y + s), (x, y + s)];
        let shots: [(&str, [Pt; 4]); 6] = [
            ("small, 3 px a module", sq(100.0, 60.0, 49.0 * 3.0)),
            ("turned 90°", turned(sq(120.0, 60.0, 240.0), 90.0, (240.0, 180.0))),
            ("upside down", turned(sq(120.0, 60.0, 240.0), 180.0, (240.0, 180.0))),
            ("turned 33°", turned(sq(130.0, 70.0, 220.0), 33.0, (240.0, 180.0))),
            ("tilted away", [(150.0, 50.0), (330.0, 50.0), (380.0, 320.0), (100.0, 320.0)]),
            ("tilted and turned", [(110.0, 90.0), (320.0, 40.0), (370.0, 300.0), (140.0, 330.0)]),
        ];
        for (what, quad) in shots {
            let pic = shoot(&g, (w, h), quad, |_, _| (20.0, 230.0), 0);
            assert_eq!(decode_qr(&pic, w, h).as_deref(), Some(URI), "{what}");
        }
        // A lamp on the left: the right half is three times darker, and grainy.
        let shaded = |x: usize, _| (10.0 + 40.0 * (1.0 - x as f64 / 480.0), 70.0 + 180.0 * (1.0 - x as f64 / 480.0));
        let pic = shoot(&g, (w, h), sq(110.0, 50.0, 260.0), shaded, 12);
        assert_eq!(decode_qr(&pic, w, h).as_deref(), Some(URI), "shaded and noisy");
        // Light on dark (a dark theme's code).
        let pic = shoot(&g, (w, h), sq(110.0, 50.0, 260.0), |_, _| (235.0, 25.0), 0);
        assert_eq!(decode_qr(&pic, w, h).as_deref(), Some(URI), "inverted");
        // A big code, from version 7 on: its version bits say which.
        let big = encode(&[Seg::Bytes(&URI.as_bytes().repeat(6))], 22, 2, 6);
        let (pic, side) = plain(&big, 3.0);
        assert_eq!(decode_qr(&pic, side, side), Some(URI.repeat(6)), "version 22");
    }

    #[test]
    fn a_picture_with_no_whole_code_reads_as_none_and_never_panics() {
        assert_eq!(decode_qr(&[], 0, 0), None);
        assert_eq!(decode_qr(&[0; 10], 4, 4), None, "a short buffer");
        assert_eq!(decode_qr(&[255; 64 * 48], 64, 48), None, "blank");
        assert_eq!(decode_qr(&[0; 7], usize::MAX, 2), None, "a size that overflows");
        let g = encode(&[Seg::Bytes(URI.as_bytes())], 6, 1, 5);
        let (pic, side) = plain(&g, 4.0);
        // Cut in half: the finders are there, the code isn't.
        assert_eq!(decode_qr(&pic[..pic.len() / 2], side, side / 2), None);
        // Noise, and noise with finder-like blobs: nothing, no panic.
        let mut seed = 1u32;
        for size in [1, 2, 7, 9, 31, 64, 200] {
            let pic: Vec<u8> = (0..size * size)
                .map(|_| {
                    seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                    (seed >> 16) as u8
                })
                .collect();
            assert_eq!(decode_qr(&pic, size, size), None, "noise {size}");
        }
        let mut damaged = Grid { n: g.n, m: g.m.clone() };
        // Every codeword after the format area's rows wrong: beyond mending.
        damaged.m[g.n * 10..].iter_mut().for_each(|d| *d = !*d);
        assert_eq!(decode_grid(&damaged), None);
    }
}
