//! The settings overlay: section navigation on the left, editable cards on
//! the right. The page shape follows Zed's settings editor — titled cards of
//! labeled controls — rendered with this app's own theme and widgets.
//!
//! Mutations route through `SourcefourWindow`'s `update_settings`, which
//! writes the file immediately; there is no separate save step.

use gpui::{Div, FocusableWrapper, FontWeight, div, prelude::*, px, svg};

use crate::{settings::AppSettings, settings::AuthMethod, theme::Theme, views::SourcefourWindow};

/// One page of the settings overlay.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum SettingsSection {
    #[default]
    GitHub,
    About,
}

impl SettingsSection {
    const ALL: [Self; 2] = [Self::GitHub, Self::About];

    fn title(self) -> &'static str {
        match self {
            Self::GitHub => "GitHub",
            Self::About => "About",
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
    settings: &AppSettings,
    section: SettingsSection,
    connection: &GithubConnection,
    token_input: &gpui::Entity<crate::text_input::TextInput>,
    theme: &Theme,
    focus: &gpui::FocusHandle,
    cx: &mut gpui::Context<SourcefourWindow>,
) -> FocusableWrapper<gpui::Stateful<Div>> {
    div()
        .id("settings-overlay")
        .key_context("Settings")
        .track_focus(focus)
        // Nothing behind the overlay may react to the mouse (§4.6 modality).
        .occlude()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(theme.scrim())
        .on_click(cx.listener(|this, _, window, cx| {
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
                .on_click(|_, _, cx| cx.stop_propagation())
                .child(nav(section, theme, cx))
                .child(content(
                    settings,
                    section,
                    connection,
                    token_input,
                    theme,
                    cx,
                )),
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
        .children(SettingsSection::ALL.into_iter().map(|section| {
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
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.set_settings_section(section, cx);
                }))
                .child(section.title())
        }))
}

/// The right column: section header with close, then that section's cards.
fn content(
    settings: &AppSettings,
    section: SettingsSection,
    connection: &GithubConnection,
    token_input: &gpui::Entity<crate::text_input::TextInput>,
    theme: &Theme,
    cx: &mut gpui::Context<SourcefourWindow>,
) -> Div {
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
                        .on_click(cx.listener(|this, _, window, cx| {
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
                .children(match section {
                    SettingsSection::GitHub => {
                        github_cards(settings, connection, token_input, theme, cx)
                    }
                    SettingsSection::About => about_cards(theme, cx),
                }),
        )
}

fn github_cards(
    settings: &AppSettings,
    connection: &GithubConnection,
    token_input: &gpui::Entity<crate::text_input::TextInput>,
    theme: &Theme,
    cx: &mut gpui::Context<SourcefourWindow>,
) -> Vec<Div> {
    let enabled = settings.github.enabled;
    let method = settings.github.auth_method;
    let mut rows = vec![
        row(
            theme,
            "Enable GitHub integration",
            "Show pull requests and checks for github.com remotes.",
            segmented(
                theme,
                "github-enabled",
                &[("Off", false), ("On", true)],
                enabled,
                cx,
                |settings, on| settings.github.enabled = on,
            ),
        ),
        row(
            theme,
            "Authentication",
            "How API requests identify you.",
            segmented(
                theme,
                "github-auth",
                &[
                    ("Off", AuthMethod::Off),
                    ("Access token", AuthMethod::Token),
                    ("gh CLI", AuthMethod::GhCli),
                ],
                method,
                cx,
                |settings, method| settings.github.auth_method = method,
            ),
        ),
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
                        .child(token_input.clone()),
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
    if enabled && let Some(status) = status_row(connection, theme, cx) {
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
        .on_click(cx.listener(move |this, _, _, cx| on_click(this, cx)))
        .child(label)
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

/// A segmented chip control: one segment per choice, the active one lit.
/// Selecting a segment applies its value through `update_settings`.
fn segmented<T: Copy + PartialEq + 'static>(
    theme: &Theme,
    id: &'static str,
    choices: &'static [(&'static str, T)],
    active: T,
    cx: &mut gpui::Context<SourcefourWindow>,
    apply: fn(&mut AppSettings, T),
) -> Div {
    div()
        .flex_none()
        .flex()
        .rounded(px(5.0))
        .border_1()
        .border_color(theme.border_strong)
        .overflow_hidden()
        .children(choices.iter().enumerate().map(|(index, &(label, value))| {
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
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.update_settings(cx, move |settings| apply(settings, value));
                }))
                .child(label)
        }))
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
            .cursor_pointer()
            .text_size(px(11.0))
            .text_color(theme.accent)
            .on_click(cx.listener(move |_, _, _, cx| {
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
            .on_click(cx.listener(|this, _, window, cx| {
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
