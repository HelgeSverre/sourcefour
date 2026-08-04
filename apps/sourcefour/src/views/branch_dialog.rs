//! The §6.13 create-branch dialog: state, overlay, and submission.

use gpui::{Div, FontWeight, IntoElement, Window, div, prelude::*, px};
use sourcefour_model::{HeadSnapshot, OperationOutcome};

use crate::theme::MONO_FONT;

use super::SourcefourWindow;

/// The §6.13 create-branch dialog's state; the name lives in its input.
pub(super) struct BranchDialog {
    checkout: bool,
    running: bool,
    error: Option<String>,
}

impl SourcefourWindow {
    /// Opens the §6.13 create-branch dialog seeded from the selection.
    pub(super) fn open_branch_dialog(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        self.branch_dialog = Some(BranchDialog {
            checkout: false,
            running: false,
            error: None,
        });
        self.branch_input
            .update(cx, |input, cx| input.set_text("", cx));
        self.branch_input
            .read(cx)
            .focus_handle
            .clone()
            .focus(window);
        cx.notify();
    }

    /// The commit a new branch starts from: the selection, or HEAD.
    pub(super) fn branch_start(&self) -> Option<sourcefour_model::Oid> {
        self.history.selected.or_else(|| {
            self.snapshot().and_then(|snapshot| match &snapshot.head {
                HeadSnapshot::Branch { oid, .. } | HeadSnapshot::Detached { oid } => Some(*oid),
                HeadSnapshot::Unborn { .. } | HeadSnapshot::Missing => None,
            })
        })
    }

    /// Runs the §6.13 creation; Git failures reopen as dialog errors.
    pub(super) fn submit_branch_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let name = self.branch_input.read(cx).content.to_string();
        let (Some(start), Some(location)) = (self.branch_start(), self.location.clone()) else {
            return;
        };
        let Some(dialog) = &mut self.branch_dialog else {
            return;
        };
        if dialog.running || !sourcefour_git::is_valid_branch_name(&name) {
            return;
        }
        let checkout = dialog.checkout;
        dialog.running = true;
        dialog.error = None;
        let request = sourcefour_model::CreateBranchRequest {
            worktree: self.active_worktree_id(),
            name,
            start,
            checkout,
        };
        self.focus.focus(window);
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_executor()
                .spawn(async move { sourcefour_git::create_branch(&location, &request) })
                .await;
            this.update(cx, |this, cx| {
                match outcome {
                    Ok(OperationOutcome::Succeeded { summary, .. }) => {
                        this.branch_dialog = None;
                        this.op_status = Some((true, summary));
                        this.begin_reload(cx);
                    }
                    Ok(OperationOutcome::Failed { error, .. }) | Err(error) => {
                        if let Some(dialog) = &mut this.branch_dialog {
                            dialog.running = false;
                            dialog.error = Some(error.user.message);
                        }
                    }
                    Ok(OperationOutcome::Cancelled { .. }) => {
                        this.branch_dialog = None;
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Closes the create-branch dialog, returning focus to the history.
    pub(crate) fn close_branch_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        self.branch_dialog = None;
        self.focus.focus(window);
        cx.notify();
    }

    /// The §6.13 create-branch dialog: name, start point, checkout box.
    pub(super) fn branch_overlay(
        &self,
        cx: &mut gpui::Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let dialog = self.branch_dialog.as_ref()?;
        let name = self.branch_input.read(cx).content.to_string();
        let creatable = !dialog.running && sourcefour_git::is_valid_branch_name(&name);
        let invalid = !name.is_empty() && !sourcefour_git::is_valid_branch_name(&name);
        let start = self
            .branch_start()
            .map_or_else(|| String::from("HEAD"), |oid| oid.abbreviated(9));
        Some(
            div()
                .id("branch-overlay")
                .occlude()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(self.theme.scrim())
                .on_click(cx.listener(|this, _, window, cx| {
                    this.close_branch_dialog(window, cx);
                }))
                .child(
                    div()
                        .id("branch-panel")
                        .w(px(400.0))
                        .flex()
                        .flex_col()
                        .gap(px(10.0))
                        .p(px(16.0))
                        .rounded(px(10.0))
                        .border_1()
                        .border_color(self.theme.border_strong)
                        .bg(self.theme.bg_panel)
                        .shadow_lg()
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .text_size(px(13.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(self.theme.text_primary)
                                .child("Create branch")
                                .child(
                                    div()
                                        .font_family(MONO_FONT)
                                        .font_weight(FontWeight::NORMAL)
                                        .text_size(px(10.5))
                                        .text_color(self.theme.text_faint)
                                        .child(format!("from {start}")),
                                ),
                        )
                        .child(
                            div()
                                .h(px(30.0))
                                .flex()
                                .items_center()
                                .px(px(10.0))
                                .rounded(px(6.0))
                                .border_1()
                                .border_color(if invalid {
                                    self.theme.red
                                } else {
                                    self.theme.accent
                                })
                                .bg(self.theme.bg_list)
                                .text_size(px(12.0))
                                .text_color(self.theme.text_primary)
                                .child(self.branch_input.clone()),
                        )
                        .child(self.branch_checkout_row(dialog.checkout, cx))
                        .children(dialog.error.clone().map(|error| {
                            div()
                                .text_size(px(11.0))
                                .text_color(self.theme.red)
                                .child(error)
                        }))
                        .child(self.branch_dialog_buttons(creatable, dialog.running, cx)),
                ),
        )
    }

    /// The dialog's checkout toggle row.
    pub(super) fn branch_checkout_row(
        &self,
        checked: bool,
        cx: &mut gpui::Context<Self>,
    ) -> gpui::Stateful<Div> {
        div()
            .id("branch-checkout")
            .flex()
            .items_center()
            .gap(px(7.0))
            .cursor_pointer()
            .text_size(px(11.5))
            .text_color(self.theme.text_secondary)
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(dialog) = &mut this.branch_dialog {
                    dialog.checkout = !dialog.checkout;
                }
                cx.notify();
            }))
            .child(
                div()
                    .size(px(13.0))
                    .rounded(px(3.0))
                    .border_1()
                    .border_color(self.theme.border_strong)
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(checked, |this| {
                        this.bg(self.theme.accent)
                            .text_color(gpui::white())
                            .text_size(px(9.0))
                            .child("✓")
                    }),
            )
            .child("Check out after creating")
    }

    /// The dialog's Cancel and Create buttons.
    pub(super) fn branch_dialog_buttons(
        &self,
        creatable: bool,
        running: bool,
        cx: &mut gpui::Context<Self>,
    ) -> Div {
        div()
            .flex()
            .justify_end()
            .gap(px(8.0))
            .child(
                div()
                    .id("branch-cancel")
                    .px(px(12.0))
                    .py(px(5.0))
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(self.theme.border_strong)
                    .cursor_pointer()
                    .text_size(px(11.5))
                    .text_color(self.theme.text_secondary)
                    .hover(|style| style.bg(self.theme.bg_hover))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.close_branch_dialog(window, cx);
                    }))
                    .child("Cancel"),
            )
            .child(
                div()
                    .id("branch-create")
                    .px(px(12.0))
                    .py(px(5.0))
                    .rounded(px(6.0))
                    .text_size(px(11.5))
                    .when(creatable, |this| {
                        this.cursor_pointer()
                            .bg(self.theme.accent)
                            .text_color(gpui::white())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.submit_branch_dialog(window, cx);
                            }))
                    })
                    .when(!creatable, |this| {
                        this.bg(self.theme.bg_hover)
                            .text_color(self.theme.text_faint)
                    })
                    .child(if running { "Creating…" } else { "Create" }),
            )
    }
}
