//! The details pane (§6.10): commit message on the left, changed files on
//! the right, with the merge-parent comparison choices.

use gpui::{Div, FontWeight, IntoElement, div, prelude::*, px};
use sourcefour_model::{ChangedFile, DiffParent};

use crate::history::display_date;

use super::{SourcefourWindow, change_color, change_letter, counted};

/// Assembled text for the details header (§6.10).
pub(super) struct DetailLines {
    hash: String,
    subject: String,
    author: String,
    date: Option<String>,
    committer: Option<String>,
    /// Abbreviated hash and comparison choice per parent, commit order.
    parent_choices: Vec<(String, DiffParent)>,
    body: Option<String>,
}

impl SourcefourWindow {
    /// The details header text, from the exact metadata when it has arrived
    /// and from the already-loaded row until then (§6.10).
    pub(super) fn detail_lines(&self) -> DetailLines {
        let now = self.now_seconds();
        let dates = self.settings.appearance.date_display;
        let row = self
            .history
            .selected_index()
            .and_then(|index| self.history.rows.get(index));
        let detail = self.detail.as_ref();
        DetailLines {
            hash: row.map_or_else(String::new, |row| row.oid.abbreviated(9)),
            subject: detail.map_or_else(
                || {
                    row.map_or_else(
                        || String::from("No commit selected"),
                        |row| row.summary.clone(),
                    )
                },
                |detail| detail.subject.clone(),
            ),
            author: detail.map_or_else(
                || row.map_or_else(String::new, |row| row.author_name.clone()),
                |detail| format!("{} <{}>", detail.author.name, detail.author.email),
            ),
            date: detail
                .map(|detail| detail.author.time)
                .or_else(|| row.map(|row| row.commit_time))
                .map(|time| display_date(dates, now, time)),
            committer: detail
                .filter(|detail| {
                    detail.committer.name != detail.author.name
                        || detail.committer.email != detail.author.email
                })
                .map(|detail| {
                    format!(
                        "committed by {} {}",
                        detail.committer.name,
                        display_date(dates, now, detail.committer.time)
                    )
                }),
            parent_choices: detail.map_or_else(Vec::new, |detail| {
                detail
                    .parents
                    .iter()
                    .enumerate()
                    .map(|(index, parent)| {
                        let choice = if index == 0 {
                            DiffParent::FirstParent
                        } else {
                            DiffParent::Parent(*parent)
                        };
                        (parent.abbreviated(7), choice)
                    })
                    .collect()
            }),
            body: detail
                .map(|detail| detail.body.clone())
                .filter(|body| !body.is_empty()),
        }
    }

    /// The details pane's first row: hash, and parent hashes — clickable
    /// comparison choices when the commit is a merge (§6.10).
    pub(super) fn details_hash_row(
        &self,
        hash: &str,
        parent_choices: &[(String, DiffParent)],
        cx: &mut gpui::Context<Self>,
    ) -> Div {
        let comparing = self.compare_parent;
        let hash = hash.to_owned();

        div()
            .flex()
            .items_center()
            .gap(px(10.0))
            .text_size(px(12.0))
            .font_family(self.mono_font())
            .text_color(self.theme.accent)
            .child(hash)
            .children((parent_choices.len() == 1).then(|| {
                div()
                    .text_size(px(10.5))
                    .text_color(self.theme.text_faint)
                    .child(format!("Parent  {}", parent_choices[0].0))
            }))
            .children((parent_choices.len() > 1).then(|| {
                // A merge: pick which parent to compare against.
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .text_size(px(10.5))
                    .text_color(self.theme.text_faint)
                    .child("vs")
                    .children(
                        parent_choices
                            .iter()
                            .enumerate()
                            .map(|(index, (hash, choice))| {
                                let choice = *choice;
                                let selected = comparing == choice;
                                div()
                                    .id(("parent-choice", index))
                                    .px(px(6.0))
                                    .rounded(px(4.0))
                                    .border_1()
                                    .cursor_pointer()
                                    .border_color(if selected {
                                        self.theme.accent
                                    } else {
                                        self.theme.border_strong
                                    })
                                    .text_color(if selected {
                                        self.theme.accent
                                    } else {
                                        self.theme.text_secondary
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.set_compare_parent(choice, cx);
                                    }))
                                    .child(hash.clone())
                            }),
                    )
            }))
    }

    /// §4.6: the collapsed details strip; Space or a click expands it.
    pub(super) fn collapsed_details(
        &self,
        hash: &str,
        subject: &str,
        cx: &mut gpui::Context<Self>,
    ) -> gpui::AnyElement {
        let hash = hash.to_owned();
        let subject = subject.to_owned();
        div()
            .id("details-collapsed")
            .h(px(30.0))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(10.0))
            .px(px(14.0))
            .border_t_1()
            .border_color(self.theme.border)
            .bg(self.theme.bg_panel)
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| {
                this.details_collapsed = false;
                this.persist_ui_state(cx);
                cx.notify();
            }))
            .child(
                div()
                    .flex_none()
                    .font_family(self.mono_font())
                    .text_size(px(11.0))
                    .text_color(self.theme.accent)
                    .child(hash),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(1.0))
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_size(px(11.5))
                    .text_color(self.theme.text_secondary)
                    .child(subject),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(10.0))
                    .text_color(self.theme.text_faint)
                    .child("Space to expand"),
            )
            .into_any_element()
    }

    pub(super) fn details(&self, cx: &mut gpui::Context<Self>) -> gpui::AnyElement {
        if !self.details_collapsed
            && self.history.selected == Some(crate::history::Selection::WorkingTree)
        {
            return self.working_tree_details(cx);
        }
        let lines = self.detail_lines();
        if self.details_collapsed {
            return self.collapsed_details(&lines.hash, &lines.subject, cx);
        }
        div()
            .h(px(self.panels.details))
            .flex_none()
            .flex()
            .bg(self.theme.bg_panel)
            .child(self.details_message_column(lines, cx))
            .child(div().w(px(1.0)).flex_none().bg(self.theme.border))
            .child(self.details_files_column(cx))
            .into_any_element()
    }

    /// The left details column: hash row, subject, author line, and the
    /// scrollable commit body.
    pub(super) fn details_message_column(
        &self,
        lines: DetailLines,
        cx: &mut gpui::Context<Self>,
    ) -> Div {
        let DetailLines {
            hash,
            subject,
            author,
            date,
            committer,
            parent_choices,
            body,
        } = lines;
        div()
            .flex_1()
            .min_w(px(1.0))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .px(px(14.0))
            .pt(px(10.0))
            .child(self.details_hash_row(&hash, &parent_choices, cx))
            .child(
                div()
                    .text_size(px(13.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(self.theme.text_primary)
                    .child(subject),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .text_size(px(11.5))
                    .text_color(self.theme.text_secondary)
                    .child(author)
                    .children(date.map(|date| div().text_color(self.theme.text_faint).child(date)))
                    .children(
                        committer.map(|committer| {
                            div().text_color(self.theme.text_faint).child(committer)
                        }),
                    ),
            )
            .child(
                div()
                    .id("details-scroll")
                    .track_focus(&self.details_focus)
                    .flex_grow()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .children(body.map(|body| {
                        div()
                            .pt(px(4.0))
                            .pb(px(4.0))
                            .text_size(px(11.5))
                            .text_color(self.theme.text_secondary)
                            .child(body)
                    }))
                    .children(self.checks_block(cx)),
            )
    }

    /// The right details column: the changed-files header and scrollable list.
    pub(super) fn details_files_column(&self, cx: &mut gpui::Context<Self>) -> Div {
        div()
            .flex_1()
            .min_w(px(1.0))
            .flex()
            .flex_col()
            .px(px(14.0))
            .pt(px(10.0))
            .child(
                div()
                    .pb(px(6.0))
                    .text_size(px(10.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(self.theme.text_faint)
                    .child(self.files.as_ref().map_or_else(
                        || String::from("CHANGED FILES"),
                        |files| format!("CHANGED FILES · {}", counted(files.files.len(), "file")),
                    )),
            )
            .child(
                div()
                    .id("details-files-scroll")
                    .flex_grow()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .pb(px(6.0))
                    .children(
                        self.files
                            .clone()
                            .iter()
                            .flat_map(|files| files.files.clone())
                            .enumerate()
                            .map(|(index, file)| self.file_row(index, &file, None, cx)),
                    ),
            )
    }

    /// The details pane while the working tree is selected: what is staged,
    /// what is not. Read-only until stage/commit land.
    fn working_tree_details(&self, cx: &mut gpui::Context<Self>) -> gpui::AnyElement {
        let status = self.working_tree_status.clone().unwrap_or_default();
        let summary = status.summary();
        div()
            .h(px(self.panels.details))
            .flex_none()
            .flex()
            .bg(self.theme.bg_panel)
            .child(
                div()
                    .flex_1()
                    .min_w(px(1.0))
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .px(px(14.0))
                    .pt(px(10.0))
                    .child(
                        div()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(self.theme.text_primary)
                            .child("Uncommitted changes"),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(self.theme.text_secondary)
                            .child(format!(
                                "{} · {}",
                                counted(summary.staged, "staged file"),
                                counted(summary.unstaged, "unstaged file")
                            )),
                    )
                    .child(
                        div()
                            .mt(px(6.0))
                            .h(px(30.0))
                            .flex()
                            .items_center()
                            .px(px(10.0))
                            .rounded(px(6.0))
                            .border_1()
                            .border_color(self.theme.border_strong)
                            .bg(self.theme.bg_list)
                            .text_size(px(12.0))
                            .text_color(self.theme.text_primary)
                            .child(self.commit_input.clone()),
                    )
                    .child(self.commit_button(summary.staged, cx)),
            )
            .child(div().w(px(1.0)).flex_none().bg(self.theme.border))
            .child(
                div()
                    .flex_1()
                    .min_w(px(1.0))
                    .flex()
                    .flex_col()
                    .px(px(14.0))
                    .pt(px(10.0))
                    .child(
                        div()
                            .id("working-tree-files-scroll")
                            .flex_grow()
                            .min_h(px(0.0))
                            .overflow_y_scroll()
                            .pb(px(6.0))
                            .children(self.working_tree_section(
                                "STAGED",
                                true,
                                summary.staged,
                                &status.staged,
                                0,
                                cx,
                            ))
                            .children(self.working_tree_section(
                                "UNSTAGED",
                                false,
                                summary.unstaged,
                                &status.unstaged,
                                status.staged.len(),
                                cx,
                            )),
                    ),
            )
            .into_any_element()
    }

    /// The commit affordance: enabled once something is staged and the
    /// summary has words; the label says which is missing otherwise.
    fn commit_button(&self, staged: usize, cx: &mut gpui::Context<Self>) -> Div {
        let summary_empty = self.commit_input.read(cx).content.trim().is_empty();
        let ready = staged > 0 && !summary_empty && !self.committing;
        let label = if self.committing {
            "Committing…"
        } else {
            "Commit"
        };
        let reason = match (staged, summary_empty) {
            (0, _) => Some("Nothing staged yet"),
            (_, true) => Some("Write a summary line"),
            _ => None,
        };
        div()
            .flex()
            .items_center()
            .gap(px(10.0))
            .child(
                div()
                    .id("commit-button")
                    .px(px(14.0))
                    .py(px(5.0))
                    .rounded(px(6.0))
                    .text_size(px(11.5))
                    .when(ready, |this| {
                        this.cursor_pointer()
                            .bg(self.theme.accent)
                            .text_color(self.theme.text_on_accent)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.start_commit(cx);
                            }))
                    })
                    .when(!ready, |this| {
                        this.bg(self.theme.bg_hover)
                            .text_color(self.theme.text_faint)
                    })
                    .child(label),
            )
            .children(reason.map(|reason| {
                div()
                    .text_size(px(10.5))
                    .text_color(self.theme.text_faint)
                    .child(reason)
            }))
    }

    /// One section of the working-tree file list: header plus rows.
    fn working_tree_section(
        &self,
        label: &'static str,
        staged: bool,
        count: usize,
        files: &[ChangedFile],
        id_offset: usize,
        cx: &mut gpui::Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let all: Vec<sourcefour_model::RepoPath> =
            files.iter().filter_map(stageable_path).collect();
        let mut rows = vec![
            div()
                .flex()
                .items_center()
                .pb(px(6.0))
                .pt(px(4.0))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(10.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(self.theme.text_faint)
                        .child(format!("{label} · {}", counted(count, "file"))),
                )
                .when(!all.is_empty(), |this| {
                    this.child(
                        div()
                            .id(("stage-section", usize::from(staged)))
                            .flex_none()
                            .px(px(5.0))
                            .rounded(px(4.0))
                            .text_size(px(10.0))
                            .text_color(self.theme.text_faint)
                            .cursor_pointer()
                            .hover(|style| {
                                style
                                    .bg(self.theme.bg_hover)
                                    .text_color(self.theme.text_primary)
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.edit_index(all.clone(), staged, cx);
                            }))
                            .child(if staged { "unstage all" } else { "stage all" }),
                    )
                })
                .into_any_element(),
        ];
        rows.extend(files.iter().enumerate().map(|(index, file)| {
            self.file_row(id_offset + index, file, Some(staged), cx)
                .into_any_element()
        }));
        rows
    }

    /// Switches the comparison parent and reloads files for the selection.
    pub(super) fn set_compare_parent(&mut self, parent: DiffParent, cx: &mut gpui::Context<Self>) {
        if self.compare_parent == parent {
            return;
        }
        self.compare_parent = parent;
        self.files_for = None;
        self.load_selected_files(cx);
        cx.notify();
    }

    /// One changed file: status letter, path, and line counts when known.
    pub(super) fn file_row(
        &self,
        index: usize,
        file: &ChangedFile,
        staged: Option<bool>,
        cx: &mut gpui::Context<Self>,
    ) -> gpui::Stateful<Div> {
        let color = change_color(&self.theme, file.status);
        let path = file
            .new_path
            .as_ref()
            .or(file.old_path.as_ref())
            .map_or_else(String::new, sourcefour_model::RepoPath::display_lossy);
        let clicked = file.clone();
        div()
            .id(("changed-file", index))
            .h(px(22.0))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.0))
            .text_size(px(11.0))
            .cursor_pointer()
            .hover(|style| style.bg(self.theme.bg_hover))
            .on_click(cx.listener(move |this, _, window, cx| match staged {
                Some(staged) => this.open_worktree_diff(&clicked, staged, window, cx),
                None => this.open_diff(&clicked, window, cx),
            }))
            .child(
                div()
                    .w(px(12.0))
                    .flex_none()
                    .font_weight(FontWeight::BOLD)
                    .text_color(color)
                    .child(change_letter(file.status)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(1.0))
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_color(self.theme.text_secondary)
                    .child(path),
            )
            .children(file.additions.map(|added| {
                div()
                    .text_color(self.theme.green)
                    .child(format!("+{added}"))
            }))
            .children(file.deletions.map(|removed| {
                div()
                    .text_color(self.theme.red)
                    .child(format!("-{removed}"))
            }))
            .when_some(staged, |this, staged| {
                // Conflicted rows only warn: staging one would mark it
                // resolved, which is the merge milestone's call to offer.
                let path = stageable_path(file);
                this.children(path.map(|path| {
                    div()
                        .id(("stage-toggle", index))
                        .flex_none()
                        .px(px(5.0))
                        .rounded(px(4.0))
                        .text_size(px(12.0))
                        .text_color(self.theme.text_faint)
                        .cursor_pointer()
                        .hover(|style| {
                            style
                                .bg(self.theme.bg_hover)
                                .text_color(self.theme.text_primary)
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.edit_index(vec![path.clone()], staged, cx);
                        }))
                        .child(if staged { "−" } else { "+" })
                }))
            })
    }
}

/// The path a stage or unstage should name; conflicted rows get none.
fn stageable_path(file: &ChangedFile) -> Option<sourcefour_model::RepoPath> {
    if file.status == sourcefour_model::ChangeKind::Unknown {
        return None;
    }
    file.new_path.clone().or_else(|| file.old_path.clone())
}
