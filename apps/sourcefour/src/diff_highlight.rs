//! Background syntax and intraline enrichment for semantic text diffs.

use std::{collections::HashMap, ops::Range, path::Path, sync::OnceLock};

use gix_imara_diff::{Algorithm, Diff, InternedInput, sources};
use sourcefour_model::{DiffSide, RepoPath, TextChange, TextDiff, TextSide};
use syntect::{
    easy::HighlightLines,
    highlighting::{FontStyle, Theme, ThemeSet},
    parsing::SyntaxSet,
};

const MAX_SIDE_BYTES: usize = 5 * 1024 * 1024;
const MAX_LINES: usize = 100_000;
const MAX_LINE_BYTES: usize = 10_000;
const MAX_INTRALINE_BYTES: usize = 1_000;

/// One foreground syntax run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SyntaxSpan {
    pub(crate) range: Range<usize>,
    pub(crate) rgb: (u8, u8, u8),
    pub(crate) bold: bool,
    pub(crate) italic: bool,
}

/// Optional styling layers keyed by stable source coordinates.
#[derive(Clone, Debug, Default)]
pub(crate) struct DiffHighlight {
    syntax: HashMap<(DiffSide, usize), Vec<SyntaxSpan>>,
    intraline: HashMap<(DiffSide, usize), Vec<Range<usize>>>,
}

impl DiffHighlight {
    pub(crate) fn syntax(&self, side: DiffSide, line: usize) -> &[SyntaxSpan] {
        self.syntax.get(&(side, line)).map_or(&[], Vec::as_slice)
    }

    pub(crate) fn intraline(&self, side: DiffSide, line: usize) -> &[Range<usize>] {
        self.intraline.get(&(side, line)).map_or(&[], Vec::as_slice)
    }
}

/// Result of attempting optional enrichment. A skipped file remains fully
/// readable as plain text and carries a short reason for the header.
pub(crate) struct HighlightResult {
    pub(crate) highlight: Option<DiffHighlight>,
    pub(crate) message: Option<&'static str>,
}

pub(crate) fn enrich(diff: &TextDiff, path: &RepoPath) -> HighlightResult {
    let _span = tracing::debug_span!("diff.enrich", path = %path.display_lossy()).entered();
    if diff
        .old
        .iter()
        .chain(diff.new.iter())
        .any(|side| side.text().len() > MAX_SIDE_BYTES)
        || diff
            .old
            .iter()
            .chain(diff.new.iter())
            .map(TextSide::line_count)
            .sum::<usize>()
            > MAX_LINES
        || diff.old.iter().chain(diff.new.iter()).any(|side| {
            (0..side.line_count()).any(|line| {
                side.line(line)
                    .is_some_and(|line| line.len() > MAX_LINE_BYTES)
            })
        })
    {
        return HighlightResult {
            highlight: None,
            message: Some("Syntax highlighting disabled for this large file"),
        };
    }

    let mut highlight = DiffHighlight::default();
    let path = path.display_lossy();
    if let Some(syntax) = syntax_set()
        .find_syntax_for_file(Path::new(&path))
        .ok()
        .flatten()
    {
        let theme = theme();
        if let Some(side) = &diff.old
            && !highlight_side(side, DiffSide::Old, syntax, theme, &mut highlight)
        {
            return HighlightResult {
                highlight: Some(highlight),
                message: Some("Some syntax highlighting could not be computed"),
            };
        }
        if let Some(side) = &diff.new
            && !highlight_side(side, DiffSide::New, syntax, theme, &mut highlight)
        {
            return HighlightResult {
                highlight: Some(highlight),
                message: Some("Some syntax highlighting could not be computed"),
            };
        }
    }
    intraline(diff, &mut highlight);
    HighlightResult {
        highlight: Some(highlight),
        message: None,
    }
}

fn syntax_set() -> &'static SyntaxSet {
    static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
    SYNTAXES.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn theme() -> &'static Theme {
    static THEME: OnceLock<Theme> = OnceLock::new();
    THEME.get_or_init(|| {
        ThemeSet::load_defaults()
            .themes
            .remove("base16-ocean.dark")
            .unwrap_or_default()
    })
}

fn highlight_side(
    side: &TextSide,
    diff_side: DiffSide,
    syntax: &syntect::parsing::SyntaxReference,
    theme: &Theme,
    output: &mut DiffHighlight,
) -> bool {
    let mut highlighter = HighlightLines::new(syntax, theme);
    for line_index in 0..side.line_count() {
        let Some(line) = side.line(line_index) else {
            continue;
        };
        let parsed = format!("{line}\n");
        let Ok(parts) = highlighter.highlight_line(&parsed, syntax_set()) else {
            return false;
        };
        let mut offset = 0;
        let spans = parts
            .into_iter()
            .filter_map(|(style, text)| {
                let start = offset;
                offset += text.len();
                let end = offset.min(line.len());
                (start < end).then_some(SyntaxSpan {
                    range: start..end,
                    rgb: (style.foreground.r, style.foreground.g, style.foreground.b),
                    bold: style.font_style.contains(FontStyle::BOLD),
                    italic: style.font_style.contains(FontStyle::ITALIC),
                })
            })
            .collect();
        output.syntax.insert((diff_side, line_index), spans);
    }
    true
}

fn intraline(diff: &TextDiff, output: &mut DiffHighlight) {
    for change in diff.changes.iter() {
        let TextChange::Replace { alignment, .. } = change else {
            continue;
        };
        for pair in alignment.iter() {
            match (pair.old_line, pair.new_line) {
                (Some(old_line), Some(new_line)) => {
                    let old = diff.old.as_ref().and_then(|side| side.line(old_line));
                    let new = diff.new.as_ref().and_then(|side| side.line(new_line));
                    let (Some(old), Some(new)) = (old, new) else {
                        continue;
                    };
                    if old.len() <= MAX_INTRALINE_BYTES && new.len() <= MAX_INTRALINE_BYTES {
                        let (old_spans, new_spans) = word_diff(old, new);
                        output
                            .intraline
                            .insert((DiffSide::Old, old_line), old_spans);
                        output
                            .intraline
                            .insert((DiffSide::New, new_line), new_spans);
                    }
                }
                (Some(line), None) => {
                    if let Some(text) = diff.old.as_ref().and_then(|side| side.line(line)) {
                        output.intraline.insert(
                            (DiffSide::Old, line),
                            std::iter::once(0..text.len()).collect(),
                        );
                    }
                }
                (None, Some(line)) => {
                    if let Some(text) = diff.new.as_ref().and_then(|side| side.line(line)) {
                        output.intraline.insert(
                            (DiffSide::New, line),
                            std::iter::once(0..text.len()).collect(),
                        );
                    }
                }
                (None, None) => {}
            }
        }
    }
}

fn word_diff(old: &str, new: &str) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    let old_tokens: Vec<_> = sources::words(old).collect();
    let new_tokens: Vec<_> = sources::words(new).collect();
    let input = InternedInput::new(sources::words(old), sources::words(new));
    let computed = Diff::compute(Algorithm::Myers, &input);
    let old_offsets = token_offsets(&old_tokens);
    let new_offsets = token_offsets(&new_tokens);
    let mut old_spans = Vec::new();
    let mut new_spans = Vec::new();
    for hunk in computed.hunks() {
        if let Some(range) = token_range(
            &old_offsets,
            hunk.before.start as usize..hunk.before.end as usize,
        ) {
            old_spans.push(range);
        }
        if let Some(range) = token_range(
            &new_offsets,
            hunk.after.start as usize..hunk.after.end as usize,
        ) {
            new_spans.push(range);
        }
    }
    (old_spans, new_spans)
}

fn token_offsets(tokens: &[&str]) -> Vec<Range<usize>> {
    let mut offset = 0;
    tokens
        .iter()
        .map(|token| {
            let start = offset;
            offset += token.len();
            start..offset
        })
        .collect()
}

fn token_range(offsets: &[Range<usize>], tokens: Range<usize>) -> Option<Range<usize>> {
    let first = offsets.get(tokens.start)?;
    let last = offsets.get(tokens.end.checked_sub(1)?)?;
    Some(first.start..last.end)
}

#[cfg(test)]
mod tests {
    use super::word_diff;

    #[test]
    fn word_diff_marks_only_the_changed_token() {
        let (old, new) = word_diff("let answer = 41;", "let answer = 42;");
        assert_eq!(&"let answer = 41;"[old[0].clone()], "41");
        assert_eq!(&"let answer = 42;"[new[0].clone()], "42");
    }

    #[test]
    fn unicode_ranges_land_on_utf8_boundaries() {
        let (old, new) = word_diff("let navn = \"blå\";", "let navn = \"grønn\";");
        for range in old {
            assert!("let navn = \"blå\";".get(range).is_some());
        }
        for range in new {
            assert!("let navn = \"grønn\";".get(range).is_some());
        }
    }
}
