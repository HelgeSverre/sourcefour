//! Linked-worktree creation dialog and path suggestions.

use std::path::{Path, PathBuf};

use gpui::{FontWeight, IntoElement, Window, div, prelude::*, px};
use sourcefour_model::{AddWorktreeRequest, OperationOutcome, WorktreeSource};

use crate::settings::WorktreeLocation;

use super::SourcefourWindow;

#[derive(Clone)]
pub(super) enum WorktreeDialogSource {
    Local {
        full_name: String,
        short_name: String,
    },
    Remote {
        full_name: String,
        short_name: String,
    },
}

pub(super) struct WorktreeDialog {
    source: WorktreeDialogSource,
    running: bool,
    error: Option<String>,
}

fn safe_component(branch: &str) -> String {
    let mut result = String::new();
    let mut separator = false;
    for character in branch.chars() {
        if character.is_alphanumeric() || matches!(character, '.' | '_') {
            result.push(character);
            separator = false;
        } else if !separator && !result.is_empty() {
            result.push('-');
            separator = true;
        }
    }
    let result = result.trim_matches('-');
    if result.is_empty() {
        String::from("worktree")
    } else {
        result.to_owned()
    }
}

fn unused_path(base: PathBuf) -> PathBuf {
    if !base.exists() {
        return base;
    }
    let parent = base.parent().unwrap_or_else(|| Path::new("."));
    let name = base.file_name().map_or_else(
        || String::from("worktree"),
        |name| name.to_string_lossy().into_owned(),
    );
    (2..=u32::MAX)
        .map(|number| parent.join(format!("{name}-{number}")))
        .find(|path| !path.exists())
        .unwrap_or(base)
}

impl SourcefourWindow {
    fn suggested_worktree_path(&self, branch: &str) -> Option<PathBuf> {
        let snapshot = self.snapshot()?;
        let main = snapshot.worktrees.iter().find(|tree| tree.is_main)?;
        let branch = safe_component(branch);
        let suggested = match self.settings.git.worktree_location {
            WorktreeLocation::Sibling => {
                let parent = main.path.parent()?;
                parent.join(format!("{}-{branch}", self.name))
            }
            WorktreeLocation::InsideRepository => main.path.join(".worktrees").join(branch),
        };
        Some(unused_path(suggested))
    }

    pub(super) fn open_worktree_dialog(
        &mut self,
        mut source: WorktreeDialogSource,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let local_name = match &source {
            WorktreeDialogSource::Local { short_name, .. } => Some(short_name.clone()),
            WorktreeDialogSource::Remote { short_name, .. } => short_name
                .split_once('/')
                .map(|(_, branch)| branch.to_owned()),
        };
        if let Some(local) = local_name.as_deref().and_then(|name| {
            self.snapshot()?
                .local_branches
                .iter()
                .find(|branch| branch.short_name == name)
                .cloned()
        }) {
            let matches_source = match &source {
                WorktreeDialogSource::Local { .. } => true,
                WorktreeDialogSource::Remote { full_name, .. } => local
                    .upstream
                    .as_ref()
                    .is_some_and(|upstream| upstream.full_name == *full_name),
            };
            if matches_source {
                if let Some(tree) = local.checked_out_in.and_then(|id| {
                    self.snapshot()?
                        .worktrees
                        .iter()
                        .find(|tree| tree.id == id)
                        .cloned()
                }) {
                    self.activate_worktree_path(tree.path, cx);
                    return;
                }
                source = WorktreeDialogSource::Local {
                    full_name: local.full_name,
                    short_name: local.short_name,
                };
            }
        }
        let local_name = match &source {
            WorktreeDialogSource::Local { short_name, .. } => short_name.clone(),
            WorktreeDialogSource::Remote { short_name, .. } => short_name
                .split_once('/')
                .map_or(short_name.as_str(), |(_, branch)| branch)
                .to_owned(),
        };
        let Some(path) = self.suggested_worktree_path(&local_name) else {
            return;
        };
        self.worktree_dialog = Some(WorktreeDialog {
            source: source.clone(),
            running: false,
            error: None,
        });
        self.branch_input.update(cx, |input, cx| {
            if matches!(source, WorktreeDialogSource::Remote { .. }) {
                input.set_text(&local_name, cx);
            }
        });
        self.worktree_path_input
            .update(cx, |input, cx| input.set_text(&path.to_string_lossy(), cx));
        self.worktree_path_input
            .read(cx)
            .focus_handle
            .clone()
            .focus(window);
        cx.notify();
    }

    pub(super) fn close_worktree_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        self.worktree_dialog = None;
        self.focus.focus(window);
        cx.notify();
    }

    pub(super) fn submit_worktree_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(location) = self.location.clone() else {
            return;
        };
        let entered_text = self.worktree_path_input.read(cx).text().to_owned();
        if entered_text.trim().is_empty() {
            return;
        }
        let entered_path = PathBuf::from(entered_text);
        let path = if entered_path.is_absolute() {
            entered_path
        } else {
            location
                .active_worktree_path
                .as_deref()
                .unwrap_or(&location.common_dir)
                .join(entered_path)
        };
        let local_name = self.branch_input.read(cx).text().to_owned();
        let worktree = self.active_worktree_id();
        let internal = self.settings.git.worktree_location == WorktreeLocation::InsideRepository;
        let Some(dialog) = &mut self.worktree_dialog else {
            return;
        };
        if dialog.running || path.as_os_str().is_empty() {
            return;
        }
        let source = match &dialog.source {
            WorktreeDialogSource::Local { full_name, .. } => WorktreeSource::LocalBranch {
                full_name: full_name.clone(),
            },
            WorktreeDialogSource::Remote { full_name, .. } => {
                if !sourcefour_git::is_valid_branch_name(&local_name) {
                    return;
                }
                WorktreeSource::RemoteBranch {
                    full_name: full_name.clone(),
                    local_name,
                }
            }
        };
        dialog.running = true;
        dialog.error = None;
        self.focus.focus(window);
        let request = AddWorktreeRequest {
            worktree,
            source,
            path: path.clone(),
        };
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_executor()
                .spawn(async move {
                    if internal {
                        sourcefour_git::ensure_internal_worktrees_excluded(&location).map_err(
                            |error| {
                                sourcefour_model::RepoFailure::new(
                                    sourcefour_model::RepoFailureKind::PermissionDenied,
                                    "Worktree directory could not be excluded",
                                    "The repository-local exclude file could not be updated.",
                                )
                                .with_details(error.to_string())
                            },
                        )?;
                    }
                    sourcefour_git::add_worktree(&location, &request)
                })
                .await;
            this.update(cx, |this, cx| {
                match outcome {
                    Ok(OperationOutcome::Succeeded { summary, .. }) => {
                        this.worktree_dialog = None;
                        this.op_status = Some((true, summary));
                        this.activate_worktree_path(path, cx);
                    }
                    Ok(OperationOutcome::Failed { error, .. }) | Err(error) => {
                        if let Some(dialog) = &mut this.worktree_dialog {
                            dialog.running = false;
                            dialog.error = Some(error.user.message);
                        }
                    }
                    Ok(OperationOutcome::Cancelled { .. }) => this.worktree_dialog = None,
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(super) fn worktree_overlay(
        &self,
        cx: &mut gpui::Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let dialog = self.worktree_dialog.as_ref()?;
        let remote = matches!(dialog.source, WorktreeDialogSource::Remote { .. });
        let valid_branch =
            !remote || sourcefour_git::is_valid_branch_name(self.branch_input.read(cx).text());
        let creatable =
            !dialog.running && valid_branch && !self.worktree_path_input.read(cx).text().is_empty();
        Some(
            super::modal_backdrop("worktree-overlay", &self.theme)
                .items_center()
                .justify_center()
                .on_click(cx.listener(|this, _, window, cx| {
                    this.close_worktree_dialog(window, cx);
                }))
                .child(
                    super::modal_panel("worktree-panel", &self.theme)
                        .w(px(460.0))
                        .flex()
                        .flex_col()
                        .gap(px(10.0))
                        .p(px(16.0))
                        .child(
                            div()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(self.theme.text_primary)
                                .child("Create worktree"),
                        )
                        .when(remote, |panel| {
                            panel.child(
                                self.worktree_input_row("Local branch", self.branch_input.clone()),
                            )
                        })
                        .child(self.worktree_input_row("Path", self.worktree_path_input.clone()))
                        .children(dialog.error.clone().map(|error| {
                            div()
                                .text_size(px(11.0))
                                .text_color(self.theme.red)
                                .child(error)
                        }))
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap(px(8.0))
                                .child(self.worktree_dialog_button(
                                    "worktree-cancel",
                                    "Cancel",
                                    true,
                                    cx,
                                ))
                                .child(self.worktree_dialog_button(
                                    "worktree-create",
                                    if dialog.running {
                                        "Creating…"
                                    } else {
                                        "Create"
                                    },
                                    creatable,
                                    cx,
                                )),
                        ),
                ),
        )
    }

    fn worktree_input_row(
        &self,
        label: &'static str,
        input: gpui::Entity<crate::text_input::TextInput>,
    ) -> gpui::Div {
        div()
            .flex()
            .flex_col()
            .gap(px(5.0))
            .text_size(px(10.5))
            .text_color(self.theme.text_faint)
            .child(label)
            .child(
                div()
                    .h(px(30.0))
                    .px(px(10.0))
                    .flex()
                    .items_center()
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(self.theme.border_strong)
                    .bg(self.theme.bg_list)
                    .text_color(self.theme.text_primary)
                    .child(input),
            )
    }

    fn worktree_dialog_button(
        &self,
        id: &'static str,
        label: &'static str,
        enabled: bool,
        cx: &mut gpui::Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let create = id == "worktree-create";
        div()
            .id(id)
            .px(px(12.0))
            .py(px(5.0))
            .rounded(px(6.0))
            .text_size(px(11.5))
            .bg(if create && enabled {
                self.theme.accent
            } else {
                self.theme.bg_hover
            })
            .text_color(if create && enabled {
                self.theme.text_on_accent
            } else if enabled {
                self.theme.text_secondary
            } else {
                self.theme.text_faint
            })
            .when(enabled, |button| {
                button
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if create {
                            this.submit_worktree_dialog(window, cx);
                        } else {
                            this.close_worktree_dialog(window, cx);
                        }
                    }))
            })
            .child(label)
    }
}

#[cfg(test)]
mod tests {
    use super::safe_component;

    #[test]
    fn branch_names_become_single_safe_path_components() {
        assert_eq!(safe_component("feature/a thing"), "feature-a-thing");
        assert_eq!(safe_component("///fix///x///"), "fix-x");
        assert_eq!(safe_component("føø/✨"), "føø");
        assert_eq!(safe_component("✨"), "worktree");
    }
}
