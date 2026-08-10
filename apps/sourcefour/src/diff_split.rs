//! Lightweight layout projections over a semantic text diff.

use std::ops::Range;

use sourcefour_model::{DiffSide, TextChange, TextDiff};

const CONTEXT_LINES: usize = 3;

/// Semantic class of one projected source cell.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CellKind {
    Context,
    Addition,
    Deletion,
}

/// A source line referenced by index; its text remains owned by `TextDiff`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DiffCell {
    pub(crate) side: DiffSide,
    pub(crate) line: usize,
    pub(crate) kind: CellKind,
}

/// One projected reader row in either unified or split mode.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DiffRow {
    Hunk {
        old_lines: Range<usize>,
        new_lines: Range<usize>,
    },
    Gap {
        old_lines: Range<usize>,
        new_lines: Range<usize>,
    },
    Marker {
        side: DiffSide,
    },
    Line {
        left: Option<DiffCell>,
        right: Option<DiffCell>,
    },
}

impl DiffRow {
    fn is_change(&self) -> bool {
        matches!(
            self,
            Self::Line {
                left: Some(DiffCell {
                    kind: CellKind::Deletion,
                    ..
                }),
                ..
            } | Self::Line {
                right: Some(DiffCell {
                    kind: CellKind::Addition,
                    ..
                }),
                ..
            }
        )
    }

    fn source_bounds(rows: &[Self]) -> (Range<usize>, Range<usize>) {
        let bounds = |side| {
            let mut lines = rows.iter().filter_map(|row| match row {
                Self::Line { left, right } => [*left, *right]
                    .into_iter()
                    .flatten()
                    .find(|cell| cell.side == side)
                    .map(|cell| cell.line),
                Self::Hunk { .. } | Self::Gap { .. } | Self::Marker { .. } => None,
            });
            let Some(first) = lines.next() else {
                return 0..0;
            };
            let last = lines.next_back().unwrap_or(first);
            first..last.saturating_add(1)
        };
        (bounds(DiffSide::Old), bounds(DiffSide::New))
    }
}

/// Unified projection: removals followed by additions in each changed block.
pub(crate) fn unified_rows_with_context(diff: &TextDiff, full_context: bool) -> Vec<DiffRow> {
    let mut rows = Vec::new();
    for change in diff.changes.iter() {
        match change {
            TextChange::Equal {
                old_lines,
                new_lines,
            } => rows.extend(old_lines.clone().zip(new_lines.clone()).map(|(old, new)| {
                DiffRow::Line {
                    left: Some(DiffCell {
                        side: DiffSide::Old,
                        line: old,
                        kind: CellKind::Context,
                    }),
                    right: Some(DiffCell {
                        side: DiffSide::New,
                        line: new,
                        kind: CellKind::Context,
                    }),
                }
            })),
            TextChange::Replace {
                old_lines,
                new_lines,
                ..
            } => {
                rows.extend(old_lines.clone().map(|line| DiffRow::Line {
                    left: Some(DiffCell {
                        side: DiffSide::Old,
                        line,
                        kind: CellKind::Deletion,
                    }),
                    right: None,
                }));
                rows.extend(new_lines.clone().map(|line| DiffRow::Line {
                    left: None,
                    right: Some(DiffCell {
                        side: DiffSide::New,
                        line,
                        kind: CellKind::Addition,
                    }),
                }));
            }
        }
    }
    append_eof_markers(diff, &mut rows);
    collapse_context(&rows, full_context)
}

/// Split projection using the alignment carried by each changed block.
pub(crate) fn split_rows_with_context(diff: &TextDiff, full_context: bool) -> Vec<DiffRow> {
    let mut rows = Vec::new();
    for change in diff.changes.iter() {
        match change {
            TextChange::Equal {
                old_lines,
                new_lines,
            } => rows.extend(old_lines.clone().zip(new_lines.clone()).map(|(old, new)| {
                DiffRow::Line {
                    left: Some(DiffCell {
                        side: DiffSide::Old,
                        line: old,
                        kind: CellKind::Context,
                    }),
                    right: Some(DiffCell {
                        side: DiffSide::New,
                        line: new,
                        kind: CellKind::Context,
                    }),
                }
            })),
            TextChange::Replace { alignment, .. } => {
                rows.extend(alignment.iter().map(|pair| DiffRow::Line {
                    left: pair.old_line.map(|line| DiffCell {
                        side: DiffSide::Old,
                        line,
                        kind: CellKind::Deletion,
                    }),
                    right: pair.new_line.map(|line| DiffCell {
                        side: DiffSide::New,
                        line,
                        kind: CellKind::Addition,
                    }),
                }));
            }
        }
    }
    append_eof_markers(diff, &mut rows);
    collapse_context(&rows, full_context)
}

fn append_eof_markers(diff: &TextDiff, rows: &mut Vec<DiffRow>) {
    for (side, text) in [(DiffSide::Old, &diff.old), (DiffSide::New, &diff.new)] {
        if text
            .as_ref()
            .is_some_and(|text| text.line_count() > 0 && !text.ends_with_newline())
        {
            rows.push(DiffRow::Marker { side });
        }
    }
}

fn collapse_context(rows: &[DiffRow], full_context: bool) -> Vec<DiffRow> {
    let changed: Vec<_> = rows
        .iter()
        .enumerate()
        .filter_map(|(index, row)| row.is_change().then_some(index))
        .collect();
    let Some(&first) = changed.first() else {
        return Vec::new();
    };
    if full_context {
        let (old_lines, new_lines) = DiffRow::source_bounds(rows);
        let mut projected = Vec::with_capacity(rows.len() + 1);
        projected.push(DiffRow::Hunk {
            old_lines,
            new_lines,
        });
        projected.extend_from_slice(rows);
        return projected;
    }

    let mut ranges = Vec::<Range<usize>>::new();
    for index in changed {
        let next = index.saturating_sub(CONTEXT_LINES)..(index + CONTEXT_LINES + 1).min(rows.len());
        if let Some(last) = ranges.last_mut()
            && next.start <= last.end
        {
            last.end = last.end.max(next.end);
        } else {
            ranges.push(next);
        }
    }

    let mut projected = Vec::new();
    let mut cursor = 0;
    for range in ranges {
        if cursor < range.start {
            let (old_lines, new_lines) = DiffRow::source_bounds(&rows[cursor..range.start]);
            projected.push(DiffRow::Gap {
                old_lines,
                new_lines,
            });
        }
        let (old_lines, new_lines) = DiffRow::source_bounds(&rows[range.clone()]);
        projected.push(DiffRow::Hunk {
            old_lines,
            new_lines,
        });
        projected.extend_from_slice(&rows[range.clone()]);
        cursor = range.end;
    }
    if cursor < rows.len() {
        let (old_lines, new_lines) = DiffRow::source_bounds(&rows[cursor..]);
        projected.push(DiffRow::Gap {
            old_lines,
            new_lines,
        });
    }
    debug_assert!(first < rows.len());
    projected
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use sourcefour_model::{LinePair, TextChange, TextDecoding, TextDiff, TextSide};

    use super::{CellKind, DiffRow, split_rows_with_context, unified_rows_with_context};

    fn diff() -> TextDiff {
        TextDiff {
            old: Some(TextSide::new(
                "zero\none\ntwo\nthree\nfour\nfive\nsix\nseven\neight\n".into(),
                TextDecoding::Utf8,
            )),
            new: Some(TextSide::new(
                "zero\none\nTWO\nthree\nfour\nfive\nsix\nseven\neight\n".into(),
                TextDecoding::Utf8,
            )),
            changes: vec![
                TextChange::Equal {
                    old_lines: 0..2,
                    new_lines: 0..2,
                },
                TextChange::Replace {
                    old_lines: 2..3,
                    new_lines: 2..3,
                    alignment: Arc::from([LinePair {
                        old_line: Some(2),
                        new_line: Some(2),
                    }]),
                },
                TextChange::Equal {
                    old_lines: 3..9,
                    new_lines: 3..9,
                },
            ]
            .into(),
        }
    }

    #[test]
    fn unified_changes_are_separate_source_rows() {
        let rows = unified_rows_with_context(&diff(), false);
        assert!(rows.iter().any(|row| matches!(
            row,
            DiffRow::Line { left: Some(cell), right: None }
                if cell.kind == CellKind::Deletion
        )));
        assert!(rows.iter().any(|row| matches!(
            row,
            DiffRow::Line { left: None, right: Some(cell) }
                if cell.kind == CellKind::Addition
        )));
    }

    #[test]
    fn split_changes_share_an_aligned_row() {
        let rows = split_rows_with_context(&diff(), false);
        assert!(rows.iter().any(|row| matches!(
            row,
            DiffRow::Line { left: Some(left), right: Some(right) }
                if left.kind == CellKind::Deletion && right.kind == CellKind::Addition
        )));
    }

    #[test]
    fn distant_context_is_collapsed() {
        let rows = unified_rows_with_context(&diff(), false);
        assert!(rows.iter().any(|row| matches!(row, DiffRow::Gap { .. })));
        assert!(matches!(rows.first(), Some(DiffRow::Hunk { .. })));
    }

    #[test]
    fn full_context_removes_gap_rows_without_losing_the_hunk() {
        let rows = unified_rows_with_context(&diff(), true);
        assert!(!rows.iter().any(|row| matches!(row, DiffRow::Gap { .. })));
        assert!(matches!(rows.first(), Some(DiffRow::Hunk { .. })));
    }
}
