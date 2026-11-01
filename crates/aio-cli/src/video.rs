//! `ax shot <app> --video <file.mp4|.webm>` (aio's `am shot --video`): the
//! app's window recorded until Ctrl-C (or `--duration`), then encoded by
//! ffmpeg. Two phases: while recording, the frames are the JPEGs Chromium
//! paints anyway (its screencast), kept in a temp folder, so the app runs at
//! its own speed; ffmpeg runs after. Each frame lasts until the next one was
//! painted: a still window is a still video, at the time it really took.
//! Where no ffmpeg is, the window encodes them itself (aio's way: Chromium's
//! WebCodecs, `encode.js`) and `mux` writes the file: slower, never missing.
use crate::cdp::Session;
use aio::serde_json::{Value, json};
use aio::video::{encode, format_of};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// While a recording with `--markers` runs: the app's logs dir holds this
/// journal, and each `ax trigger` aimed at the app adds a line to it.
pub const JOURNAL: &str = "video.markers.jsonl";

/// Note a trigger (`path`, `action`, the element's `rect` in CSS px) in the
/// journal `file`, if a recording keeps one; one append per line, so
/// triggers at once never interleave.
pub fn note(file: &Path, path: &str, action: &str, rect: Option<Value>) {
    use std::io::Write;
    let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(file) else { return };
    let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    _ = f.write_all(format!("{}\n", json!({ "at": at, "path": path, "action": action, "rect": rect })).as_bytes());
}

/// The journal's lines as markers on the video's clock (pure): `t` ms after
/// `t0` (epoch ms of frame 0), within `duration` ms, rects in video pixels
/// (`scale` per CSS px), by time. A line that doesn't read (a trigger cut off
/// mid-write) or falls outside the video is left out.
pub fn markers(journal: &str, t0: u64, duration: u64, scale: f64) -> Value {
    let px = |v: &Value| (v.as_f64().unwrap_or(0.0) * scale).round();
    let mut out: Vec<Value> = journal
        .lines()
        .filter_map(|l| aio::serde_json::from_str::<Value>(l).ok())
        .filter_map(|n| {
            let t = n["at"].as_u64()?.checked_sub(t0).filter(|t| *t <= duration)?;
            let r = &n["rect"];
            let rect = r
                .is_object()
                .then(|| json!({ "x": px(&r["x"]), "y": px(&r["y"]), "w": px(&r["w"]), "h": px(&r["h"]) }));
            Some(json!({ "t": t, "path": n["path"], "action": n["action"], "rect": rect }))
        })
        .collect();
    out.sort_by_key(|m| m["t"].as_u64());
    Value::Array(out)
}

/// A frame's JPEG (`data`, base64) into `dir` as frame `n`, painted `at`.
fn keep(dir: &Path, n: usize, at: Duration, data: &Value) -> Result<(String, Duration), String> {
    use base64::Engine;
    let b = base64::engine::general_purpose::STANDARD
        .decode(data.as_str().unwrap_or_default())
        .map_err(|e| e.to_string())?;
    let file = format!("f{n:06}.jpg");
    std::fs::write(dir.join(&file), b).map_err(|e| e.to_string())?;
    Ok((file, at))
}

/// What a recording took: its frames, how long it ran, whether the window
/// went away, when frame 0 was (epoch ms) and the video px per CSS px.
type Taken = (Vec<(String, Duration)>, Duration, bool, u64, f64);

/// Record the app's window into `dir` until `until` passed (none: Ctrl-C)
/// or the window went away; the `journal` of its triggers opens at frame 0.
/// `announce`: once frame 0 is taken, `{"event":"recording","file","t0"}`
/// on stderr (`--json`), so a driver starts acting on the video's clock.
async fn record(
    at: &crate::cdp::Reach,
    dir: &Path,
    until: Option<Duration>,
    journal: Option<&Path>,
    announce: Option<&Path>,
) -> Result<Taken, String> {
    let mut s = crate::cdp::window(at, 0).await?;
    let scale = s.eval("devicePixelRatio").await?.as_f64().unwrap_or(1.0);
    let start = Instant::now();
    let t0 = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    if let Some(j) = journal {
        aio::paths::write_atomic(j, b"", true).map_err(|e| format!("{}: {e}", j.display()))?;
    }
    // Frame 0: the window as it is (a still window paints nothing new).
    let first = s.call("Page.captureScreenshot", json!({ "format": "jpeg", "quality": 85 })).await?;
    let mut frames = vec![keep(dir, 0, Duration::ZERO, &first["data"])?];
    if let Some(file) = announce {
        eprintln!("{}", json!({ "event": "recording", "file": file, "t0": t0 }));
    }
    s.call("Page.startScreencast", json!({ "format": "jpeg", "quality": 85 })).await?;
    let stop = async {
        match until {
            Some(d) => tokio::time::sleep(d).await,
            None => _ = tokio::signal::ctrl_c().await,
        }
    };
    tokio::pin!(stop);
    let lost = loop {
        tokio::select! {
            _ = &mut stop => break false,
            f = s.event("Page.screencastFrame", Duration::from_millis(250)) => match f {
                Ok(Some(p)) => {
                    frames.push(keep(dir, frames.len(), start.elapsed(), &p["data"])?);
                    if s.call("Page.screencastFrameAck", json!({ "sessionId": p["sessionId"] })).await.is_err() {
                        break true;
                    }
                }
                Ok(None) => {}
                Err(_) => break true,
            },
        }
    };
    let end = start.elapsed();
    if !lost {
        _ = s.call("Page.stopScreencast", json!({})).await;
    }
    Ok((frames, end, lost, t0, scale))
}

/// The window's encoder, where no ffmpeg is.
const ENCODER: &str = include_str!("encode.js");

/// One step of `encode.js` in the window: its answer, or its refusal.
async fn step(s: &mut Session, op: &str, arg: Value) -> Result<Value, String> {
    let r = s.eval(&format!("({ENCODER})({}, {arg})", json!(op))).await?;
    match r["error"].as_str() {
        Some(e) => Err(format!("the window's encoder: {e}")),
        None => Ok(r),
    }
}

/// The encoded frames an answer of `encode.js` carries: key or not, when
/// (µs), the bytes.
fn chunks(r: &Value) -> Result<Vec<(bool, u64, Vec<u8>)>, String> {
    use base64::Engine;
    let b64 = |v: &Value| base64::engine::general_purpose::STANDARD.decode(v.as_str().unwrap_or_default());
    r["chunks"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| Ok((c["key"] == true, c["us"].as_u64().unwrap_or(0), b64(&c["data"]).map_err(|e| e.to_string())?)))
        .collect()
}

/// The encoded frames, refused when no player could play them (aio
/// assertChunks): none, a first that is no key frame, or times that don't
/// rise.
fn playable(chunks: Vec<(bool, u64, Vec<u8>)>) -> Result<Vec<(bool, u64, Vec<u8>)>, String> {
    match chunks.first() {
        None => Err("the window's encoder gave no frames".into()),
        Some((false, ..)) => Err("the window's encoder began with no key frame".into()),
        _ if chunks.windows(2).any(|w| w[1].1 <= w[0].1) => Err("the window's encoder gave frames out of time".into()),
        _ => Ok(chunks),
    }
}

/// The encoded frames on the video's clock (pure): each in ms, lasting
/// until the next one (the last until `end`), a ms at least.
fn timed(chunks: Vec<(bool, u64, Vec<u8>)>, end: Duration) -> Vec<crate::mux::Frame> {
    let ats: Vec<u64> = chunks.iter().map(|(_, us, _)| us / 1000).collect();
    let end = end.as_millis() as u64;
    chunks
        .into_iter()
        .enumerate()
        .map(|(i, (key, _, data))| {
            let until = ats.get(i + 1).copied().unwrap_or(end);
            crate::mux::Frame { key, at: ats[i], dur: until.saturating_sub(ats[i]).max(1), data }
        })
        .collect()
}

/// `frames` (JPEGs in `dir`, painted at their times, the last shown until
/// `end`) encoded by the window `s` itself as `format`: the file's bytes.
/// A key frame every 2 s, so a player can seek.
pub async fn encode_in(
    s: &mut Session,
    dir: &Path,
    frames: &[(String, Duration)],
    end: Duration,
    format: &str,
) -> Result<Vec<u8>, String> {
    use base64::Engine;
    let jpeg = |f: &str| {
        let p = dir.join(f);
        std::fs::read(&p)
            .map(|b| base64::engine::general_purpose::STANDARD.encode(b))
            .map_err(|e| format!("{}: {e}", p.display()))
    };
    let (first, _) = frames.first().ok_or("no frames were recorded")?;
    let o = step(s, "open", json!({ "format": format, "data": jpeg(first)? })).await?;
    let (w, h) = (o["w"].as_u64().unwrap_or(0) as u32, o["h"].as_u64().unwrap_or(0) as u32);
    let (mut got, mut key_at) = (vec![], None);
    for (i, (file, at)) in frames.iter().enumerate() {
        let until = frames.get(i + 1).map_or(end, |(_, next)| *next);
        let key = key_at.is_none_or(|k| *at >= k + Duration::from_secs(2));
        if key {
            key_at = Some(*at);
        }
        let us = |d: Duration| d.as_micros() as u64;
        let arg = json!({ "data": jpeg(file)?, "us": us(*at), "dur": us(until.saturating_sub(*at)), "key": key });
        got.extend(chunks(&step(s, "put", arg).await?)?);
    }
    let done = step(s, "done", json!({})).await?;
    got.extend(chunks(&done)?);
    let frames = timed(playable(got)?, end);
    Ok(match format {
        "webm" => crate::mux::webm(&frames, w, h, end.as_millis() as u64),
        _ => {
            use base64::Engine;
            let avcc = base64::engine::general_purpose::STANDARD
                .decode(done["description"].as_str().ok_or("the window's encoder gave no H.264 configuration")?)
                .map_err(|e| e.to_string())?;
            crate::mux::mp4(&frames, w, h, &avcc)?
        }
    })
}

/// Is ffmpeg here to encode?
fn ffmpeg_here() -> bool {
    std::process::Command::new("ffmpeg").arg("-version").output().is_ok_and(|o| o.status.success())
}

/// Record the app's window into `out`; with `journal` (`--markers`, the
/// app's [`JOURNAL`]), its triggers into `<out>.markers.json`.
pub fn run(
    out: &Path,
    at: &crate::cdp::Reach,
    until: Option<Duration>,
    journal: Option<&Path>,
    json_out: bool,
) -> Result<String, String> {
    format_of(out)?;
    let dir = std::env::temp_dir().join(format!("aio-shot-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    struct Gone(PathBuf, Option<PathBuf>);
    impl Drop for Gone {
        fn drop(&mut self) {
            _ = std::fs::remove_dir_all(&self.0);
            if let Some(j) = &self.1 {
                _ = std::fs::remove_file(j);
            }
        }
    }
    let _gone = Gone(dir.clone(), journal.map(Path::to_path_buf));
    if let Some(j) = journal {
        // One left by a recording that was cut short is no recording.
        _ = std::fs::remove_file(j);
    }
    if until.is_none() && !json_out {
        eprintln!("ax shot: recording; Ctrl-C to stop");
    }
    let (frames, end, lost, t0, scale) =
        aio::rt().block_on(record(at, &dir, until, journal, json_out.then_some(out)))?;
    let (marked, marks) = match journal {
        Some(j) => {
            let m = markers(&std::fs::read_to_string(j).unwrap_or_default(), t0, end.as_millis() as u64, scale);
            let file = PathBuf::from(format!("{}.markers.json", out.display()));
            let n = m.as_array().map_or(0, Vec::len);
            std::fs::write(&file, m.to_string()).map_err(|e| format!("{}: {e}", file.display()))?;
            (format!(", {n} markers in {}", file.display()), Some(file))
        }
        None => (String::new(), None),
    };
    let by = match ffmpeg_here() {
        true => encode(&dir, &frames, end, out).map(|_| ""),
        false => aio::rt()
            .block_on(async {
                encode_in(&mut crate::cdp::window(at, 0).await?, &dir, &frames, end, format_of(out)?).await
            })
            .and_then(|b| std::fs::write(out, b).map_err(|e| format!("{}: {e}", out.display())))
            .map(|_| ", encoded by the window: no ffmpeg on PATH")
            .map_err(|e| format!("no ffmpeg on PATH, and the window could not encode it: {e}")),
    }?;
    if json_out {
        let bytes = std::fs::metadata(out).map_or(0, |m| m.len());
        return Ok(json!({
            "file": out, "bytes": bytes, "frames": frames.len(), "seconds": end.as_secs_f64(), "t0": t0,
            "markers": marks, "lost": lost, "ffmpeg": by.is_empty(),
        })
        .to_string());
    }
    Ok(format!(
        "wrote {} ({} frames, {:.1} s{marked}{by}){}",
        out.display(),
        frames.len(),
        end.as_secs_f64(),
        if lost { "; the window went away, so it ends there" } else { "" }
    ))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn a_trigger_is_a_marker_on_the_videos_clock_in_its_pixels() {
        // aio am-shot-video-markers.test.ts.
        let j = [
            r#"{"at":1500,"path":"b","action":"type","rect":null}"#,
            r#"{"at":1200,"path":"a","action":"click","rect":{"x":10,"y":5,"w":20.4,"h":8}}"#,
            r#"{"at":900,"path":"early","action":"click"}"#,
            r#"{"at":9000,"path":"late","action":"click"}"#,
            r#"{"at":1300,"path":"torn"#,
        ]
        .join("\n");
        assert_eq!(
            markers(&j, 1000, 1000, 2.0),
            json!([
                { "t": 200, "path": "a", "action": "click", "rect": { "x": 20.0, "y": 10.0, "w": 41.0, "h": 16.0 } },
                { "t": 500, "path": "b", "action": "type", "rect": null },
            ])
        );
    }

    /// A window that paints two frames, recorded and encoded (with ffmpeg;
    /// skipped without it).
    #[test]
    fn a_window_is_recorded_and_encoded() {
        let ok = |c: &mut std::process::Command| c.output().is_ok_and(|o| o.status.success());
        if !ok(std::process::Command::new("ffmpeg").arg("-version")) {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let jpeg = |c: &str| {
            let p = d.path().join(format!("{c}.jpg"));
            let src = format!("color=c={c}:s=64x48");
            assert!(ok(std::process::Command::new("ffmpeg")
                .args(["-loglevel", "error", "-f", "lavfi", "-i", &src, "-frames:v", "1"])
                .arg(&p)));
            use base64::Engine;
            base64::engine::general_purpose::STANDARD.encode(std::fs::read(p).unwrap())
        };
        let (red, blue) = (jpeg("red"), jpeg("blue"));
        let frame = |data: &str, n: u32| json!({ "method": "Page.screencastFrame", "params": { "data": data, "sessionId": n } });
        let answers = crate::cdp::tests::Answers::from([
            ("Page.captureScreenshot".into(), json!({ "data": red })),
            ("Page.startScreencast".into(), json!({ "$events": [frame(&blue, 1), frame(&red, 2)] })),
        ]);
        let port = &crate::cdp::Reach::Port(aio::rt().block_on(crate::cdp::tests::endpoint(answers)));
        let out = d.path().join("v.mp4");
        let journal = d.path().join(JOURNAL);
        let marking = {
            let j = journal.clone();
            std::thread::spawn(move || {
                while !j.exists() {
                    std::thread::sleep(Duration::from_millis(5));
                }
                note(&j, "SaveButton", "click", Some(json!({ "x": 10, "y": 20, "w": 30, "h": 8 })));
            })
        };
        let said = run(&out, port, Some(Duration::from_millis(400)), Some(&journal), false).unwrap();
        marking.join().unwrap();
        assert!(said.contains("3 frames") && said.contains("1 markers"), "{said}");
        let m: Value =
            aio::serde_json::from_slice(&std::fs::read(d.path().join("v.mp4.markers.json")).unwrap()).unwrap();
        assert_eq!((m[0]["path"].as_str(), m[0]["action"].as_str()), (Some("SaveButton"), Some("click")));
        assert!(m[0]["t"].as_u64().is_some_and(|t| t <= 400), "{m}");
        assert!(!journal.exists(), "the journal ends with the recording");
        note(&journal, "late", "click", None);
        assert!(!journal.exists(), "a trigger with no recording notes nothing");
        let mp4 = std::fs::read(&out).unwrap();
        assert_eq!(&mp4[4..8], b"ftyp");
        assert!(
            !std::env::temp_dir().join(format!("aio-shot-{}", std::process::id())).exists(),
            "no frames left behind"
        );
    }

    #[test]
    fn each_encoded_frame_lasts_until_the_next() {
        let c = |key, us| (key, us, vec![1]);
        let f = timed(vec![c(true, 0), c(false, 40_400), c(false, 40_900), c(true, 2_000_000)], Duration::from_secs(3));
        let got: Vec<_> = f.iter().map(|f| (f.key, f.at, f.dur)).collect();
        assert_eq!(got, [(true, 0, 40), (false, 40, 1), (false, 40, 1960), (true, 2000, 1000)], "a ms at least");
    }

    #[test]
    fn frames_no_player_could_play_are_refused() {
        let c = |key, us| (key, us, vec![1]);
        assert!(playable(vec![]).unwrap_err().contains("no frames"));
        assert!(playable(vec![c(false, 0)]).unwrap_err().contains("no key frame"));
        assert!(playable(vec![c(true, 0), c(false, 0)]).unwrap_err().contains("out of time"));
        assert_eq!(playable(vec![c(true, 0), c(false, 1000)]).unwrap().len(), 2);
    }

    /// aio's encoder without ffmpeg: a real Chromium window encodes what it
    /// painted (skipped without one), checked by ffprobe when it is here.
    #[test]
    fn without_ffmpeg_the_window_encodes_its_own_frames() {
        let Some(page) =
            crate::cdp::tests::Chromium::open("<div id=b style='width:100vw;height:100vh;background:red'>")
        else {
            return;
        };
        let d = tempfile::tempdir().unwrap();
        let ms = Duration::from_millis;
        let frames = aio::rt().block_on(async {
            let mut s = page.session().await;
            let mut frames = vec![];
            for (i, color) in ["red", "blue", "green"].iter().enumerate() {
                s.eval(&format!("document.getElementById('b').style.background = '{color}'")).await.unwrap();
                let shot = s.call("Page.captureScreenshot", json!({ "format": "jpeg", "quality": 85 })).await.unwrap();
                frames.push(keep(d.path(), i, ms(i as u64 * 1500), &shot["data"]).unwrap());
            }
            let webm = encode_in(&mut s, d.path(), &frames, ms(4000), "webm").await.unwrap();
            let mp4 = encode_in(&mut s, d.path(), &frames, ms(4000), "mp4").await;
            (frames, webm, mp4)
        });
        let (frames, webm, mp4) = frames;
        assert_eq!(frames.len(), 3);
        assert_eq!(&webm[..4], [0x1A, 0x45, 0xDF, 0xA3], "an EBML file");
        let mut files = vec![("v.webm", webm, "vp8")];
        match mp4 {
            Ok(b) => files.push(("v.mp4", b, "h264")),
            // A Chromium built without H.264 (Electron has it).
            Err(e) => assert!(e.contains("encodes no H.264"), "{e}"),
        }
        for (name, bytes, codec) in files {
            let f = d.path().join(name);
            std::fs::write(&f, bytes).unwrap();
            let Ok(o) = std::process::Command::new("ffprobe")
                .args(["-v", "error", "-count_frames", "-select_streams", "v:0", "-show_entries"])
                .args(["stream=codec_name,nb_read_frames,width,height:format=duration", "-of", "json"])
                .arg(&f)
                .output()
            else {
                continue;
            };
            let p: Value = aio::serde_json::from_slice(&o.stdout).unwrap();
            let st = &p["streams"][0];
            assert_eq!((st["codec_name"].as_str(), st["nb_read_frames"].as_str()), (Some(codec), Some("3")), "{p}");
            assert!(st["width"].as_u64().unwrap() > 0, "{p}");
            let dur: f64 = p["format"]["duration"].as_str().unwrap().parse().unwrap();
            assert!((3.9..=4.1).contains(&dur), "the video lasts as long as the recording: {p}");
        }
    }
}
