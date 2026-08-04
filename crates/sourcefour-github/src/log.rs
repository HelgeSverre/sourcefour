//! Reading Actions job logs: every line starts with an ISO timestamp, and
//! steps carry start/completion times, so a step's lines are a time window.

use crate::time::parse_iso8601;

/// Splits a log line into its leading timestamp and the text after it;
/// lines without a parseable timestamp are all text.
#[must_use]
pub fn split_timestamp(line: &str) -> (Option<i64>, &str) {
    let Some((prefix, rest)) = line.split_once(' ') else {
        return (None, line);
    };
    match parse_iso8601(prefix) {
        Some(seconds) => (Some(seconds), rest),
        None => (None, line),
    }
}

/// The index range of the lines within a step's `[started, completed]`
/// window. Absent boundaries fall back to the log's edges, and lines
/// without timestamps belong to the window of the last timestamp seen.
#[must_use]
pub fn step_slice(
    lines: &[String],
    started_at: Option<i64>,
    completed_at: Option<i64>,
) -> std::ops::Range<usize> {
    let mut start = 0;
    let mut end = lines.len();
    let mut last_seen: Option<i64> = None;
    let mut start_found = started_at.is_none();
    for (index, line) in lines.iter().enumerate() {
        if let (Some(seconds), _) = split_timestamp(line) {
            last_seen = Some(seconds);
        }
        let Some(seconds) = last_seen else { continue };
        if !start_found {
            if started_at.is_some_and(|started| seconds >= started) {
                start = index;
                start_found = true;
            }
            continue;
        }
        if completed_at.is_some_and(|completed| seconds > completed) {
            end = index;
            break;
        }
    }
    if !start_found {
        return 0..0;
    }
    start..end
}

/// The first line that reads as an error: an Actions `##[error]` marker or
/// a compiler-style line starting with `error`.
#[must_use]
pub fn first_error(lines: &[String]) -> Option<usize> {
    lines.iter().position(|line| {
        let (_, text) = split_timestamp(line);
        text.starts_with("error") || text.contains("##[error]")
    })
}

#[cfg(test)]
mod tests {
    use super::{first_error, split_timestamp, step_slice};

    fn log() -> Vec<String> {
        [
            "2026-08-04T14:30:04.1111111Z ##[group]Run actions/checkout",
            "2026-08-04T14:30:05.1111111Z Syncing repository",
            "2026-08-04T14:30:59.2222222Z ##[group]Run cargo clippy",
            "2026-08-04T14:31:40.2222222Z     Checking sourcefour v0.1.0",
            "wrapped output without a timestamp",
            "2026-08-04T14:31:41.2222222Z error[E0308]: mismatched types",
            "2026-08-04T14:32:01.3333333Z ##[error]Process completed with exit code 101.",
            "2026-08-04T14:32:02.3333333Z Post job cleanup",
        ]
        .map(String::from)
        .to_vec()
    }

    #[test]
    fn timestamps_split_off_the_text() {
        assert_eq!(
            split_timestamp("2026-08-04T14:30:04.1111111Z hello world"),
            (Some(1_785_853_804), "hello world")
        );
        assert_eq!(
            split_timestamp("no timestamp here"),
            (None, "no timestamp here")
        );
    }

    #[test]
    fn a_steps_window_slices_its_lines_including_wrapped_ones() {
        // The clippy step: 14:30:59 ..= 14:32:01.
        let range = step_slice(&log(), Some(1_785_853_859), Some(1_785_853_921));

        assert_eq!(range, 2..7, "wrapped line rides with its window");
    }

    #[test]
    fn absent_boundaries_fall_back_to_the_log_edges() {
        assert_eq!(step_slice(&log(), None, None), 0..8);
        assert_eq!(step_slice(&log(), Some(1_785_853_859), None), 2..8);
    }

    #[test]
    fn the_first_error_is_the_compiler_line_not_the_marker() {
        assert_eq!(first_error(&log()), Some(5));
        assert_eq!(
            first_error(&[String::from("2026-08-04T14:32:01Z ##[error]exit 101")]),
            Some(0),
            "the Actions marker counts when no compiler line precedes it"
        );
        assert_eq!(first_error(&[String::from("all fine")]), None);
    }
}
