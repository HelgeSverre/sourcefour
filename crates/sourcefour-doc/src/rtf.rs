//! A deliberately narrow RTF interpreter for readable diff previews.

use encoding_rs::{Encoding, WINDOWS_1252};
use rtf_grimoire::tokenizer::{Token, read_token};

use crate::{DocBlock, DocBlockKind, DocSpan, DocumentKind, parse_markdown};

/// Why document bytes could not be converted into preview blocks.
#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum DocumentParseError {
    /// The requested document kind is recognised but has no preview parser.
    #[error("{0:?} preview is not implemented")]
    Unsupported(DocumentKind),
    /// The RTF token stream is malformed.
    #[error("invalid RTF near byte {offset}")]
    InvalidRtf {
        /// Byte offset at which tokenization stopped.
        offset: usize,
    },
    /// The input did not declare an RTF document at its root.
    #[error("missing RTF document header")]
    MissingHeader,
    /// A closing group had no corresponding opening group.
    #[error("unbalanced RTF group near byte {offset}")]
    UnbalancedGroup {
        /// Byte offset of the unmatched group boundary.
        offset: usize,
    },
}

/// Parses document bytes according to their detected kind.
///
/// # Errors
///
/// Returns an error when the kind has no parser or RTF input is malformed.
pub fn parse_document(
    kind: DocumentKind,
    bytes: &[u8],
) -> Result<Vec<DocBlock>, DocumentParseError> {
    match kind {
        DocumentKind::Markdown => Ok(parse_markdown(&String::from_utf8_lossy(bytes))),
        DocumentKind::Rtf => parse_rtf(bytes),
        DocumentKind::AsciiDoc | DocumentKind::Latex => Err(DocumentParseError::Unsupported(kind)),
    }
}

/// Parses RTF into the normalized block tree used by the preview renderer.
///
/// # Errors
///
/// Returns an error when the token stream is malformed, has unbalanced groups,
/// or does not declare an RTF document at its root.
pub fn parse_rtf(bytes: &[u8]) -> Result<Vec<DocBlock>, DocumentParseError> {
    let mut interpreter = Interpreter::default();
    let mut remaining = bytes;

    while !remaining.is_empty() {
        let offset = bytes.len() - remaining.len();
        let (next, token) =
            read_token(remaining).map_err(|_| DocumentParseError::InvalidRtf { offset })?;
        if next.len() == remaining.len() {
            return Err(DocumentParseError::InvalidRtf { offset });
        }
        let end = bytes.len() - next.len();
        interpreter.token(token, offset, end)?;
        remaining = next;
    }

    interpreter.finish(bytes.len())
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "one flag per normalized inline mark"
)]
struct Style {
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    link: Option<String>,
}

impl Style {
    fn span(&self, text: String) -> DocSpan {
        DocSpan {
            text,
            bold: self.bold,
            italic: self.italic,
            code: false,
            strike: self.strike,
            underline: self.underline,
            link: self.link.clone(),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
enum GroupKind {
    #[default]
    Normal,
    Field {
        url: Option<String>,
    },
    FieldInstruction {
        text: String,
    },
    FieldResult,
}

#[derive(Clone, Debug)]
struct GroupState {
    style: Style,
    unicode_fallback: usize,
    skip: bool,
    at_start: bool,
    starred: bool,
    kind: GroupKind,
}

impl Default for GroupState {
    fn default() -> Self {
        Self {
            style: Style::default(),
            unicode_fallback: 1,
            skip: false,
            at_start: true,
            starred: false,
            kind: GroupKind::Normal,
        }
    }
}

#[derive(Default)]
struct Interpreter {
    groups: Vec<GroupState>,
    blocks: Vec<DocBlock>,
    spans: Vec<DocSpan>,
    paragraph_start: Option<usize>,
    paragraph_end: usize,
    code_page: u16,
    fallback_remaining: usize,
    pending_surrogate: Option<u16>,
    hex_bytes: Vec<u8>,
    hex_start: usize,
    hex_end: usize,
    hex_style: Option<Style>,
    saw_header: bool,
}

impl Interpreter {
    fn token(&mut self, token: Token, start: usize, end: usize) -> Result<(), DocumentParseError> {
        if !matches!(token, Token::ControlWord { ref name, .. } if name == "'") {
            self.flush_hex();
        }

        match token {
            Token::StartGroup => self.start_group(),
            Token::EndGroup => self.end_group(start)?,
            Token::Newline(_) | Token::ControlBin(_) => {}
            Token::ControlSymbol(symbol) => self.control_symbol(symbol, start, end),
            Token::ControlWord { name, arg } => self.control_word(&name, arg, start, end),
            Token::Text(bytes) => self.text(&bytes, start, end),
        }
        Ok(())
    }

    fn start_group(&mut self) {
        let inherited = self.groups.last().cloned().unwrap_or_default();
        self.groups.push(GroupState {
            at_start: true,
            starred: false,
            kind: GroupKind::Normal,
            ..inherited
        });
    }

    fn end_group(&mut self, offset: usize) -> Result<(), DocumentParseError> {
        self.flush_pending_surrogate(offset);
        let ended = self
            .groups
            .pop()
            .ok_or(DocumentParseError::UnbalancedGroup { offset })?;
        if let GroupKind::FieldInstruction { text } = ended.kind
            && let Some(field) = self
                .groups
                .iter_mut()
                .rev()
                .find(|group| matches!(group.kind, GroupKind::Field { .. }))
            && let GroupKind::Field { url } = &mut field.kind
        {
            *url = hyperlink_target(&text);
        }
        Ok(())
    }

    fn control_symbol(&mut self, symbol: char, start: usize, end: usize) {
        let Some(group) = self.groups.last_mut() else {
            return;
        };
        if symbol == '*' && group.at_start {
            group.starred = true;
            return;
        }
        group.at_start = false;
        if group.skip {
            return;
        }
        let text = match symbol {
            '\\' => "\\",
            '{' => "{",
            '}' => "}",
            '~' => "\u{a0}",
            '_' => "\u{2011}",
            '-' => "",
            _ => return,
        };
        self.emit(text, start, end);
    }

    fn control_word(&mut self, name: &str, arg: Option<i32>, start: usize, end: usize) {
        if name == "'" {
            self.hex(arg, start, end);
            return;
        }

        let at_start = self.groups.last().is_some_and(|group| group.at_start);
        if name == "rtf" && self.groups.len() == 1 && at_start {
            self.saw_header = true;
        }
        if at_start {
            self.open_destination(name);
        }
        let Some(group) = self.groups.last_mut() else {
            return;
        };
        group.at_start = false;
        if group.skip {
            return;
        }

        if matches!(group.kind, GroupKind::FieldInstruction { .. }) {
            return;
        }

        match name {
            "ansi" => self.code_page = 1252,
            "mac" => self.code_page = 10_000,
            "ansicpg" => {
                self.code_page = arg
                    .and_then(|value| u16::try_from(value).ok())
                    .unwrap_or(1252);
            }
            "b" => group.style.bold = enabled(arg),
            "i" => group.style.italic = enabled(arg),
            "ul" | "uld" | "uldash" | "uldashd" | "uldashdd" | "uldb" | "ulth" | "ulw" => {
                group.style.underline = enabled(arg);
            }
            "ulnone" => group.style.underline = false,
            "strike" => group.style.strike = enabled(arg),
            "plain" => {
                let link = group.style.link.take();
                group.style = Style {
                    link,
                    ..Style::default()
                };
            }
            "uc" => {
                group.unicode_fallback = arg
                    .and_then(|value| usize::try_from(value).ok())
                    .unwrap_or(1);
            }
            "u" => self.unicode(arg, start, end),
            "par" => self.flush_paragraph(end),
            "line" => self.emit("\n", start, end),
            "tab" => self.emit("\t", start, end),
            "emdash" => self.emit("—", start, end),
            "endash" => self.emit("–", start, end),
            "bullet" => self.emit("•", start, end),
            "lquote" => self.emit("‘", start, end),
            "rquote" => self.emit("’", start, end),
            "ldblquote" => self.emit("“", start, end),
            "rdblquote" => self.emit("”", start, end),
            _ => {}
        }
    }

    fn open_destination(&mut self, name: &str) {
        let field_url = if name == "fldrslt" {
            self.groups
                .iter()
                .rev()
                .find_map(|group| match &group.kind {
                    GroupKind::Field { url } => url.clone(),
                    GroupKind::Normal
                    | GroupKind::FieldInstruction { .. }
                    | GroupKind::FieldResult => None,
                })
        } else {
            None
        };
        let Some(group) = self.groups.last_mut() else {
            return;
        };
        match name {
            "field" => group.kind = GroupKind::Field { url: None },
            "fldinst" => {
                group.kind = GroupKind::FieldInstruction {
                    text: String::new(),
                }
            }
            "fldrslt" => {
                group.kind = GroupKind::FieldResult;
                group.style.link = field_url;
            }
            name if group.starred || ignored_destination(name) => group.skip = true,
            _ => {}
        }
    }

    fn text(&mut self, bytes: &[u8], start: usize, end: usize) {
        let Some(group) = self.groups.last() else {
            return;
        };
        if group.skip {
            return;
        }
        let skip = self.fallback_remaining.min(bytes.len());
        self.fallback_remaining -= skip;
        let bytes = &bytes[skip..];
        if bytes.is_empty() {
            return;
        }
        let decoded = decode(bytes, self.code_page);
        if let Some(GroupState {
            kind: GroupKind::FieldInstruction { text },
            ..
        }) = self.groups.last_mut()
        {
            text.push_str(&decoded);
        } else {
            self.flush_pending_surrogate(start);
            self.emit(&decoded, start + skip, end);
        }
    }

    fn hex(&mut self, arg: Option<i32>, start: usize, end: usize) {
        if self.groups.last().is_none_or(|group| group.skip) {
            return;
        }
        if self.fallback_remaining > 0 {
            self.fallback_remaining -= 1;
            return;
        }
        let Some(byte) = arg.and_then(|value| u8::try_from(value).ok()) else {
            return;
        };
        let style = self.groups.last().map(|group| group.style.clone());
        if !self.hex_bytes.is_empty() && self.hex_style != style {
            self.flush_hex();
        }
        if self.hex_bytes.is_empty() {
            self.hex_start = start;
            self.hex_style = style;
        }
        self.hex_end = end;
        self.hex_bytes.push(byte);
    }

    fn flush_hex(&mut self) {
        if self.hex_bytes.is_empty() {
            return;
        }
        let decoded = decode(&self.hex_bytes, self.code_page);
        self.hex_bytes.clear();
        if let Some(GroupState {
            kind: GroupKind::FieldInstruction { text },
            ..
        }) = self.groups.last_mut()
        {
            text.push_str(&decoded);
        } else {
            let style = self.hex_style.take();
            self.emit_with_style(&decoded, self.hex_start, self.hex_end, style);
        }
    }

    fn unicode(&mut self, arg: Option<i32>, start: usize, end: usize) {
        let Some(value) = arg else {
            return;
        };
        let Ok(unit) = i16::try_from(value).map(i16::cast_unsigned) else {
            return;
        };
        self.fallback_remaining = self.groups.last().map_or(1, |group| group.unicode_fallback);
        if (0xD800..=0xDBFF).contains(&unit) {
            self.flush_pending_surrogate(start);
            self.pending_surrogate = Some(unit);
        } else if (0xDC00..=0xDFFF).contains(&unit) {
            if let Some(high) = self.pending_surrogate.take() {
                let scalar = 0x1_0000 + (u32::from(high - 0xD800) << 10) + u32::from(unit - 0xDC00);
                if let Some(character) = char::from_u32(scalar) {
                    self.emit(&character.to_string(), start, end);
                }
            } else {
                self.emit("�", start, end);
            }
        } else {
            self.flush_pending_surrogate(start);
            if let Some(character) = char::from_u32(u32::from(unit)) {
                self.emit(&character.to_string(), start, end);
            }
        }
    }

    fn flush_pending_surrogate(&mut self, offset: usize) {
        if self.pending_surrogate.take().is_some() {
            self.emit("�", offset, offset);
        }
    }

    fn emit(&mut self, text: &str, start: usize, end: usize) {
        let style = self.groups.last().map(|group| group.style.clone());
        self.emit_with_style(text, start, end, style);
    }

    fn emit_with_style(&mut self, text: &str, start: usize, end: usize, style: Option<Style>) {
        if text.is_empty() {
            return;
        }
        let style = style.unwrap_or_default();
        self.paragraph_start.get_or_insert(start);
        self.paragraph_end = self.paragraph_end.max(end);
        if let Some(last) = self.spans.last_mut()
            && last.bold == style.bold
            && last.italic == style.italic
            && last.underline == style.underline
            && last.strike == style.strike
            && last.link == style.link
        {
            last.text.push_str(text);
        } else {
            self.spans.push(style.span(text.to_owned()));
        }
    }

    fn flush_paragraph(&mut self, end: usize) {
        self.flush_pending_surrogate(end);
        if self.spans.is_empty() {
            return;
        }
        let start = self.paragraph_start.take().unwrap_or(end);
        self.blocks.push(DocBlock {
            kind: DocBlockKind::Paragraph {
                spans: std::mem::take(&mut self.spans),
            },
            source_range: start..end.max(self.paragraph_end),
        });
        self.paragraph_end = 0;
    }

    fn finish(mut self, offset: usize) -> Result<Vec<DocBlock>, DocumentParseError> {
        self.flush_hex();
        self.flush_pending_surrogate(offset);
        if !self.groups.is_empty() {
            return Err(DocumentParseError::UnbalancedGroup { offset });
        }
        if !self.saw_header {
            return Err(DocumentParseError::MissingHeader);
        }
        self.flush_paragraph(offset);
        Ok(self.blocks)
    }
}

fn enabled(arg: Option<i32>) -> bool {
    arg != Some(0)
}

fn ignored_destination(name: &str) -> bool {
    matches!(
        name,
        "fonttbl"
            | "colortbl"
            | "stylesheet"
            | "info"
            | "pict"
            | "object"
            | "objdata"
            | "header"
            | "headerl"
            | "headerr"
            | "footer"
            | "footerl"
            | "footerr"
            | "footnote"
            | "annotation"
            | "datastore"
            | "themedata"
            | "colorschememapping"
            | "generator"
            | "listtable"
            | "listoverridetable"
            | "revtbl"
            | "rsidtbl"
            | "xmlnstbl"
            | "mmathpr"
            | "shp"
            | "shpinst"
            | "shprslt"
            | "nonshppict"
    )
}

fn encoding(code_page: u16) -> &'static Encoding {
    match code_page {
        65001 => encoding_rs::UTF_8,
        1250 => encoding_rs::WINDOWS_1250,
        1251 => encoding_rs::WINDOWS_1251,
        1252 => encoding_rs::WINDOWS_1252,
        1253 => encoding_rs::WINDOWS_1253,
        1254 => encoding_rs::WINDOWS_1254,
        1255 => encoding_rs::WINDOWS_1255,
        1256 => encoding_rs::WINDOWS_1256,
        932 => encoding_rs::SHIFT_JIS,
        936 => encoding_rs::GBK,
        949 => encoding_rs::EUC_KR,
        950 => encoding_rs::BIG5,
        10_000 => encoding_rs::MACINTOSH,
        _ => WINDOWS_1252,
    }
}

fn decode(bytes: &[u8], code_page: u16) -> String {
    let (text, _, _) = encoding(code_page).decode(bytes);
    text.into_owned()
}

fn hyperlink_target(instruction: &str) -> Option<String> {
    let instruction = instruction.trim();
    let rest = instruction.strip_prefix("HYPERLINK")?.trim_start();
    if let Some(quoted) = rest.strip_prefix('"') {
        let end = quoted.find('"')?;
        Some(quoted[..end].to_owned())
    } else {
        rest.split_whitespace()
            .next()
            .filter(|url| !url.is_empty())
            .map(str::to_owned)
    }
}

#[cfg(test)]
mod tests {
    use super::{DocumentParseError, parse_rtf};
    use crate::{DocBlock, DocBlockKind, DocSpan};

    fn paragraphs(input: &[u8]) -> Result<Vec<Vec<DocSpan>>, DocumentParseError> {
        parse_rtf(input).map(|blocks| {
            blocks
                .into_iter()
                .filter_map(|block| match block.kind {
                    DocBlockKind::Paragraph { spans } => Some(spans),
                    _ => None,
                })
                .collect()
        })
    }

    #[test]
    fn parses_paragraphs_and_nested_styles() {
        let parsed =
            paragraphs(br"{\rtf1\ansi One {\b bold {\i both} bold} plain\par Two}").unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(
            parsed[0]
                .iter()
                .map(|span| span.text.as_str())
                .collect::<String>(),
            "One bold both bold plain"
        );
        assert!(parsed[0][1].bold);
        assert!(parsed[0][2].bold && parsed[0][2].italic);
        assert!(parsed[0][3].bold);
        assert_eq!(parsed[1][0].text, "Two");
    }

    #[test]
    fn parses_underlining_strike_and_special_characters() {
        let parsed =
            paragraphs(br"{\rtf1 {\ul under}\tab {\strike struck}\line x\emdash y}").unwrap();
        assert!(parsed[0][0].underline);
        assert!(
            parsed[0]
                .iter()
                .any(|span| span.strike && span.text == "struck")
        );
        let text = parsed[0]
            .iter()
            .map(|span| span.text.as_str())
            .collect::<String>();
        assert_eq!(text, "under\tstruck\nx—y");
    }

    #[test]
    fn parses_unicode_fallback_surrogates_and_code_pages() {
        let parsed =
            paragraphs(br"{\rtf1\ansi\ansicpg1252 caf\'e9 \uc1\u8212? \u-10179?\u-8704?}").unwrap();
        let text = parsed[0]
            .iter()
            .map(|span| span.text.as_str())
            .collect::<String>();
        assert_eq!(text, "café — 😀");
    }

    #[test]
    fn parses_hyperlink_result_and_skips_instruction() {
        let parsed = paragraphs(br#"{\rtf1 See {\field{\*\fldinst HYPERLINK "https://example.com"}{\fldrslt\plain example}}.}"#).unwrap();
        assert_eq!(
            parsed[0]
                .iter()
                .map(|span| span.text.as_str())
                .collect::<String>(),
            "See example."
        );
        assert_eq!(parsed[0][1].link.as_deref(), Some("https://example.com"));
    }

    #[test]
    fn skips_metadata_pictures_objects_and_unknown_destinations() {
        let parsed = paragraphs(
            br"{\rtf1{\fonttbl hidden}{\pict 00ff}{\object nope}{\*\unknown secret}Visible}",
        )
        .unwrap();
        assert_eq!(parsed[0][0].text, "Visible");
    }

    #[test]
    fn rejects_unbalanced_groups() {
        assert!(matches!(
            parse_rtf(br"{\rtf1 broken"),
            Err(DocumentParseError::UnbalancedGroup { .. })
        ));
        assert!(matches!(
            parse_rtf(br"{\rtf1} }"),
            Err(DocumentParseError::UnbalancedGroup { .. })
        ));
    }

    #[test]
    fn rejects_non_rtf_text() {
        assert_eq!(
            parse_rtf(b"plain text"),
            Err(DocumentParseError::MissingHeader)
        );
        assert_eq!(
            parse_rtf(br"{not RTF}"),
            Err(DocumentParseError::MissingHeader)
        );
    }

    #[test]
    fn source_ranges_are_valid() {
        let input = br"{\rtf1 first\par second}";
        let blocks = parse_rtf(input).unwrap();
        assert!(
            blocks
                .iter()
                .all(
                    |DocBlock { source_range, .. }| source_range.start <= source_range.end
                        && source_range.end <= input.len()
                )
        );
    }

    #[test]
    fn deeply_nested_groups_do_not_recurse() {
        let mut input = Vec::from(&b"{\\rtf1 "[..]);
        input.extend(std::iter::repeat_n(b'{', 10_000));
        input.extend_from_slice(b"text");
        input.extend(std::iter::repeat_n(b'}', 10_001));
        assert_eq!(paragraphs(&input).unwrap()[0][0].text, "text");
    }
}
