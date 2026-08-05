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
// Four flags mirror the four inline marks Markdown carries; a bitflag set would
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
    /// Link destination when the run is part of a link.
    pub link: Option<String>,
}

/// A markup language Sourcefour recognises by file extension.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DocumentKind {
    /// `CommonMark` plus strikethrough and tables.
    Markdown,
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
            DocBlockKind::Quote { blocks } => collect_image_sources(blocks, sources),
            DocBlockKind::List { items, .. } => {
                for item in items {
                    collect_image_sources(item, sources);
                }
            }
            // A table cell holds spans, and a span cannot be an image.
            DocBlockKind::Heading { .. }
            | DocBlockKind::Paragraph { .. }
            | DocBlockKind::Code { .. }
            | DocBlockKind::Table { .. }
            | DocBlockKind::Rule => {}
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
        let cases: [(&[u8], Option<DocumentKind>); 11] = [
            (b"README.md", Some(DocumentKind::Markdown)),
            (b"docs/GUIDE.MARKDOWN", Some(DocumentKind::Markdown)),
            (b"notes.mdown", Some(DocumentKind::Markdown)),
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
