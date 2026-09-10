//! The settings overlay: section navigation on the left, editable cards on
//! the right. The page shape follows Zed's settings editor — titled cards of
//! labeled controls — rendered with this app's own theme and widgets.
//!
//! Mutations route through `SourcefourWindow`'s `update_settings`, which
//! writes the file immediately; there is no separate save step.

use crate::context_menu::{ContextMenuExt as _, PrimaryClickExt as _};
use gpui::{Div, FontWeight, div, prelude::*, px, svg};

use crate::{
    settings::{AppSettings, AuthMethod, DateDisplay, Density},
    theme::Theme,
    views::SourcefourWindow,
};

/// Everything one frame of the overlay reads, borrowed from the window.
///
/// The window owns all of it and none of it is the overlay's to keep, so
/// this is a parameter list with names rather than a state object.
pub(crate) struct SettingsView<'a> {
    pub(crate) settings: &'a AppSettings,
    pub(crate) section: SettingsSection,
    pub(crate) connection: &'a GithubConnection,
    pub(crate) token_input: &'a gpui::Entity<crate::text_input::TextInput>,
    pub(crate) theme: &'a Theme,
    pub(crate) focus: &'a gpui::FocusHandle,
}

/// One page of the settings overlay.
///
/// A new section is a variant, a `title` arm and a `cards` arm — the three
/// are adjacent so none of them can be the one that gets forgotten.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum SettingsSection {
    #[default]
    General,
    GitHub,
    Diffs,
    About,
}

impl SettingsSection {
    const ALL: &'static [Self] = &[Self::General, Self::GitHub, Self::Diffs, Self::About];

    const fn title(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::GitHub => "GitHub",
            Self::Diffs => "Diffs",
            Self::About => "About",
        }
    }

    fn cards(self, view: &SettingsView<'_>, cx: &mut gpui::Context<SourcefourWindow>) -> Vec<Div> {
        match self {
            Self::General => general_cards(view, cx),
            Self::GitHub => github_cards(view, cx),
            Self::Diffs => diffs_cards(view, cx),
            Self::About => about_cards(view.theme, cx),
        }
    }
}

/// Where the GitHub connection stands, shown in the GitHub section.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) enum GithubConnection {
    /// Nothing checked yet.
    #[default]
    Idle,
    /// A `GET /user` is in flight.
    Checking,
    /// The credentials work; `login` is who they authenticate as.
    Connected { login: String },
    /// The last check failed, with the words to show.
    Failed { message: String },
}

/// The full-window settings overlay: backdrop, nav, and the active section.
pub(crate) fn overlay(
    view: &SettingsView<'_>,
    cx: &mut gpui::Context<SourcefourWindow>,
) -> gpui::Stateful<Div> {
    let theme = view.theme;
    crate::views::modal_backdrop("settings-overlay", theme)
        .key_context("Settings")
        .track_focus(view.focus)
        .items_center()
        .justify_center()
        .on_primary_click(cx.listener(|this, _, window, cx| {
            this.close_settings(window, cx);
        }))
        .child(
            div()
                .id("settings-panel")
                .w(px(700.0))
                .h(px(480.0))
                .flex()
                .rounded(px(10.0))
                .border_1()
                .border_color(theme.border_strong)
                .bg(theme.bg_panel)
                .shadow_lg()
                .overflow_hidden()
                .on_primary_click(|_, _, cx| cx.stop_propagation())
                .child(nav(view.section, theme, cx))
                .child(content(view, cx)),
        )
}

/// The left column: overlay title and one row per section.
fn nav(active: SettingsSection, theme: &Theme, cx: &mut gpui::Context<SourcefourWindow>) -> Div {
    div()
        .w(px(160.0))
        .flex_none()
        .h_full()
        .flex()
        .flex_col()
        .py(px(12.0))
        .bg(theme.bg_chrome)
        .border_r_1()
        .border_color(theme.border)
        .child(
            div()
                .px(px(14.0))
                .pb(px(10.0))
                .text_size(px(10.0))
                .font_weight(FontWeight::BOLD)
                .text_color(theme.text_faint)
                .child("SETTINGS"),
        )
        .children(SettingsSection::ALL.iter().copied().map(|section| {
            let selected = section == active;
            div()
                .id(section.title())
                .h(px(28.0))
                .px(px(14.0))
                .flex()
                .items_center()
                .text_size(px(12.0))
                .cursor_pointer()
                .bg(if selected {
                    theme.bg_selected
                } else {
                    theme.bg_chrome
                })
                .text_color(if selected {
                    theme.text_primary
                } else {
                    theme.text_secondary
                })
                .hover({
                    let hover = theme.bg_hover;
                    move |style| if selected { style } else { style.bg(hover) }
                })
                .on_primary_click(cx.listener(move |this, _, _, cx| {
                    this.set_settings_section(section, cx);
                }))
                .child(section.title())
        }))
}

/// The right column: section header with close, then that section's cards.
fn content(view: &SettingsView<'_>, cx: &mut gpui::Context<SourcefourWindow>) -> Div {
    let (section, theme) = (view.section, view.theme);
    div()
        .flex_1()
        .min_w(px(1.0))
        .h_full()
        .flex()
        .flex_col()
        .child(
            div()
                .h(px(40.0))
                .flex_none()
                .flex()
                .items_center()
                .px(px(16.0))
                .border_b_1()
                .border_color(theme.border)
                .child(
                    div()
                        .flex_1()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text_primary)
                        .child(section.title()),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(px(11.0))
                        .text_color(theme.text_faint)
                        .child("Esc"),
                )
                .child(
                    div()
                        .id("settings-close")
                        .flex_none()
                        .ml(px(8.0))
                        .px(px(8.0))
                        .rounded(px(4.0))
                        .cursor_pointer()
                        .text_size(px(13.0))
                        .text_color(theme.text_secondary)
                        .hover({
                            let hover = theme.bg_hover;
                            move |style| style.bg(hover)
                        })
                        .on_primary_click(cx.listener(|this, _, window, cx| {
                            this.close_settings(window, cx);
                        }))
                        .child("✕"),
                ),
        )
        .child(
            div()
                .id("settings-content")
                .flex_1()
                .min_h(px(1.0))
                .overflow_y_scroll()
                .p(px(16.0))
                .flex()
                .flex_col()
                .gap(px(14.0))
                .children(section.cards(view, cx)),
        )
}

/// The settings that belong to the window rather than to one feature of it.
fn general_cards(view: &SettingsView<'_>, cx: &mut gpui::Context<SourcefourWindow>) -> Vec<Div> {
    let theme = view.theme;
    vec![
        card(
            theme,
            vec![
                Choice {
                    id: "date-display",
                    name: "Dates",
                    description: "How old a commit is, or the clock it was written by.",
                    choices: &[
                        ("Relative", DateDisplay::Relative),
                        ("Exact", DateDisplay::Absolute),
                    ],
                    active: view.settings.appearance.date_display,
                    apply: |settings, display| settings.appearance.date_display = display,
                }
                .row(theme, cx),
                Choice {
                    id: "mono-font",
                    name: "Monospace font",
                    description: "Diffs, hashes and logs. Any installed family works from \
                                  settings.json; these are the ones worth a button.",
                    choices: MONO_PRESETS,
                    active: active_mono_font(view.settings),
                    apply: |settings, family| {
                        settings.appearance.mono_font = family.map(String::from);
                    },
                }
                .row(theme, cx),
                Choice {
                    id: "history-density",
                    name: "History density",
                    description: "How much room each commit gets in the list.",
                    choices: &[("Cozy", Density::Cozy), ("Compact", Density::Compact)],
                    active: view.settings.history.density,
                    apply: |settings, density| settings.history.density = density,
                }
                .row(theme, cx),
                Choice {
                    id: "fetch-prune",
                    name: "Prune on fetch",
                    description: "Drop remote branches here once the remote has deleted them.",
                    choices: &[("Off", false), ("On", true)],
                    active: view.settings.git.fetch_prune,
                    apply: |settings, prune| settings.git.fetch_prune = prune,
                }
                .row(theme, cx),
            ],
        ),
        card(theme, vec![video_row(view.settings, theme)]),
    ]
}

/// The families the row offers. `None` is the platform's own, which is what
/// an unset `appearance.mono_font` means.
const MONO_PRESETS: &[(&str, Option<&'static str>)] = &[
    ("System", None),
    ("SF Mono", Some("SF Mono")),
    ("JetBrains Mono", Some("JetBrains Mono")),
    ("Fira Code", Some("Fira Code")),
];

/// Which preset the file currently holds, or a value none of them equals —
/// a hand-edited family lights no segment rather than mislighting "System".
fn active_mono_font(settings: &AppSettings) -> Option<&'static str> {
    let configured = settings.appearance.mono_font.as_deref();
    MONO_PRESETS
        .iter()
        .map(|&(_, preset)| preset)
        .find(|preset| *preset == configured)
        .unwrap_or(Some(""))
}

fn github_cards(view: &SettingsView<'_>, cx: &mut gpui::Context<SourcefourWindow>) -> Vec<Div> {
    let theme = view.theme;
    let enabled = view.settings.github.enabled;
    let method = view.settings.github.auth_method;
    let mut rows = vec![
        Choice {
            id: "github-enabled",
            name: "Enable GitHub integration",
            description: "Show pull requests and checks for github.com remotes.",
            choices: &[("Off", false), ("On", true)],
            active: enabled,
            apply: |settings, on| settings.github.enabled = on,
        }
        .row(theme, cx),
        Choice {
            id: "github-auth",
            name: "Authentication",
            description: "How API requests identify you.",
            choices: &[
                ("Off", AuthMethod::Off),
                ("Access token", AuthMethod::Token),
                ("gh CLI", AuthMethod::GhCli),
            ],
            active: method,
            apply: |settings, method| settings.github.auth_method = method,
        }
        .row(theme, cx),
    ];
    if enabled && method == AuthMethod::Token {
        rows.push(row(
            theme,
            "Personal access token",
            "Stored owner-only in credentials.json; needs repository read access.",
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .w(px(220.0))
                        .h(px(24.0))
                        .flex()
                        .items_center()
                        .px(px(8.0))
                        .rounded(px(5.0))
                        .border_1()
                        .border_color(theme.border_strong)
                        .bg(theme.bg_page)
                        .text_size(px(11.0))
                        .child(view.token_input.clone()),
                )
                .child(button(
                    theme,
                    "github-connect",
                    "Connect",
                    cx,
                    |this, cx| {
                        this.connect_github(cx);
                    },
                )),
        ));
    }
    if enabled && method == AuthMethod::GhCli {
        rows.push(row(
            theme,
            "GitHub CLI",
            "Borrows the token of a signed-in gh; nothing is stored.",
            button(theme, "github-connect", "Connect", cx, |this, cx| {
                this.connect_github(cx);
            }),
        ));
    }
    if enabled && let Some(status) = status_row(view.connection, theme, cx) {
        rows.push(status);
    }
    vec![card(theme, rows)]
}

/// The connection outcome, with Disconnect once one exists; nothing shows
/// before the first check.
fn status_row(
    connection: &GithubConnection,
    theme: &Theme,
    cx: &mut gpui::Context<SourcefourWindow>,
) -> Option<Div> {
    let (text, color) = match connection {
        GithubConnection::Idle => return None,
        GithubConnection::Checking => (String::from("Checking connection…"), theme.text_faint),
        GithubConnection::Connected { login } => (format!("Connected as {login}"), theme.green),
        GithubConnection::Failed { message } => (message.clone(), theme.red),
    };
    let row = div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(16.0))
        .px(px(14.0))
        .py(px(10.0))
        .child(div().text_size(px(11.5)).text_color(color).child(text))
        .children(
            matches!(connection, GithubConnection::Connected { .. }).then(|| {
                button(theme, "github-disconnect", "Disconnect", cx, |this, cx| {
                    this.disconnect_github(cx);
                })
            }),
        );
    Some(row)
}

/// A bordered chip button.
fn button(
    theme: &Theme,
    id: &'static str,
    label: &'static str,
    cx: &mut gpui::Context<SourcefourWindow>,
    on_click: impl Fn(&mut SourcefourWindow, &mut gpui::Context<SourcefourWindow>) + 'static,
) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .px(px(10.0))
        .py(px(3.0))
        .rounded(px(5.0))
        .border_1()
        .border_color(theme.border_strong)
        .bg(theme.bg_list)
        .cursor_pointer()
        .text_size(px(11.0))
        .text_color(theme.text_primary)
        .hover({
            let hover = theme.bg_hover;
            move |style| style.bg(hover)
        })
        .on_primary_click(cx.listener(move |this, _, _, cx| on_click(this, cx)))
        .child(label)
}

/// Diff overlay typography: line height as a unitless multiplier, offered as
/// two presets. The settings file itself accepts any number; only this
/// control is preset-based.
fn diffs_cards(view: &SettingsView<'_>, cx: &mut gpui::Context<SourcefourWindow>) -> Vec<Div> {
    let theme = view.theme;
    vec![card(
        theme,
        vec![
            Choice {
                id: "diff-line-height",
                name: "Line height",
                description: "Spacing between diff lines, as a multiple of the mono font size.",
                choices: &[("Standard", 1.55), ("Comfortable", 1.8)],
                active: view.settings.diff.line_height,
                apply: |settings, height| settings.diff.line_height = height,
            }
            .row(theme, cx),
        ],
    )]
}

/// Whether a video diff will show a frame, and how to say where the decoder
/// is when the search did not find it.
///
/// Read-only by design: the path is a rescue for an unusual install, not a
/// setting worth a text field in front of everyone who will never need it.
/// The lookup is the probe's own and answers once per session, so asking here
/// is what fixes the answer for the diffs that follow.
fn video_row(settings: &AppSettings, theme: &Theme) -> Div {
    let found = sourcefour_git::ffmpeg_path(settings.video.ffmpeg_dir.as_deref()).is_some();
    row(
        theme,
        "Video posters",
        r#"Set "video": { "ffmpeg_dir": "/path/to/bin" } in settings.json."#,
        value_text(
            theme,
            if found {
                "ffmpeg · found"
            } else {
                "ffmpeg · not found"
            },
        ),
    )
}

fn about_cards(theme: &Theme, cx: &mut gpui::Context<SourcefourWindow>) -> Vec<Div> {
    vec![card(
        theme,
        vec![
            row(
                theme,
                "Sourcefour",
                "A fast Git history browser.",
                value_text(theme, concat!("Version ", env!("CARGO_PKG_VERSION"))),
            ),
            row(
                theme,
                "Repository",
                "Source, issues, and releases.",
                link(
                    theme,
                    "github-link",
                    "github.com/HelgeSverre/sourcefour",
                    "https://github.com/HelgeSverre/sourcefour",
                    cx,
                ),
            ),
        ],
    )]
}

/// One elevated card: rows separated by hairlines, Zed's section shape.
fn card(theme: &Theme, rows: Vec<Div>) -> Div {
    let mut container = div()
        .flex()
        .flex_col()
        .rounded(px(8.0))
        .border_1()
        .border_color(theme.border)
        .bg(theme.bg_list)
        .overflow_hidden();
    let count = rows.len();
    for (index, row) in rows.into_iter().enumerate() {
        if index + 1 < count {
            container = container.child(row.border_b_1().border_color(theme.border));
        } else {
            container = container.child(row);
        }
    }
    container
}

/// One setting row: name and description on the left, its control right.
fn row(
    theme: &Theme,
    name: &'static str,
    description: &'static str,
    control: impl gpui::IntoElement,
) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(16.0))
        .px(px(14.0))
        .py(px(10.0))
        .child(
            div()
                // The text column yields, or a long description shoves the
                // control past the card edge where it clips invisibly.
                .flex_1()
                .min_w(px(1.0))
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(theme.text_primary)
                        .child(name),
                )
                .child(
                    div()
                        .text_size(px(10.5))
                        .text_color(theme.text_faint)
                        .child(description),
                ),
        )
        .child(control)
}

/// A row whose control is a fixed set of choices: the shape almost every
/// setting has, and the reason most of this section is data rather than
/// layout. `apply` is a plain fn pointer, so a choice carries no captures
/// and the whole row is a literal.
struct Choice<T: Copy + PartialEq + 'static> {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    choices: &'static [(&'static str, T)],
    active: T,
    apply: fn(&mut AppSettings, T),
}

impl<T: Copy + PartialEq + 'static> Choice<T> {
    /// The row, its control a segmented chip strip with the active one lit.
    /// Selecting a segment applies its value through `update_settings`.
    fn row(self, theme: &Theme, cx: &mut gpui::Context<SourcefourWindow>) -> Div {
        let (id, active, apply) = (self.id, self.active, self.apply);
        let control = div()
            .flex_none()
            .flex()
            .rounded(px(5.0))
            .border_1()
            .border_color(theme.border_strong)
            .overflow_hidden()
            .children(
                self.choices
                    .iter()
                    .enumerate()
                    .map(|(index, &(label, value))| {
                        let selected = value == active;
                        div()
                            .id((id, index))
                            .px(px(9.0))
                            .py(px(2.0))
                            .cursor_pointer()
                            .text_size(px(10.5))
                            .bg(if selected {
                                theme.bg_selected
                            } else {
                                theme.bg_list
                            })
                            .text_color(if selected {
                                theme.text_primary
                            } else {
                                theme.text_faint
                            })
                            .on_primary_click(cx.listener(move |this, _, _, cx| {
                                this.update_settings(cx, move |settings| apply(settings, value));
                            }))
                            .child(label)
                    }),
            );
        row(theme, self.name, self.description, control)
    }
}

/// A plain read-only value on the control side.
fn value_text(theme: &Theme, text: &'static str) -> Div {
    div()
        .flex_none()
        .text_size(px(11.0))
        .text_color(theme.text_secondary)
        .child(text)
}

/// An external link; clicking opens the browser.
fn link(
    theme: &Theme,
    id: &'static str,
    label: &'static str,
    url: &'static str,
    cx: &mut gpui::Context<SourcefourWindow>,
) -> Div {
    div().flex_none().child(
        div()
            .id(id)
            .on_context_menu(
                cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                    this.show_menu(
                        event.position,
                        crate::context_menu::link_entries(url, false),
                        window,
                        cx,
                    );
                }),
            )
            .cursor_pointer()
            .text_size(px(11.0))
            .text_color(theme.accent)
            .on_primary_click(cx.listener(move |_, _, _, cx| {
                cx.open_url(url);
            }))
            .child(label),
    )
}

/// The toolbar's gear, opening the overlay like `cmd-,` does.
pub(crate) fn toolbar_button(theme: &Theme, cx: &mut gpui::Context<SourcefourWindow>) -> Div {
    div().flex_none().child(
        div()
            .id("settings-action")
            .size(px(26.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(5.0))
            .cursor_pointer()
            .hover({
                let hover = theme.bg_hover;
                move |style| style.bg(hover)
            })
            .on_primary_click(cx.listener(|this, _, window, cx| {
                // The titlebar behind this button zooms on double-click; a
                // fast second click on the gear must not reach it.
                cx.stop_propagation();
                this.open_settings(window, cx);
            }))
            .child(
                svg()
                    .path("icons/settings.svg")
                    .size(px(14.0))
                    .text_color(theme.text_secondary),
            ),
    )
}
