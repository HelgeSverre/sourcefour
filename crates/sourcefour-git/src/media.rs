//! Poster frames and container facts for video blobs.
//!
//! This module is the whole of the viewer's video knowledge, and it is
//! deliberately one seam wide: everything above it sees two functions and a
//! [`VideoInfo`], never a subprocess. Today those functions drive `ffmpeg` and
//! `ffprobe`, found where they are installed rather than where `PATH` admits
//! to; ADR 0006 records the intent to swap in a pure-Rust decoder behind the
//! same signatures.
//!
//! Nothing here fails. A missing binary, an unreadable container, a codec
//! nobody has, a blob too large to be worth writing out — each drops one field
//! and leaves the rest, because a card that fills in as far as it can is more
//! useful than a diff that refuses to open. The one absence that is said out
//! loud is a missing decoder, since that one the reader can fix.

use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Mutex, OnceLock, atomic::AtomicBool, atomic::Ordering},
    time::{Duration, Instant},
};

use sourcefour_model::VideoInfo;

/// How long either tool may run before it is killed.
///
/// A wedged decoder holds a background-executor thread, and a malformed blob
/// is exactly the input that wedges one. Five seconds cut off large files on
/// slow disks before they finished decoding, degrading the poster for a clip
/// that was only ever going to be slow, not stuck; eight still bounds a
/// wedged decoder without punishing a working one.
const TOOL_TIMEOUT: Duration = Duration::from_secs(8);

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

/// Absolute directories a tool is looked for in after `PATH` has been asked.
///
/// A bundle opened from Finder inherits launchd's `PATH` — `/usr/bin:/bin:
/// /usr/sbin:/sbin` and nothing more — so a Homebrew, `MacPorts` or
/// `/usr/local` install is invisible to the lookup that works in a terminal,
/// and every video in the app degrades to the metadata card. These are where
/// the three package managers put it, most common first.
#[cfg(unix)]
const STANDARD_DIRS: [&str; 3] = ["/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin"];
/// Windows has no equivalent convention: an install either lands on `PATH` or
/// is named in settings.
#[cfg(not(unix))]
const STANDARD_DIRS: [&str; 0] = [];

/// The same, relative to the user's home directory.
#[cfg(unix)]
const HOME_DIRS: [&str; 1] = [".local/bin"];
#[cfg(not(unix))]
const HOME_DIRS: [&str; 0] = [];

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
/// machine has installed, and `ffmpeg_dir` is the user's answer to where that
/// is when neither `PATH` nor the standard prefixes hold it.
pub(crate) fn probe(
    bytes: &[u8],
    format: &str,
    ffmpeg_dir: Option<&Path>,
) -> (Option<Vec<u8>>, VideoInfo) {
    let tools = tools(ffmpeg_dir);
    let mut info = VideoInfo {
        bytes: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        tools_missing: tools.is_none(),
        ..VideoInfo::default()
    };
    // Decided before the blob is written rather than after: on a machine
    // without a decoder, every video in every diff would otherwise pay for a
    // full-size temporary file to learn the same thing again.
    let Some((ffmpeg, ffprobe)) = tools else {
        return (None, info);
    };
    if bytes.len() > MAX_PROBE_BYTES {
        return (None, info);
    }
    let Some(file) = spill(bytes, format) else {
        return (None, info);
    };
    describe(file.path(), ffprobe, &mut info);
    (poster(file.path(), ffmpeg, info.duration_ms), info)
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
fn describe(path: &Path, ffprobe: &Path, info: &mut VideoInfo) {
    let Some(stdout) = run_bounded(Command::new(ffprobe).args([
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
fn poster(path: &Path, ffmpeg: &Path, duration_ms: Option<u64>) -> Option<Vec<u8>> {
    let seek = poster_seek(duration_ms);
    let stdout = run_bounded(Command::new(ffmpeg).args([
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

/// The `ffmpeg` this machine will decode with, if any.
///
/// The same search and the same once-per-process answer [`probe`] uses, so a
/// settings row that asks whether posters will work costs one lookup at most,
/// and asking is itself what fixes the resolution for the session.
pub fn ffmpeg_path(ffmpeg_dir: Option<&Path>) -> Option<PathBuf> {
    tools(ffmpeg_dir).map(|(ffmpeg, _)| ffmpeg.clone())
}

/// A fresh, uncached look for both tools — what a settings page wants right
/// after the user changes `ffmpeg_dir`, instead of [`tools`]'s answer from
/// whenever this session first probed a video.
///
/// # Errors
///
/// Returns which of the two tools did not answer `-version`.
pub fn verify_video_tools(ffmpeg_dir: Option<&Path>) -> Result<(PathBuf, PathBuf), &'static str> {
    let ffmpeg = resolve_tool("ffmpeg", ffmpeg_dir).ok_or("ffmpeg not found")?;
    let ffprobe = resolve_tool("ffprobe", ffmpeg_dir).ok_or("ffprobe not found")?;
    Ok((ffmpeg, ffprobe))
}

/// Both tools, found once per process, or nothing when either is absent.
///
/// A poster needs `ffmpeg` and a caption needs `ffprobe`; a machine with one
/// and not the other is a broken install, not half a feature, so the pair is
/// resolved together.
///
/// The search runs on the first video of the session and is remembered with
/// the `override_dir` it was given, so pointing the setting somewhere else
/// takes a restart. That is the price of not spawning six processes per
/// diff, and installing ffmpeg is itself a restart-shaped event.
fn tools(override_dir: Option<&Path>) -> Option<&'static (PathBuf, PathBuf)> {
    static TOOLS: OnceLock<Option<(PathBuf, PathBuf)>> = OnceLock::new();
    TOOLS
        .get_or_init(|| {
            Some((
                resolve_tool("ffmpeg", override_dir)?,
                resolve_tool("ffprobe", override_dir)?,
            ))
        })
        .as_ref()
}

/// The first candidate that answers `-version`, or nothing.
///
/// Running each candidate is the only honest test: a file can exist and be a
/// shim, a broken symlink, or the wrong architecture, and the answer wanted
/// here is whether a frame can be decoded rather than whether a path exists.
fn resolve_tool(name: &str, override_dir: Option<&Path>) -> Option<PathBuf> {
    let home = std::env::home_dir();
    tool_candidates(name, override_dir, home.as_deref())
        .into_iter()
        .find(|candidate| answers_to_version(candidate))
}

/// Every place `name` might be, in the order they are worth trying: what the
/// user named, then whatever `PATH` resolves, then the standard prefixes.
///
/// The setting comes first so it can override a stale copy on `PATH`, which is
/// the whole reason to have one; a bare name comes before the prefixes so a
/// deliberate `PATH` still wins over a guess.
fn tool_candidates(name: &str, override_dir: Option<&Path>, home: Option<&Path>) -> Vec<PathBuf> {
    override_dir
        .map(|directory| directory.join(name))
        .into_iter()
        .chain(std::iter::once(PathBuf::from(name)))
        .chain(STANDARD_DIRS.iter().map(|dir| Path::new(dir).join(name)))
        .chain(
            home.into_iter()
                .flat_map(|home| HOME_DIRS.iter().map(move |dir| home.join(dir).join(name))),
        )
        .collect()
}

/// Whether `candidate` starts and reports a version.
fn answers_to_version(candidate: &Path) -> bool {
    Command::new(candidate)
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
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
    #[cfg(unix)]
    fn a_tool_is_looked_for_where_it_was_pointed_before_where_it_is_usually_installed() {
        assert_eq!(
            tool_candidates(
                "ffmpeg",
                Some(Path::new("/opt/ffmpeg/bin")),
                Some(Path::new("/home/ada")),
            ),
            [
                PathBuf::from("/opt/ffmpeg/bin/ffmpeg"),
                PathBuf::from("ffmpeg"),
                PathBuf::from("/opt/homebrew/bin/ffmpeg"),
                PathBuf::from("/usr/local/bin/ffmpeg"),
                PathBuf::from("/opt/local/bin/ffmpeg"),
                PathBuf::from("/home/ada/.local/bin/ffmpeg"),
            ]
        );
    }

    #[test]
    #[cfg(unix)]
    fn without_a_setting_or_a_home_the_search_is_path_and_the_standard_prefixes() {
        assert_eq!(
            tool_candidates("ffprobe", None, None),
            [
                PathBuf::from("ffprobe"),
                PathBuf::from("/opt/homebrew/bin/ffprobe"),
                PathBuf::from("/usr/local/bin/ffprobe"),
                PathBuf::from("/opt/local/bin/ffprobe"),
            ]
        );
    }

    #[test]
    #[cfg(not(unix))]
    fn windows_knows_no_prefix_worth_guessing() {
        assert_eq!(
            tool_candidates(
                "ffmpeg",
                Some(Path::new(r"C:\ffmpeg\bin")),
                Some(Path::new(r"C:\Users\ada")),
            ),
            [
                PathBuf::from(r"C:\ffmpeg\bin\ffmpeg"),
                PathBuf::from("ffmpeg"),
            ]
        );
    }

    #[test]
    fn a_tool_no_directory_holds_resolves_to_nothing() {
        assert_eq!(resolve_tool("sourcefour-not-a-decoder", None), None);
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
        let (poster, info) = probe(b"not a video, just some bytes", "mp4", None);
        assert_eq!(poster, None);
        assert_eq!(info.bytes, 28);
        assert_eq!(info.duration_ms, None);
        assert_eq!(info.dimensions, None);
    }

    #[test]
    fn a_real_clip_yields_a_poster_and_its_facts() {
        let Some(ffmpeg) = ffmpeg_path(None) else {
            return;
        };
        // Generated rather than committed: the test already requires ffmpeg,
        // so a binary fixture in the tree would only be a second way to say so.
        let clip = tempfile::Builder::new()
            .suffix(".mp4")
            .tempfile()
            .expect("a temporary file");
        let made = Command::new(ffmpeg)
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
        let (poster, info) = probe(&bytes, "mp4", None);

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
