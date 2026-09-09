//! Surface adapters for the shared context-menu host.
//!
//! Build entries from cached state on demand. Mutable commands revalidate the
//! repository and target before entering the same controllers as buttons.

use std::path::{Path, PathBuf};

use gpui::{Context, Pixels, Point, Window};
use sourcefour_model::{ChangedFile, DiffParent, Oid, RefKind, RepoPath};

use super::{SourcefourWindow, details::stageable_path};
use crate::context_menu::MenuEntry;

#[derive(Clone, Copy)]
pub(super) enum FileTarget {
    Commit { oid: Oid, parent: DiffParent },
    WorkingTree { staged: bool },
}

pub(super) fn path_entries(path: &Path, accessible: bool) -> Vec<MenuEntry> {
    let path = path.to_path_buf();
    vec![
        path.to_str().map_or_else(
            || MenuEntry::disabled("Copy path"),
            |path| MenuEntry::copy("Copy path", path.to_owned()),
        ),
        if accessible {
            MenuEntry::new("Reveal in file manager", move |_, cx| cx.reveal_path(&path))
        } else {
            MenuEntry::disabled("Reveal in file manager")
        },
    ]
}

/// Conversion for native operations must not use the lossy display label.
#[cfg_attr(
    unix,
    expect(
        clippy::unnecessary_wraps,
        reason = "non-Unix platforms cannot represent every Git byte path"
    )
)]
fn native_path(path: &RepoPath) -> Option<PathBuf> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt as _;
        Some(PathBuf::from(std::ffi::OsStr::from_bytes(&path.0)))
    }
    #[cfg(not(unix))]
    std::str::from_utf8(&path.0).ok().map(PathBuf::from)
}

pub(super) fn copy_repo_path(label: &'static str, path: &RepoPath) -> MenuEntry {
    std::str::from_utf8(&path.0).map_or_else(
        |_| MenuEntry::disabled(label),
        |path| MenuEntry::copy(label, path.to_owned()),
    )
}

impl SourcefourWindow {
    pub(super) fn dismiss_menu(&self, cx: &mut Context<Self>) {
        self.menus
            .update(cx, crate::context_menu::MenuHost::invalidate);
    }

    pub(super) fn keyboard_commit_menu(&self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.focus.is_focused(window) {
            return;
        }
        let Some(oid) = self.history.selected_commit() else {
            return;
        };
        let Some(index) = self.history.selected_display_index() else {
            return;
        };
        let scroll = self.list_scroll.0.borrow();
        let bounds = scroll.base_handle.bounds();
        let y = bounds.top()
            + scroll.base_handle.offset().y
            + gpui::px(super::row_count_as_f32(index) * self.settings.history.row_height());
        // A selection scrolled out of view has no visible anchor.
        if y < bounds.top() || y >= bounds.bottom() {
            return;
        }
        let position = gpui::point(bounds.left() + gpui::px(self.panels.graph), y);
        drop(scroll);
        self.show_commit_menu(oid, position, window, cx);
    }
    pub(crate) fn show_menu(
        &self,
        position: Point<Pixels>,
        entries: Vec<MenuEntry>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.menus.update(cx, |menus, cx| {
            menus.open(position, entries, None, window, cx);
        });
    }

    pub(super) fn show_commit_menu(
        &self,
        oid: Oid,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let entries = self.commit_entries(oid, cx);
        self.menus.update(cx, |menus, cx| {
            menus.open(position, entries, Some(oid.to_hex().into()), window, cx);
        });
    }

    pub(super) fn commit_entries(&self, oid: Oid, cx: &Context<Self>) -> Vec<MenuEntry> {
        let mut entries = vec![MenuEntry::new("Copy SHA", move |_, cx| {
            crate::context_menu::copy_text(oid.to_hex(), cx);
        })];
        if let Some(row) = self.history.rows.iter().find(|row| row.oid == oid) {
            entries.push(MenuEntry::copy("Copy subject", row.summary.clone()));
        }
        if self.detail.as_ref().is_some_and(|detail| detail.oid == oid) {
            entries.push(MenuEntry::command(
                "Copy commit message",
                cx,
                move |this, _, cx| {
                    if let Some(detail) = this.detail.as_ref().filter(|detail| detail.oid == oid) {
                        let message = if detail.body.is_empty() {
                            detail.subject.clone()
                        } else {
                            format!("{}\n\n{}", detail.subject, detail.body)
                        };
                        crate::context_menu::copy_text(message, cx);
                    }
                },
            ));
        }
        if self.location.is_some() {
            entries.push(MenuEntry::Separator);
            let session = self.session;
            entries.push(MenuEntry::command(
                "Create branch here…",
                cx,
                move |this, window, cx| {
                    if this.session == session {
                        this.open_branch_at(Some(oid), window, cx);
                    }
                },
            ));
        }
        entries
    }

    pub(super) fn ref_entries(
        &self,
        name: &str,
        full_name: Option<&str>,
        kind: RefKind,
        oid: Oid,
        cx: &Context<Self>,
    ) -> Vec<MenuEntry> {
        let mut entries = vec![MenuEntry::copy(
            match kind {
                RefKind::Tag => "Copy tag name",
                RefKind::LocalBranch | RefKind::RemoteBranch => "Copy branch name",
                _ => "Copy name",
            },
            name.to_owned(),
        )];
        if let Some(full_name) = full_name {
            entries.push(MenuEntry::copy("Copy full ref", full_name.to_owned()));
        }
        entries.push(MenuEntry::new("Copy commit SHA", move |_, cx| {
            crate::context_menu::copy_text(oid.to_hex(), cx);
        }));
        if self.location.is_some() {
            let session = self.session;
            entries.push(MenuEntry::Separator);
            entries.push(MenuEntry::command(
                "Create branch here…",
                cx,
                move |this, window, cx| {
                    if this.session == session {
                        this.open_branch_at(Some(oid), window, cx);
                    }
                },
            ));
        }
        entries
    }

    pub(super) fn remote_entries(&self, name: &str, cx: &Context<Self>) -> Vec<MenuEntry> {
        let mut entries = vec![MenuEntry::copy("Copy remote name", name.to_owned())];
        if let Some(url) = self
            .snapshot()
            .and_then(|snapshot| snapshot.remotes.iter().find(|remote| remote.name == name))
            .and_then(|remote| remote.fetch_url.as_ref())
        {
            entries.push(MenuEntry::copy("Copy remote URL", url.clone()));
        }
        let name = name.to_owned();
        let session = self.session;
        entries.push(MenuEntry::Separator);
        entries.push(
            if self.location.is_some() && self.network_operation.running_op().is_none() {
                MenuEntry::command("Fetch from this remote", cx, move |this, _, cx| {
                    if this.session == session
                        && this.snapshot().is_some_and(|snapshot| {
                            snapshot.remotes.iter().any(|remote| remote.name == name)
                        })
                    {
                        this.fetch_remote(name.clone(), cx);
                    }
                })
            } else {
                MenuEntry::disabled("Fetch from this remote")
            },
        );
        entries
    }

    pub(super) fn file_target(&self, staged: Option<bool>) -> Option<FileTarget> {
        match staged {
            Some(staged) => Some(FileTarget::WorkingTree { staged }),
            None => self.files_for.map(|oid| FileTarget::Commit {
                oid,
                parent: self.compare_parent,
            }),
        }
    }

    pub(super) fn file_entries(
        &self,
        file: &ChangedFile,
        target: FileTarget,
        open: bool,
        cx: &Context<Self>,
    ) -> Vec<MenuEntry> {
        let mut entries = Vec::new();
        let session = self.session;
        if open {
            let file = file.clone();
            entries.push(MenuEntry::command(
                "Open diff",
                cx,
                move |this, window, cx| {
                    if this.session != session {
                        return;
                    }
                    match target {
                        FileTarget::Commit { oid, parent } => {
                            this.open_commit_diff(&file, oid, parent, window, cx);
                        }
                        FileTarget::WorkingTree { staged } => {
                            this.open_worktree_diff(&file, staged, window, cx);
                        }
                    }
                },
            ));
            entries.push(MenuEntry::Separator);
        }
        if let Some(path) = file.new_path.as_ref().or(file.old_path.as_ref()) {
            entries.push(copy_repo_path("Copy relative path", path));
            let absolute = self
                .location
                .as_ref()
                .and_then(|location| location.active_worktree_path.as_ref())
                .zip(native_path(path))
                .map(|(base, path)| base.join(path));
            entries.push(
                absolute
                    .as_ref()
                    .and_then(|path| path.to_str())
                    .map_or_else(
                        || MenuEntry::disabled("Copy absolute path"),
                        |path| MenuEntry::copy("Copy absolute path", path.to_owned()),
                    ),
            );
            entries.push(match absolute.filter(|_| file.new_path.is_some()) {
                Some(path) => {
                    MenuEntry::new("Reveal working copy", move |_, cx| cx.reveal_path(&path))
                }
                None => MenuEntry::disabled("Reveal working copy"),
            });
        }
        if let (Some(old), Some(new)) = (&file.old_path, &file.new_path)
            && old != new
        {
            entries.push(copy_repo_path("Copy old relative path", old));
        }
        if let FileTarget::WorkingTree { staged } = target {
            let label = if staged { "Unstage file" } else { "Stage file" };
            entries.push(MenuEntry::Separator);
            entries.push(if let Some(path) = stageable_path(file) {
                let path = path.clone();
                MenuEntry::command(label, cx, move |this, _, cx| {
                    let present = this.working_tree_status.as_ref().is_some_and(|status| {
                        (if staged {
                            &status.staged
                        } else {
                            &status.unstaged
                        })
                        .iter()
                        .any(|file| stageable_path(file) == Some(&path))
                    });
                    if this.session == session && present {
                        this.edit_index(vec![path.clone()], staged, cx);
                    }
                })
            } else {
                MenuEntry::disabled(label)
            });
        }
        entries
    }

    pub(super) fn edit_index_section(&mut self, staged: bool, cx: &mut Context<Self>) {
        let Some(status) = &self.working_tree_status else {
            return;
        };
        let files = if staged {
            &status.staged
        } else {
            &status.unstaged
        };
        let paths = files.iter().filter_map(stageable_path).cloned().collect();
        self.edit_index(paths, staged, cx);
    }

    pub(super) fn section_entries(&self, staged: bool, cx: &Context<Self>) -> Vec<MenuEntry> {
        let label = if staged { "Unstage all" } else { "Stage all" };
        let enabled = self.working_tree_status.as_ref().is_some_and(|status| {
            (if staged {
                &status.staged
            } else {
                &status.unstaged
            })
            .iter()
            .any(|file| stageable_path(file).is_some())
        });
        let session = self.session;
        vec![if enabled {
            MenuEntry::command(label, cx, move |this, _, cx| {
                if this.session == session {
                    this.edit_index_section(staged, cx);
                }
            })
        } else {
            MenuEntry::disabled(label)
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{IntoElement as _, Modifiers, MouseButton, TestAppContext, point, px, size};

    #[gpui::test]
    fn picker_input_menu_pastes_without_submitting_the_repository(cx: &mut TestAppContext) {
        cx.update(|cx| {
            cx.bind_keys(crate::app::repo_picker_keymap());
            cx.bind_keys(crate::text_input::keymap());
            crate::context_menu::copy_text("/tmp/pasted-repository".into(), cx);
        });
        let failure = sourcefour_model::RepoFailure::new(
            sourcefour_model::RepoFailureKind::NotARepository,
            "No repository",
            "Pick a repository.",
        );
        let recovered = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (view, cx) = cx.add_window_view(|window, cx| {
            super::super::ErrorWindow::new(&failure, recovered.clone(), window, cx)
        });
        cx.draw(Point::default(), size(px(500.0), px(300.0)), |_, _| {
            view.clone().into_any_element()
        });
        cx.simulate_keystrokes("shift-f10");
        cx.draw(Point::default(), size(px(500.0), px(300.0)), |_, _| {
            view.clone().into_any_element()
        });
        cx.simulate_keystrokes("enter");
        cx.update(|window, cx| {
            let view = view.read(cx);
            assert_eq!(view.path_input.read(cx).text(), "/tmp/pasted-repository");
            assert!(view.path_input.read(cx).focus_handle.is_focused(window));
            assert!(!view.menus.read(cx).is_open());
        });
        assert!(!recovered.load(std::sync::atomic::Ordering::SeqCst));
    }

    fn draw(view: &gpui::Entity<SourcefourWindow>, cx: &mut gpui::VisualTestContext) {
        cx.draw(Point::default(), size(px(1200.0), px(800.0)), |_, _| {
            view.clone().into_any_element()
        });
    }

    #[gpui::test]
    fn commit_menu_copies_the_clicked_full_oid_without_selection_or_reads(cx: &mut TestAppContext) {
        let (view, cx) = super::super::test_window(cx, crate::demo::Scene::Overview);
        for sha in ["ab".repeat(20), "cd".repeat(32)] {
            let target = Oid::from_hex(&sha).unwrap();
            let (selected, requests) = view.update(cx, |view, cx| {
                view.history.rows[1].oid = target;
                cx.notify();
                (
                    view.history.selected,
                    (
                        view.files_request,
                        view.diff_request,
                        view.status_request,
                        view.github_request,
                    ),
                )
            });
            draw(&view, cx);
            let row = cx.debug_bounds("commit-row-1").unwrap();
            cx.simulate_mouse_down(
                point(row.right() - px(12.0), row.center().y),
                MouseButton::Right,
                Modifiers::default(),
            );
            draw(&view, cx);
            cx.simulate_keystrokes("j k pagedown pageup space down up enter");
            assert_eq!(
                cx.read_from_clipboard().unwrap().text().as_deref(),
                Some(sha.as_str())
            );
            cx.update(|_, cx| {
                let view = view.read(cx);
                assert_eq!(view.history.selected, selected);
                assert_eq!(
                    (
                        view.files_request,
                        view.diff_request,
                        view.status_request,
                        view.github_request
                    ),
                    requests
                );
                assert!(!view.menus.read(cx).is_open());
            });
        }
    }

    #[gpui::test]
    fn keyboard_commit_menu_closes_on_filter_and_keeps_nested_ref_target(cx: &mut TestAppContext) {
        let (view, cx) = super::super::test_window(cx, crate::demo::Scene::Overview);
        draw(&view, cx);
        cx.simulate_keystrokes("shift-f10");
        draw(&view, cx);
        assert!(cx.update(|_, cx| view.read(cx).menus.read(cx).is_open()));
        let filter = cx.update(|_, cx| view.read(cx).filter_input.clone());
        filter.update(cx, |input, cx| input.set_text("no match", cx));
        assert!(!cx.update(|_, cx| view.read(cx).menus.read(cx).is_open()));
        filter.update(cx, |input, cx| input.set_text("", cx));
        draw(&view, cx);
        let chip = cx.debug_bounds("ref-chip-main").unwrap();
        cx.simulate_mouse_down(chip.center(), MouseButton::Right, Modifiers::default());
        draw(&view, cx);
        cx.simulate_keystrokes("enter");
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().as_deref(),
            Some("main")
        );
    }

    #[gpui::test]
    fn file_menu_keeps_rename_paths_and_excludes_conflicted_staging(cx: &mut TestAppContext) {
        let (view, cx) = super::super::test_window(cx, crate::demo::Scene::Overview);
        let file = ChangedFile {
            old_path: Some(RepoPath(b"old:name.svg".to_vec())),
            new_path: Some(RepoPath(b"new name.svg".to_vec())),
            status: sourcefour_model::ChangeKind::Renamed,
            additions: None,
            deletions: None,
            is_binary: false,
        };
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                let entries =
                    view.file_entries(&file, FileTarget::WorkingTree { staged: false }, false, cx);
                let MenuEntry::Action {
                    invoke: Some(copy), ..
                } = &entries[0]
                else {
                    panic!("relative path is copyable");
                };
                copy(window, cx);
                assert_eq!(
                    cx.read_from_clipboard().unwrap().text().as_deref(),
                    Some("new name.svg")
                );
                let MenuEntry::Action {
                    invoke: Some(copy), ..
                } = &entries[3]
                else {
                    panic!("old path is copyable");
                };
                copy(window, cx);
                assert_eq!(
                    cx.read_from_clipboard().unwrap().text().as_deref(),
                    Some("old:name.svg")
                );
                let mut conflict = file.clone();
                conflict.status = sourcefour_model::ChangeKind::Unknown;
                let entries = view.file_entries(
                    &conflict,
                    FileTarget::WorkingTree { staged: false },
                    false,
                    cx,
                );
                assert!(!entries.last().unwrap().enabled());
            });
        });
    }
}
