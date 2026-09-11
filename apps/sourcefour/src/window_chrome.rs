//! Platform title-bar behavior and client-side window controls.

use std::{cell::Cell, rc::Rc};

use gpui::{
    App, Decorations, Div, MouseButton, Stateful, Window, WindowButton, WindowButtonLayout,
    WindowControlArea, div, prelude::*, px, rgb, svg,
};

use crate::{
    context_menu::PrimaryClickExt as _,
    theme::{TITLEBAR_HEIGHT, Theme},
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Platform {
    Mac,
    Windows,
    Linux,
}

impl Platform {
    const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::Mac
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else {
            Self::Linux
        }
    }
}

const DEFAULT_BUTTONS: WindowButtonLayout = WindowButtonLayout {
    left: [None; 3],
    right: [
        Some(WindowButton::Minimize),
        Some(WindowButton::Maximize),
        Some(WindowButton::Close),
    ],
};

fn button_layout(
    platform: Platform,
    decorations: Decorations,
    preference: Option<WindowButtonLayout>,
) -> Option<WindowButtonLayout> {
    match platform {
        Platform::Mac => None,
        Platform::Windows => Some(DEFAULT_BUTTONS),
        Platform::Linux => match decorations {
            Decorations::Server => None,
            Decorations::Client { .. } => Some(preference.unwrap_or(DEFAULT_BUTTONS)),
        },
    }
}

/// Drag state belongs to the window so asynchronous redraws cannot lose a press.
#[derive(Default)]
pub(crate) struct WindowChrome {
    dragging: Rc<Cell<bool>>,
}

impl WindowChrome {
    /// The caller adds its title and content, then `right_controls`.
    pub(crate) fn titlebar(&self, window: &Window, cx: &App, theme: &Theme) -> Stateful<Div> {
        let platform = Platform::current();
        let dragging = Rc::clone(&self.dragging);
        let down = Rc::clone(&dragging);
        let up = Rc::clone(&dragging);
        let outside = Rc::clone(&dragging);
        div()
            .id("titlebar")
            .window_control_area(WindowControlArea::Drag)
            .h(px(TITLEBAR_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .pl(px(
                if platform == Platform::Mac
                    && !window.is_fullscreen()
                    && !window.is_simple_fullscreen()
                {
                    76.0
                } else {
                    8.0
                },
            ))
            .gap(px(8.0))
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.bg_chrome)
            .text_size(px(12.5))
            .text_color(theme.text_secondary)
            // Windows handles dragging and double-clicks through native hit testing.
            .when(platform != Platform::Windows, |bar| {
                bar.on_mouse_down(MouseButton::Left, move |_, _, _| down.set(true))
                    .on_mouse_up(MouseButton::Left, move |_, _, _| up.set(false))
                    .on_mouse_down_out(move |_, _, _| outside.set(false))
                    .on_mouse_move(move |event, window, _| {
                        if dragging.replace(false)
                            && event.pressed_button == Some(MouseButton::Left)
                        {
                            window.start_window_move();
                        }
                    })
                    .on_primary_click(move |event, window, _| {
                        if event.click_count() == 2 {
                            if platform == Platform::Mac {
                                window.titlebar_double_click();
                            } else if window.window_controls().maximize && window.is_resizable() {
                                window.zoom_window();
                            }
                        }
                    })
            })
            .child(controls(window, cx, theme, true))
    }
}

pub(crate) fn right_controls(window: &Window, cx: &App, theme: &Theme) -> Div {
    controls(window, cx, theme, false)
}

fn controls(window: &Window, cx: &App, theme: &Theme, left: bool) -> Div {
    let platform = Platform::current();
    let layout = button_layout(platform, window.window_decorations(), cx.button_layout());
    let buttons = layout.map_or(
        [None; 3],
        |layout| if left { layout.left } else { layout.right },
    );
    let supported = window.window_controls();
    div().flex().items_center().flex_none().h_full().children(
        buttons
            .into_iter()
            .flatten()
            .filter(|button| {
                platform != Platform::Linux
                    || match button {
                        WindowButton::Minimize => supported.minimize,
                        WindowButton::Maximize => supported.maximize,
                        WindowButton::Close => true,
                    }
            })
            .map(|button| control(button, platform, window, theme)),
    )
}

fn control(
    button: WindowButton,
    platform: Platform,
    window: &Window,
    theme: &Theme,
) -> Stateful<Div> {
    let enabled = match button {
        WindowButton::Minimize => window.is_minimizable(),
        WindowButton::Maximize => window.is_resizable(),
        WindowButton::Close => true,
    };
    let (id, icon, area) = match button {
        WindowButton::Minimize => (
            "window-minimize",
            "icons/window-minimize.svg",
            WindowControlArea::Min,
        ),
        WindowButton::Maximize => (
            "window-maximize",
            if window.is_maximized() {
                "icons/window-restore.svg"
            } else {
                "icons/window-maximize.svg"
            },
            WindowControlArea::Max,
        ),
        WindowButton::Close => (
            "window-close",
            "icons/window-close.svg",
            WindowControlArea::Close,
        ),
    };
    let close = button == WindowButton::Close;
    let hover = if close {
        rgb(0x00e8_1123).into()
    } else {
        theme.bg_hover
    };
    div()
        .id(id)
        .group("window-control")
        .occlude()
        .w(px(if platform == Platform::Windows {
            36.0
        } else {
            30.0
        }))
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .when(enabled, |control| {
            control.hover(move |style| style.bg(hover))
        })
        .when(platform == Platform::Windows && enabled, |control| {
            control.window_control_area(area)
        })
        // Windows must receive the propagated events to handle native caption
        // buttons. Only Linux dispatches these actions through our callbacks.
        .when(platform == Platform::Linux, |control| {
            control
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_move(|_, _, cx| cx.stop_propagation())
                .on_primary_click(move |_, window, cx| {
                    cx.stop_propagation();
                    if enabled {
                        match button {
                            WindowButton::Minimize => window.minimize_window(),
                            WindowButton::Maximize => window.zoom_window(),
                            WindowButton::Close => window.remove_window(),
                        }
                    }
                })
        })
        .child(
            svg()
                .path(icon)
                .size(px(12.0))
                .text_color(if enabled {
                    theme.text_secondary
                } else {
                    theme.text_faint
                })
                .when(enabled && close, |icon| {
                    icon.group_hover("window-control", |style| style.text_color(rgb(0x00ff_ffff)))
                }),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_controls_are_not_duplicated() {
        assert!(
            button_layout(
                Platform::Mac,
                Decorations::Client {
                    tiling: gpui::Tiling::default()
                },
                None
            )
            .is_none()
        );
        assert!(button_layout(Platform::Linux, Decorations::Server, None).is_none());
        assert_eq!(
            button_layout(Platform::Windows, Decorations::Server, None),
            Some(DEFAULT_BUTTONS)
        );
    }

    #[test]
    fn linux_client_controls_follow_desktop_order_with_a_fallback() {
        let decorations = Decorations::Client {
            tiling: gpui::Tiling::default(),
        };
        let preference = WindowButtonLayout {
            left: [Some(WindowButton::Close), None, None],
            right: [None; 3],
        };
        assert_eq!(
            button_layout(Platform::Linux, decorations, Some(preference)),
            Some(preference)
        );
        assert_eq!(
            button_layout(Platform::Linux, decorations, None),
            Some(DEFAULT_BUTTONS)
        );
    }
}
