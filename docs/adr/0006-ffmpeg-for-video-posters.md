# ADR 0006: Video posters come from ffmpeg, for now

## Status

Accepted, amended 2026-08-06. The seam this decision creates is the point of
it, and the replacement is still stated.

The amendment is about finding the tools, not about which tools. "From `PATH`"
turned out to mean "from whatever `PATH` the process was launched with", and a
bundle opened from Finder inherits launchd's — `/usr/bin:/bin:/usr/sbin:/sbin`
— which holds no ffmpeg anyone installed. Every video degraded to the metadata
card on machines that had a decoder, and this ADR's "absence is a normal
outcome" was covering for it. The lookup is now an ordered search: a directory
named in settings, then `PATH`, then the prefixes the package managers use
(`/opt/homebrew/bin`, `/usr/local/bin`, `/opt/local/bin`, `~/.local/bin`), each
candidate confirmed by running it. Absence is still a normal outcome, but it is
now a stated one: `VideoInfo::tools_missing` separates "nothing here to ask"
from "asked, and the container would not say", and only the first earns a line
on the card — "Install ffmpeg to see a preview frame" — because only the first
is something the reader can act on. `video.ffmpeg_dir` in settings.json covers
the install that is somewhere else entirely; the Diffs section reports whether
the search found anything.

## Context

The diff overlay renders image blobs natively (§6.11): `content_for` recognizes
an image extension before the binary heuristic and hands both sides to the
juxtapose slider and the side-by-side pair. Every other binary file collapses to
"X is binary." A commit that swaps a hero video, a screen recording, or a demo
clip therefore says nothing beyond the filename.

Showing a video the way the viewer shows an image needs a decoded frame. GPUI
offers nothing that will produce one:

- `gpui::img` decodes still images only, and its multi-frame path
  (`Image::to_image_data`) is GIF-exclusive — animated WebP already comes back
  as a single frame.
- `gpui::surface` accepts one source, `media::core_video::CVImageBuffer`, behind
  `#[cfg(target_os = "macos")]`. Feeding it means AVFoundation, and `objc2`'s
  `msg_send!` expands to an `unsafe` block at the call site, which
  `unsafe_code = "forbid"` in `[workspace.lints.rust]` rejects outright — a
  `forbid` cannot be lifted by an `#[allow]` in a module. It would also strand
  the Linux AppImage and the Windows MSI, both of which ship today.

That leaves decoding it ourselves or asking something else to. The pure-Rust
options are all partial: `re_mp4` demuxes but does not decode; `openh264` covers
H.264 and neither H.265, AV1, VP9, nor anything in a WebM container, and its
binary-distribution licensing is unresolved for a signed, notarized `.pkg`.

## Decision

`crates/sourcefour-git/src/media.rs` shells out to `ffprobe` and `ffmpeg`,
found by the search the amendment above describes. Everything above it sees two
functions and a `VideoInfo`:

```rust
pub(crate) fn video_format(path: &[u8]) -> Option<String>
pub(crate) fn probe(
    bytes: &[u8],
    format: &str,
    ffmpeg_dir: Option<&Path>,
) -> (Option<Vec<u8>>, VideoInfo)
```

Nothing outside that module knows a subprocess exists, beyond the directory to
look in — threaded from settings through `file_diff` — and `ffmpeg_path`, which
the settings page asks to say whether posters will work. Both name a tool, not a
process, and a pure-Rust decoder would answer the second and ignore the first.
`DiffContent::Video`
carries a PNG poster per side, so `render_image`, `ensure_images`,
`image_split_view` and `image_slider_view` all work on it unchanged — a video
comparison is an image comparison with a caption.

Absence is a normal outcome, not an error. A missing binary, an unreadable
container, a codec nobody has, a blob past `MAX_PROBE_BYTES` — each drops one
field and leaves the rest, down to a card showing the byte count. The viewer
never refuses to open a diff because of what is not installed.

Blobs are written to a temporary file first. Both tools need a path, and the old
side of a comparison exists only in memory.

## Consequences

No new crates: `serde_json` and `tempfile` were already workspace dependencies.
One code path serves macOS, Linux and Windows, and it reads every container and
codec the local ffmpeg does — MP4, MOV, WebM, MKV, AVI — which no pure-Rust
option currently matches.

Against that:

- **A runtime dependency no installer ships.** Users without ffmpeg get the
  metadata card. This is the main cost, and the main reason to revisit.
- **Untrusted bytes through a large C attack surface.** Blob contents are
  arbitrary repository data handed to ffmpeg's demuxers, historically a source
  of CVEs. Acceptable for a tool pointed at repositories the user already
  trusts enough to check out; not something to be relaxed about.
- **A process per side per video.** Bounded by a five-second watchdog, because
  `std::process::Command` has no timeout and a malformed blob is exactly the
  input that wedges a decoder — a wedged child would otherwise hold a
  background-executor thread for the life of the process.
- **Probing is synchronous within the diff read.** The overlay holds on
  "Computing diff…" until both sides return.

## Future work

Replace the module's body with a pure-Rust decoder behind the same two
signatures. The seam is deliberately that narrow so the swap touches one file
and no caller.

Revisit when any of these becomes true:

- A pure-Rust H.264/H.265/AV1 decoder exists with licensing that survives a
  signed, notarized binary.
- Users report the metadata-only card often enough that a bundled decoder is
  worth its size.
- The security surface stops being acceptable — for instance if the viewer ever
  opens repositories the user did not choose.

## Pure-Rust replacement, researched 2026-08

The landscape moved since this was written, enough to record where it now
stands. Not enough to act on.

Demuxing is solved. `re_mp4` reads MP4 and MOV under MIT in about 6,600 lines;
`matroska-demuxer` reads WebM and MKV under Zlib/MIT/Apache in about 3,500.
Both are small enough to read in an afternoon, and either would give duration
and dimensions with no decoder at all.

Decoding is where it splits by codec:

- **H.264 has two viable pure-Rust decoders.** `rust_h264` covers Baseline,
  Main and High with both entropy coders, CAVLC and CABAC, and has NEON paths.
  `rusty_h264` is openh264 rebuilt in Rust under BSD-2 with
  `forbid(unsafe_code)` — the same lint this workspace sets — and tests
  bit-exact against Cisco's output, which is the strongest correctness claim
  any of these make.
- **AV1 is upstream `rav1d`, BSD-2, with the assembly turned off.** The
  assembly is what makes dav1d fast, and it is also what this workspace's
  `forbid` rules out. `rav1d-safe` exists and is the wrong door: its additions
  are AGPL-3.0 or a commercial license, which an MIT application cannot take.
- **HEVC and VP9 have nothing mature.** A screen recording from a recent iPhone
  is HEVC, so this is not an exotic gap.

The cost of the ones that do exist: roughly 150,000 to 200,000 lines entering
the build, 30 to 60 seconds on a cold release build, and 2 to 4 MB of binary.
For a card that today says the file's size and duration.

So the revisit conditions above stand, with one sharpened: a pure-Rust route
covering H.264 and AV1 exists now, and the thing still missing is HEVC — plus a
reason to spend a minute of every release build on posters.

A partial step is available and deliberately not taken: parsing the ISO-BMFF box
tree in about forty lines of safe Rust would give MP4 and MOV duration and
dimensions with no decoder at all, so the card would fill in without ffmpeg. It
is skipped because it covers neither WebM nor MKV and would leave two
half-migrated metadata paths to maintain. Add it if the bare card turns out to
matter before a full decoder lands.
