//! The Actions run detail overlay (mockup/actions.html): failure-first.
//!
//! Opening a run pre-selects its failing job and shows the failing step's
//! log slice, jumped to the first error — zero clicks from a red dot to
//! why. Duration bars answer where the time went, and the timeline strip
//! shows job parallelism on one wall-clock axis.

use gpui::{
    Div, FontWeight, IntoElement, ScrollStrategy, UniformListScrollHandle, Window, div, prelude::*,
    px, uniform_list,
};
use sourcefour_model::{CheckConclusion, CheckStatus, WorkflowJob, WorkflowRun};

use crate::theme::MONO_FONT;

use super::{SourcefourWindow, github::check_glyph, row_count_as_f32};

/// Row height of one virtualized log line.
const LOG_ROW_HEIGHT: f32 = 16.0;

/// Jobs warmed by a press before the click that opens the overlay: the
/// mouse-down starts the fetch, the mouse-up finds it in flight or done.
pub(super) struct ActionsPrefetch {
    pub(super) run_id: u64,
    pub(super) jobs: Option<Result<Vec<WorkflowJob>, String>>,
}

/// One open run: its identity plus lazily arriving jobs and log.
pub(super) struct ActionsView {
    pub(super) run: WorkflowRun,
    /// `None` while the first jobs read is in flight.
    pub(super) jobs: Option<Result<Vec<WorkflowJob>, String>>,
    pub(super) selected_job: usize,
    /// The step whose log slice shows; `None` means the failing step.
    pub(super) selected_step: Option<usize>,
    /// The selected job's complete log, split into lines.
    pub(super) log: Option<Result<Vec<String>, String>>,
    /// Show the whole job log instead of the selected step's slice.
    pub(super) full_log: bool,
}

impl ActionsView {
    fn jobs_ok(&self) -> &[WorkflowJob] {
        match &self.jobs {
            Some(Ok(jobs)) => jobs,
            _ => &[],
        }
    }

    fn selected(&self) -> Option<&WorkflowJob> {
        self.jobs_ok().get(self.selected_job)
    }

    /// The step whose log shows: the chosen one, else the failing one,
    /// else the longest.
    fn focused_step(&self) -> Option<usize> {
        let job = self.selected()?;
        if let Some(chosen) = self.selected_step {
            return Some(chosen.min(job.steps.len().saturating_sub(1)));
        }
        default_step(job)
    }
}

/// The job a failed run opens on: first failed, else first still running,
/// else the longest.
fn default_job(jobs: &[WorkflowJob]) -> usize {
    let failed = jobs
        .iter()
        .position(|job| matches!(job.status, CheckStatus::Completed(CheckConclusion::Failure)));
    let running = || {
        jobs.iter()
            .position(|job| matches!(job.status, CheckStatus::InProgress | CheckStatus::Queued))
    };
    let longest = || {
        jobs.iter()
            .enumerate()
            .max_by_key(|(_, job)| duration_of(job.started_at, job.completed_at).unwrap_or(0))
            .map(|(index, _)| index)
    };
    failed.or_else(running).or_else(longest).unwrap_or(0)
}

/// The step a job opens on: first failed, else the longest.
fn default_step(job: &WorkflowJob) -> Option<usize> {
    job.steps
        .iter()
        .position(|step| {
            matches!(
                step.status,
                CheckStatus::Completed(CheckConclusion::Failure)
            )
        })
        .or_else(|| {
            job.steps
                .iter()
                .enumerate()
                .max_by_key(|(_, step)| {
                    duration_of(step.started_at, step.completed_at).unwrap_or(0)
                })
                .map(|(index, _)| index)
        })
}

/// The run's status as the fetched jobs tell it — `run.status` froze when
/// the overlay opened, so a live run's header follows the jobs instead.
fn effective_run_status(view: &ActionsView) -> CheckStatus {
    let jobs = view.jobs_ok();
    if jobs.is_empty() {
        return view.run.status;
    }
    if jobs
        .iter()
        .any(|job| matches!(job.status, CheckStatus::InProgress | CheckStatus::Queued))
    {
        return CheckStatus::InProgress;
    }
    if jobs
        .iter()
        .any(|job| matches!(job.status, CheckStatus::Completed(CheckConclusion::Failure)))
    {
        return CheckStatus::Completed(CheckConclusion::Failure);
    }
    CheckStatus::Completed(CheckConclusion::Success)
}

/// Wall seconds between two boundaries when both are known.
fn duration_of(started: Option<i64>, completed: Option<i64>) -> Option<i64> {
    Some((completed? - started?).max(0))
}

/// Like [`duration_of`], but something still running measures to `now`, so
/// live views show elapsed time instead of `—`.
fn elapsed_of(started: Option<i64>, completed: Option<i64>, now: i64) -> Option<i64> {
    duration_of(started, completed.or(Some(now)))
}

/// Durations the way the mockup writes them: `3s`, `1m 58s`, `—` unknown.
fn fmt_duration(seconds: Option<i64>) -> String {
    match seconds {
        None => String::from("—"),
        Some(seconds) if seconds < 60 => format!("{seconds}s"),
        Some(seconds) => format!("{}m {:02}s", seconds / 60, seconds % 60),
    }
}

/// A step's share of its job's longest step, floored so a bar always shows.
fn bar_fraction(step_seconds: i64, longest_seconds: i64) -> f32 {
    if longest_seconds <= 0 {
        return 0.02;
    }
    #[expect(clippy::cast_precision_loss, reason = "display fraction, not math")]
    let fraction = step_seconds as f32 / longest_seconds as f32;
    fraction.clamp(0.02, 1.0)
}

/// How one log line tints.
fn line_tint(text: &str) -> LineTint {
    if text.starts_with("error") || text.contains("##[error]") {
        LineTint::Error
    } else if text.starts_with("warning") {
        LineTint::Warning
    } else {
        LineTint::Plain
    }
}

enum LineTint {
    Error,
    Warning,
    Plain,
}

/// The §12.4 capture fixture: the mockup's failed Windows run, seeded so
/// the overlay renders without a repository or network.
pub(super) fn demo_view() -> ActionsView {
    ActionsView {
        run: demo_run(),
        jobs: Some(Ok(demo_jobs())),
        selected_job: 1,
        selected_step: None,
        log: Some(Ok(demo_log())),
        full_log: false,
    }
}

/// The demo run's identity, matching the mockup header.
fn demo_run() -> WorkflowRun {
    WorkflowRun {
        id: 143,
        name: String::from("CI"),
        display_title: String::from("fix: Windows paths, in the interface and in fixtures"),
        run_number: 143,
        event: String::from("push"),
        actor: String::from("HelgeSverre"),
        branch: String::from("main"),
        sha: String::from("b61a08d2f4c"),
        status: CheckStatus::Completed(CheckConclusion::Failure),
        started_at: Some(DEMO_BASE),
        completed_at: Some(DEMO_BASE + 125),
        html_url: String::new(),
    }
}

/// Two hours before the demo's fixed "now", so relative dates and
/// elapsed math stay coherent with [`crate::demo::NOW_SECONDS`].
const DEMO_BASE: i64 = crate::demo::NOW_SECONDS - 7200;

/// The demo jobs, matching the mockup rail and timeline.
fn demo_jobs() -> Vec<WorkflowJob> {
    let step = |name: &str, conclusion, start: i64, end: i64| sourcefour_model::WorkflowStep {
        name: name.to_owned(),
        status: CheckStatus::Completed(conclusion),
        started_at: Some(DEMO_BASE + start),
        completed_at: Some(DEMO_BASE + end),
    };
    let skipped = |name: &str| sourcefour_model::WorkflowStep {
        name: name.to_owned(),
        status: CheckStatus::Completed(CheckConclusion::Skipped),
        started_at: None,
        completed_at: None,
    };
    let job = |id: u64, name: &str, conclusion, start: i64, end: i64, steps: Vec<_>| WorkflowJob {
        id,
        name: name.to_owned(),
        status: CheckStatus::Completed(conclusion),
        started_at: Some(DEMO_BASE + start),
        completed_at: Some(DEMO_BASE + end),
        steps,
        html_url: String::new(),
    };
    vec![
        job(
            1,
            "quality (ubuntu-latest)",
            CheckConclusion::Success,
            2,
            123,
            vec![
                step("Set up job", CheckConclusion::Success, 2, 5),
                step("Checkout", CheckConclusion::Success, 5, 9),
                step("Install Rust toolchain", CheckConclusion::Success, 9, 47),
                step(
                    "cargo clippy --all-targets",
                    CheckConclusion::Success,
                    47,
                    95,
                ),
                step("cargo test", CheckConclusion::Success, 95, 121),
                step("Post checkout", CheckConclusion::Success, 121, 123),
            ],
        ),
        job(
            2,
            "quality (windows-latest)",
            CheckConclusion::Failure,
            3,
            121,
            vec![
                step("Set up job", CheckConclusion::Success, 3, 6),
                step("Checkout", CheckConclusion::Success, 6, 10),
                step("Install Rust toolchain", CheckConclusion::Success, 10, 51),
                step("cargo fmt --check", CheckConclusion::Success, 51, 53),
                step(
                    "cargo clippy --all-targets",
                    CheckConclusion::Failure,
                    53,
                    115,
                ),
                skipped("cargo test"),
                skipped("Upload artifacts"),
                step("Post checkout", CheckConclusion::Success, 115, 116),
            ],
        ),
        job(
            3,
            "quality (macos-14)",
            CheckConclusion::Success,
            4,
            88,
            vec![
                step("Set up job", CheckConclusion::Success, 4, 7),
                step("Checkout", CheckConclusion::Success, 7, 11),
                step("Install Rust toolchain", CheckConclusion::Success, 11, 42),
                step(
                    "cargo clippy --all-targets",
                    CheckConclusion::Success,
                    42,
                    74,
                ),
                step("cargo test", CheckConclusion::Success, 74, 88),
            ],
        ),
        job(
            4,
            "package (.pkg, .msi)",
            CheckConclusion::Skipped,
            121,
            122,
            Vec::new(),
        ),
    ]
}

/// The demo windows job's log, matching the mockup's failing clippy step.
fn demo_log() -> Vec<String> {
    [
        "2024-08-02T10:00:04Z ##[group]Run actions/checkout@v4",
        "2024-08-02T10:00:07Z Syncing repository: HelgeSverre/sourcefour",
        "2024-08-02T10:00:12Z ##[group]Run dtolnay/rust-toolchain@stable",
        "2024-08-02T10:00:51Z installed rustc 1.97.0",
        "2024-08-02T10:00:54Z     Checking sourcefour v0.1.0 (D:\\a\\sourcefour\\apps\\sourcefour)",
        "2024-08-02T10:01:40Z error[E0308]: mismatched types",
        "2024-08-02T10:01:40Z   --> apps\\sourcefour\\src\\views\\chrome.rs:221:36",
        "2024-08-02T10:01:40Z     |",
        "2024-08-02T10:01:40Z 221 |    .unwrap_or_else(|| WorktreeId(String::from(\"active\"))),",
        "2024-08-02T10:01:40Z     |                       ^^^^^^^^^^ expected `PathBuf`, found `String`",
        "2024-08-02T10:01:41Z warning: unused import: `std::path::PathBuf`",
        "2024-08-02T10:01:44Z error: could not compile `sourcefour` (bin \"sourcefour\") due to 1 previous error",
        "2024-08-02T10:01:55Z ##[error]Process completed with exit code 101.",
    ]
    .map(String::from)
    .to_vec()
}

impl SourcefourWindow {
    /// Opens the run's overlay and starts its jobs read and live poll.
    pub(super) fn open_actions_run(
        &mut self,
        run: WorkflowRun,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        self.actions_focus.focus(window);
        self.actions_log_scroll = UniformListScrollHandle::new();
        let run_id = run.id;
        self.actions_view = Some(ActionsView {
            run,
            jobs: None,
            selected_job: 0,
            selected_step: None,
            log: None,
            full_log: false,
        });
        match self.actions_prefetch.take_if(|warm| warm.run_id == run_id) {
            // The press already finished the fetch: seed instantly.
            Some(ActionsPrefetch {
                jobs: Some(outcome),
                ..
            }) => self.apply_actions_jobs(run_id, outcome, cx),
            // A press's fetch may be in flight — but a later press for a
            // different run retires it through the shared request counter,
            // so never trust it: re-request. The newest counter wins, so
            // the cost is one redundant read in the benign race.
            Some(ActionsPrefetch { jobs: None, .. }) | None => {
                self.fetch_actions_jobs(run_id, cx);
            }
        }
        self.poll_actions(cx);
    }

    /// Starts warming a run's jobs on mouse-down, ahead of the click.
    pub(super) fn prefetch_actions_jobs(&mut self, run_id: u64, cx: &mut gpui::Context<Self>) {
        if self.github_remote.is_none()
            || self
                .actions_prefetch
                .as_ref()
                .is_some_and(|warm| warm.run_id == run_id)
            || self
                .actions_view
                .as_ref()
                .is_some_and(|view| view.run.id == run_id)
        {
            return;
        }
        self.actions_prefetch = Some(ActionsPrefetch { run_id, jobs: None });
        self.fetch_actions_jobs(run_id, cx);
    }

    /// Lands one jobs outcome in the open view — or the prefetch stash when
    /// the click has not arrived yet.
    fn apply_actions_jobs(
        &mut self,
        run_id: u64,
        outcome: Result<Vec<WorkflowJob>, String>,
        cx: &mut gpui::Context<Self>,
    ) {
        if let Some(view) = &mut self.actions_view
            && view.run.id == run_id
        {
            let first_arrival = view.jobs.is_none();
            // A poll refresh keeps the user's selection; a failed refresh
            // keeps the last good jobs.
            if first_arrival || outcome.is_ok() {
                view.jobs = Some(outcome);
            }
            if first_arrival {
                view.selected_job = default_job(view.jobs_ok());
                self.load_actions_log(cx);
            }
            cx.notify();
        } else if let Some(warm) = &mut self.actions_prefetch
            && warm.run_id == run_id
        {
            warm.jobs = Some(outcome);
        }
    }

    /// Closes the overlay, returning focus to the history.
    pub(super) fn close_actions(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        self.actions_view = None;
        self.focus.focus(window);
        cx.notify();
    }

    /// Moves the job selection by `delta` and reloads its log.
    pub(super) fn select_actions_job(&mut self, delta: isize, cx: &mut gpui::Context<Self>) {
        let Some(view) = &self.actions_view else {
            return;
        };
        let count = view.jobs_ok().len();
        if count == 0 {
            return;
        }
        let next = view
            .selected_job
            .saturating_add_signed(delta)
            .min(count - 1);
        self.set_actions_job(next, cx);
    }

    /// Selects one job outright and reloads its log.
    fn set_actions_job(&mut self, index: usize, cx: &mut gpui::Context<Self>) {
        let Some(view) = &mut self.actions_view else {
            return;
        };
        if index == view.selected_job || index >= view.jobs_ok().len() {
            return;
        }
        view.selected_job = index;
        view.selected_step = None;
        view.full_log = false;
        view.log = None;
        self.load_actions_log(cx);
        cx.notify();
    }

    /// Moves the step selection; the log slice follows.
    pub(super) fn select_actions_step(&mut self, delta: isize, cx: &mut gpui::Context<Self>) {
        let Some(view) = &mut self.actions_view else {
            return;
        };
        let Some(job) = view.selected() else {
            return;
        };
        let count = job.steps.len();
        if count == 0 {
            return;
        }
        let current = view.focused_step().unwrap_or(0);
        let next = current.saturating_add_signed(delta).min(count - 1);
        view.selected_step = Some(next);
        view.full_log = false;
        self.scroll_actions_log_to_slice();
        cx.notify();
    }

    /// Reads one run's jobs; the outcome lands wherever the run lives now
    /// (the open view or the prefetch stash).
    fn fetch_actions_jobs(&mut self, run_id: u64, cx: &mut gpui::Context<Self>) {
        let Some(remote) = self.github_remote.clone() else {
            return;
        };
        self.fetch_github(
            |this| &mut this.actions_request,
            move |token| {
                sourcefour_github::run_jobs(
                    &sourcefour_github::UreqTransport,
                    &remote,
                    &token,
                    run_id,
                )
            },
            move |this, outcome, cx| {
                this.apply_actions_jobs(run_id, outcome, cx);
            },
            cx,
        );
    }

    /// Reads the selected job's log and jumps the list to the slice.
    fn load_actions_log(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(remote) = self.github_remote.clone() else {
            return;
        };
        let Some(view) = &self.actions_view else {
            return;
        };
        let run_id = view.run.id;
        let Some(job) = view.selected() else {
            return;
        };
        let job_id = job.id;
        self.fetch_github(
            |this| &mut this.actions_log_request,
            move |token| {
                sourcefour_github::job_log(
                    &sourcefour_github::UreqTransport,
                    &remote,
                    &token,
                    job_id,
                )
            },
            move |this, outcome, cx| {
                let Some(view) = &mut this.actions_view else {
                    return;
                };
                if view.run.id != run_id || view.selected().is_none_or(|job| job.id != job_id) {
                    return;
                }
                view.log =
                    Some(outcome.map(|log| log.lines().map(str::to_owned).collect::<Vec<_>>()));
                this.scroll_actions_log_to_slice();
                cx.notify();
            },
            cx,
        );
    }

    /// Re-reads jobs and log past every cache, the ↻ affordance.
    pub(super) fn refresh_actions(&mut self, cx: &mut gpui::Context<Self>) {
        if let Some(view) = &self.actions_view {
            self.fetch_actions_jobs(view.run.id, cx);
        }
        self.load_actions_log(cx);
    }

    /// While the run is alive and its overlay open, jobs re-read every five
    /// seconds so steps tick live. The poll token retires stale loops.
    fn poll_actions(&mut self, cx: &mut gpui::Context<Self>) {
        self.actions_poll += 1;
        let poll = self.actions_poll;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(5))
                    .await;
                let keep_going = this
                    .update(cx, |this, cx| {
                        if this.actions_poll != poll {
                            return false;
                        }
                        let Some(view) = &this.actions_view else {
                            return false;
                        };
                        // `run.status` froze at open time, so completion is
                        // read from the fetched jobs.
                        let done = !view.jobs_ok().is_empty()
                            && view
                                .jobs_ok()
                                .iter()
                                .all(|job| matches!(job.status, CheckStatus::Completed(_)));
                        if done {
                            return false;
                        }
                        if let Some(view) = &this.actions_view {
                            this.fetch_actions_jobs(view.run.id, cx);
                        }
                        true
                    })
                    .unwrap_or(false);
                if !keep_going {
                    return;
                }
            }
        })
        .detach();
    }

    /// The log rows the list currently shows: the whole log, or the focused
    /// step's slice.
    fn actions_log_window(&self) -> Option<(usize, usize, usize)> {
        let view = self.actions_view.as_ref()?;
        let Some(Ok(lines)) = &view.log else {
            return None;
        };
        if view.full_log {
            return Some((0, lines.len(), lines.len()));
        }
        let job = view.selected()?;
        let step = view.focused_step().and_then(|index| job.steps.get(index))?;
        let range = sourcefour_github::step_slice(lines, step.started_at, step.completed_at);
        Some((range.start, range.end, lines.len()))
    }

    /// Jumps the log list to the first error inside the visible window.
    fn scroll_actions_log_to_slice(&mut self) {
        let Some((start, end, _)) = self.actions_log_window() else {
            return;
        };
        let Some(view) = &self.actions_view else {
            return;
        };
        let target = match &view.log {
            Some(Ok(lines)) => sourcefour_github::first_error(&lines[start..end])
                .map_or(0, |offset| offset.saturating_sub(2)),
            _ => 0,
        };
        self.actions_log_scroll
            .scroll_to_item(target, ScrollStrategy::Top);
    }

    /// The overlay, while a run is open.
    pub(super) fn actions_overlay(
        &self,
        cx: &mut gpui::Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let view = self.actions_view.as_ref()?;
        Some(
            div()
                .id("actions-overlay")
                .key_context("Actions")
                .track_focus(&self.actions_focus)
                .occlude()
                .absolute()
                .inset_0()
                .flex()
                .p(px(26.0))
                .bg(gpui::black().opacity(0.55))
                .on_click(cx.listener(|this, event: &gpui::ClickEvent, window, cx| {
                    let (down, up) = (event.down.position, event.up.position);
                    if (down.x.0 - up.x.0).abs() > 3.0 || (down.y.0 - up.y.0).abs() > 3.0 {
                        return;
                    }
                    this.close_actions(window, cx);
                }))
                .child(
                    div()
                        .id("actions-panel")
                        .flex_1()
                        .flex()
                        .flex_col()
                        .rounded(px(10.0))
                        .border_1()
                        .border_color(self.theme.border_strong)
                        .bg(self.theme.bg_panel)
                        .shadow_lg()
                        .overflow_hidden()
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .child(self.actions_header(view, cx))
                        .child(
                            div()
                                .flex_1()
                                .min_h(px(1.0))
                                .flex()
                                .child(self.actions_jobs_rail(view, cx))
                                .child(self.actions_steps_pane(view, cx)),
                        )
                        .child(self.actions_waterfall(view)),
                ),
        )
    }

    /// Title row and chip row, per the mockup's header.
    fn actions_header(&self, view: &ActionsView, cx: &mut gpui::Context<Self>) -> Div {
        let (glyph, color) = check_glyph(&self.theme, effective_run_status(view));
        let wall = elapsed_of(
            view.run.started_at,
            view.run.completed_at,
            self.now_seconds(),
        );
        let url = view.run.html_url.clone();
        div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(7.0))
            .px(px(16.0))
            .py(px(11.0))
            .border_b_1()
            .border_color(self.theme.border)
            .bg(self.theme.bg_chrome)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .child(
                        div()
                            .flex_none()
                            .font_weight(FontWeight::BOLD)
                            .text_color(color)
                            .child(glyph),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(1.0))
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_size(px(13.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(self.theme.text_primary)
                            .child(view.run.display_title.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(11.5))
                            .text_color(self.theme.text_secondary)
                            .child(fmt_duration(wall)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(11.0))
                            .text_color(self.theme.text_faint)
                            .child("Esc"),
                    )
                    .child(
                        div()
                            .id("actions-close")
                            .flex_none()
                            .px(px(8.0))
                            .rounded(px(4.0))
                            .cursor_pointer()
                            .text_size(px(13.0))
                            .text_color(self.theme.text_secondary)
                            .hover(|style| style.bg(self.theme.bg_hover))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.close_actions(window, cx);
                            }))
                            .child("✕"),
                    ),
            )
            .child(self.actions_header_chips(view, url, cx))
    }

    /// The header's second row: identity chips, refresh, and the escape
    /// hatch to github.com.
    fn actions_header_chips(
        &self,
        view: &ActionsView,
        url: String,
        cx: &mut gpui::Context<Self>,
    ) -> Div {
        let chip = |label: String| {
            div()
                .flex_none()
                .px(px(6.0))
                .rounded(px(4.0))
                .border_1()
                .border_color(self.theme.border_strong)
                .text_size(px(10.0))
                .text_color(self.theme.text_secondary)
                .child(label)
        };
        let tinted = |label: String, color: gpui::Hsla| {
            div()
                .flex_none()
                .px(px(6.0))
                .rounded(px(4.0))
                .border_1()
                .border_color(color.opacity(0.4))
                .bg(color.opacity(0.08))
                .text_size(px(10.0))
                .text_color(color)
                .child(label)
        };
        div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .child(tinted(
                format!("{} #{}", view.run.name, view.run.run_number),
                self.theme.accent,
            ))
            .child(tinted(view.run.branch.clone(), self.theme.green))
            .child(chip(view.run.event.clone()))
            .child(chip(view.run.actor.clone()))
            .child(
                div()
                    .flex_none()
                    .px(px(6.0))
                    .rounded(px(4.0))
                    .border_1()
                    .border_color(self.theme.border_strong)
                    .font_family(MONO_FONT)
                    .text_size(px(10.0))
                    .text_color(self.theme.text_secondary)
                    .child(view.run.sha.chars().take(7).collect::<String>()),
            )
            .child(
                div()
                    .id("actions-refresh")
                    .flex_none()
                    .px(px(5.0))
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .text_size(px(11.0))
                    .text_color(self.theme.text_faint)
                    .hover(|style| style.bg(self.theme.bg_hover))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.refresh_actions(cx);
                    }))
                    .child("↻"),
            )
            .child(div().flex_grow())
            .child(
                div()
                    .id("actions-gh-link")
                    .flex_none()
                    .cursor_pointer()
                    .text_size(px(10.5))
                    .text_color(self.theme.text_faint)
                    .hover(|style| style.text_color(self.theme.accent))
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        cx.open_url(&url);
                    })
                    .child("Open on GitHub ↗"),
            )
    }

    /// The left rail: one row per job.
    fn actions_jobs_rail(&self, view: &ActionsView, cx: &mut gpui::Context<Self>) -> Div {
        let failed = view
            .jobs_ok()
            .iter()
            .filter(|job| !matches!(job.status, CheckStatus::Completed(CheckConclusion::Success)))
            .count();
        div()
            .w(px(232.0))
            .flex_none()
            .flex()
            .flex_col()
            .pt(px(8.0))
            .border_r_1()
            .border_color(self.theme.border)
            .child(
                div()
                    .px(px(14.0))
                    .pb(px(6.0))
                    .text_size(px(10.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(self.theme.text_faint)
                    .child(match &view.jobs {
                        None => String::from("JOBS · LOADING…"),
                        Some(Err(_)) => String::from("JOBS"),
                        Some(Ok(jobs)) if failed > 0 => {
                            format!("JOBS · {failed} OF {} NOT GREEN", jobs.len())
                        }
                        Some(Ok(jobs)) => format!("JOBS · {}", jobs.len()),
                    }),
            )
            .children(match &view.jobs {
                Some(Err(message)) => vec![
                    div()
                        .px(px(14.0))
                        .text_size(px(10.5))
                        .text_color(self.theme.text_faint)
                        .child(message.clone()),
                ],
                _ => Vec::new(),
            })
            .children(view.jobs_ok().iter().enumerate().map(|(index, job)| {
                let (glyph, color) = check_glyph(&self.theme, job.status);
                let selected = index == view.selected_job;
                div()
                    .id(("actions-job", index))
                    .h(px(34.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(14.0))
                    .cursor_pointer()
                    .border_l_2()
                    .border_color(if selected {
                        self.theme.accent
                    } else {
                        gpui::transparent_black()
                    })
                    .bg(if selected {
                        self.theme.bg_selected
                    } else {
                        self.theme.bg_panel
                    })
                    .hover(|style| style.bg(self.theme.bg_hover))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_actions_job(index, cx);
                    }))
                    .child(
                        div()
                            .flex_none()
                            .font_weight(FontWeight::BOLD)
                            .text_size(px(11.0))
                            .text_color(color)
                            .child(glyph),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(1.0))
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_size(px(12.0))
                            .text_color(if selected {
                                self.theme.text_primary
                            } else {
                                self.theme.text_secondary
                            })
                            .child(job.name.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(10.0))
                            .text_color(self.theme.text_faint)
                            .child(fmt_duration(elapsed_of(
                                job.started_at,
                                job.completed_at,
                                self.now_seconds(),
                            ))),
                    )
            }))
    }

    /// The selected job's steps, with the focused step's log slice.
    fn actions_steps_pane(&self, view: &ActionsView, cx: &mut gpui::Context<Self>) -> Div {
        let pane = div()
            .flex_1()
            .min_w(px(1.0))
            .flex()
            .flex_col()
            .bg(self.theme.bg_list);
        let Some(job) = view.selected() else {
            return pane.child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(12.0))
                    .text_color(self.theme.text_faint)
                    .child("Loading jobs…"),
            );
        };
        let (glyph, color) = check_glyph(&self.theme, job.status);
        let longest = job
            .steps
            .iter()
            .filter_map(|step| duration_of(step.started_at, step.completed_at))
            .max()
            .unwrap_or(0);
        let focused = view.focused_step();
        pane.child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(10.0))
                .px(px(16.0))
                .py(px(9.0))
                .border_b_1()
                .border_color(self.theme.border)
                .child(
                    div()
                        .flex_none()
                        .font_weight(FontWeight::BOLD)
                        .text_color(color)
                        .child(glyph),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(1.0))
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_size(px(12.5))
                        .child(job.name.clone()),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(px(11.5))
                        .text_color(self.theme.text_secondary)
                        .child(fmt_duration(elapsed_of(
                            job.started_at,
                            job.completed_at,
                            self.now_seconds(),
                        ))),
                ),
        )
        .child(
            div()
                .id("actions-steps")
                .flex_1()
                .min_h(px(1.0))
                .overflow_y_scroll()
                .py(px(6.0))
                .children(job.steps.iter().enumerate().flat_map(|(index, step)| {
                    let mut rows = vec![self.actions_step_row(index, step, longest, focused, cx)];
                    if focused == Some(index) {
                        rows.extend(self.actions_log_strip(view, cx));
                    }
                    rows
                })),
        )
    }

    /// One step row: glyph, name, proportional duration bar, duration.
    fn actions_step_row(
        &self,
        index: usize,
        step: &sourcefour_model::WorkflowStep,
        longest: i64,
        focused: Option<usize>,
        cx: &mut gpui::Context<Self>,
    ) -> gpui::AnyElement {
        let (glyph, color) = check_glyph(&self.theme, step.status);
        let seconds = duration_of(step.started_at, step.completed_at);
        let is_focused = focused == Some(index);
        div()
            .id(("actions-step", index))
            .h(px(26.0))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(16.0))
            .cursor_pointer()
            .when(is_focused, |this| this.bg(self.theme.bg_hover))
            .hover(|style| style.bg(self.theme.bg_hover))
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(view) = &mut this.actions_view {
                    view.selected_step = Some(index);
                    view.full_log = false;
                    this.scroll_actions_log_to_slice();
                    cx.notify();
                }
            }))
            .child(
                div()
                    .w(px(14.0))
                    .flex_none()
                    .font_weight(FontWeight::BOLD)
                    .text_size(px(11.0))
                    .text_color(color)
                    .child(glyph),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(1.0))
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_size(px(12.0))
                    .text_color(if is_focused {
                        self.theme.text_primary
                    } else {
                        self.theme.text_secondary
                    })
                    .child(step.name.clone()),
            )
            .child(
                div()
                    .w(px(120.0))
                    .flex_none()
                    .h(px(4.0))
                    .rounded(px(2.0))
                    .bg(self.theme.bg_hover)
                    .children(seconds.map(|seconds| {
                        div()
                            .h(px(4.0))
                            .rounded(px(2.0))
                            .w(gpui::relative(bar_fraction(seconds, longest)))
                            .bg(match step.status {
                                CheckStatus::Completed(CheckConclusion::Failure) => {
                                    self.theme.red.opacity(0.75)
                                }
                                CheckStatus::Completed(CheckConclusion::Success) => {
                                    self.theme.green.opacity(0.55)
                                }
                                _ => self.theme.border_strong,
                            })
                    })),
            )
            .child(
                div()
                    .w(px(52.0))
                    .flex_none()
                    .text_size(px(10.5))
                    .text_color(self.theme.text_faint)
                    .child(fmt_duration(seconds)),
            )
            .into_any_element()
    }

    /// The flat log strip under the focused step: virtualized lines plus
    /// the footer with the full-log toggle and copy.
    fn actions_log_strip(
        &self,
        view: &ActionsView,
        cx: &mut gpui::Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        match &view.log {
            None => vec![
                div()
                    .px(px(42.0))
                    .py(px(6.0))
                    .text_size(px(10.5))
                    .text_color(self.theme.text_faint)
                    .child("Fetching log…")
                    .into_any_element(),
            ],
            Some(Err(message)) => {
                // GitHub withholds job logs until the job finishes.
                let running = view
                    .selected()
                    .is_some_and(|job| !matches!(job.status, CheckStatus::Completed(_)));
                let text = if running {
                    String::from("The log appears once this job finishes.")
                } else {
                    message.clone()
                };
                vec![
                    div()
                        .px(px(42.0))
                        .py(px(6.0))
                        .text_size(px(10.5))
                        .text_color(self.theme.text_faint)
                        .child(text)
                        .into_any_element(),
                ]
            }
            Some(Ok(lines)) => {
                let Some((start, end, total)) = self.actions_log_window() else {
                    return Vec::new();
                };
                let shown = end - start;
                let all = lines.join("\n");
                let list = uniform_list(
                    cx.entity(),
                    "actions-log",
                    shown,
                    move |this, range, _window, _cx| {
                        let Some((start, end, _)) = this.actions_log_window() else {
                            return Vec::new();
                        };
                        let Some(view) = &this.actions_view else {
                            return Vec::new();
                        };
                        let Some(Ok(lines)) = &view.log else {
                            return Vec::new();
                        };
                        range
                            .filter_map(|offset| {
                                lines.get(start + offset).filter(|_| start + offset < end)
                            })
                            .map(|line| this.actions_log_line(line))
                            .collect()
                    },
                )
                .track_scroll(self.actions_log_scroll.clone())
                .h(px(row_count_as_f32(shown.min(16)) * LOG_ROW_HEIGHT + 8.0))
                .w_full()
                .into_any_element();
                vec![
                    div()
                        .border_t_1()
                        .border_color(self.theme.border)
                        .bg(self.theme.bg_page)
                        .py(px(4.0))
                        .child(list)
                        .into_any_element(),
                    self.actions_log_footer(view.full_log, shown, total, all, cx),
                ]
            }
        }
    }

    /// The log strip's footer: what shows, the slice/full toggle, copy.
    fn actions_log_footer(
        &self,
        full_log: bool,
        shown: usize,
        total: usize,
        all: String,
        cx: &mut gpui::Context<Self>,
    ) -> gpui::AnyElement {
        let footer_label = if full_log {
            format!("showing all {total} lines")
        } else {
            format!("showing {shown} of {total} lines · jumped to first error")
        };
        div()
            .flex()
            .items_center()
            .gap(px(14.0))
            .px(px(42.0))
            .py(px(5.0))
            .border_b_1()
            .border_color(self.theme.border)
            .bg(self.theme.bg_page)
            .text_size(px(10.0))
            .text_color(self.theme.text_faint)
            .child(footer_label)
            .child(
                div()
                    .id("actions-log-toggle")
                    .cursor_pointer()
                    .text_color(self.theme.accent)
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(view) = &mut this.actions_view {
                            view.full_log = !view.full_log;
                            this.scroll_actions_log_to_slice();
                            cx.notify();
                        }
                    }))
                    .child(if full_log { "step log" } else { "full log" }),
            )
            .child(
                div()
                    .id("actions-log-copy")
                    .cursor_pointer()
                    .text_color(self.theme.accent)
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(all.clone()));
                    })
                    .child("copy"),
            )
            .into_any_element()
    }

    /// One virtualized log line: faint timestamp, tinted text.
    fn actions_log_line(&self, line: &str) -> Div {
        let (timestamp, text) = sourcefour_github::split_timestamp(line);
        let tint = match line_tint(text) {
            LineTint::Error => self.theme.red,
            LineTint::Warning => self.theme.orange,
            LineTint::Plain => self.theme.text_secondary,
        };
        div()
            .h(px(LOG_ROW_HEIGHT))
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(16.0))
            .font_family(MONO_FONT)
            .text_size(px(10.5))
            .whitespace_nowrap()
            .children(timestamp.map(|seconds| {
                let clock = seconds.rem_euclid(86_400);
                div()
                    .flex_none()
                    .text_color(self.theme.text_faint)
                    .child(format!(
                        "{:02}:{:02}:{:02}",
                        clock / 3600,
                        (clock % 3600) / 60,
                        clock % 60
                    ))
            }))
            .child(div().text_color(tint).child(text.to_owned()))
    }

    /// The wall-clock strip: every job as a bar on the run's time axis.
    fn actions_waterfall(&self, view: &ActionsView) -> Div {
        let jobs = view.jobs_ok();
        let run_start = view
            .run
            .started_at
            .or_else(|| jobs.iter().filter_map(|job| job.started_at).min());
        let now = self.now_seconds();
        let run_end = view
            .run
            .completed_at
            .or_else(|| jobs.iter().filter_map(|job| job.completed_at).max())
            // A live run measures to now, so bars grow as it works.
            .or(Some(now));
        let wall = duration_of(run_start, run_end).unwrap_or(0).max(1);
        div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .px(px(16.0))
            .py(px(9.0))
            .border_t_1()
            .border_color(self.theme.border)
            .child(
                div()
                    .text_size(px(10.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(self.theme.text_faint)
                    .child(format!(
                        "TIMELINE · WALL TIME {}",
                        fmt_duration(duration_of(run_start, run_end)).to_uppercase()
                    )),
            )
            .child(
                div()
                    .flex()
                    .gap(px(8.0))
                    .child(div().w(px(150.0)).flex_none().flex().flex_col().children(
                        jobs.iter().map(|job| {
                            div()
                                .h(px(16.0))
                                .flex()
                                .items_center()
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .text_size(px(10.0))
                                .text_color(self.theme.text_secondary)
                                .child(job.name.clone())
                        }),
                    ))
                    .child(
                        // The plot itself: a slightly darker ruled surface,
                        // so the time axis reads as its own area.
                        div()
                            .flex_1()
                            .relative()
                            .border_1()
                            .border_color(self.theme.border)
                            .bg(gpui::black().opacity(0.18))
                            .children([0.25_f32, 0.5, 0.75].map(|fraction| {
                                div()
                                    .absolute()
                                    .top_0()
                                    .bottom_0()
                                    .left(gpui::relative(fraction))
                                    .w(px(1.0))
                                    .bg(self.theme.border)
                            }))
                            .child(
                                div().flex().flex_col().py(px(2.0)).children(
                                    jobs.iter()
                                        .map(|job| self.waterfall_bar(job, run_start, wall, now)),
                                ),
                            ),
                    ),
            )
    }

    /// One job's bar on the shared wall-clock axis.
    fn waterfall_bar(&self, job: &WorkflowJob, run_start: Option<i64>, wall: i64, now: i64) -> Div {
        let offset = duration_of(run_start, job.started_at).unwrap_or(0);
        let length = elapsed_of(job.started_at, job.completed_at, now).unwrap_or(0);
        #[expect(clippy::cast_precision_loss, reason = "display fractions")]
        let (left, width) = (
            (offset as f32 / wall as f32).clamp(0.0, 0.98),
            (length as f32 / wall as f32).clamp(0.005, 1.0),
        );
        div().h(px(16.0)).relative().child(
            div()
                .absolute()
                .top(px(5.0))
                .left(gpui::relative(left))
                .w(gpui::relative(width))
                .h(px(6.0))
                .rounded(px(3.0))
                .bg(match job.status {
                    CheckStatus::Completed(CheckConclusion::Failure) => self.theme.red.opacity(0.7),
                    CheckStatus::Completed(CheckConclusion::Success) => {
                        self.theme.green.opacity(0.6)
                    }
                    CheckStatus::InProgress => self.theme.orange.opacity(0.7),
                    _ => self.theme.border_strong,
                }),
        )
    }
}

#[cfg(test)]
mod tests {
    use sourcefour_model::{CheckConclusion, CheckStatus, WorkflowJob, WorkflowStep};

    use super::{bar_fraction, default_job, default_step, fmt_duration};

    fn job(name: &str, status: CheckStatus, started: i64, completed: i64) -> WorkflowJob {
        WorkflowJob {
            id: 1,
            name: name.to_owned(),
            status,
            started_at: Some(started),
            completed_at: Some(completed),
            steps: Vec::new(),
            html_url: String::new(),
        }
    }

    #[test]
    fn the_failing_job_wins_then_running_then_longest() {
        let success = CheckStatus::Completed(CheckConclusion::Success);
        let failure = CheckStatus::Completed(CheckConclusion::Failure);

        assert_eq!(
            default_job(&[job("a", success, 0, 10), job("b", failure, 0, 5)]),
            1
        );
        assert_eq!(
            default_job(&[
                job("a", success, 0, 10),
                job("b", CheckStatus::InProgress, 0, 0)
            ]),
            1
        );
        assert_eq!(
            default_job(&[job("a", success, 0, 10), job("b", success, 0, 90)]),
            1,
            "all green opens the longest job"
        );
        assert_eq!(default_job(&[]), 0);
    }

    #[test]
    fn the_failing_step_wins_else_the_longest() {
        let step = |status, started, completed| WorkflowStep {
            name: String::new(),
            status,
            started_at: Some(started),
            completed_at: Some(completed),
        };
        let success = CheckStatus::Completed(CheckConclusion::Success);
        let failure = CheckStatus::Completed(CheckConclusion::Failure);
        let mut with_failure = job("j", failure, 0, 100);
        with_failure.steps = vec![step(success, 0, 50), step(failure, 50, 60)];
        let mut all_green = job("j", success, 0, 100);
        all_green.steps = vec![step(success, 0, 50), step(success, 50, 60)];

        assert_eq!(default_step(&with_failure), Some(1));
        assert_eq!(default_step(&all_green), Some(0), "longest step wins");
    }

    #[test]
    fn durations_format_like_the_mockup() {
        assert_eq!(fmt_duration(Some(3)), "3s");
        assert_eq!(fmt_duration(Some(118)), "1m 58s");
        assert_eq!(fmt_duration(Some(222)), "3m 42s");
        assert_eq!(fmt_duration(None), "—");
    }

    #[test]
    fn bars_always_show_and_never_overflow() {
        assert!((bar_fraction(62, 62) - 1.0).abs() < f32::EPSILON);
        assert!(bar_fraction(1, 1000) >= 0.02);
        assert!(bar_fraction(0, 0) >= 0.02);
    }
}
