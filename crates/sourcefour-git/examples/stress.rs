//! Headless stress harness for real repositories.
//!
//! Drives the exact pipeline the window uses — discovery, metadata snapshot,
//! history cursor, §7.3 lane layout — over each repository given on the
//! command line, timing every stage and validating the graph stream the §7.4
//! painter consumes.
//!
//! ```sh
//! cargo run --release -p sourcefour-git --example stress -- ~/code/crescat …
//! ```

use std::{collections::HashMap, path::Path, time::Instant};

use sourcefour_git::{GixHistoryCursor, HistoryCursor as _, discover, snapshot};
use sourcefour_model::{
    CommitRow, GRAPH_COLOR_COUNT, GRAPH_MAX_LANES, GraphRow, GraphSegment, HistoryQuery,
    HistoryScope,
};

fn main() {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    if paths.is_empty() {
        eprintln!("usage: stress <repository>...");
        std::process::exit(2);
    }
    let mut failures = 0_u32;
    for path in &paths {
        if let Err(error) = run(Path::new(path)) {
            eprintln!("{path}: {error}");
            failures += 1;
        }
    }
    if failures > 0 {
        std::process::exit(1);
    }
}

fn run(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let started = Instant::now();
    let location = discover(path)?;
    let discover_ms = started.elapsed().as_millis();

    let started = Instant::now();
    let snap = snapshot(&location)?;
    let snapshot_ms = started.elapsed().as_millis();

    let mut cursor = GixHistoryCursor::start(
        &location,
        HistoryQuery {
            scope: HistoryScope::AllRefs,
        },
        snap.labels_by_object.clone(),
    )?;

    let mut validator = Validator::default();
    let mut rows = 0_usize;
    let mut batches = 0_u32;
    let mut first_batch_ms = 0_u128;
    let mut max_batch_ms = 0_u128;
    let walk_started = Instant::now();
    loop {
        let requested = cursor.next_batch_size();
        let batch_started = Instant::now();
        let batch = cursor.next_batch(requested)?;
        let batch_ms = batch_started.elapsed().as_millis();
        if batches == 0 {
            first_batch_ms = batch_ms;
        }
        max_batch_ms = max_batch_ms.max(batch_ms);
        batches += 1;
        if batch.rows.len() != batch.graph_rows.len() {
            validator.violations.push(format!(
                "batch {batches}: {} commit rows but {} graph rows",
                batch.rows.len(),
                batch.graph_rows.len()
            ));
        }
        for (commit, graph) in batch.rows.iter().zip(&batch.graph_rows) {
            validator.check(rows, commit, graph);
            rows += 1;
        }
        if !batch.has_more {
            break;
        }
    }
    let walk_ms = walk_started.elapsed().as_millis().max(1);
    let rate = u128::try_from(rows).unwrap_or_default() * 1000 / walk_ms;

    println!(
        "{name:24} {rows:>7} rows  discover {discover_ms:>4}ms  snapshot {snapshot_ms:>4}ms  \
         first batch {first_batch_ms:>4}ms  full walk {walk_ms:>6}ms  {rate:>6} rows/s  \
         max batch {max_batch_ms:>4}ms  max lanes {lanes:>3}  violations {violations}  \
         capped {capped}",
        name = path.file_name().map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into_owned()
        ),
        lanes = validator.max_lanes + 1,
        violations = validator.violations.len(),
        capped = validator.cap_degradations,
    );
    for violation in validator.violations.iter().take(5) {
        println!("    !! {violation}");
    }
    Ok(())
}

/// Checks the invariants the §7.4 painter relies on, row by row.
#[derive(Default)]
struct Validator {
    /// Lane → color leaving the previous row's bottom edge.
    leaving: Option<HashMap<u16, u8>>,
    violations: Vec<String>,
    /// Continuity mismatches confined to the §7.6 overflow lane.
    cap_degradations: usize,
    max_lanes: u16,
}

impl Validator {
    fn check(&mut self, index: usize, commit: &CommitRow, graph: &GraphRow) {
        let flags = graph.flags;
        if flags.is_merge != (commit.parents.len() > 1) {
            self.violations
                .push(format!("row {index}: is_merge disagrees with parent count"));
        }
        if flags.is_root != commit.parents.is_empty() {
            self.violations
                .push(format!("row {index}: is_root disagrees with parent count"));
        }
        if graph.node_color >= GRAPH_COLOR_COUNT {
            self.violations.push(format!(
                "row {index}: node color {} out of range",
                graph.node_color
            ));
        }
        if graph.node_lane >= GRAPH_MAX_LANES {
            self.violations.push(format!(
                "row {index}: node lane {} exceeds the cap",
                graph.node_lane
            ));
        }
        let merges = graph
            .segments
            .iter()
            .filter(|segment| matches!(segment, GraphSegment::Merge { .. }))
            .count();
        if merges != commit.parents.len().saturating_sub(1) {
            self.violations.push(format!(
                "row {index}: {merges} merge segments for {} parents",
                commit.parents.len()
            ));
        }

        // Every line entering this row from above must be exactly the set of
        // lines that left the previous row, color for color — otherwise the
        // painter draws a dangling or missing connection between the rows.
        let mut entering = HashMap::new();
        if flags.continues_above {
            entering.insert(graph.node_lane, graph.node_color);
        }
        let mut leaving = HashMap::new();
        if !flags.is_root {
            leaving.insert(graph.node_lane, graph.node_color);
        }
        for segment in &graph.segments {
            match *segment {
                GraphSegment::Vertical { lane, color } => {
                    entering.insert(lane, color);
                    leaving.insert(lane, color);
                    self.max_lanes = self.max_lanes.max(lane);
                }
                GraphSegment::Fork { from, to, color } => {
                    if to != graph.node_lane {
                        self.violations.push(format!(
                            "row {index}: fork lands in lane {to}, not the node"
                        ));
                    }
                    entering.insert(from, color);
                    self.max_lanes = self.max_lanes.max(from);
                }
                GraphSegment::Merge { from, to, color } => {
                    if from != graph.node_lane {
                        self.violations.push(format!(
                            "row {index}: merge leaves lane {from}, not the node"
                        ));
                    }
                    leaving.insert(to, color);
                    self.max_lanes = self.max_lanes.max(to);
                }
                GraphSegment::Terminate { .. } => {}
            }
        }
        self.max_lanes = self.max_lanes.max(graph.node_lane);
        if let Some(previous) = &self.leaving
            && *previous != entering
        {
            // §7.6 parks every line beyond the cap in the last lane, so
            // bookkeeping there degrades by design. A mismatch anywhere below
            // the cap lane would be a real painter bug.
            let overflow_lane = GRAPH_MAX_LANES - 1;
            let confined = previous
                .iter()
                .filter(|(lane, color)| entering.get(lane) != Some(color))
                .chain(
                    entering
                        .iter()
                        .filter(|(lane, color)| previous.get(lane) != Some(color)),
                )
                .all(|(lane, _)| *lane >= overflow_lane);
            if confined {
                self.cap_degradations += 1;
            } else {
                self.violations.push(format!(
                    "row {index}: lines entering do not match lines leaving row {}: \
                     left {previous:?}, entered {entering:?}",
                    index - 1
                ));
            }
        }
        self.leaving = Some(leaving);
    }
}
