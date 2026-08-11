//! The Markdown reader: a flat event stream folded into the block tree.

use std::{mem, ops::Range};

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::model::{CellAlignment, DocBlock, DocBlockKind, DocSpan};

/// Parses Markdown into blocks.
///
/// Strikethrough and tables are enabled. Raw HTML and footnote markers are
/// dropped, since the preview cannot render either.
pub fn parse_markdown(text: &str) -> Vec<DocBlock> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);

    let mut builder = Builder::default();
    let mut events = Parser::new_ext(text, options).into_offset_iter();
    while let Some((event, range)) = events.next() {
        builder.event(event, range, &mut events);
    }
    builder.finish()
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
    /// The header row is the one before `End(TableHead)`; every later row is
    /// a body row, so the frame needs no flag to tell them apart.
    Table {
        alignments: Vec<CellAlignment>,
        header: Vec<Vec<DocSpan>>,
        rows: Vec<Vec<Vec<DocSpan>>>,
        /// Cells harvested since the last row ended.
        current_row: Vec<Vec<DocSpan>>,
        range: Range<usize>,
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
#[derive(Default)]
struct Builder {
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

impl Builder {
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
                if matches!(self.stack.last(), Some(Frame::Table { .. })) {
                    // A cell holds spans, not blocks. Lifting the image out
                    // would drop it after the whole table with nothing left
                    // saying which cell wrote it, so the alt text stands in.
                    self.push_run(&alt, false);
                } else {
                    self.images.push(DocBlock {
                        kind: DocBlockKind::Image {
                            src: dest_url.into_string(),
                            alt,
                        },
                        source_range: range,
                    });
                }
            }
            // Cell content arrives as the inline events above, so these
            // boundaries must not reach the flushing arm below: a flush would
            // turn the cell being read into a paragraph of its own.
            Event::Start(Tag::TableHead | Tag::TableRow | Tag::TableCell) => {}
            Event::End(TagEnd::TableCell) => {
                let spans = self.take_spans();
                if let Some(Frame::Table { current_row, .. }) = self.stack.last_mut() {
                    current_row.push(spans);
                }
            }
            Event::End(TagEnd::TableHead) => self.end_row(true),
            Event::End(TagEnd::TableRow) => self.end_row(false),
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
            Event::Start(Tag::Table(alignments)) => self.stack.push(Frame::Table {
                alignments: alignments.into_iter().map(cell_alignment).collect(),
                header: Vec::new(),
                rows: Vec::new(),
                current_row: Vec::new(),
                range,
            }),
            Event::End(TagEnd::Table) => {
                if let Some(Frame::Table {
                    alignments,
                    header,
                    rows,
                    range,
                    ..
                }) = self.stack.pop()
                {
                    self.push_block(DocBlock {
                        kind: DocBlockKind::Table {
                            alignments,
                            header,
                            rows,
                        },
                        source_range: range,
                    });
                }
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
        let spans = self.take_spans();
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

    /// Closes the row being read, as the header row or as a body row.
    fn end_row(&mut self, is_header: bool) {
        if let Some(Frame::Table {
            header,
            rows,
            current_row,
            ..
        }) = self.stack.last_mut()
        {
            let row = mem::take(current_row);
            if is_header {
                *header = row;
            } else {
                rows.push(row);
            }
        }
    }

    /// Harvests the pending inline run and clears what it accumulated.
    ///
    /// The style stack and the covered range go with the spans, so whatever
    /// the caller does with them — a paragraph, or nothing — the next run
    /// starts from empty and cannot inherit a range it never covered.
    fn take_spans(&mut self) -> Vec<DocSpan> {
        self.covered = None;
        self.styles.clear();
        mem::take(&mut self.spans)
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
            underline: false,
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
            // A list holds only items and a table only spans, so nothing can
            // land in either.
            Some(Frame::List { .. } | Frame::Table { .. }) | None => self.blocks.push(block),
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

/// An unmarked column renders left-aligned, so the model says so outright
/// rather than carrying a fourth case every renderer would collapse anyway.
fn cell_alignment(alignment: Alignment) -> CellAlignment {
    match alignment {
        Alignment::None | Alignment::Left => CellAlignment::Left,
        Alignment::Center => CellAlignment::Center,
        Alignment::Right => CellAlignment::Right,
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
    use super::parse_markdown;
    use crate::model::{CellAlignment, DocBlock, DocBlockKind, DocSpan};

    /// Alignments, header cells, body rows — a borrowed `DocBlockKind::Table`.
    type TableParts<'a> = (
        &'a [CellAlignment],
        &'a [Vec<DocSpan>],
        &'a [Vec<Vec<DocSpan>>],
    );

    /// The three parts of the one table a block holds, or a panic.
    fn table(block: &DocBlock) -> TableParts<'_> {
        let DocBlockKind::Table {
            alignments,
            header,
            rows,
        } = &block.kind
        else {
            panic!("a table, got {block:?}");
        };
        (alignments, header, rows)
    }

    /// A cell's text, ignoring style.
    fn cell(spans: &[DocSpan]) -> String {
        spans.iter().map(|span| span.text.as_str()).collect()
    }

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
            | DocBlockKind::Table { .. }
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
    fn tables_parse_into_structure() {
        let source = "| a | b |\n|---|---|\n| 1 | 2 |\n";
        let blocks = parse_markdown(source);
        let [block] = blocks.as_slice() else {
            panic!("one table, got {blocks:?}");
        };
        let (alignments, header, rows) = table(block);

        assert_eq!(alignments, [CellAlignment::Left, CellAlignment::Left]);
        assert_eq!(
            header.iter().map(|spans| cell(spans)).collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].iter().map(|spans| cell(spans)).collect::<Vec<_>>(),
            ["1", "2"]
        );
        assert_eq!(
            source.get(block.source_range.clone()),
            Some(source),
            "the range slices the table's own source"
        );
    }

    #[test]
    fn table_rows_keep_their_cells_and_the_styling_inside_them() {
        let blocks = parse_markdown(
            "| name | note |\n|---|---|\n| **bold** | plain |\n| `code` | more |\n\nafter\n",
        );
        let [table_block, paragraph] = blocks.as_slice() else {
            panic!("a table and a paragraph, got {blocks:?}");
        };
        let (_, header, rows) = table(table_block);

        assert_eq!(header.len(), 2);
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| row.len() == 2));
        assert_eq!(runs(&rows[0][0]), ["bold|b"]);
        assert_eq!(runs(&rows[1][0]), ["code|c"]);
        assert_eq!(
            plain(paragraph),
            "after",
            "the table leaves no run behind for the next block to pick up"
        );
    }

    #[test]
    fn the_delimiter_row_decides_each_column_s_alignment() {
        let blocks =
            parse_markdown("| a | b | c | d |\n| :--- | :---: | ---: | --- |\n| 1 | 2 | 3 | 4 |\n");
        let [block] = blocks.as_slice() else {
            panic!("one table, got {blocks:?}");
        };

        assert_eq!(
            table(block).0,
            [
                CellAlignment::Left,
                CellAlignment::Center,
                CellAlignment::Right,
                CellAlignment::Left,
            ]
        );
    }

    #[test]
    fn an_escaped_pipe_stays_inside_its_cell() {
        let blocks = parse_markdown("| a | b |\n|---|---|\n| one \\| two | three |\n");
        let [block] = blocks.as_slice() else {
            panic!("one table, got {blocks:?}");
        };
        let (_, _, rows) = table(block);

        assert_eq!(
            rows[0].iter().map(|spans| cell(spans)).collect::<Vec<_>>(),
            ["one | two", "three"]
        );
    }

    #[test]
    fn an_image_in_a_cell_becomes_its_alt_text() {
        let blocks = parse_markdown("| icon |\n|---|\n| ![the logo](logo.png) |\n");
        let [block] = blocks.as_slice() else {
            panic!("one table and no lifted image, got {blocks:?}");
        };

        assert_eq!(cell(&table(block).2[0][0]), "the logo");
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
}
