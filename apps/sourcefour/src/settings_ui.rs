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

/// The overlay's open state, held by the window while it shows.
#[derive(Debug, Default)]
pub(crate) struct SettingsView {
    pub(crate) section: SettingsSection,
}

/// The full-window settings overlay: backdrop, nav, and the active section.
pub(crate) fn overlay(
    settings: &AppSettings,
    section: SettingsSection,
    theme: &Theme,
    focus: &gpui::FocusHandle,
    cx: &mut gpui::Context<SourcefourWindow>,
) -> FocusableWrapper<gpui::Stateful<Div>> {
    div()
        .id("settings-overlay")
        .key_context("Settings")
        .track_focus(focus)
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(gpui::black().opacity(0.55))
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
                .child(content(settings, section, theme, cx)),
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
                    SettingsSection::GitHub => github_cards(settings, theme, cx),
                    SettingsSection::About => about_cards(theme, cx),
                }),
        )
}

fn github_cards(
    settings: &AppSettings,
    theme: &Theme,
    cx: &mut gpui::Context<SourcefourWindow>,
) -> Vec<Div> {
    let enabled = settings.github.enabled;
    let method = settings.github.auth_method;
    vec![card(
        theme,
        vec![
            row(
                theme,
                "Enable GitHub integration",
                "Show pull requests and checks for github.com remotes.",
                toggle(theme, "github-enabled", enabled, cx, move |settings, on| {
                    settings.github.enabled = on;
                }),
            ),
            row(
                theme,
                "Authentication",
                "How API requests identify you. Connection arrives in a later step.",
                segmented(
                    theme,
                    &[
                        ("Off", AuthMethod::Off),
                        ("Access token", AuthMethod::Token),
                        ("gh CLI", AuthMethod::GhCli),
                    ],
                    method,
                    cx,
                ),
            ),
        ],
    )]
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
fn row(theme: &Theme, name: &'static str, description: &'static str, control: Div) -> Div {
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

/// An On/Off toggle rendered as the app's segmented chip.
fn toggle(
    theme: &Theme,
    id: &'static str,
    on: bool,
    cx: &mut gpui::Context<SourcefourWindow>,
    apply: impl Fn(&mut AppSettings, bool) + 'static,
) -> Div {
    let apply = std::rc::Rc::new(apply);
    div()
        .flex_none()
        .flex()
        .rounded(px(5.0))
        .border_1()
        .border_color(theme.border_strong)
        .overflow_hidden()
        .children([false, true].map(|value| {
            let selected = on == value;
            let apply = apply.clone();
            div()
                .id((id, u64::from(value)))
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
                    let apply = apply.clone();
                    this.update_settings(cx, move |settings| apply(settings, value));
                }))
                .child(if value { "On" } else { "Off" })
        }))
}

/// The auth-method selector, one segment per choice.
fn segmented(
    theme: &Theme,
    choices: &[(&'static str, AuthMethod)],
    active: AuthMethod,
    cx: &mut gpui::Context<SourcefourWindow>,
) -> Div {
    div()
        .flex_none()
        .flex()
        .rounded(px(5.0))
        .border_1()
        .border_color(theme.border_strong)
        .overflow_hidden()
        .children(choices.iter().map(|&(label, method)| {
            let selected = method == active;
            div()
                .id(label)
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
                    this.update_settings(cx, move |settings| {
                        settings.github.auth_method = method;
                    });
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
