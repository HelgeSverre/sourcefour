//! The GitHub read surfaces: connection checking, the cached pull/check/run
//! loads, and the elements that render them (§ settings, GitHub).

use gpui::{Div, FontWeight, div, prelude::*, px};

use crate::theme::Theme;

use super::SourcefourWindow;

/// A fetched GitHub payload and when it arrived. Data older than a minute
/// refetches on the next request, bounding API use far under the rate limit.
pub(super) struct Cached<T> {
    pub(super) value: T,
    pub(super) fetched_at: std::time::Instant,
}

impl<T> Cached<T> {
    fn now(value: T) -> Self {
        Self {
            value,
            fetched_at: std::time::Instant::now(),
        }
    }

    fn fresh(&self) -> bool {
        self.fetched_at.elapsed() < std::time::Duration::from_mins(1)
    }
}

/// A commit's check runs, or the words for why they could not load.
pub(super) type GithubChecks = Result<Vec<sourcefour_model::CheckRun>, String>;

/// The 0600 token file, next to the other per-user files.
pub(super) fn credentials_path() -> Option<std::path::PathBuf> {
    crate::ui_state::support_file("credentials.json")
}

/// The glyph and color a check status renders as.
pub(super) fn check_glyph(
    theme: &Theme,
    status: sourcefour_model::CheckStatus,
) -> (&'static str, gpui::Hsla) {
    use sourcefour_model::{CheckConclusion, CheckStatus};
    match status {
        CheckStatus::Queued => ("○", theme.text_faint),
        CheckStatus::InProgress => ("●", theme.orange),
        CheckStatus::Completed(CheckConclusion::Success) => ("✓", theme.green),
        CheckStatus::Completed(CheckConclusion::Failure | CheckConclusion::TimedOut) => {
            ("✗", theme.red)
        }
        CheckStatus::Completed(CheckConclusion::ActionRequired) => ("!", theme.orange),
        CheckStatus::Completed(
            CheckConclusion::Neutral
            | CheckConclusion::Cancelled
            | CheckConclusion::Skipped
            | CheckConclusion::Unknown,
        ) => ("−", theme.text_faint),
    }
}

/// The token the configured auth method yields right now, or the words to
/// show for why it cannot.
pub(super) fn resolve_github_token(
    method: crate::settings::AuthMethod,
    credentials: Option<&std::path::Path>,
) -> Result<String, String> {
    use crate::settings::AuthMethod;
    match method {
        AuthMethod::Off => Err(String::from("GitHub authentication is off.")),
        AuthMethod::Token => credentials
            .and_then(|path| sourcefour_github::load_token(path, sourcefour_github::GITHUB_HOST))
            .ok_or_else(|| String::from("No token is stored.")),
        AuthMethod::GhCli => sourcefour_github::gh_cli_token(),
    }
}

impl SourcefourWindow {
    /// One Actions run: status glyph, workflow and number, branch; clicking
    /// opens the run on github.com.
    pub(super) fn workflow_run_row(
        &self,
        index: usize,
        run: &sourcefour_model::WorkflowRun,
        cx: &mut gpui::Context<Self>,
    ) -> gpui::Stateful<Div> {
        let (glyph, color) = check_glyph(&self.theme, run.status);
        let clicked = run.clone();
        let run_id = run.id;
        let subtitle = format!(
            "{} #{} · {} · {}",
            run.name,
            run.run_number,
            run.branch,
            run.started_at.map_or_else(String::new, |started| {
                crate::history::relative_date(
                    self.now_seconds(),
                    sourcefour_model::GitTime {
                        seconds_since_epoch: started,
                        offset_minutes: 0,
                    },
                )
            }),
        );
        div()
            .id(("workflow-run", index))
            .h(px(38.0))
            .flex()
            .flex_col()
            .justify_center()
            .gap(px(1.0))
            .px(px(15.0))
            .cursor_pointer()
            .hover(|style| style.bg(self.theme.bg_hover))
            // The press warms the jobs fetch; the click that follows opens
            // the overlay onto data already in flight.
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.prefetch_actions_jobs(run_id, cx);
                }),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.open_actions_run(clicked.clone(), window, cx);
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(7.0))
                    .child(
                        div()
                            .w(px(12.0))
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
                            .text_size(px(11.5))
                            .text_color(self.theme.text_primary)
                            .child(run.display_title.clone()),
                    ),
            )
            .child(
                div()
                    .pl(px(19.0))
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_size(px(9.5))
                    .text_color(self.theme.text_faint)
                    .child(subtitle),
            )
    }

    /// The selected commit's checks (§ GitHub): rendered only once an answer
    /// for exactly this commit exists, so the block never flickers.
    pub(super) fn checks_block(&self, cx: &mut gpui::Context<Self>) -> Option<Div> {
        let selected = self.history.selected_commit()?;
        let (cached, outcome) = &self.github_checks.as_ref()?.value;
        if *cached != selected {
            return None;
        }
        let body: Vec<Div> = match outcome {
            Ok(runs) if runs.is_empty() => return None,
            Ok(runs) => runs
                .iter()
                .enumerate()
                .map(|(index, run)| {
                    let (glyph, color) = check_glyph(&self.theme, run.status);
                    let url = run.html_url.clone();
                    div().child(
                        div()
                            .id(("check-run", index))
                            .h(px(20.0))
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .cursor_pointer()
                            .hover(|style| style.bg(self.theme.bg_hover))
                            .on_click(move |_, _, cx| {
                                cx.stop_propagation();
                                cx.open_url(&url);
                            })
                            .child(
                                div()
                                    .w(px(12.0))
                                    .flex_none()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(color)
                                    .child(glyph),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(self.theme.text_secondary)
                                    .child(run.name.clone()),
                            ),
                    )
                })
                .collect(),
            Err(message) => vec![
                div()
                    .text_size(px(10.5))
                    .text_color(self.theme.text_faint)
                    .child(message.clone()),
            ],
        };
        Some(
            div()
                .mt(px(8.0))
                .pt(px(8.0))
                .border_t_1()
                .border_color(self.theme.border)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .pb(px(4.0))
                        .child(
                            div()
                                .text_size(px(10.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(self.theme.text_faint)
                                .child("CHECKS"),
                        )
                        .child(
                            div()
                                .id("checks-refresh")
                                .px(px(4.0))
                                .rounded(px(3.0))
                                .cursor_pointer()
                                .text_size(px(10.0))
                                .text_color(self.theme.text_faint)
                                .hover(|style| style.bg(self.theme.bg_hover))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.load_selected_checks(true, cx);
                                }))
                                .child("↻"),
                        ),
                )
                .children(body),
        )
    }

    /// Stores the pasted token (if any) and verifies the connection with
    /// `GET /user` on the background executor (§ settings, GitHub).
    pub(crate) fn connect_github(&mut self, cx: &mut gpui::Context<Self>) {
        use crate::settings::AuthMethod;
        use crate::settings_ui::GithubConnection;

        let pasted = self.token_input.read(cx).content.trim().to_string();
        if self.settings.github.auth_method == AuthMethod::Token {
            let Some(path) = credentials_path() else {
                return;
            };
            if !pasted.is_empty()
                && let Err(error) =
                    sourcefour_github::store_token(&path, sourcefour_github::GITHUB_HOST, &pasted)
            {
                self.github_connection = GithubConnection::Failed {
                    message: format!("The token could not be stored: {error}"),
                };
                cx.notify();
                return;
            }
            if pasted.is_empty()
                && sourcefour_github::load_token(&path, sourcefour_github::GITHUB_HOST).is_none()
            {
                self.github_connection = GithubConnection::Failed {
                    message: String::from("Paste a personal access token first."),
                };
                cx.notify();
                return;
            }
        }

        self.github_connection = GithubConnection::Checking;
        cx.notify();
        self.fetch_github(
            |this| &mut this.github_request,
            |token| sourcefour_github::whoami(&sourcefour_github::UreqTransport, &token),
            |this, outcome, cx| {
                this.github_connection = match outcome {
                    Ok(account) => GithubConnection::Connected {
                        login: account.login,
                    },
                    Err(message) => GithubConnection::Failed { message },
                };
                cx.notify();
            },
            cx,
        );
    }

    /// Runs one GitHub read on the background executor: bumps the surface's
    /// request counter, resolves the token there, and drops stale results.
    /// Every read surface shares this skeleton so staleness has exactly one
    /// implementation.
    pub(super) fn fetch_github<T: Send + 'static>(
        &mut self,
        counter: fn(&mut Self) -> &mut u64,
        call: impl FnOnce(String) -> Result<T, String> + Send + 'static,
        apply: impl FnOnce(&mut Self, Result<T, String>, &mut gpui::Context<Self>) + 'static,
        cx: &mut gpui::Context<Self>,
    ) {
        let method = self.settings.github.auth_method;
        let credentials = credentials_path();
        *counter(self) += 1;
        let request = *counter(self);
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_executor()
                .spawn(async move {
                    let token = resolve_github_token(method, credentials.as_deref())?;
                    call(token)
                })
                .await;
            this.update(cx, |this, cx| {
                if *counter(this) != request {
                    return;
                }
                apply(this, outcome, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Re-resolves which GitHub repository the remotes point at and refreshes
    /// the read surfaces; origin wins when several remotes are GitHub.
    pub(super) fn refresh_github(&mut self, cx: &mut gpui::Context<Self>) {
        self.github_remote = None;
        if !self.settings.github.enabled {
            self.github_pulls = None;
            self.github_checks = None;
            self.github_runs = None;
            self.github_states = None;
            cx.notify();
            return;
        }
        let Some(snapshot) = self.snapshot() else {
            return;
        };
        self.github_remote = snapshot
            .remotes
            .iter()
            .filter(|remote| remote.name == "origin")
            .chain(snapshot.remotes.iter())
            .find_map(|remote| {
                remote
                    .fetch_url
                    .as_deref()
                    .and_then(sourcefour_github::GithubRemote::parse)
            });
        self.load_pulls(false, cx);
        self.load_selected_checks(false, cx);
        self.load_runs(false, cx);
        self.load_commit_states(false, cx);
    }

    /// Fetches the rolled-up CI state of the first loaded commits in one
    /// GraphQL request, unless the cache is still fresh. Called again as
    /// batches arrive; the cache keeps that cheap.
    pub(super) fn load_commit_states(&mut self, force: bool, cx: &mut gpui::Context<Self>) {
        let Some(remote) = self.github_remote.clone() else {
            return;
        };
        if !force && self.github_states.as_ref().is_some_and(Cached::fresh) {
            return;
        }
        let oids: Vec<sourcefour_model::Oid> = self
            .history
            .rows
            .iter()
            .take(100)
            .map(|row| row.oid)
            .collect();
        if oids.is_empty() {
            return;
        }
        self.fetch_github(
            |this| &mut this.github_states_request,
            move |token| {
                sourcefour_github::commit_states(
                    &sourcefour_github::UreqTransport,
                    &remote,
                    &token,
                    &oids,
                )
            },
            |this, outcome, cx| match outcome {
                Ok(states) => {
                    this.github_states = Some(Cached::now(states));
                    cx.notify();
                }
                // Stale dots beat rows that flicker on every hiccup.
                Err(message) => tracing::warn!(message, "commit states could not load"),
            },
            cx,
        );
    }

    /// The small CI dot a history row wears once its rolled-up state is
    /// known; commits without checks wear nothing.
    pub(super) fn commit_state_dot(&self, oid: sourcefour_model::Oid) -> Option<Div> {
        let status = *self.github_states.as_ref()?.value.get(&oid)?;
        let (_, color) = check_glyph(&self.theme, status);
        Some(div().flex_none().size(px(6.0)).rounded_full().bg(color))
    }

    /// Fetches recent Actions workflow runs unless the cache is still fresh.
    pub(super) fn load_runs(&mut self, force: bool, cx: &mut gpui::Context<Self>) {
        let Some(remote) = self.github_remote.clone() else {
            return;
        };
        if !force && self.github_runs.as_ref().is_some_and(Cached::fresh) {
            return;
        }
        self.fetch_github(
            |this| &mut this.github_runs_request,
            move |token| {
                sourcefour_github::workflow_runs(
                    &sourcefour_github::UreqTransport,
                    &remote,
                    &token,
                    20,
                )
            },
            |this, outcome, cx| match outcome {
                Ok(runs) => {
                    this.github_runs = Some(Cached::now(runs));
                    cx.notify();
                }
                // A stale list beats a section that flickers empty.
                Err(message) => tracing::warn!(message, "workflow runs could not load"),
            },
            cx,
        );
    }

    /// Fetches the open pull requests unless the cache is still fresh.
    pub(super) fn load_pulls(&mut self, force: bool, cx: &mut gpui::Context<Self>) {
        let Some(remote) = self.github_remote.clone() else {
            return;
        };
        if !force && self.github_pulls.as_ref().is_some_and(Cached::fresh) {
            return;
        }
        self.fetch_github(
            |this| &mut this.github_pulls_request,
            move |token| {
                sourcefour_github::open_pulls(&sourcefour_github::UreqTransport, &remote, &token)
            },
            |this, outcome, cx| match outcome {
                Ok(pulls) => {
                    this.github_pulls = Some(Cached::now(pulls));
                    cx.notify();
                }
                // Stale chips beat a sidebar that flickers on every
                // network hiccup; the message lands in the log.
                Err(message) => tracing::warn!(message, "pull requests could not load"),
            },
            cx,
        );
    }

    /// Fetches the selected commit's check runs unless the cache is fresh.
    pub(super) fn load_selected_checks(&mut self, force: bool, cx: &mut gpui::Context<Self>) {
        let Some(remote) = self.github_remote.clone() else {
            return;
        };
        let Some(oid) = self.history.selected_commit() else {
            self.github_checks = None;
            return;
        };
        if !force
            && self
                .github_checks
                .as_ref()
                .is_some_and(|cache| cache.value.0 == oid && cache.fresh())
        {
            return;
        }
        self.fetch_github(
            |this| &mut this.github_checks_request,
            move |token| {
                sourcefour_github::check_runs(
                    &sourcefour_github::UreqTransport,
                    &remote,
                    &token,
                    &oid.to_hex(),
                )
            },
            move |this, outcome, cx| {
                // Failures render in the block, so they cache like results.
                this.github_checks = Some(Cached::now((oid, outcome)));
                cx.notify();
            },
            cx,
        );
    }

    /// Deletes the stored token and forgets the connection.
    pub(crate) fn disconnect_github(&mut self, cx: &mut gpui::Context<Self>) {
        if let Some(path) = credentials_path() {
            sourcefour_github::delete_token(&path, sourcefour_github::GITHUB_HOST).ok();
        }
        self.token_input
            .update(cx, |input, cx| input.set_text("", cx));
        self.github_request += 1;
        self.github_connection = crate::settings_ui::GithubConnection::Idle;
        cx.notify();
    }
    /// The clickable `#number` chip a branch row wears when an open pull
    /// request heads that branch.
    pub(super) fn pr_chip(&self, branch: &str) -> Option<gpui::Stateful<Div>> {
        let pull = self
            .github_pulls
            .as_ref()
            .and_then(|cache| cache.value.iter().find(|pull| pull.head_branch == branch))?;
        let color = if pull.draft {
            self.theme.text_faint
        } else {
            self.theme.green
        };
        let url = pull.html_url.clone();
        Some(
            div()
                .id(("pr-chip", pull.number))
                .flex_none()
                .px(px(4.0))
                .rounded(px(3.0))
                .border_1()
                .border_color(self.theme.chip_border(color))
                .text_size(px(9.0))
                .text_color(color)
                .cursor_pointer()
                .hover(|style| style.bg(self.theme.bg_hover))
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    cx.open_url(&url);
                })
                .child(format!("#{}", pull.number)),
        )
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn check_glyphs_read_at_a_glance() {
        use sourcefour_model::{CheckConclusion, CheckStatus};

        let theme = crate::theme::Theme::dark();
        let glyph = |status| super::check_glyph(&theme, status).0;

        assert_eq!(glyph(CheckStatus::Completed(CheckConclusion::Success)), "✓");
        assert_eq!(glyph(CheckStatus::Completed(CheckConclusion::Failure)), "✗");
        assert_eq!(glyph(CheckStatus::InProgress), "●");
        assert_eq!(glyph(CheckStatus::Queued), "○");
        assert_eq!(glyph(CheckStatus::Completed(CheckConclusion::Skipped)), "−");
    }
}
