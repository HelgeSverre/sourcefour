//! Document model and Markdown parsing for the diff viewer's preview pane.
//!
//! The crate is pure: no GPUI, no gix, no I/O. Input is source text some other
//! crate already read, output is an owned block tree a renderer walks.

use std::{mem, ops::Range};

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

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

/// Parses Markdown into blocks.
///
/// Strikethrough and tables are enabled; a table degrades to a `Code` block
/// holding its own source until a renderer for tables exists. Raw HTML and
/// footnote markers are dropped, since the preview cannot render either.
pub fn parse_markdown(text: &str) -> Vec<DocBlock> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);

    let mut builder = Builder::new(text);
    let mut events = Parser::new_ext(text, options).into_offset_iter();
    while let Some((event, range)) = events.next() {
        builder.event(event, range, &mut events);
    }
    builder.finish()
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
            DocBlockKind::Heading { .. }
            | DocBlockKind::Paragraph { .. }
            | DocBlockKind::Code { .. }
            | DocBlockKind::Rule => {}
        }
    }
}

/// A container the parser is inside of, holding what it has collected so far.
enum Frame {
    Quote {
        blocks: Vec<DocBlock>,
        range: Range<usize>,
    },
    List {
        ordered: bool,
        items: Vec<Vec<DocBlock>>,
        range: Range<usize>,
    },
    Item {
        blocks: Vec<DocBlock>,
    },
}

/// The inline marks in effect at one point in the event stream.
#[derive(Clone, Default)]
struct Style {
    bold: bool,
    italic: bool,
    strike: bool,
    link: Option<String>,
}

/// Accumulates blocks from a flat event stream.
///
/// Containers live on an explicit stack rather than the call stack, so deeply
/// nested source cannot overflow it. Inline events accumulate into `spans`
/// until a block boundary flushes them, which is what lets a tight list item's
/// bare text become a paragraph without a `Paragraph` event of its own.
struct Builder<'s> {
    source: &'s str,
    blocks: Vec<DocBlock>,
    stack: Vec<Frame>,
    spans: Vec<DocSpan>,
    styles: Vec<Style>,
    /// Images seen in the current inline run, emitted after it.
    images: Vec<DocBlock>,
    /// Set while the current inline run belongs to a heading.
    heading: Option<u8>,
    /// Range of the paragraph or heading the run was opened by.
    opened: Option<Range<usize>>,
    /// Range covered by the inline events themselves, used when no block
    /// opened the run.
    covered: Option<Range<usize>>,
}

impl<'s> Builder<'s> {
    fn new(source: &'s str) -> Self {
        Self {
            source,
            blocks: Vec::new(),
            stack: Vec::new(),
            spans: Vec::new(),
            styles: Vec::new(),
            images: Vec::new(),
            heading: None,
            opened: None,
            covered: None,
        }
    }

    fn finish(mut self) -> Vec<DocBlock> {
        self.flush();
        self.blocks
    }

    /// Handles one event, consuming the events of any element it owns whole.
    fn event<'e, I>(&mut self, event: Event<'e>, range: Range<usize>, events: &mut I)
    where
        I: Iterator<Item = (Event<'e>, Range<usize>)>,
    {
        match event {
            Event::Text(text) => {
                self.cover(range);
                self.push_run(&text, false);
            }
            Event::Code(text) => {
                self.cover(range);
                self.push_run(&text, true);
            }
            Event::SoftBreak => {
                self.cover(range);
                self.push_run(" ", false);
            }
            Event::HardBreak => {
                self.cover(range);
                self.push_run("\n", false);
            }
            Event::Start(Tag::Emphasis) => self.push_style(|style| style.italic = true),
            Event::Start(Tag::Strong) => self.push_style(|style| style.bold = true),
            Event::Start(Tag::Strikethrough) => self.push_style(|style| style.strike = true),
            Event::Start(Tag::Link { dest_url, .. }) => {
                self.push_style(|style| style.link = Some(dest_url.into_string()));
            }
            Event::End(
                TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link,
            ) => {
                self.styles.pop();
            }
            Event::Start(Tag::Image { dest_url, .. }) => {
                self.cover(range.clone());
                let alt = take_text(events, TagEnd::Image);
                self.images.push(DocBlock {
                    kind: DocBlockKind::Image {
                        src: dest_url.into_string(),
                        alt,
                    },
                    source_range: range,
                });
            }
            other => {
                self.flush();
                self.block(other, range, events);
            }
        }
    }

    /// Handles an event that cannot appear inside an inline run.
    fn block<'e, I>(&mut self, event: Event<'e>, range: Range<usize>, events: &mut I)
    where
        I: Iterator<Item = (Event<'e>, Range<usize>)>,
    {
        match event {
            Event::Start(Tag::Paragraph) => self.opened = Some(range),
            Event::Start(Tag::Heading { level, .. }) => {
                self.opened = Some(range);
                self.heading = Some(heading_level(level));
            }
            Event::Start(Tag::BlockQuote(_)) => self.stack.push(Frame::Quote {
                blocks: Vec::new(),
                range,
            }),
            Event::End(TagEnd::BlockQuote(_)) => {
                if let Some(Frame::Quote { blocks, range }) = self.stack.pop() {
                    self.push_block(DocBlock {
                        kind: DocBlockKind::Quote { blocks },
                        source_range: range,
                    });
                }
            }
            Event::Start(Tag::List(first)) => self.stack.push(Frame::List {
                ordered: first.is_some(),
                items: Vec::new(),
                range,
            }),
            Event::End(TagEnd::List(_)) => {
                if let Some(Frame::List {
                    ordered,
                    items,
                    range,
                }) = self.stack.pop()
                {
                    self.push_block(DocBlock {
                        kind: DocBlockKind::List { ordered, items },
                        source_range: range,
                    });
                }
            }
            Event::Start(Tag::Item) => self.stack.push(Frame::Item { blocks: Vec::new() }),
            Event::End(TagEnd::Item) => {
                if let Some(Frame::Item { blocks }) = self.stack.pop()
                    && let Some(Frame::List { items, .. }) = self.stack.last_mut()
                {
                    items.push(blocks);
                }
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                let language = match kind {
                    // Rustdoc-style info strings carry flags after the
                    // language: `rust,ignore` highlights as `rust`.
                    CodeBlockKind::Fenced(info) => info
                        .split([' ', '\t', ','])
                        .find(|token| !token.is_empty())
                        .map(str::to_owned),
                    CodeBlockKind::Indented => None,
                };
                let mut text = take_text(events, TagEnd::CodeBlock);
                // The closing fence's own line break is not part of the code.
                if text.ends_with('\n') {
                    text.pop();
                }
                self.push_block(DocBlock {
                    kind: DocBlockKind::Code { language, text },
                    source_range: range,
                });
            }
            Event::Start(Tag::Table(_)) => {
                let text = self
                    .source
                    .get(range.clone())
                    .unwrap_or_default()
                    .trim_end()
                    .to_owned();
                skip_to(events, TagEnd::Table);
                // The language marks the degradation, so a renderer can say
                // "this was a table" instead of passing it off as code.
                self.push_block(DocBlock {
                    kind: DocBlockKind::Code {
                        language: Some(String::from("table")),
                        text,
                    },
                    source_range: range,
                });
            }
            Event::Rule => self.push_block(DocBlock {
                kind: DocBlockKind::Rule,
                source_range: range,
            }),
            _ => {}
        }
    }

    /// Turns the pending inline run into a block, then emits its images.
    fn flush(&mut self) {
        let range = self.opened.take().or_else(|| self.covered.take());
        self.covered = None;
        self.styles.clear();
        let spans = mem::take(&mut self.spans);
        let images = mem::take(&mut self.images);
        let heading = self.heading.take();

        if let Some(range) = range
            && spans.iter().any(|span| !span.text.trim().is_empty())
        {
            let kind = match heading {
                Some(level) => DocBlockKind::Heading { level, spans },
                None => DocBlockKind::Paragraph { spans },
            };
            self.push_block(DocBlock {
                kind,
                source_range: range,
            });
        }

        for image in images {
            self.push_block(image);
        }
    }

    /// Appends text to the pending run, extending the last span when the style
    /// is unchanged.
    fn push_run(&mut self, text: &str, code: bool) {
        if text.is_empty() {
            return;
        }
        let style = self.styles.last().cloned().unwrap_or_default();
        if let Some(last) = self.spans.last_mut()
            && last.bold == style.bold
            && last.italic == style.italic
            && last.strike == style.strike
            && last.code == code
            && last.link == style.link
        {
            last.text.push_str(text);
            return;
        }
        self.spans.push(DocSpan {
            text: text.to_owned(),
            bold: style.bold,
            italic: style.italic,
            code,
            strike: style.strike,
            link: style.link,
        });
    }

    fn push_style(&mut self, mark: impl FnOnce(&mut Style)) {
        let mut style = self.styles.last().cloned().unwrap_or_default();
        mark(&mut style);
        self.styles.push(style);
    }

    /// Widens the range the pending inline run covers.
    fn cover(&mut self, range: Range<usize>) {
        match &mut self.covered {
            Some(covered) => {
                covered.start = covered.start.min(range.start);
                covered.end = covered.end.max(range.end);
            }
            None => self.covered = Some(range),
        }
    }

    fn push_block(&mut self, block: DocBlock) {
        match self.stack.last_mut() {
            Some(Frame::Quote { blocks, .. } | Frame::Item { blocks }) => blocks.push(block),
            // A list holds only items, so nothing else can land here.
            Some(Frame::List { .. }) | None => self.blocks.push(block),
        }
    }
}

/// Concatenates the text of an element, consuming through its end tag.
fn take_text<'e, I>(events: &mut I, end: TagEnd) -> String
where
    I: Iterator<Item = (Event<'e>, Range<usize>)>,
{
    let mut text = String::new();
    for (event, _) in events.by_ref() {
        match event {
            Event::Text(chunk) | Event::Code(chunk) => text.push_str(&chunk),
            Event::End(tag) if tag == end => break,
            _ => {}
        }
    }
    text
}

/// Discards events through an element's end tag.
fn skip_to<'e, I>(events: &mut I, end: TagEnd)
where
    I: Iterator<Item = (Event<'e>, Range<usize>)>,
{
    for (event, _) in events.by_ref() {
        if matches!(event, Event::End(tag) if tag == end) {
            break;
        }
    }
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

#[cfg(test)]
mod tests {
    use super::{DocBlock, DocBlockKind, DocSpan, DocumentKind, image_sources, parse_markdown};

    /// Renders spans as `text|flags`, one flag letter per set attribute.
    fn runs(spans: &[DocSpan]) -> Vec<String> {
        spans
            .iter()
            .map(|span| {
                let mut flags = String::new();
                for (set, flag) in [
                    (span.bold, 'b'),
                    (span.italic, 'i'),
                    (span.code, 'c'),
                    (span.strike, 's'),
                    (span.link.is_some(), 'l'),
                ] {
                    if set {
                        flags.push(flag);
                    }
                }
                format!("{}|{flags}", span.text)
            })
            .collect()
    }

    /// Concatenates a block's text, ignoring style.
    fn plain(block: &DocBlock) -> String {
        match &block.kind {
            DocBlockKind::Heading { spans, .. } | DocBlockKind::Paragraph { spans } => {
                spans.iter().map(|span| span.text.as_str()).collect()
            }
            DocBlockKind::Code { text, .. } => text.clone(),
            DocBlockKind::Quote { .. }
            | DocBlockKind::List { .. }
            | DocBlockKind::Rule
            | DocBlockKind::Image { .. } => String::new(),
        }
    }

    #[test]
    fn headings_carry_their_level_and_styled_runs() {
        for (source, expected) in [
            ("# a", 1_u8),
            ("## a", 2),
            ("### a", 3),
            ("#### a", 4),
            ("##### a", 5),
            ("###### a", 6),
        ] {
            let blocks = parse_markdown(source);
            let [
                DocBlock {
                    kind: DocBlockKind::Heading { level, .. },
                    ..
                },
            ] = blocks.as_slice()
            else {
                panic!("{source} is one heading, got {blocks:?}");
            };
            assert_eq!(*level, expected, "{source}");
        }

        let blocks = parse_markdown("## plain **bold** *italic* `code`");
        let [
            DocBlock {
                kind: DocBlockKind::Heading { spans, .. },
                ..
            },
        ] = blocks.as_slice()
        else {
            panic!("one heading, got {blocks:?}");
        };
        assert_eq!(
            runs(spans),
            ["plain |", "bold|b", " |", "italic|i", " |", "code|c"]
        );
    }

    #[test]
    fn paragraph_spans_carry_link_targets_and_strikethrough() {
        let blocks = parse_markdown("see [the docs](https://example.com/a) and ~~not this~~");
        let [
            DocBlock {
                kind: DocBlockKind::Paragraph { spans },
                ..
            },
        ] = blocks.as_slice()
        else {
            panic!("one paragraph, got {blocks:?}");
        };

        assert_eq!(runs(spans), ["see |", "the docs|l", " and |", "not this|s"]);
        assert_eq!(spans[1].link.as_deref(), Some("https://example.com/a"));
    }

    #[test]
    fn fenced_code_keeps_the_language_it_was_tagged_with() {
        for (source, language, text) in [
            ("```rust\nfn main() {}\n```", Some("rust"), "fn main() {}"),
            ("```\nplain text\n```", None, "plain text"),
            (
                "```rust,ignore\nlet x = 1;\n```",
                Some("rust"),
                "let x = 1;",
            ),
            ("```  sh  \nls\n```", Some("sh"), "ls"),
        ] {
            let blocks = parse_markdown(source);
            let [
                DocBlock {
                    kind:
                        DocBlockKind::Code {
                            language: parsed,
                            text: body,
                        },
                    ..
                },
            ] = blocks.as_slice()
            else {
                panic!("{source} is one code block, got {blocks:?}");
            };
            assert_eq!(parsed.as_deref(), language, "{source}");
            assert_eq!(body, text, "{source}");
        }
    }

    #[test]
    fn nested_lists_keep_each_item_as_its_own_blocks() {
        let blocks = parse_markdown("1. first\n   - inner a\n   - inner b\n2. second\n");
        let [
            DocBlock {
                kind: DocBlockKind::List { ordered, items },
                ..
            },
        ] = blocks.as_slice()
        else {
            panic!("one list, got {blocks:?}");
        };

        assert!(*ordered);
        assert_eq!(items.len(), 2);

        let [first, nested] = items[0].as_slice() else {
            panic!(
                "the first item holds text and a nested list, got {:?}",
                items[0]
            );
        };
        assert_eq!(plain(first), "first");
        let DocBlockKind::List {
            ordered: inner_ordered,
            items: inner,
        } = &nested.kind
        else {
            panic!("a nested list, got {nested:?}");
        };
        assert!(!inner_ordered);
        assert_eq!(
            inner.iter().map(|item| plain(&item[0])).collect::<Vec<_>>(),
            ["inner a", "inner b"]
        );

        assert_eq!(items[1].len(), 1);
        assert_eq!(plain(&items[1][0]), "second");
    }

    #[test]
    fn blockquotes_own_the_blocks_inside_them() {
        let blocks = parse_markdown("> intro line\n>\n> ```sh\n> ls -la\n> ```\n");
        let [
            DocBlock {
                kind: DocBlockKind::Quote { blocks: inner },
                ..
            },
        ] = blocks.as_slice()
        else {
            panic!("one quote, got {blocks:?}");
        };

        assert_eq!(inner.len(), 2);
        assert_eq!(plain(&inner[0]), "intro line");
        let DocBlockKind::Code { language, text } = &inner[1].kind else {
            panic!("a quoted code block, got {:?}", inner[1]);
        };
        assert_eq!(language.as_deref(), Some("sh"));
        assert_eq!(text, "ls -la");
    }

    #[test]
    fn images_lift_out_of_text_inline_and_by_reference() {
        let blocks = parse_markdown(
            "before ![inline alt](inline.png) after\n\n![ref alt][badge]\n\n[badge]: https://example.com/badge.svg\n",
        );

        assert_eq!(blocks.len(), 3, "{blocks:?}");
        assert_eq!(plain(&blocks[0]), "before  after");
        assert_eq!(
            blocks[1].kind,
            DocBlockKind::Image {
                src: String::from("inline.png"),
                alt: String::from("inline alt"),
            }
        );
        assert_eq!(
            blocks[2].kind,
            DocBlockKind::Image {
                src: String::from("https://example.com/badge.svg"),
                alt: String::from("ref alt"),
            }
        );
    }

    #[test]
    fn a_standalone_image_is_the_only_block_its_paragraph_produces() {
        let blocks = parse_markdown("![solo](solo.png)\n");
        let [
            DocBlock {
                kind: DocBlockKind::Image { src, alt },
                ..
            },
        ] = blocks.as_slice()
        else {
            panic!("one image, got {blocks:?}");
        };
        assert_eq!((src.as_str(), alt.as_str()), ("solo.png", "solo"));
    }

    #[test]
    fn tables_degrade_to_a_code_block_holding_their_source() {
        let source = "| a | b |\n|---|---|\n| 1 | 2 |\n";
        let blocks = parse_markdown(source);
        let [
            DocBlock {
                kind: DocBlockKind::Code { language, text },
                ..
            },
        ] = blocks.as_slice()
        else {
            panic!("one code block, got {blocks:?}");
        };

        assert_eq!(
            language.as_deref(),
            Some("table"),
            "the degradation is marked, not passed off as plain code"
        );
        assert_eq!(text, source.trim_end());
    }

    #[test]
    fn source_ranges_stay_inside_the_text_and_never_move_backwards() {
        let source = "# Title\n\nA paragraph with [a link](https://example.com).\n\n```rs\nfn main() {}\n```\n\n> quoted\n\n- one\n- two\n\n---\n\n![solo](solo.png)\n";
        let blocks = parse_markdown(source);

        assert_eq!(blocks.len(), 7, "{blocks:?}");
        let mut previous = 0;
        for block in &blocks {
            assert!(
                source.get(block.source_range.clone()).is_some(),
                "{:?} does not slice the source",
                block.source_range
            );
            assert!(
                block.source_range.start >= previous,
                "{:?} starts before {previous}",
                block.source_range
            );
            previous = block.source_range.start;
        }
    }

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
