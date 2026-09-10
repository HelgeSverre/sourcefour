//! The details pane (§6.10): commit message on the left, changed files on
//! the right, with the merge-parent comparison choices.

use crate::context_menu::{ContextMenuExt as _, PrimaryClickExt as _};
use gpui::{Div, FontWeight, IntoElement, div, prelude::*, px, uniform_list};
use sourcefour_model::{ChangedFile, DiffParent, RepoPath, WorkingTreeStatus, WorkingTreeSummary};

use crate::history::display_date;

use super::{SourcefourWindow, change_color, change_letter, counted};

/// Headers and files share a height so the entire list can be virtualized.
const FILE_ROW_HEIGHT: f32 = 22.0;

enum WorkingTreeRow<'a> {
    Header {
        staged: bool,
        files: &'a [ChangedFile],
    },
    File {
        staged: bool,
        file: &'a ChangedFile,
    },
}

/// Resolve a display index without assembling or copying the full file list.
fn working_tree_row(status: &WorkingTreeStatus, index: usize) -> Option<WorkingTreeRow<'_>> {
    if index == 0 {
        Some(WorkingTreeRow::Header {
            staged: true,
            files: &status.staged,
        })
    } else if index <= status.staged.len() {
        status
            .staged
            .get(index - 1)
            .map(|file| WorkingTreeRow::File { staged: true, file })
    } else if index == status.staged.len() + 1 {
        Some(WorkingTreeRow::Header {
            staged: false,
            files: &status.unstaged,
        })
    } else {
        status
            .unstaged
            .get(index - status.staged.len() - 2)
            .map(|file| WorkingTreeRow::File {
                staged: false,
                file,
            })
    }
}

/// Assembled text for the details header (§6.10).
pub(super) struct DetailLines {
    hash: String,
    subject: String,
    author: String,
    date: Option<String>,
    committer: Option<String>,
    /// Full identity and comparison choice per parent, commit order.
    parent_choices: Vec<(sourcefour_model::Oid, DiffParent)>,
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
                        (*parent, choice)
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
        parent_choices: &[(sourcefour_model::Oid, DiffParent)],
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
                    .child(format!("Parent  {}", parent_choices[0].0.abbreviated(7)))
                    .on_context_menu({
                        let parent = parent_choices[0].0;
                        cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                            this.show_commit_menu(parent, event.position, window, cx);
                        })
                    })
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
                                let parent = *hash;
                                let selected = comparing == choice;
                                div()
                                    .id(("parent-choice", index))
                                    .on_context_menu(cx.listener(
                                        move |this, event: &gpui::MouseDownEvent, window, cx| {
                                            this.show_commit_menu(
                                                parent,
                                                event.position,
                                                window,
                                                cx,
                                            );
                                        },
                                    ))
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
                                    .on_primary_click(cx.listener(move |this, _, _, cx| {
                                        this.set_compare_parent(choice, cx);
                                    }))
                                    .child(hash.abbreviated(7))
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
            .when_some(self.history.selected_commit(), |row, oid| {
                row.on_context_menu(cx.listener(
                    move |this, event: &gpui::MouseDownEvent, window, cx| {
                        this.show_commit_menu(oid, event.position, window, cx);
                    },
                ))
            })
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
            .on_primary_click(cx.listener(|this, _, _, cx| {
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
        let oid = self.history.selected_commit();
        div()
            .when_some(oid, |column, oid| {
                column.on_context_menu(cx.listener(
                    move |this, event: &gpui::MouseDownEvent, window, cx| {
                        this.show_commit_menu(oid, event.position, window, cx);
                    },
                ))
            })
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
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_hidden()
                    .pb(px(6.0))
                    .child(
                        uniform_list(
                            "details-files-scroll",
                            self.files.as_ref().map_or(0, |files| files.files.len()),
                            cx.processor(|this, visible: std::ops::Range<usize>, _, cx| {
                                let Some(files) = &this.files else {
                                    return Vec::new();
                                };
                                visible
                                    .filter_map(|index| {
                                        files
                                            .files
                                            .get(index)
                                            .map(|file| this.file_row(index, file, None, cx))
                                    })
                                    .collect()
                            }),
                        )
                        .track_scroll(self.details_files_scroll.clone())
                        .size_full(),
                    ),
            )
    }

    /// The details pane while the working tree is selected.
    fn working_tree_details(&self, cx: &mut gpui::Context<Self>) -> gpui::AnyElement {
        let summary = self.working_tree_status.as_ref().map_or(
            WorkingTreeSummary {
                staged: 0,
                unstaged: 0,
            },
            WorkingTreeStatus::summary,
        );
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
                            .flex()
                            .px(px(10.0))
                            .py(px(8.0))
                            .rounded(px(6.0))
                            .border_1()
                            .border_color(self.theme.border_strong)
                            .bg(self.theme.bg_list)
                            .overflow_hidden()
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
                            .flex_1()
                            .min_h(px(0.0))
                            .overflow_hidden()
                            .pb(px(6.0))
                            .child(self.working_tree_files_list(cx).size_full()),
                    ),
            )
            .into_any_element()
    }

    /// Only the visible headers and files allocate UI elements in a frame.
    fn working_tree_files_list(&self, cx: &mut gpui::Context<Self>) -> gpui::UniformList {
        let count = self
            .working_tree_status
            .as_ref()
            .map_or(2, |status| status.staged.len() + status.unstaged.len() + 2);
        uniform_list(
            "working-tree-files-scroll",
            count,
            cx.processor(|this, visible: std::ops::Range<usize>, _, cx| {
                let empty = WorkingTreeStatus::default();
                let status = this.working_tree_status.as_ref().unwrap_or(&empty);
                visible
                    .filter_map(|index| {
                        working_tree_row(status, index).map(|row| match row {
                            WorkingTreeRow::Header { staged, files } => this
                                .working_tree_header(staged, files, cx)
                                .into_any_element(),
                            WorkingTreeRow::File { staged, file } => this
                                .file_row(index, file, Some(staged), cx)
                                .into_any_element(),
                        })
                    })
                    .collect()
            }),
        )
        .track_scroll(self.working_tree_files_scroll.clone())
    }

    /// The commit affordance: enabled once something is staged and the
    /// summary has words; the label says which is missing otherwise.
    fn commit_button(&self, staged: usize, cx: &mut gpui::Context<Self>) -> Div {
        let summary_empty = self.commit_input.read(cx).text().trim().is_empty();
        let ready = staged > 0 && !summary_empty && !self.committing;
        let label = if self.committing {
            "Committing…"
        } else {
            "Commit"
        };
        let reason = match (staged, summary_empty) {
            (0, _) => Some("Nothing staged yet"),
            (_, true) => Some("Write a commit message"),
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
                            .on_primary_click(cx.listener(|this, _, _, cx| {
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

    /// Collect bulk-action paths only when clicked, never during rendering.
    fn working_tree_header(
        &self,
        staged: bool,
        files: &[ChangedFile],
        cx: &mut gpui::Context<Self>,
    ) -> Div {
        let label = if staged { "STAGED" } else { "UNSTAGED" };
        let count = files.len();
        div()
            .on_context_menu(
                cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                    let entries = this.section_entries(staged, cx);
                    this.show_menu(event.position, entries, window, cx);
                }),
            )
            .h(px(FILE_ROW_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .child(
                div()
                    .flex_1()
                    .text_size(px(10.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(self.theme.text_faint)
                    .child(format!("{label} · {}", counted(count, "file"))),
            )
            .when(
                files.iter().any(|file| stageable_path(file).is_some()),
                |this| {
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
                            .on_primary_click(cx.listener(move |this, _, _, cx| {
                                this.edit_index_section(staged, cx);
                            }))
                            .child(if staged { "unstage all" } else { "stage all" }),
                    )
                },
            )
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
        let context_file = file.clone();
        let target = self.file_target(staged);
        div()
            .id(("changed-file", index))
            .debug_selector(|| format!("changed-file-{index}"))
            .when_some(target, |row, target| {
                row.on_context_menu(cx.listener(
                    move |this, event: &gpui::MouseDownEvent, window, cx| {
                        let entries = this.file_entries(&context_file, target, true, cx);
                        this.show_menu(event.position, entries, window, cx);
                    },
                ))
            })
            .h(px(FILE_ROW_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.0))
            .text_size(px(11.0))
            .cursor_pointer()
            .hover(|style| style.bg(self.theme.bg_hover))
            .on_primary_click(cx.listener(move |this, _, window, cx| match staged {
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
                let path = stageable_path(file).cloned();
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
                        .on_primary_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.edit_index(vec![path.clone()], staged, cx);
                        }))
                        .child(if staged { "−" } else { "+" })
                }))
            })
    }
}

/// The path a stage or unstage should name; conflicted rows get none.
pub(super) fn stageable_path(file: &ChangedFile) -> Option<&RepoPath> {
    if file.status == sourcefour_model::ChangeKind::Unknown {
        return None;
    }
    file.new_path.as_ref().or(file.old_path.as_ref())
}

#[cfg(test)]
mod tests {
    use gpui::{
        AppContext, Entity, Render, ScrollStrategy, TestAppContext, VisualTestContext, prelude::*,
        px,
    };
    use sourcefour_model::{ChangeKind, ChangedFile, RepoPath, WorkingTreeStatus};

    use super::{FILE_ROW_HEIGHT, SourcefourWindow, WorkingTreeRow, working_tree_row};
    use crate::{app::WindowLaunch, demo::Scene, history::Selection, ui_state::UiState};

    fn files(count: usize) -> Vec<ChangedFile> {
        (0..count)
            .map(|index| ChangedFile {
                old_path: None,
                new_path: Some(RepoPath(format!("icons/file-{index:06}.svg").into_bytes())),
                status: ChangeKind::Added,
                additions: Some(1),
                deletions: Some(0),
                is_binary: false,
            })
            .collect()
    }

    #[test]
    fn virtual_rows_cover_both_sections_including_empty_ones() {
        for staged in [0, 1, 5] {
            for unstaged in [0, 1, 5] {
                let status = WorkingTreeStatus {
                    staged: files(staged),
                    unstaged: files(unstaged),
                };
                assert!(
                    matches!(working_tree_row(&status, 0), Some(WorkingTreeRow::Header { staged: true, files }) if files.len() == staged)
                );
                assert!(
                    matches!(working_tree_row(&status, staged + 1), Some(WorkingTreeRow::Header { staged: false, files }) if files.len() == unstaged)
                );
                for (index, expected) in status.staged.iter().enumerate() {
                    assert!(
                        matches!(working_tree_row(&status, index + 1), Some(WorkingTreeRow::File { staged: true, file }) if std::ptr::eq(file, expected))
                    );
                }
                for (index, expected) in status.unstaged.iter().enumerate() {
                    assert!(
                        matches!(working_tree_row(&status, staged + index + 2), Some(WorkingTreeRow::File { staged: false, file }) if std::ptr::eq(file, expected))
                    );
                }
                assert!(working_tree_row(&status, staged + unstaged + 2).is_none());
            }
        }
    }

    /// Exercise the shipping details pane in a real GPUI layout/paint pass.
    /// Rendering all these rows eagerly exhausted GPUI's element arena.
    struct DetailsFixture(Entity<SourcefourWindow>);

    impl Render for DetailsFixture {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            gpui::div()
                .size_full()
                .flex()
                .flex_col()
                .child(self.0.update(cx, |view, cx| view.details(cx)))
                .child(self.0.read(cx).menus.clone())
        }
    }

    fn fixture(
        cx: &mut TestAppContext,
        scene: Scene,
    ) -> (Entity<DetailsFixture>, &mut VisualTestContext) {
        cx.add_window_view(|window, cx| {
            DetailsFixture(cx.new(|cx| {
                SourcefourWindow::new(
                    WindowLaunch {
                        demo: true,
                        name: String::from("large-repository"),
                        path: String::new(),
                        location: None,
                        scene,
                    },
                    &UiState::default(),
                    window,
                    cx,
                )
            }))
        })
    }

    fn draw(fixture: &Entity<DetailsFixture>, cx: &mut VisualTestContext) {
        cx.draw(
            gpui::Point::default(),
            gpui::size(px(1200.0), px(800.0)),
            |_, _| fixture.clone().into_any_element(),
        );
    }

    #[gpui::test]
    fn a_context_menu_can_copy_the_last_of_a_hundred_thousand_files(cx: &mut TestAppContext) {
        let (fixture, cx) = fixture(cx, Scene::Commit);
        let view = cx.update(|_, cx| fixture.read(cx).0.clone());
        view.update(cx, |view, _| {
            view.working_tree_status = Some(WorkingTreeStatus {
                staged: files(50_000),
                unstaged: files(50_000),
            });
        });
        draw(&fixture, cx);
        let scroll = cx.update(|_, cx| view.read(cx).working_tree_files_scroll.clone());
        scroll.scroll_to_item(100_001, ScrollStrategy::Top);
        draw(&fixture, cx);
        let row = cx.debug_bounds("changed-file-100001").unwrap();
        let started = std::time::Instant::now();
        for _ in 0..5 {
            draw(&fixture, cx);
        }
        let closed = started.elapsed();
        cx.simulate_mouse_down(
            row.center(),
            gpui::MouseButton::Right,
            gpui::Modifiers::default(),
        );
        let started = std::time::Instant::now();
        for _ in 0..5 {
            draw(&fixture, cx);
        }
        let opened = started.elapsed();
        cx.simulate_keystrokes("down enter");
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().as_deref(),
            Some("icons/file-049999.svg")
        );
        assert!(!cx.update(|_, cx| view.read(cx).menus.read(cx).is_open()));
        eprintln!("100k file fixture, five GPUI draws: closed {closed:?}, open {opened:?}");
    }

    #[gpui::test]
    fn a_hundred_thousand_worktree_files_render_and_scroll_without_exhausting_the_arena(
        cx: &mut TestAppContext,
    ) {
        let (fixture, cx) = fixture(cx, Scene::Commit);
        let view = cx.update(|_, cx| fixture.read(cx).0.clone());
        view.update(cx, |view, _| {
            view.working_tree_status = Some(WorkingTreeStatus {
                staged: files(50_000),
                unstaged: files(50_000),
            });
        });
        draw(&fixture, cx);
        let scroll = cx.update(|_, cx| view.read(cx).working_tree_files_scroll.clone());
        assert_eq!(
            scroll
                .0
                .borrow()
                .last_item_size
                .expect("list was laid out")
                .contents
                .height,
            px(FILE_ROW_HEIGHT) * 100_002
        );
        assert!(scroll.0.borrow().base_handle.bounds().size.height < px(800.0));
        // Cross the staged/unstaged boundary, then reach the last file.
        for index in [50_000, 50_001, 100_001] {
            scroll.scroll_to_item(index, ScrollStrategy::Top);
            draw(&fixture, cx);
            assert!(scroll.0.borrow().base_handle.offset().y < px(-1000.0));
        }
        // A refresh can empty the list while scrolled to the very bottom.
        view.update(cx, |view, _| {
            view.working_tree_status = Some(WorkingTreeStatus::default());
        });
        draw(&fixture, cx);
        assert_eq!(scroll.0.borrow().base_handle.offset().y, px(0.0));
    }

    #[gpui::test]
    fn a_hundred_thousand_commit_files_render_and_scroll_without_exhausting_the_arena(
        cx: &mut TestAppContext,
    ) {
        let (fixture, cx) = fixture(cx, Scene::Overview);
        let view = cx.update(|_, cx| fixture.read(cx).0.clone());
        view.update(cx, |view, _| {
            assert!(matches!(view.history.selected, Some(Selection::Commit(_))));
            view.files.as_mut().expect("demo commit has files").files = files(100_000);
        });
        draw(&fixture, cx);
        let scroll = cx.update(|_, cx| view.read(cx).details_files_scroll.clone());
        assert_eq!(
            scroll
                .0
                .borrow()
                .last_item_size
                .expect("list was laid out")
                .contents
                .height,
            px(FILE_ROW_HEIGHT) * 100_000
        );
        scroll.scroll_to_item(99_999, ScrollStrategy::Top);
        draw(&fixture, cx);
        assert!(scroll.0.borrow().base_handle.offset().y < px(-1000.0));
        view.update(cx, |view, _| {
            view.files = None;
        });
        draw(&fixture, cx);
        assert_eq!(scroll.0.borrow().base_handle.offset().y, px(0.0));
    }
}
