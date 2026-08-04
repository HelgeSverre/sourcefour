//! Pairing of unified diff lines into side-by-side rows.

use sourcefour_model::{DiffLine, DiffLineKind};

/// One half of a split row: line number and content for one side.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SplitSide {
    pub(crate) number: Option<u32>,
    pub(crate) text: String,
    pub(crate) kind: DiffLineKind,
}

/// One visual row of the side-by-side view.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct SplitRow {
    /// A full-width hunk header; when set, both sides are empty.
    pub(crate) hunk: Option<String>,
    pub(crate) left: Option<SplitSide>,
    pub(crate) right: Option<SplitSide>,
}

/// Pairs unified lines into split rows: context on both sides, each run of
/// deletions aligned against the following run of additions.
pub(crate) fn split_rows(lines: &[DiffLine]) -> Vec<SplitRow> {
    let side = |line: &DiffLine, number: Option<u32>| SplitSide {
        number,
        text: line.text.clone(),
        kind: line.kind,
    };
    let mut rows = Vec::with_capacity(lines.len());
    let mut index = 0;
    while index < lines.len() {
        let line = &lines[index];
        match line.kind {
            DiffLineKind::Hunk | DiffLineKind::Meta | DiffLineKind::Marker => {
                rows.push(SplitRow {
                    hunk: Some(line.text.clone()),
                    ..SplitRow::default()
                });
                index += 1;
            }
            DiffLineKind::Context => {
                rows.push(SplitRow {
                    hunk: None,
                    left: Some(side(line, line.old_line)),
                    right: Some(side(line, line.new_line)),
                });
                index += 1;
            }
            DiffLineKind::Deletion | DiffLineKind::Addition => {
                // Collect the whole change run, then zip the two sides.
                let mut deletions = Vec::new();
                let mut additions = Vec::new();
                while index < lines.len() {
                    match lines[index].kind {
                        DiffLineKind::Deletion => deletions.push(&lines[index]),
                        DiffLineKind::Addition => additions.push(&lines[index]),
                        _ => break,
                    }
                    index += 1;
                }
                for row in 0..deletions.len().max(additions.len()) {
                    rows.push(SplitRow {
                        hunk: None,
                        left: deletions.get(row).map(|line| side(line, line.old_line)),
                        right: additions.get(row).map(|line| side(line, line.new_line)),
                    });
                }
            }
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use sourcefour_model::{DiffLine, DiffLineKind};

    use super::{SplitSide, split_rows};

    fn line(kind: DiffLineKind, text: &str) -> DiffLine {
        DiffLine {
            kind,
            text: text.to_owned(),
            old_line: (kind != DiffLineKind::Addition).then_some(1),
            new_line: (kind != DiffLineKind::Deletion).then_some(1),
        }
    }

    fn texts(side: Option<&SplitSide>) -> Option<String> {
        side.map(|side| side.text.clone())
    }

    #[test]
    fn context_appears_on_both_sides() {
        let rows = split_rows(&[line(DiffLineKind::Context, "same")]);

        assert_eq!(rows.len(), 1);
        assert_eq!(texts(rows[0].left.as_ref()).as_deref(), Some("same"));
        assert_eq!(texts(rows[0].right.as_ref()).as_deref(), Some("same"));
    }

    #[test]
    fn paired_changes_share_a_row_and_extras_stand_alone() {
        let rows = split_rows(&[
            line(DiffLineKind::Deletion, "old one"),
            line(DiffLineKind::Deletion, "old two"),
            line(DiffLineKind::Addition, "new one"),
        ]);

        assert_eq!(rows.len(), 2, "two deletions pair against one addition");
        assert_eq!(texts(rows[0].left.as_ref()).as_deref(), Some("old one"));
        assert_eq!(texts(rows[0].right.as_ref()).as_deref(), Some("new one"));
        assert_eq!(texts(rows[1].left.as_ref()).as_deref(), Some("old two"));
        assert!(rows[1].right.is_none(), "the unpaired deletion sits alone");
    }

    #[test]
    fn hunk_headers_span_the_full_width() {
        let rows = split_rows(&[
            line(DiffLineKind::Hunk, "@@ -1 +1 @@"),
            line(DiffLineKind::Addition, "added"),
        ]);

        assert_eq!(rows[0].hunk.as_deref(), Some("@@ -1 +1 @@"));
        assert!(rows[0].left.is_none() && rows[0].right.is_none());
        assert!(rows[1].left.is_none());
        assert_eq!(texts(rows[1].right.as_ref()).as_deref(), Some("added"));
    }

    #[test]
    fn a_change_run_after_context_still_pairs() {
        let rows = split_rows(&[
            line(DiffLineKind::Context, "keep"),
            line(DiffLineKind::Deletion, "gone"),
            line(DiffLineKind::Addition, "here"),
            line(DiffLineKind::Context, "keep too"),
        ]);

        assert_eq!(rows.len(), 3);
        assert_eq!(texts(rows[1].left.as_ref()).as_deref(), Some("gone"));
        assert_eq!(texts(rows[1].right.as_ref()).as_deref(), Some("here"));
    }
}
