//! The ANSI escapes an Actions job log carries: cargo and clippy color
//! their diagnostics, and the runner wraps some paths in OSC-8 hyperlinks.
//! One line becomes styled runs, or the text alone.

/// One styled fragment of a log line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "SGR names these four independently"
)]
pub struct AnsiRun {
    pub text: String,
    pub color: Option<AnsiColor>,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
}

/// The palette-independent color an SGR sequence named.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnsiColor {
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
    BrightBlack,
    BrightRed,
    BrightGreen,
    BrightYellow,
    BrightBlue,
    BrightMagenta,
    BrightCyan,
    BrightWhite,
    Rgb(u8, u8, u8),
}

/// The line's text split at every style change. Escapes that name no style
/// are dropped, so the runs concatenate to what a terminal would show.
#[must_use]
pub fn parse_ansi_line(line: &str) -> Vec<AnsiRun> {
    let mut runs: Vec<AnsiRun> = Vec::new();
    let mut style = AnsiRun::default();
    visit_chunks(line, |chunk| match chunk {
        Chunk::Text(text) => match runs.last_mut() {
            Some(last) if styled_alike(last, &style) => last.text.push_str(text),
            _ => runs.push(AnsiRun {
                text: String::from(text),
                ..style.clone()
            }),
        },
        Chunk::Sgr(params) => apply_sgr(params, &mut style),
    });
    runs
}

/// The line's text with every escape removed.
#[must_use]
pub fn strip_ansi(line: &str) -> String {
    let mut text = String::with_capacity(line.len());
    visit_chunks(line, |chunk| {
        if let Chunk::Text(chunk) = chunk {
            text.push_str(chunk);
        }
    });
    text
}

const ESCAPE: u8 = 0x1b;

/// What the walk hands back: text to keep, or the parameters of an `ESC[…m`.
enum Chunk<'a> {
    Text(&'a str),
    Sgr(&'a str),
}

/// Walks the line, emitting every stretch of text and every SGR sequence.
/// Escapes are ASCII, so byte indices into `line` stay on char boundaries.
/// A sequence cut short by the end of the line is consumed and dropped —
/// half an escape never reaches a chunk.
fn visit_chunks(line: &str, mut visit: impl FnMut(Chunk<'_>)) {
    let bytes = line.as_bytes();
    let mut index = 0;
    let mut text_start = 0;
    while index < bytes.len() {
        if bytes[index] != ESCAPE {
            index += 1;
            continue;
        }
        if text_start < index {
            visit(Chunk::Text(&line[text_start..index]));
        }
        index = match bytes.get(index + 1) {
            Some(b'[') => end_of_csi(line, index, &mut visit),
            Some(b']') => end_of_osc(bytes, index),
            Some(0x20..=0x2f) => end_of_nf(bytes, index),
            Some(_) => index + 2,
            None => bytes.len(),
        };
        text_start = index;
    }
    if text_start < bytes.len() {
        visit(Chunk::Text(&line[text_start..]));
    }
}

/// The index past a CSI sequence starting at `start`, emitting its
/// parameters when the final byte is `m`. Parameter and intermediate bytes
/// are 0x20–0x3F, the final byte 0x40–0x7E; anything else ends the sequence
/// where it stands and resumes text there, the way a terminal aborts one.
fn end_of_csi(line: &str, start: usize, visit: &mut impl FnMut(Chunk<'_>)) -> usize {
    let bytes = line.as_bytes();
    let mut end = start + 2;
    while end < bytes.len() && (0x20..=0x3f).contains(&bytes[end]) {
        end += 1;
    }
    match bytes.get(end) {
        Some(&final_byte) if (0x40..=0x7e).contains(&final_byte) => {
            if final_byte == b'm' {
                visit(Chunk::Sgr(&line[start + 2..end]));
            }
            end + 1
        }
        _ => end,
    }
}

/// The index past an OSC sequence starting at `start`: everything through
/// BEL or ST. An OSC-8 hyperlink's visible text sits between two of these,
/// so dropping the wrappers keeps it.
fn end_of_osc(bytes: &[u8], start: usize) -> usize {
    let mut end = start + 2;
    while end < bytes.len() {
        match bytes[end] {
            0x07 => return end + 1,
            ESCAPE if bytes.get(end + 1) == Some(&b'\\') => return end + 2,
            _ => end += 1,
        }
    }
    end
}

/// The index past an `ESC` sequence carrying intermediate bytes (0x20–0x2F)
/// before its final byte: `tput sgr0`, which plenty of workflows call, writes
/// `ESC ( B` ahead of its reset, and the `B` is not text.
fn end_of_nf(bytes: &[u8], start: usize) -> usize {
    let mut end = start + 1;
    while end < bytes.len() && (0x20..=0x2f).contains(&bytes[end]) {
        end += 1;
    }
    (end + 1).min(bytes.len())
}

fn styled_alike(a: &AnsiRun, b: &AnsiRun) -> bool {
    (a.color, a.bold, a.dim, a.italic, a.underline)
        == (b.color, b.bold, b.dim, b.italic, b.underline)
}

/// Folds one SGR sequence into the running style. An empty parameter reads
/// as 0, and every code this app has no use for — backgrounds, blink,
/// inverse — is still consumed at its full arity so the codes after it land
/// in the right place.
fn apply_sgr(params: &str, style: &mut AnsiRun) {
    let codes: Vec<u32> = params
        .split(';')
        .map(|code| code.parse().unwrap_or(0))
        .collect();
    let mut index = 0;
    while let Some(&code) = codes.get(index) {
        index += 1;
        match code {
            0 => *style = AnsiRun::default(),
            1 => style.bold = true,
            2 => style.dim = true,
            3 => style.italic = true,
            4 => style.underline = true,
            21 | 22 => {
                style.bold = false;
                style.dim = false;
            }
            23 => style.italic = false,
            24 => style.underline = false,
            30..=37 => style.color = Some(NAMED[(code - 30) as usize]),
            90..=97 => style.color = Some(NAMED[(code - 82) as usize]),
            38 => {
                if let Some(color) = extended_color(&codes, &mut index) {
                    style.color = Some(color);
                }
            }
            39 => style.color = None,
            48 => {
                extended_color(&codes, &mut index);
            }
            _ => {}
        }
    }
}

/// The color a `38`/`48` names, consuming its parameters: `5;N` picks from
/// the 256-color palette, `2;R;G;B` is truecolor. A sequence cut short
/// leaves the style alone.
fn extended_color(codes: &[u32], index: &mut usize) -> Option<AnsiColor> {
    let mut next = || {
        let code = codes.get(*index).copied();
        *index += 1;
        code
    };
    let channel = |code: u32| u8::try_from(code).ok();
    match next()? {
        5 => palette_color(next()?),
        2 => Some(AnsiColor::Rgb(
            channel(next()?)?,
            channel(next()?)?,
            channel(next()?)?,
        )),
        _ => None,
    }
}

/// The 256-color palette: 0–15 are the named colors, 16–231 a 6×6×6 cube
/// over [`CUBE_LEVELS`], 232–255 a 24-step gray ramp.
fn palette_color(index: u32) -> Option<AnsiColor> {
    let index = u8::try_from(index).ok()?;
    match index {
        0..=15 => Some(NAMED[index as usize]),
        16..=231 => {
            let index = u32::from(index) - 16;
            let level = |shift: u32| CUBE_LEVELS[((index / shift) % 6) as usize];
            Some(AnsiColor::Rgb(level(36), level(6), level(1)))
        }
        232..=255 => {
            let gray = 8 + (index - 232) * 10;
            Some(AnsiColor::Rgb(gray, gray, gray))
        }
    }
}

const CUBE_LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];

const NAMED: [AnsiColor; 16] = [
    AnsiColor::Black,
    AnsiColor::Red,
    AnsiColor::Green,
    AnsiColor::Yellow,
    AnsiColor::Blue,
    AnsiColor::Magenta,
    AnsiColor::Cyan,
    AnsiColor::White,
    AnsiColor::BrightBlack,
    AnsiColor::BrightRed,
    AnsiColor::BrightGreen,
    AnsiColor::BrightYellow,
    AnsiColor::BrightBlue,
    AnsiColor::BrightMagenta,
    AnsiColor::BrightCyan,
    AnsiColor::BrightWhite,
];

#[cfg(test)]
mod tests {
    use super::{AnsiColor, AnsiRun, parse_ansi_line, strip_ansi};

    fn plain(text: &str) -> AnsiRun {
        AnsiRun {
            text: String::from(text),
            ..AnsiRun::default()
        }
    }

    fn colored(text: &str, color: AnsiColor) -> AnsiRun {
        AnsiRun {
            color: Some(color),
            ..plain(text)
        }
    }

    /// The cargo diagnostic header, as cargo actually emits it.
    const CARGO_ERROR: &str =
        "\x1b[0m\x1b[1m\x1b[38;5;9merror[E0308]\x1b[0m\x1b[1m: mismatched types\x1b[0m";

    #[expect(clippy::too_many_lines, reason = "a table, one row per case")]
    fn cases() -> Vec<(&'static str, Vec<AnsiRun>)> {
        vec![
            ("", Vec::new()),
            ("plain output", vec![plain("plain output")]),
            (
                "\x1b[31merror:\x1b[0m rest",
                vec![colored("error:", AnsiColor::Red), plain(" rest")],
            ),
            (
                "\x1b[1;31mbold red\x1b[0mplain",
                vec![
                    AnsiRun {
                        bold: true,
                        ..colored("bold red", AnsiColor::Red)
                    },
                    plain("plain"),
                ],
            ),
            (
                "\x1b[1;2mboth\x1b[22mneither",
                vec![
                    AnsiRun {
                        bold: true,
                        dim: true,
                        ..plain("both")
                    },
                    plain("neither"),
                ],
            ),
            (
                "\x1b[2mdim \x1b[3mitalic \x1b[4munder\x1b[23;24mdim again",
                vec![
                    AnsiRun {
                        dim: true,
                        ..plain("dim ")
                    },
                    AnsiRun {
                        dim: true,
                        italic: true,
                        ..plain("italic ")
                    },
                    AnsiRun {
                        dim: true,
                        italic: true,
                        underline: true,
                        ..plain("under")
                    },
                    AnsiRun {
                        dim: true,
                        ..plain("dim again")
                    },
                ],
            ),
            (
                "\x1b[1;31mred\x1b[39mdefault",
                vec![
                    AnsiRun {
                        bold: true,
                        ..colored("red", AnsiColor::Red)
                    },
                    AnsiRun {
                        bold: true,
                        ..plain("default")
                    },
                ],
            ),
            (
                "\x1b[96mbright cyan",
                vec![colored("bright cyan", AnsiColor::BrightCyan)],
            ),
            (
                "\x1b[38;5;9mpalette bright",
                vec![colored("palette bright", AnsiColor::BrightRed)],
            ),
            (
                "\x1b[38;5;196mcube",
                vec![colored("cube", AnsiColor::Rgb(255, 0, 0))],
            ),
            (
                "\x1b[38;5;244mgray",
                vec![colored("gray", AnsiColor::Rgb(128, 128, 128))],
            ),
            (
                "\x1b[38;2;12;34;56mtruecolor",
                vec![colored("truecolor", AnsiColor::Rgb(12, 34, 56))],
            ),
            (
                "\x1b[41;48;5;20;48;2;1;2;3mbackgrounds are ignored",
                vec![plain("backgrounds are ignored")],
            ),
            ("\x1b[2Kerased\x1b[1;5Hmoved", vec![plain("erasedmoved")]),
            (
                "\x1b]8;;https://example.com/x\x07link text\x1b]8;;\x07 tail",
                vec![plain("link text tail")],
            ),
            (
                "\x1b]8;;https://example.com/x\x1b\\st ends it\x1b]8;;\x1b\\",
                vec![plain("st ends it")],
            ),
            ("truncated\x1b[3", vec![plain("truncated")]),
            ("truncated\x1b", vec![plain("truncated")]),
            ("\x1b(Bcharset \x1b7pair", vec![plain("charset pair")]),
            (
                CARGO_ERROR,
                vec![
                    AnsiRun {
                        bold: true,
                        ..colored("error[E0308]", AnsiColor::BrightRed)
                    },
                    AnsiRun {
                        bold: true,
                        ..plain(": mismatched types")
                    },
                ],
            ),
        ]
    }

    #[test]
    fn a_line_parses_into_its_styled_runs() {
        for (line, expected) in cases() {
            assert_eq!(parse_ansi_line(line), expected, "line: {line:?}");
        }
    }

    #[test]
    fn stripping_leaves_what_the_runs_spell() {
        for (line, expected) in cases() {
            let text: String = expected.iter().map(|run| run.text.as_str()).collect();
            assert_eq!(strip_ansi(line), text, "line: {line:?}");
        }
        assert_eq!(strip_ansi(CARGO_ERROR), "error[E0308]: mismatched types");
    }
}
