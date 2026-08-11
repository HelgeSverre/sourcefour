//! The block tree a preview renders, and the queries a renderer runs on it.

use std::ops::Range;

/// One rendered block, with the byte range of the source that produced it.
///
/// The range is unused by rendering today and load-bearing tomorrow: diff
/// change indicators will map changed source lines onto blocks through it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocBlock {
    /// What the block renders as.
    pub kind: DocBlockKind,
    /// Byte range in the parsed source, always a valid slice of it.
    pub source_range: Range<usize>,
}

/// The block shapes a preview can render.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DocBlockKind {
    /// A section heading, `level` in `1..=6`.
    Heading {
        /// Heading depth, `1..=6`.
        level: u8,
        /// Heading text, split by style.
        spans: Vec<DocSpan>,
    },
    /// A run of prose.
    Paragraph {
        /// Paragraph text, split by style.
        spans: Vec<DocSpan>,
    },
    /// A paragraph carrying layout metadata supplied by a rich-text format.
    RichParagraph {
        /// Paragraph text, split by style.
        spans: Vec<DocSpan>,
        /// Layout hints normalized for the preview renderer.
        style: ParagraphStyle,
    },
    /// Preformatted text, verbatim except for a stripped trailing newline.
    Code {
        /// Info-string language tag, `None` when untagged or indented.
        language: Option<String>,
        /// Code text without the fences.
        text: String,
    },
    /// A quotation, holding blocks of its own.
    Quote {
        /// Quoted blocks.
        blocks: Vec<DocBlock>,
    },
    /// A list, each item holding blocks of its own.
    List {
        /// Whether the list is numbered.
        ordered: bool,
        /// One block sequence per item.
        items: Vec<Vec<DocBlock>>,
    },
    /// A thematic break.
    Rule,
    /// A page or section boundary in a paginated source document.
    PageBreak,
    /// Secondary document content such as a header, footer, or footnote.
    Aside {
        /// Human-readable role of the content.
        label: String,
        /// Captured content in source order.
        blocks: Vec<DocBlock>,
    },
    /// Media stored inside the document rather than referenced by path.
    EmbeddedMedia {
        /// Media payload, or a reason it cannot be displayed.
        media: EmbeddedMedia,
        /// Accessible fallback text.
        alt: String,
    },
    /// An image, always its own block even when written inside a paragraph.
    Image {
        /// Destination as written, already resolved for reference-style images.
        src: String,
        /// Alt text, empty when the image has none.
        alt: String,
    },
    /// A table, its cells holding spans rather than blocks.
    ///
    /// A cell is inline content only — Markdown gives it no way to hold a
    /// list or a code block, so nothing here recurses.
    Table {
        /// One entry per column, from the delimiter row.
        alignments: Vec<CellAlignment>,
        /// One spans-vec per header cell.
        header: Vec<Vec<DocSpan>>,
        /// Body rows, each a row of cells, each cell a run of spans. A row
        /// may be shorter than `alignments`; the source decides.
        rows: Vec<Vec<Vec<DocSpan>>>,
    },
}

/// Paragraph layout retained from rich-text documents.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ParagraphStyle {
    /// Horizontal text alignment.
    pub alignment: ParagraphAlignment,
    /// Left indentation in twips (1/1440 inch).
    pub left_indent_twips: i32,
    /// Right indentation in twips.
    pub right_indent_twips: i32,
    /// First-line indentation in twips; negative values are hanging indents.
    pub first_line_indent_twips: i32,
    /// Space before the paragraph in twips.
    pub space_before_twips: i32,
    /// Space after the paragraph in twips.
    pub space_after_twips: i32,
}

/// Horizontal alignment requested by a rich-text paragraph.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ParagraphAlignment {
    /// Leading-edge alignment.
    #[default]
    Left,
    /// Centered text.
    Center,
    /// Trailing-edge alignment.
    Right,
    /// Justified text; renderers may normalize this to leading-edge alignment.
    Justify,
}

/// An RGB color declared by a document.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DocColor {
    /// Red channel.
    pub red: u8,
    /// Green channel.
    pub green: u8,
    /// Blue channel.
    pub blue: u8,
}

/// Baseline semantics retained even when the renderer normalizes their size.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum VerticalPosition {
    /// Ordinary baseline.
    #[default]
    Normal,
    /// Superscript text.
    Superscript,
    /// Subscript text.
    Subscript,
}

/// Media embedded in a rich-text document.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EmbeddedMedia {
    /// A PNG or JPEG payload the renderer can draw.
    Image {
        /// Encoded image format.
        format: EmbeddedImageFormat,
        /// Encoded image bytes.
        bytes: Vec<u8>,
    },
    /// An object that is valid RTF but unsupported by the preview.
    Unsupported {
        /// Short format or failure description.
        label: String,
    },
}

/// Embedded raster formats GPUI can decode directly.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmbeddedImageFormat {
    /// Portable Network Graphics.
    Png,
    /// JPEG image.
    Jpeg,
}

/// One table column's alignment, from the delimiter row.
///
/// An unmarked column is `Left`, which is what a renderer would do with it
/// anyway; the model carries no "unspecified" the renderer must decide about.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CellAlignment {
    /// `:---`, or no marker at all.
    Left,
    /// `:---:`.
    Center,
    /// `---:`.
    Right,
}

/// One styled text run inside a block.
///
/// Runs are maximal: adjacent text sharing every attribute is one span.
// Five flags mirror the inline marks the supported formats carry; a bitflag set would
// only move the cost to the renderer.
#[expect(
    clippy::struct_excessive_bools,
    reason = "one flag per inline mark is the model"
)]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DocSpan {
    /// Run text.
    pub text: String,
    /// Strong emphasis.
    pub bold: bool,
    /// Emphasis.
    pub italic: bool,
    /// Inline code.
    pub code: bool,
    /// Strikethrough.
    pub strike: bool,
    /// Underline, independent of link decoration.
    pub underline: bool,
    /// Link destination when the run is part of a link.
    pub link: Option<String>,
    /// Document foreground color, adapted for contrast by the renderer.
    pub foreground: Option<DocColor>,
    /// Document highlight color, adapted for contrast by the renderer.
    pub highlight: Option<DocColor>,
    /// Font family declared by the document.
    pub font_family: Option<String>,
    /// Source font size in half-points; used to choose a paragraph's dominant size.
    pub font_size_half_points: Option<u16>,
    /// Small-caps semantics.
    pub small_caps: bool,
    /// Superscript/subscript semantics.
    pub vertical: VerticalPosition,
}

/// A markup language Sourcefour recognises by file extension.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DocumentKind {
    /// `CommonMark` plus strikethrough and tables.
    Markdown,
    /// Rich Text Format.
    Rtf,
    /// `AsciiDoc`, detected but not yet rendered.
    AsciiDoc,
    /// `LaTeX`, detected but not yet rendered.
    Latex,
}

impl DocumentKind {
    /// Returns the kind implied by a path's extension, `None` when the
    /// extension is absent or unrecognised.
    ///
    /// Takes bytes because Git paths are not required to be UTF-8.
    pub fn detect(path: &[u8]) -> Option<Self> {
        let name = match path.iter().rposition(|byte| *byte == b'/') {
            Some(slash) => path.get(slash + 1..)?,
            None => path,
        };
        let dot = name.iter().rposition(|byte| *byte == b'.')?;
        match name.get(dot + 1..)?.to_ascii_lowercase().as_slice() {
            b"md" | b"markdown" | b"mdown" => Some(Self::Markdown),
            b"rtf" => Some(Self::Rtf),
            b"adoc" | b"asciidoc" => Some(Self::AsciiDoc),
            b"tex" => Some(Self::Latex),
            _ => None,
        }
    }
}

/// Returns every image source in document order, reaching into quotes and
/// list items.
pub fn image_sources(blocks: &[DocBlock]) -> Vec<&str> {
    let mut sources = Vec::new();
    collect_image_sources(blocks, &mut sources);
    sources
}

fn collect_image_sources<'a>(blocks: &'a [DocBlock], sources: &mut Vec<&'a str>) {
    for block in blocks {
        match &block.kind {
            DocBlockKind::Image { src, .. } => sources.push(src),
            DocBlockKind::Quote { blocks } | DocBlockKind::Aside { blocks, .. } => {
                collect_image_sources(blocks, sources);
            }
            DocBlockKind::List { items, .. } => {
                for item in items {
                    collect_image_sources(item, sources);
                }
            }
            // A table cell holds spans, and a span cannot be an image.
            DocBlockKind::Heading { .. }
            | DocBlockKind::Paragraph { .. }
            | DocBlockKind::RichParagraph { .. }
            | DocBlockKind::Code { .. }
            | DocBlockKind::Table { .. }
            | DocBlockKind::Rule
            | DocBlockKind::PageBreak
            | DocBlockKind::EmbeddedMedia { .. } => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DocumentKind, image_sources};
    use crate::parse_markdown;

    #[test]
    fn image_sources_reach_through_quotes_and_lists() {
        let blocks = parse_markdown(
            "![top](top.png)\n\n> ![quoted](quoted.png)\n\n- ![listed](listed.png)\n- text only\n",
        );

        assert_eq!(
            image_sources(&blocks),
            ["top.png", "quoted.png", "listed.png"]
        );
    }

    #[test]
    fn detect_reads_the_extension_case_insensitively() {
        let cases: [(&[u8], Option<DocumentKind>); 14] = [
            (b"README.md", Some(DocumentKind::Markdown)),
            (b"docs/GUIDE.MARKDOWN", Some(DocumentKind::Markdown)),
            (b"notes.mdown", Some(DocumentKind::Markdown)),
            (b"notes.rtf", Some(DocumentKind::Rtf)),
            (b"NOTES.RTF", Some(DocumentKind::Rtf)),
            (b"notes.rtfd", None),
            (b"book.AdOc", Some(DocumentKind::AsciiDoc)),
            (b"book.asciidoc", Some(DocumentKind::AsciiDoc)),
            (b"paper.TeX", Some(DocumentKind::Latex)),
            (b"src/main.rs", None),
            (b"Makefile", None),
            (b"", None),
            (b"docs.md/inner", None),
            (b".gitignore", None),
        ];

        for (path, expected) in cases {
            assert_eq!(
                DocumentKind::detect(path),
                expected,
                "{}",
                String::from_utf8_lossy(path)
            );
        }
    }
}
