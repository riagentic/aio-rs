//! Frames into a video (aio's `media/encoder`): which container a file asks
//! for, how long each frame stays on screen, and ffmpeg's encode of them.
//! `ax shot --video` records a window with it, and a UI test run under
//! `AIO_TEST_VIDEO` its steps (`aio::testing`).
use std::path::Path;
use std::time::Duration;

/// A video's container, from its file name (pure).
pub fn format_of(path: &Path) -> Result<&'static str, String> {
    match path.extension().and_then(|e| e.to_str()) {
        Some("mp4") => Ok("mp4"),
        Some("webm") => Ok("webm"),
        _ => Err(format!("--video {}: a .mp4 or .webm file", path.display())),
    }
}

/// The concat list ffmpeg plays (pure): each frame (`file`, painted `at`)
/// for as long as it was on screen, the last until `end`; a ms at least.
pub fn plan(frames: &[(String, Duration)], end: Duration) -> String {
    let mut out = String::from("ffconcat version 1.0\n");
    for (i, (file, at)) in frames.iter().enumerate() {
        let until = frames.get(i + 1).map_or(end, |(_, next)| *next);
        let d = until.saturating_sub(*at).max(Duration::from_millis(1));
        out += &format!("file '{file}'\nduration {:.3}\n", d.as_secs_f64());
    }
    // The concat demuxer takes the last duration only with the file again.
    if let Some((file, _)) = frames.last() {
        out += &format!("file '{file}'\n");
    }
    out
}

/// ffmpeg's arguments to encode `list` into `out` as `format` (pure): the
/// frames' own times kept, sizes made even (yuv420p needs it).
pub fn ffmpeg_args(list: &Path, out: &Path, format: &str) -> Vec<String> {
    let codec: &[&str] = match format {
        "webm" => &["-c:v", "libvpx-vp9", "-b:v", "0", "-crf", "32"],
        _ => &["-c:v", "libx264", "-movflags", "+faststart"],
    };
    let head = ["-y", "-hide_banner", "-loglevel", "error", "-f", "concat", "-safe", "0", "-i"];
    let vf = ["-vf", "scale=trunc(iw/2)*2:trunc(ih/2)*2,format=yuv420p", "-fps_mode", "vfr"];
    head.iter()
        .map(|s| s.to_string())
        .chain([list.display().to_string()])
        .chain(vf.iter().chain(codec).map(|s| s.to_string()))
        .chain([out.display().to_string()])
        .collect()
}

/// Encode `frames` (files in `dir`, painted at their times) into `out`,
/// the last one shown until `end`.
pub fn encode(dir: &Path, frames: &[(String, Duration)], end: Duration, out: &Path) -> Result<(), String> {
    let format = format_of(out)?;
    let list = dir.join("frames.txt");
    std::fs::write(&list, plan(frames, end)).map_err(|e| e.to_string())?;
    let out = std::path::absolute(out).map_err(|e| e.to_string())?;
    match std::process::Command::new("ffmpeg").args(ffmpeg_args(&list, &out, format)).output() {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err("--video needs ffmpeg on PATH to encode".into()),
        Err(e) => Err(format!("ffmpeg: {e}")),
        Ok(o) if !o.status.success() => Err(format!("ffmpeg: {}", String::from_utf8_lossy(&o.stderr).trim())),
        Ok(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_frame_lasts_until_the_next_was_painted() {
        let ms = Duration::from_millis;
        let f = |n: &str, at| (n.to_string(), ms(at));
        assert_eq!(
            plan(&[f("a.jpg", 0), f("b.jpg", 40), f("c.jpg", 40)], ms(1000)),
            "ffconcat version 1.0\nfile 'a.jpg'\nduration 0.040\nfile 'b.jpg'\nduration 0.001\nfile 'c.jpg'\nduration 0.960\nfile 'c.jpg'\n"
        );
        assert_eq!(plan(&[], ms(5)), "ffconcat version 1.0\n");
        assert_eq!(format_of(Path::new("x.webm")), Ok("webm"));
        assert!(format_of(Path::new("x.gif")).unwrap_err().contains(".mp4 or .webm"));
        let a = ffmpeg_args(Path::new("/t/l.txt"), Path::new("/o/v.webm"), "webm");
        assert!(a.windows(2).any(|w| w == ["-i", "/t/l.txt"]) && a.contains(&"libvpx-vp9".into()));
        assert_eq!(a.last().map(String::as_str), Some("/o/v.webm"));
    }
}
