//! Poster frames and container facts for video blobs.
//!
//! This module is the whole of the viewer's video knowledge, and it is
//! deliberately one seam wide: everything above it sees two functions and a
//! [`VideoInfo`], never a subprocess. Today those functions drive `ffmpeg` and
//! `ffprobe` off `PATH`; ADR 0006 records the intent to swap in a pure-Rust
//! decoder behind the same two signatures.
//!
//! Nothing here fails. A missing binary, an unreadable container, a codec
//! nobody has, a blob too large to be worth writing out — each drops one field
//! and leaves the rest, because a card that fills in as far as it can is more
//! useful than a diff that refuses to open.

use std::{
    io::{Read, Write},
    path::Path,
    process::{Command, Stdio},
    sync::{Mutex, OnceLock, atomic::AtomicBool, atomic::Ordering},
    time::{Duration, Instant},
};

use sourcefour_model::VideoInfo;

/// How long either tool may run before it is killed.
///
/// A wedged decoder holds a background-executor thread, and a malformed blob
/// is exactly the input that wedges one.
const TOOL_TIMEOUT: Duration = Duration::from_secs(5);

/// Largest blob worth writing to disk to look at a single frame of.
///
/// Both tools need a file, and the old side of a comparison only exists in
/// memory, so every probe costs a full write. Past this the card shows the
/// byte count and nothing else.
const MAX_PROBE_BYTES: usize = 128 * 1024 * 1024;

/// Longest the poster may be taken from, in seconds.
///
/// Frame zero is a fade-in or a slate often enough to be worth stepping past,
/// but a clip shorter than this has to yield something, hence the fraction.
const POSTER_SEEK_CEILING: f64 = 1.0;

/// Fraction into the clip the poster is taken from when the duration is known.
const POSTER_SEEK_FRACTION: f64 = 0.1;

/// Widest poster kept; taller sources scale down, narrower ones are untouched.
const POSTER_WIDTH: u32 = 1280;

/// The lowercased video extension of `path` when the viewer will probe it,
/// normalized to one spelling per container.
pub(crate) fn video_format(path: &[u8]) -> Option<String> {
    let dot = path.iter().rposition(|&byte| byte == b'.')?;
    let extension = std::str::from_utf8(&path[dot + 1..]).ok()?.to_lowercase();
    match extension.as_str() {
        "mp4" | "m4v" => Some(String::from("mp4")),
        "mov" | "qt" => Some(String::from("mov")),
        "webm" | "mkv" | "avi" => Some(extension),
        _ => None,
    }
}

/// A poster frame as PNG bytes, and whatever the container admits to.
///
/// The byte count is always reported; everything else depends on what the
/// machine has installed.
pub(crate) fn probe(bytes: &[u8], format: &str) -> (Option<Vec<u8>>, VideoInfo) {
    let mut info = VideoInfo {
        bytes: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        ..VideoInfo::default()
    };
    if bytes.len() > MAX_PROBE_BYTES || !tools_present() {
        return (None, info);
    }
    let Some(file) = spill(bytes, format) else {
        return (None, info);
    };
    describe(file.path(), &mut info);
    (poster(file.path(), info.duration_ms), info)
}

/// Writes the blob somewhere both tools can open it.
///
/// The suffix is what lets a demuxer that guesses by name guess right; the
/// handle is kept so the file outlives both runs and is removed after.
fn spill(bytes: &[u8], format: &str) -> Option<tempfile::NamedTempFile> {
    let mut file = tempfile::Builder::new()
        .prefix("sourcefour-")
        .suffix(&format!(".{format}"))
        .tempfile()
        .ok()?;
    file.write_all(bytes).ok()?;
    file.flush().ok()?;
    Some(file)
}

/// Fills in what `ffprobe` says about the container, leaving `info` alone for
/// anything it will not answer.
fn describe(path: &Path, info: &mut VideoInfo) {
    let Some(stdout) = run_bounded(Command::new("ffprobe").args([
        "-v",
        "quiet",
        "-print_format",
        "json",
        "-show_format",
        "-show_streams",
        &path.to_string_lossy(),
    ])) else {
        return;
    };
    let Ok(probe) = serde_json::from_slice::<serde_json::Value>(&stdout) else {
        return;
    };
    info.duration_ms = probe["format"]["duration"]
        .as_str()
        .and_then(|seconds| seconds.parse::<f64>().ok())
        .and_then(seconds_as_millis);
    // The first track is regularly the audio one, which reports no size at
    // all, so the video track has to be picked out by kind rather than index.
    info.dimensions = probe["streams"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|stream| stream["codec_type"] == "video")
        .and_then(|track| {
            Some((
                u32::try_from(track["width"].as_u64()?).ok()?,
                u32::try_from(track["height"].as_u64()?).ok()?,
            ))
        });
}

/// A single frame as PNG bytes, taken a moment into the clip.
fn poster(path: &Path, duration_ms: Option<u64>) -> Option<Vec<u8>> {
    let seek = poster_seek(duration_ms);
    let stdout = run_bounded(Command::new("ffmpeg").args([
        "-v",
        "error",
        // Without this a tool that decides to prompt inherits the terminal and
        // waits on a keystroke nobody is there to press.
        "-nostdin",
        // Seeking before the input seeks by index rather than by decoding up
        // to the timestamp, which is the difference between instant and
        // reading the whole file.
        "-ss",
        &format!("{seek}"),
        "-i",
        &path.to_string_lossy(),
        "-frames:v",
        "1",
        "-vf",
        &format!("scale='min({POSTER_WIDTH},iw)':-2"),
        "-f",
        "image2pipe",
        "-vcodec",
        "png",
        "-",
    ]))?;
    (!stdout.is_empty()).then_some(stdout)
}

/// Where in the clip to take the poster from, in seconds.
fn poster_seek(duration_ms: Option<u64>) -> f64 {
    let Some(duration_ms) = duration_ms else {
        return 0.0;
    };
    #[expect(
        clippy::cast_precision_loss,
        reason = "a duration long enough to lose precision here is longer than any clip"
    )]
    let seconds = duration_ms as f64 / 1000.0;
    (seconds * POSTER_SEEK_FRACTION).min(POSTER_SEEK_CEILING)
}

/// Seconds as whole milliseconds, rejecting what cannot be one.
fn seconds_as_millis(seconds: f64) -> Option<u64> {
    /// The largest whole millisecond an `f64` still counts one at a time —
    /// 2^53, or about 285,000 years. A duration this side of it converts
    /// exactly; one past it is a corrupt header, not a long clip.
    const CEILING: f64 = 9_007_199_254_740_992.0;

    let millis = (seconds * 1000.0).round();
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the range check is what makes the cast exact"
    )]
    (millis.is_finite() && (0.0..=CEILING).contains(&millis)).then_some(millis as u64)
}

/// Whether this machine can decode a frame at all, for tests that need one.
#[cfg(test)]
pub(crate) fn probe_available() -> bool {
    tools_present()
}

/// Whether `ffmpeg` can be started at all, decided once per process.
///
/// Checked before the blob is written rather than after: on a machine without
/// it, every video in every diff would otherwise pay for a full-size temporary
/// file to learn the same thing again.
fn tools_present() -> bool {
    static PRESENT: OnceLock<bool> = OnceLock::new();
    *PRESENT.get_or_init(|| {
        Command::new("ffmpeg")
            .arg("-version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    })
}

/// Runs `command`, returning its stdout when it succeeds within the timeout.
///
/// A reader thread is not needed the way it is for a Git operation: stderr goes
/// to the void, so only stdout can fill, and this thread is the one draining
/// it. The watcher exists solely to bound a decoder that never returns — when
/// it kills the child the pipe closes and the read below ends.
fn run_bounded(command: &mut Command) -> Option<Vec<u8>> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let pipe = child.stdout.take();
    let child = Mutex::new(child);
    let finished = AtomicBool::new(false);
    let stdout = std::thread::scope(|scope| {
        scope.spawn(|| {
            let deadline = Instant::now() + TOOL_TIMEOUT;
            while !finished.load(Ordering::Acquire) {
                if Instant::now() >= deadline {
                    if let Ok(mut child) = child.lock() {
                        child.kill().ok();
                    }
                    return;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        });
        let mut buffer = Vec::new();
        if let Some(mut pipe) = pipe {
            pipe.read_to_end(&mut buffer).ok();
        }
        // The pipe has closed, so the child is done one way or the other.
        finished.store(true, Ordering::Release);
        buffer
    });
    let status = child.into_inner().ok()?.wait().ok()?;
    status.success().then_some(stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_mp4_extension_is_video() {
        assert_eq!(video_format(b"clips/intro.mp4").as_deref(), Some("mp4"));
        assert_eq!(video_format(b"INTRO.MP4").as_deref(), Some("mp4"));
    }

    #[test]
    fn container_spellings_normalize_to_one_name() {
        assert_eq!(video_format(b"a.m4v").as_deref(), Some("mp4"));
        assert_eq!(video_format(b"a.qt").as_deref(), Some("mov"));
        assert_eq!(video_format(b"a.mov").as_deref(), Some("mov"));
        assert_eq!(video_format(b"a.webm").as_deref(), Some("webm"));
    }

    #[test]
    fn a_png_extension_is_not_video() {
        assert_eq!(video_format(b"logo.png"), None);
        assert_eq!(video_format(b"README"), None);
        assert_eq!(video_format(b"archive.mp4.gz"), None);
    }

    #[test]
    fn the_poster_seek_stays_inside_short_clips() {
        assert!((poster_seek(None) - 0.0).abs() < f64::EPSILON);
        // A half-second clip must not seek past its own end.
        assert!((poster_seek(Some(500)) - 0.05).abs() < f64::EPSILON);
        // A long one stops at the ceiling rather than a tenth of the way in.
        assert!((poster_seek(Some(600_000)) - POSTER_SEEK_CEILING).abs() < f64::EPSILON);
    }

    #[test]
    fn a_duration_that_is_not_a_duration_is_dropped() {
        assert_eq!(seconds_as_millis(1.5), Some(1500));
        assert_eq!(seconds_as_millis(0.0), Some(0));
        assert_eq!(seconds_as_millis(-1.0), None);
        assert_eq!(seconds_as_millis(f64::NAN), None);
        assert_eq!(seconds_as_millis(f64::INFINITY), None);
    }

    #[test]
    fn garbage_bytes_probe_to_a_size_and_nothing_else() {
        // Deterministic whether or not a decoder is installed: no container
        // claims these bytes, so every optional field has to stay empty.
        let (poster, info) = probe(b"not a video, just some bytes", "mp4");
        assert_eq!(poster, None);
        assert_eq!(info.bytes, 28);
        assert_eq!(info.duration_ms, None);
        assert_eq!(info.dimensions, None);
    }

    #[test]
    fn a_real_clip_yields_a_poster_and_its_facts() {
        if !tools_present() {
            return;
        }
        // Generated rather than committed: the test already requires ffmpeg,
        // so a binary fixture in the tree would only be a second way to say so.
        let clip = tempfile::Builder::new()
            .suffix(".mp4")
            .tempfile()
            .expect("a temporary file");
        let made = Command::new("ffmpeg")
            .args(["-v", "error", "-nostdin", "-y"])
            .args([
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=320x240:rate=10:duration=2",
            ])
            .args(["-pix_fmt", "yuv420p"])
            .arg(clip.path())
            .status()
            .expect("ffmpeg to run");
        assert!(made.success(), "ffmpeg could not generate the fixture");

        let bytes = std::fs::read(clip.path()).expect("the generated clip");
        let (poster, info) = probe(&bytes, "mp4");

        let poster = poster.expect("a poster frame");
        assert_eq!(&poster[..4], b"\x89PNG", "the poster is not a PNG");
        assert_eq!(info.dimensions, Some((320, 240)));
        assert_eq!(
            info.bytes,
            u64::try_from(bytes.len()).expect("a byte count")
        );
        let duration = info.duration_ms.expect("a duration");
        assert!(
            (1_900..=2_100).contains(&duration),
            "expected roughly two seconds, got {duration}ms"
        );
    }
}
