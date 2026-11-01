// A UI test as a video (aio's `testUI --video`), asked for from outside the
// test: `AIO_TEST_VIDEO=<dir>` (one file per test, named after it) or
// `=<file.mp4|.webm>`, and `AIO_TEST_VIDEO_PACE=<ms>` for how long each step
// stays on screen (800). After each settle the page is captured as the
// test left it (the browser's own screenshot); a step that changed nothing
// adds no frame. When the test ends, the frames are encoded by ffmpeg
// (`aio::video`) and the temp frames removed.
use std::path::{Path, PathBuf};
use std::time::Duration;

const PACE_MS: u64 = 800;

/// Where test `name`'s video goes, from `AIO_TEST_VIDEO` (`video`) and how
/// long each step shows, from `AIO_TEST_VIDEO_PACE` (pure).
pub(crate) fn asked(video: Option<&str>, pace: Option<&str>, name: &str) -> Option<(PathBuf, Duration)> {
    let path = Path::new(video.filter(|v| !v.is_empty())?);
    let out = match crate::video::format_of(path) {
        Ok(_) => path.to_path_buf(),
        Err(_) => path.join(format!("{}.mp4", file_name(name))),
    };
    let ms = pace.and_then(|p| p.trim().parse().ok()).filter(|ms| *ms > 0).unwrap_or(PACE_MS);
    Some((out, Duration::from_millis(ms)))
}

/// A test's name as a file name: `ui::adds_a_todo` → `ui-adds_a_todo`.
fn file_name(name: &str) -> String {
    let s = name.replace("::", "-");
    s.chars().map(|c| if c.is_alphanumeric() || "-_.".contains(c) { c } else { '_' }).collect()
}

/// One test's frames, encoded when it is dropped.
pub(crate) struct Recorder {
    out: PathBuf,
    pace: Duration,
    dir: PathBuf,
    frames: Vec<(String, Duration)>,
    last: Vec<u8>,
}

impl Recorder {
    /// A recorder for this thread's test, when a video was asked for.
    pub(crate) fn asked() -> Option<Self> {
        let name = std::thread::current().name().unwrap_or("test").to_string();
        let env = |k: &str| std::env::var(k).ok();
        let (out, pace) = asked(env("AIO_TEST_VIDEO").as_deref(), env("AIO_TEST_VIDEO_PACE").as_deref(), &name)?;
        let dir =
            std::env::temp_dir().join(format!("aio-video-{}-{}", std::process::id(), crate::uuid_v4().simple()));
        Some(Self { out, pace, dir, frames: vec![], last: vec![] })
    }

    /// The page as it is now, a frame if it changed.
    pub(crate) fn snap(&mut self, page: &super::electron::Page) {
        self.frame(page.shot());
    }

    fn frame(&mut self, png: Vec<u8>) {
        if png == self.last {
            return;
        }
        let file = format!("f{:06}.png", self.frames.len());
        match std::fs::create_dir_all(&self.dir).and_then(|_| std::fs::write(self.dir.join(&file), &png)) {
            Ok(()) => self.frames.push((file, self.pace * self.frames.len() as u32)),
            Err(e) => eprintln!("aio: test video: frame {file}: {e}"),
        }
        self.last = png;
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        let end = self.pace * self.frames.len() as u32;
        let made = self.out.parent().map_or(Ok(()), std::fs::create_dir_all).map_err(|e| e.to_string());
        match made.and_then(|_| crate::video::encode(&self.dir, &self.frames, end, &self.out)) {
            Ok(()) => eprintln!("aio: test video: {} ({} steps)", self.out.display(), self.frames.len()),
            Err(e) => eprintln!("aio: test video {}: {e}", self.out.display()),
        }
        _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_video_is_asked_for_from_outside_the_test() {
        let ms = Duration::from_millis;
        assert_eq!(asked(None, None, "t"), None);
        assert_eq!(asked(Some(""), None, "t"), None);
        assert_eq!(
            asked(Some("videos"), None, "ui::adds a/todo"),
            Some((PathBuf::from("videos/ui-adds_a_todo.mp4"), ms(800)))
        );
        assert_eq!(asked(Some("demo.webm"), Some("200"), "t"), Some((PathBuf::from("demo.webm"), ms(200))));
        assert_eq!(asked(Some("v"), Some("x"), "t").unwrap().1, ms(800), "a bad pace is the default");
    }

    fn swatch() -> crate::ui::View {
        use crate::ui::prelude::*;
        let blue = signal(|| false);
        let bg = if blue.get() { "rgb(0,0,255)" } else { "rgb(255,0,0)" };
        rsx! {
            div { style: "margin:0;width:200px;height:100px;background:{bg}" }
            button { "data-t": "flip", onclick: move |_| blue.set(true), "Flip" }
        }
    }

    #[test]
    fn each_changed_step_is_captured_and_the_steps_encoded() {
        let mut ui = crate::testing::Mount::default().size(320, 240).app(swatch);
        let d = tempfile::tempdir().unwrap();
        let out = d.path().join("v.mp4");
        let frames = d.path().join("frames");
        let mut r = Recorder {
            out: out.clone(),
            pace: Duration::from_millis(100),
            dir: frames.clone(),
            frames: vec![],
            last: vec![],
        };
        let first = ui.page().shot();
        assert_eq!(&first[1..4], b"PNG", "the browser's own capture");
        r.frame(first.clone());
        r.frame(first.clone());
        assert_eq!(r.frames.len(), 1, "a step that changed nothing adds no frame");
        ui.click("flip");
        // The new colour is drawn a frame or more after the click (later on a
        // loaded machine): captured once it shows.
        let end = std::time::Instant::now() + Duration::from_secs(20);
        let flipped = loop {
            let s = ui.page().shot();
            if s != first || std::time::Instant::now() > end {
                break s;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        r.frame(flipped);
        assert_eq!(r.frames.iter().map(|(_, at)| at.as_millis()).collect::<Vec<_>>(), [0, 100], "the new colour");
        let has_ffmpeg =
            std::process::Command::new("ffmpeg").arg("-version").output().is_ok_and(|o| o.status.success());
        drop(r);
        assert!(!frames.exists(), "no frames left behind");
        if has_ffmpeg {
            assert_eq!(&std::fs::read(&out).unwrap()[4..8], b"ftyp");
        }
    }
}
