//! A deliberately narrow RTF interpreter for readable diff previews.

use encoding_rs::{Encoding, WINDOWS_1252};
use rtf_grimoire::tokenizer::{Token, read_token};

use crate::{
    DocBlock, DocBlockKind, DocColor, DocSpan, DocumentKind, EmbeddedImageFormat, EmbeddedMedia,
    ParagraphAlignment, ParagraphStyle, VerticalPosition, parse_markdown,
};

const EMBEDDED_MEDIA_LIMIT: usize = 20 * 1024 * 1024;
const EMBEDDED_MEDIA_TOTAL_LIMIT: usize = 50 * 1024 * 1024;

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
    let mut interpreter = Interpreter::new(Catalogs::parse(bytes));
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
    foreground: Option<usize>,
    highlight: Option<usize>,
    font: Option<i32>,
    font_size_half_points: Option<u16>,
    small_caps: bool,
    vertical: VerticalPosition,
}

impl Style {
    fn span(&self, text: String, catalogs: &Catalogs) -> DocSpan {
        DocSpan {
            text,
            bold: self.bold,
            italic: self.italic,
            code: false,
            strike: self.strike,
            underline: self.underline,
            link: self.link.clone(),
            foreground: self
                .foreground
                .and_then(|index| catalogs.colors.get(index).copied()),
            highlight: self
                .highlight
                .and_then(|index| catalogs.colors.get(index).copied()),
            font_family: self.font.and_then(|index| {
                catalogs
                    .fonts
                    .iter()
                    .find(|(id, _)| *id == index)
                    .map(|(_, name)| name.clone())
            }),
            font_size_half_points: self.font_size_half_points,
            small_caps: self.small_caps,
            vertical: self.vertical,
        }
    }
}

#[derive(Clone, Debug, Default)]
struct Catalogs {
    fonts: Vec<(i32, String)>,
    colors: Vec<DocColor>,
}

impl Catalogs {
    fn parse(bytes: &[u8]) -> Self {
        Self {
            fonts: destination(bytes, b"fonttbl").map_or_else(Vec::new, parse_fonts),
            colors: destination(bytes, b"colortbl").map_or_else(Vec::new, parse_colors),
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
    paragraph: ParagraphStyle,
    heading_level: Option<u8>,
    list_level: Option<u8>,
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
            paragraph: ParagraphStyle::default(),
            heading_level: None,
            list_level: None,
        }
    }
}

struct Interpreter {
    groups: Vec<GroupState>,
    blocks: Vec<DocBlock>,
    spans: Vec<DocSpan>,
    paragraph_start: Option<usize>,
    paragraph_end: usize,
    paragraph_style: Option<ParagraphStyle>,
    paragraph_heading: Option<u8>,
    paragraph_list_level: Option<u8>,
    code_page: u16,
    fallback_remaining: usize,
    pending_surrogate: Option<u16>,
    hex_bytes: Vec<u8>,
    hex_start: usize,
    hex_end: usize,
    hex_style: Option<Style>,
    saw_header: bool,
    catalogs: Catalogs,
    media_bytes: usize,
    picture: Option<Picture>,
    aside: Option<AsideCapture>,
    table: Option<TableCapture>,
}

#[derive(Clone, Debug, Default)]
struct Picture {
    depth: usize,
    format: Option<EmbeddedImageFormat>,
    unsupported: Option<&'static str>,
    hex: Vec<u8>,
    nibble: Option<u8>,
    start: usize,
}

#[derive(Clone, Debug)]
struct AsideCapture {
    depth: usize,
    label: &'static str,
    block_start: usize,
    start: usize,
}

#[derive(Clone, Debug, Default)]
struct TableCapture {
    start: usize,
    cells: Vec<Vec<DocSpan>>,
}

impl Interpreter {
    fn new(catalogs: Catalogs) -> Self {
        Self {
            catalogs,
            code_page: 1252,
            ..Self::default_fields()
        }
    }

    fn default_fields() -> Self {
        Self {
            groups: Vec::new(),
            blocks: Vec::new(),
            spans: Vec::new(),
            paragraph_start: None,
            paragraph_end: 0,
            paragraph_style: None,
            paragraph_heading: None,
            paragraph_list_level: None,
            code_page: 0,
            fallback_remaining: 0,
            pending_surrogate: None,
            hex_bytes: Vec::new(),
            hex_start: 0,
            hex_end: 0,
            hex_style: None,
            saw_header: false,
            catalogs: Catalogs::default(),
            media_bytes: 0,
            picture: None,
            aside: None,
            table: None,
        }
    }
    fn token(&mut self, token: Token, start: usize, end: usize) -> Result<(), DocumentParseError> {
        if !matches!(token, Token::ControlWord { ref name, .. } if name == "'") {
            self.flush_hex();
        }

        match token {
            Token::StartGroup => self.start_group(),
            Token::EndGroup => self.end_group(start)?,
            Token::Newline(_) => {}
            Token::ControlBin(bytes) => self.binary(&bytes, start, end),
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
        if self
            .picture
            .as_ref()
            .is_some_and(|picture| picture.depth == self.groups.len() + 1)
        {
            self.finish_picture(offset);
        }
        if self
            .aside
            .as_ref()
            .is_some_and(|aside| aside.depth == self.groups.len() + 1)
        {
            self.finish_aside(offset);
        }
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

    #[expect(
        clippy::too_many_lines,
        reason = "one flat RTF control-word dispatch is easier to audit against the format"
    )]
    fn control_word(&mut self, name: &str, arg: Option<i32>, start: usize, end: usize) {
        if let Some(picture) = &mut self.picture {
            match name {
                "pngblip" => picture.format = Some(EmbeddedImageFormat::Png),
                "jpegblip" => {
                    picture.format = Some(EmbeddedImageFormat::Jpeg);
                }
                "emfblip" => picture.unsupported = Some("EMF image"),
                "wmetafile" => picture.unsupported = Some("WMF image"),
                _ => {}
            }
            return;
        }
        if name == "'" {
            self.hex(arg, start, end);
            return;
        }

        let at_start = self.groups.last().is_some_and(|group| group.at_start);
        if name == "rtf" && self.groups.len() == 1 && at_start {
            self.saw_header = true;
        }
        if at_start {
            if matches!(
                name,
                "pict"
                    | "header"
                    | "headerl"
                    | "headerr"
                    | "footer"
                    | "footerl"
                    | "footerr"
                    | "footnote"
            ) {
                self.flush_paragraph(start);
            }
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
            "f" => group.style.font = arg,
            "fs" => {
                group.style.font_size_half_points = arg.and_then(|value| u16::try_from(value).ok());
            }
            "cf" => group.style.foreground = arg.and_then(|value| usize::try_from(value).ok()),
            "highlight" => {
                group.style.highlight = arg.and_then(|value| usize::try_from(value).ok());
            }
            "scaps" => group.style.small_caps = enabled(arg),
            "super" => group.style.vertical = VerticalPosition::Superscript,
            "sub" => group.style.vertical = VerticalPosition::Subscript,
            "nosupersub" => group.style.vertical = VerticalPosition::Normal,
            "ql" => group.paragraph.alignment = ParagraphAlignment::Left,
            "qc" => group.paragraph.alignment = ParagraphAlignment::Center,
            "qr" => group.paragraph.alignment = ParagraphAlignment::Right,
            "qj" => group.paragraph.alignment = ParagraphAlignment::Justify,
            "li" => group.paragraph.left_indent_twips = arg.unwrap_or_default(),
            "ri" => group.paragraph.right_indent_twips = arg.unwrap_or_default(),
            "fi" => group.paragraph.first_line_indent_twips = arg.unwrap_or_default(),
            "sb" => group.paragraph.space_before_twips = arg.unwrap_or_default(),
            "sa" => group.paragraph.space_after_twips = arg.unwrap_or_default(),
            "pard" => {
                group.paragraph = ParagraphStyle::default();
                group.heading_level = None;
                group.list_level = None;
            }
            "outlinelevel" => {
                group.heading_level = arg
                    .and_then(|value| u8::try_from(value + 1).ok())
                    .filter(|level| (1..=6).contains(level));
            }
            "ilvl" => group.list_level = arg.and_then(|value| u8::try_from(value).ok()),
            "ls" => {
                group.list_level.get_or_insert(0);
            }
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
            "page" | "sect" => {
                self.flush_paragraph(start);
                self.blocks.push(DocBlock {
                    kind: DocBlockKind::PageBreak,
                    source_range: start..end,
                });
            }
            "trowd" => {
                self.flush_paragraph(start);
                self.table = Some(TableCapture {
                    start,
                    ..TableCapture::default()
                });
            }
            "cell" => self.finish_cell(),
            "row" => self.finish_table(end),
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
        let depth = self.groups.len();
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
            "pict" => {
                group.skip = true;
                self.picture = Some(Picture {
                    depth,
                    start: 0,
                    ..Picture::default()
                });
            }
            "header" | "headerl" | "headerr" => {
                self.aside = Some(AsideCapture {
                    depth,
                    label: "Header",
                    block_start: self.blocks.len(),
                    start: 0,
                });
            }
            "footer" | "footerl" | "footerr" => {
                self.aside = Some(AsideCapture {
                    depth,
                    label: "Footer",
                    block_start: self.blocks.len(),
                    start: 0,
                });
            }
            "footnote" => {
                self.aside = Some(AsideCapture {
                    depth,
                    label: "Footnote",
                    block_start: self.blocks.len(),
                    start: 0,
                });
            }
            name if group.starred || ignored_destination(name) => group.skip = true,
            _ => {}
        }
    }

    fn text(&mut self, bytes: &[u8], start: usize, end: usize) {
        if let Some(picture) = &mut self.picture {
            if picture.start == 0 {
                picture.start = start;
            }
            for byte in bytes
                .iter()
                .copied()
                .filter(|byte| !byte.is_ascii_whitespace())
            {
                let Some(value) = hex_value(byte) else {
                    continue;
                };
                if let Some(high) = picture.nibble.take() {
                    if picture.hex.len() < EMBEDDED_MEDIA_LIMIT {
                        picture.hex.push((high << 4) | value);
                    }
                } else {
                    picture.nibble = Some(value);
                }
            }
            self.paragraph_end = self.paragraph_end.max(end);
            return;
        }
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

    fn binary(&mut self, bytes: &[u8], start: usize, end: usize) {
        let Some(picture) = &mut self.picture else {
            return;
        };
        if picture.start == 0 {
            picture.start = start;
        }
        let remaining = EMBEDDED_MEDIA_LIMIT.saturating_sub(picture.hex.len());
        picture
            .hex
            .extend_from_slice(&bytes[..bytes.len().min(remaining)]);
        self.paragraph_end = self.paragraph_end.max(end);
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
        if self.paragraph_style.is_none()
            && let Some(group) = self.groups.last()
        {
            self.paragraph_style = Some(group.paragraph);
            self.paragraph_heading = group.heading_level;
            self.paragraph_list_level = group.list_level;
        }
        if let Some(aside) = &mut self.aside
            && aside.start == 0
        {
            aside.start = start;
        }
        self.paragraph_end = self.paragraph_end.max(end);
        if let Some(last) = self.spans.last_mut()
            && last.bold == style.bold
            && last.italic == style.italic
            && last.underline == style.underline
            && last.strike == style.strike
            && last.link == style.link
            && last.foreground
                == style
                    .foreground
                    .and_then(|index| self.catalogs.colors.get(index).copied())
            && last.highlight
                == style
                    .highlight
                    .and_then(|index| self.catalogs.colors.get(index).copied())
            && last.font_family.as_deref()
                == style.font.and_then(|index| {
                    self.catalogs
                        .fonts
                        .iter()
                        .find(|(id, _)| *id == index)
                        .map(|(_, name)| name.as_str())
                })
            && last.font_size_half_points == style.font_size_half_points
            && last.small_caps == style.small_caps
            && last.vertical == style.vertical
        {
            last.text.push_str(text);
        } else {
            self.spans.push(style.span(text.to_owned(), &self.catalogs));
        }
    }

    fn flush_paragraph(&mut self, end: usize) {
        self.flush_pending_surrogate(end);
        if self.spans.is_empty() {
            return;
        }
        let start = self.paragraph_start.take().unwrap_or(end);
        if self.table.is_some() {
            self.finish_cell();
            self.paragraph_start = None;
            self.paragraph_end = 0;
            self.paragraph_style = None;
            self.paragraph_heading = None;
            self.paragraph_list_level = None;
            return;
        }
        let style = self.paragraph_style.take().unwrap_or_default();
        let heading = self.paragraph_heading.take();
        let list_level = self.paragraph_list_level.take();
        let spans = std::mem::take(&mut self.spans);
        let source_range = start..end.max(self.paragraph_end);
        if let Some(level) = heading {
            self.blocks.push(DocBlock {
                kind: DocBlockKind::Heading { level, spans },
                source_range,
            });
        } else if list_level.is_some() {
            let item = vec![DocBlock {
                kind: DocBlockKind::RichParagraph { spans, style },
                source_range: source_range.clone(),
            }];
            if let Some(DocBlock {
                kind:
                    DocBlockKind::List {
                        ordered: false,
                        items,
                    },
                source_range: range,
            }) = self.blocks.last_mut()
            {
                items.push(item);
                range.end = source_range.end;
            } else {
                self.blocks.push(DocBlock {
                    kind: DocBlockKind::List {
                        ordered: false,
                        items: vec![item],
                    },
                    source_range,
                });
            }
        } else {
            self.blocks.push(DocBlock {
                kind: DocBlockKind::RichParagraph { spans, style },
                source_range,
            });
        }
        self.paragraph_end = 0;
    }

    fn finish_cell(&mut self) {
        if let Some(table) = &mut self.table
            && !self.spans.is_empty()
        {
            table.cells.push(std::mem::take(&mut self.spans));
            self.paragraph_start = None;
            self.paragraph_end = 0;
            self.paragraph_style = None;
            self.paragraph_heading = None;
            self.paragraph_list_level = None;
        }
    }

    fn finish_table(&mut self, end: usize) {
        self.finish_cell();
        let Some(table) = self.table.take() else {
            return;
        };
        if table.cells.is_empty() {
            return;
        }
        self.blocks.push(DocBlock {
            kind: DocBlockKind::Table {
                alignments: vec![crate::CellAlignment::Left; table.cells.len()],
                header: Vec::new(),
                rows: vec![table.cells],
            },
            source_range: table.start..end,
        });
    }

    fn finish_aside(&mut self, end: usize) {
        self.flush_paragraph(end);
        let Some(aside) = self.aside.take() else {
            return;
        };
        let blocks = self.blocks.split_off(aside.block_start);
        if !blocks.is_empty() {
            self.blocks.push(DocBlock {
                kind: DocBlockKind::Aside {
                    label: aside.label.to_owned(),
                    blocks,
                },
                source_range: aside.start..end,
            });
        }
    }

    fn finish_picture(&mut self, end: usize) {
        let Some(picture) = self.picture.take() else {
            return;
        };
        self.flush_paragraph(picture.start);
        let media = if picture.nibble.is_some() {
            EmbeddedMedia::Unsupported {
                label: String::from("malformed embedded image"),
            }
        } else if picture.hex.len() >= EMBEDDED_MEDIA_LIMIT
            || self.media_bytes.saturating_add(picture.hex.len()) > EMBEDDED_MEDIA_TOTAL_LIMIT
        {
            EmbeddedMedia::Unsupported {
                label: String::from("embedded image exceeds preview limit"),
            }
        } else if let Some(label) = picture.unsupported {
            EmbeddedMedia::Unsupported {
                label: label.to_owned(),
            }
        } else if let Some(format) = picture.format {
            self.media_bytes += picture.hex.len();
            EmbeddedMedia::Image {
                format,
                bytes: picture.hex,
            }
        } else {
            EmbeddedMedia::Unsupported {
                label: String::from("unsupported embedded image"),
            }
        };
        self.blocks.push(DocBlock {
            kind: DocBlockKind::EmbeddedMedia {
                media,
                alt: String::from("Embedded RTF image"),
            },
            source_range: picture.start..end,
        });
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

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Finds one top-level destination without allocating or recursively parsing.
fn destination<'a>(bytes: &'a [u8], name: &[u8]) -> Option<&'a [u8]> {
    let needle = [b"{\\".as_slice(), name].concat();
    let start = bytes
        .windows(needle.len())
        .position(|window| window == needle)?;
    let mut depth = 0usize;
    let mut escaped = false;
    for (index, &byte) in bytes[start..].iter().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        if byte == b'\\' {
            escaped = true;
        } else if byte == b'{' {
            depth += 1;
        } else if byte == b'}' {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return bytes.get(start..=start + index);
            }
        }
    }
    None
}

fn parse_fonts(bytes: &[u8]) -> Vec<(i32, String)> {
    let text = String::from_utf8_lossy(bytes);
    text.split(';')
        .filter_map(|entry| {
            let marker = entry.match_indices("\\f").find_map(|(index, _)| {
                entry
                    .as_bytes()
                    .get(index + 2)
                    .is_some_and(u8::is_ascii_digit)
                    .then_some(index + 2)
            })?;
            let digits = entry[marker..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>();
            let id = digits.parse().ok()?;
            let name = entry
                .split(' ')
                .next_back()?
                .trim_matches(|character: char| matches!(character, '{' | '}' | '\n' | '\r'));
            (!name.is_empty()).then(|| (id, name.to_owned()))
        })
        .collect()
}

fn parse_colors(bytes: &[u8]) -> Vec<DocColor> {
    let text = String::from_utf8_lossy(bytes);
    text.split(';')
        .map(|entry| DocColor {
            red: color_component(entry, "\\red"),
            green: color_component(entry, "\\green"),
            blue: color_component(entry, "\\blue"),
        })
        .collect()
}

fn color_component(entry: &str, marker: &str) -> u8 {
    entry
        .find(marker)
        .and_then(|start| {
            entry[start + marker.len()..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse()
                .ok()
        })
        .unwrap_or_default()
}

fn ignored_destination(name: &str) -> bool {
    matches!(
        name,
        "fonttbl"
            | "colortbl"
            | "stylesheet"
            | "info"
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
                    DocBlockKind::Paragraph { spans }
                    | DocBlockKind::RichParagraph { spans, .. } => Some(spans),
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

    #[test]
    fn retains_catalog_colors_fonts_and_paragraph_layout() {
        let blocks = parse_rtf(br"{\rtf1{\fonttbl{\f0 Helvetica;}}{\colortbl;\red240\green20\blue40;}\qc\li300\sa120\f0\fs28\cf1\highlight1 Hello}").unwrap();
        let DocBlockKind::RichParagraph { spans, style } = &blocks[0].kind else {
            panic!("expected rich paragraph");
        };
        assert_eq!(style.alignment, crate::ParagraphAlignment::Center);
        assert_eq!(style.left_indent_twips, 300);
        assert_eq!(style.space_after_twips, 120);
        assert_eq!(spans[0].font_family.as_deref(), Some("Helvetica"));
        assert_eq!(spans[0].font_size_half_points, Some(28));
        assert_eq!(
            spans[0].foreground,
            Some(crate::DocColor {
                red: 240,
                green: 20,
                blue: 40
            })
        );
        assert_eq!(spans[0].highlight, spans[0].foreground);
    }

    #[test]
    fn retains_headings_lists_tables_and_page_breaks() {
        let blocks = parse_rtf(br"{\rtf1\outlinelevel1 Heading\par\pard\ls1\ilvl0 One\par Two\par\pard\trowd A\cell B\cell\row\page End}").unwrap();
        assert!(matches!(
            blocks[0].kind,
            DocBlockKind::Heading { level: 2, .. }
        ));
        assert!(matches!(blocks[1].kind, DocBlockKind::List { ref items, .. } if items.len() == 2));
        assert!(
            matches!(blocks[2].kind, DocBlockKind::Table { ref rows, .. } if rows[0].len() == 2)
        );
        assert!(
            blocks
                .iter()
                .any(|block| matches!(block.kind, DocBlockKind::PageBreak))
        );
    }

    #[test]
    fn decodes_supported_pictures_and_labels_unsupported_ones() {
        let blocks = parse_rtf(br"{\rtf1{\pict\pngblip 89504e47}{\pict\emfblip 0102}}").unwrap();
        assert!(
            matches!(blocks[0].kind, DocBlockKind::EmbeddedMedia { media: crate::EmbeddedMedia::Image { format: crate::EmbeddedImageFormat::Png, ref bytes }, .. } if bytes == &[0x89, 0x50, 0x4e, 0x47])
        );
        assert!(
            matches!(blocks[1].kind, DocBlockKind::EmbeddedMedia { media: crate::EmbeddedMedia::Unsupported { ref label }, .. } if label == "EMF image")
        );
    }

    #[test]
    fn accepts_binary_picture_payloads() {
        let blocks = parse_rtf(br"{\rtf1{\pict\jpegblip\bin4 ABCD}}").unwrap();
        assert!(
            matches!(blocks[0].kind, DocBlockKind::EmbeddedMedia { media: crate::EmbeddedMedia::Image { format: crate::EmbeddedImageFormat::Jpeg, ref bytes }, .. } if bytes == b"ABCD")
        );
    }

    #[test]
    fn captures_secondary_document_areas_as_labeled_asides() {
        let blocks =
            parse_rtf(br"{\rtf1{\header Running title\par}Body{\footnote Note text}}").unwrap();
        assert!(
            matches!(blocks[0].kind, DocBlockKind::Aside { ref label, .. } if label == "Header")
        );
        assert!(blocks.iter().any(|block| matches!(block.kind, DocBlockKind::Aside { ref label, .. } if label == "Footnote")));
    }
}
