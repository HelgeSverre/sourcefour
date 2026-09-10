//! One transient, command-neutral context menu per window.
//!
//! Surfaces build entries on the opening gesture. Invocations are bound to
//! their owner and target, never dispatched through the menu's keyboard focus.

use gpui::{
    App, ClipboardItem, Context, FocusHandle, IntoElement, MouseButton, MouseDownEvent, Pixels,
    Point, Render, SharedString, Subscription, WeakEntity, Window, anchored, deferred, div,
    prelude::*, px,
};

use crate::theme::Theme;

type Invocation = Box<dyn Fn(&mut Window, &mut App)>;

#[derive(Default)]
struct ClipboardSequence(u64);
impl gpui::Global for ClipboardSequence {}

/// A background copy may finish only until the next copy anywhere in the app.
pub(crate) fn reserve_clipboard(cx: &mut App) -> u64 {
    let sequence = cx.default_global::<ClipboardSequence>();
    sequence.0 = sequence.0.wrapping_add(1);
    sequence.0
}

pub(crate) fn finish_clipboard(ticket: u64, text: String, cx: &mut App) {
    if cx.default_global::<ClipboardSequence>().0 == ticket {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }
}

pub(crate) fn copy_text(text: String, cx: &mut App) {
    let ticket = reserve_clipboard(cx);
    finish_clipboard(ticket, text, cx);
}

pub(crate) enum MenuEntry {
    Action {
        label: SharedString,
        invoke: Option<Invocation>,
    },
    Separator,
}

impl MenuEntry {
    pub(crate) fn new(
        label: impl Into<SharedString>,
        invoke: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self::Action {
            label: label.into(),
            invoke: Some(Box::new(invoke)),
        }
    }

    pub(crate) fn disabled(label: impl Into<SharedString>) -> Self {
        Self::Action {
            label: label.into(),
            invoke: None,
        }
    }

    pub(crate) fn command<T: 'static>(
        label: impl Into<SharedString>,
        cx: &Context<T>,
        invoke: impl Fn(&mut T, &mut Window, &mut Context<T>) + 'static,
    ) -> Self {
        let owner = cx.weak_entity();
        Self::new(label, move |window, cx| {
            owner.update(cx, |owner, cx| invoke(owner, window, cx)).ok();
        })
    }

    pub(crate) fn copy(label: impl Into<SharedString>, text: impl Into<SharedString>) -> Self {
        let text = text.into();
        Self::new(label, move |_, cx| {
            copy_text(text.to_string(), cx);
        })
    }

    pub(crate) fn enabled(&self) -> bool {
        matches!(
            self,
            Self::Action {
                invoke: Some(_),
                ..
            }
        )
    }
}

pub(crate) fn link_entries(url: &str, github: bool) -> Vec<MenuEntry> {
    let open_label = if github {
        "Open on GitHub"
    } else {
        "Open in browser"
    };
    if url.is_empty() {
        return vec![
            MenuEntry::disabled(open_label),
            MenuEntry::disabled("Copy link"),
        ];
    }
    let url = SharedString::from(url.to_owned());
    let open_url = url.clone();
    vec![
        MenuEntry::new(open_label, move |_, cx| {
            cx.open_url(&open_url);
        }),
        MenuEntry::copy("Copy link", url),
    ]
}

pub(crate) fn is_context_click(event: &MouseDownEvent) -> bool {
    event.button == MouseButton::Right
        || (cfg!(target_os = "macos")
            && event.button == MouseButton::Left
            && event.modifiers.control)
}

pub(crate) trait ContextMenuExt: InteractiveElement + Sized {
    fn named_link_menu(
        self,
        host: WeakEntity<MenuHost>,
        url: impl Into<SharedString>,
        github: bool,
        name: impl Into<SharedString>,
    ) -> Self {
        let url = url.into();
        let name = name.into();
        self.on_context_menu(move |event, window, cx| {
            let mut entries = link_entries(&url, github);
            entries.push(MenuEntry::copy("Copy name", name.clone()));
            show(&host, event.position, entries, window, cx);
        })
    }
    fn link_menu(
        self,
        host: WeakEntity<MenuHost>,
        url: impl Into<SharedString>,
        github: bool,
    ) -> Self {
        let url = url.into();
        self.on_context_menu(move |event, window, cx| {
            show(
                &host,
                event.position,
                link_entries(&url, github),
                window,
                cx,
            );
        })
    }

    fn on_context_menu(
        self,
        listener: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_any_mouse_down(move |event, window, cx| {
            if is_context_click(event) {
                window.prevent_default();
                cx.stop_propagation();
                listener(event, window, cx);
            }
        })
    }
}
impl<T: InteractiveElement> ContextMenuExt for T {}

/// GPUI also records a left-button click for macOS Control-click. Guard the
/// release as well as consuming the opening press, including nested buttons.
pub(crate) trait PrimaryClickExt: StatefulInteractiveElement + Sized {
    fn on_primary_click(
        self,
        listener: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click(move |event, window, cx| {
            if !matches!(event, gpui::ClickEvent::Mouse(event) if is_context_click(&event.down)) {
                listener(event, window, cx);
            }
        })
    }
}
impl<T: StatefulInteractiveElement> PrimaryClickExt for T {}

struct Popup {
    entries: Vec<MenuEntry>,
    position: Point<Pixels>,
    highlighted: Option<usize>,
    previous_focus: Option<FocusHandle>,
    target: Option<SharedString>,
}

pub(crate) struct MenuHost {
    window: gpui::AnyWindowHandle,
    popup: Option<Popup>,
    focus: FocusHandle,
    theme: Theme,
    serial: u64,
    _subscriptions: Vec<Subscription>,
}

impl MenuHost {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let subscriptions = vec![
            cx.observe_window_bounds(window, Self::dismiss),
            cx.observe_window_activation(window, |this, window, cx| {
                if !window.is_window_active() {
                    this.dismiss(window, cx);
                }
            }),
            cx.on_focus_out(&focus, window, |this, _, window, cx| {
                // A queued focus-out from a previous popup cannot close its
                // replacement, nor take focus back from a newly opened dialog.
                if !this.focus.is_focused(window) {
                    this.dismiss(window, cx);
                }
            }),
        ];
        Self {
            window: window.window_handle(),
            popup: None,
            focus,
            theme: Theme::dark(),
            serial: 0,
            _subscriptions: subscriptions,
        }
    }

    pub(crate) fn set_theme(&mut self, theme: &Theme) {
        self.theme = *theme;
    }

    pub(crate) fn is_open(&self) -> bool {
        self.popup.is_some()
    }

    pub(crate) fn targets(&self, target: &str) -> bool {
        self.popup
            .as_ref()
            .and_then(|popup| popup.target.as_ref().map(AsRef::as_ref))
            == Some(target)
    }

    pub(crate) fn open(
        &mut self,
        position: Point<Pixels>,
        entries: Vec<MenuEntry>,
        target: Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if entries.is_empty() {
            return;
        }
        let previous_focus = if self.focus.is_focused(window) {
            self.popup
                .as_mut()
                .and_then(|popup| popup.previous_focus.take())
        } else {
            window.focused(cx)
        };
        self.serial = self.serial.wrapping_add(1);
        let highlighted = entries.iter().position(MenuEntry::enabled);
        let position = gpui::point(position.x.max(px(8.0)), position.y.max(px(8.0)));
        self.popup = Some(Popup {
            entries,
            position,
            highlighted,
            previous_focus,
            target,
        });
        self.focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(popup) = self.popup.take() {
            if self.focus.is_focused(window)
                && let Some(previous) = popup.previous_focus
            {
                previous.focus(window, cx);
            }
            cx.notify();
        }
    }

    /// Used by state changes without a Window argument. Remove the popup now,
    /// and restore focus after the current window/entity update has finished.
    pub(crate) fn invalidate(&mut self, cx: &mut Context<Self>) {
        let Some(popup) = self.popup.take() else {
            return;
        };
        let serial = self.serial;
        cx.notify();
        let weak = cx.weak_entity();
        cx.defer(move |cx| {
            weak.update(cx, |this, cx| {
                if this.serial != serial || this.popup.is_some() {
                    return;
                }
                let focus = this.focus.clone();
                this.window
                    .update(cx, |_, window, cx| {
                        if focus.is_focused(window)
                            && let Some(previous) = popup.previous_focus
                        {
                            previous.focus(window, cx);
                        }
                    })
                    .ok();
            })
            .ok();
        });
    }

    pub(crate) fn invalidate_for_focus(&mut self, focus: &FocusHandle, cx: &mut Context<Self>) {
        if self
            .popup
            .as_ref()
            .and_then(|popup| popup.previous_focus.as_ref())
            == Some(focus)
        {
            self.invalidate(cx);
        }
    }

    fn invoke(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(MenuEntry::Action { invoke, .. }) = self
            .popup
            .as_mut()
            .and_then(|popup| popup.entries.get_mut(index))
        else {
            return;
        };
        let Some(invoke) = invoke.take() else {
            return;
        };
        self.dismiss(window, cx);
        window.defer(cx, move |window, cx| invoke(window, cx));
    }

    fn navigate(&mut self, backwards: bool, edge: bool, cx: &mut Context<Self>) {
        let Some(popup) = self.popup.as_mut() else {
            return;
        };
        let count = popup.entries.len();
        let start = if edge {
            if backwards { 0 } else { count - 1 }
        } else {
            popup
                .highlighted
                .unwrap_or(if backwards { 0 } else { count - 1 })
        };
        popup.highlighted = (1..=count)
            .map(|step| {
                if backwards {
                    (start + count - step) % count
                } else {
                    (start + step) % count
                }
            })
            .find(|&index| popup.entries[index].enabled());
        cx.notify();
    }

    fn key_down(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        window.prevent_default();
        match event.keystroke.key.as_str() {
            "escape" | "tab" => self.dismiss(window, cx),
            "up" => self.navigate(true, false, cx),
            "down" => self.navigate(false, false, cx),
            "home" => self.navigate(false, true, cx),
            "end" => self.navigate(true, true, cx),
            "enter" => {
                if let Some(index) = self.popup.as_ref().and_then(|popup| popup.highlighted) {
                    self.invoke(index, window, cx);
                }
            }
            _ => {}
        }
    }
}

impl Render for MenuHost {
    #[expect(
        clippy::too_many_lines,
        reason = "the small popup composes its dismissal and entry interactions together"
    )]
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Focus notifications run after paint in GPUI; do not paint a popup
        // whose focus has already moved to a different surface.
        if self.popup.is_some() && !self.focus.is_focused(window) {
            self.dismiss(window, cx);
        }
        let Some(popup) = &self.popup else {
            return div().into_any_element();
        };
        let serial = self.serial;
        let weak = cx.weak_entity();
        let scroll_dismiss = gpui::canvas(
            |_, _, _| (),
            move |_, (), window, _| {
                let weak = weak.clone();
                window.on_mouse_event(move |_: &gpui::ScrollWheelEvent, phase, window, cx| {
                    if phase == gpui::DispatchPhase::Capture {
                        weak.update(cx, |this, cx| {
                            if this.serial == serial {
                                this.dismiss(window, cx);
                            }
                        })
                        .ok();
                    }
                });
            },
        )
        .absolute()
        .size_full();
        let theme = self.theme;
        let menu = div()
            .id("context-menu")
            .relative()
            .debug_selector(|| "context-menu".into())
            .key_context("ContextMenu")
            .track_focus(&self.focus)
            .occlude()
            .w(px(240.0))
            .p(px(4.0))
            .flex()
            .flex_col()
            .rounded(px(6.0))
            .border_1()
            .border_color(theme.border_strong)
            .bg(theme.bg_panel)
            .text_color(theme.text_primary)
            .text_size(px(12.0))
            .shadow_lg()
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_down_out(cx.listener(move |this, event, window, cx| {
                if this.serial != serial {
                    return;
                }
                this.dismiss(window, cx);
                if !is_context_click(event) {
                    window.prevent_default();
                    cx.stop_propagation();
                }
            }))
            .on_any_mouse_down(|_, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
            })
            .child(scroll_dismiss)
            .children(popup.entries.iter().enumerate().map(|(index, entry)| {
                match entry {
                    MenuEntry::Separator => div()
                        .h(px(1.0))
                        .my(px(4.0))
                        .bg(theme.border)
                        .into_any_element(),
                    MenuEntry::Action { label, invoke } => {
                        let enabled = invoke.is_some();
                        div()
                            .id(("context-menu-entry", index))
                            .debug_selector(|| format!("context-entry-{index}"))
                            .h(px(26.0))
                            .px(px(8.0))
                            .flex()
                            .items_center()
                            .rounded(px(3.0))
                            .text_color(if enabled {
                                theme.text_primary
                            } else {
                                theme.text_faint
                            })
                            .when(enabled && popup.highlighted == Some(index), |row| {
                                row.bg(theme.bg_selected)
                            })
                            .when(enabled, |row| {
                                row.cursor_pointer()
                                    .on_mouse_move(cx.listener(move |this, _, _, cx| {
                                        if this.serial == serial
                                            && let Some(popup) = &mut this.popup
                                            && popup.highlighted != Some(index)
                                        {
                                            popup.highlighted = Some(index);
                                            cx.notify();
                                        }
                                    }))
                                    .on_primary_click(cx.listener(move |this, _, window, cx| {
                                        if this.serial == serial {
                                            this.invoke(index, window, cx);
                                        }
                                    }))
                            })
                            .child(label.clone())
                            .into_any_element()
                    }
                }
            }));
        deferred(
            anchored()
                .position(popup.position)
                .snap_to_window_with_margin(px(8.0))
                .child(menu),
        )
        .with_priority(100)
        .into_any_element()
    }
}

pub(crate) fn show(
    host: &WeakEntity<MenuHost>,
    position: Point<Pixels>,
    entries: Vec<MenuEntry>,
    window: &mut Window,
    cx: &mut App,
) {
    host.update(cx, |host, cx| {
        host.open(position, entries, None, window, cx);
    })
    .ok();
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext, point, size};

    struct Fixture {
        host: Entity<MenuHost>,
        focus: FocusHandle,
        activations: usize,
        builds: usize,
    }

    impl Render for Fixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .track_focus(&self.focus)
                .child(
                    div()
                        .id("target")
                        .size(px(100.0))
                        .on_primary_click(cx.listener(|this, _, _, cx| {
                            this.activations += 1;
                            cx.notify();
                        }))
                        .on_context_menu(cx.listener(
                            |this, event: &MouseDownEvent, window, cx| {
                                this.builds += 1;
                                let entries = vec![
                                    MenuEntry::copy("First", "first"),
                                    MenuEntry::Separator,
                                    MenuEntry::disabled("Disabled"),
                                    MenuEntry::copy("Last", "last"),
                                ];
                                this.host.update(cx, |host, cx| {
                                    host.open(event.position, entries, None, window, cx);
                                });
                            },
                        )),
                )
                .child(self.host.clone())
        }
    }

    fn fixture(cx: &mut TestAppContext) -> (Entity<Fixture>, &mut VisualTestContext) {
        let (fixture, cx) = cx.add_window_view(|window, cx| {
            let host = cx.new(|cx| MenuHost::new(window, cx));
            cx.observe(&host, |_, _, cx| cx.notify()).detach();
            let focus = cx.focus_handle();
            focus.focus(window, cx);
            Fixture {
                host,
                focus,
                activations: 0,
                builds: 0,
            }
        });
        cx.simulate_resize(size(px(800.0), px(600.0)));
        cx.run_until_parked();
        (fixture, cx)
    }

    fn draw(fixture: &Entity<Fixture>, cx: &mut VisualTestContext) {
        cx.update(|_, cx| {
            fixture.update(cx, |_, cx| cx.notify());
        });
        cx.run_until_parked();
    }

    #[gpui::test]
    fn a_new_copy_supersedes_an_older_background_copy(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let old = reserve_clipboard(cx);
            copy_text("new SHA".into(), cx);
            finish_clipboard(old, "old job log".into(), cx);
            assert_eq!(
                cx.read_from_clipboard().unwrap().text().as_deref(),
                Some("new SHA")
            );
        });
    }

    #[gpui::test]
    fn menu_keyboard_skips_disabled_entries_and_restores_focus(cx: &mut TestAppContext) {
        let (fixture, cx) = fixture(cx);
        draw(&fixture, cx);
        cx.simulate_mouse_down(
            point(px(10.0), px(10.0)),
            MouseButton::Right,
            Modifiers::default(),
        );
        draw(&fixture, cx);
        cx.simulate_keystrokes("down enter");
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().as_deref(),
            Some("last")
        );
        cx.update(|window, cx| {
            let fixture = fixture.read(cx);
            assert_eq!(fixture.builds, 1);
            assert_eq!(fixture.activations, 0);
            assert!(fixture.focus.is_focused(window));
            assert!(!fixture.host.read(cx).is_open());
        });
    }

    #[gpui::test]
    fn menu_pointer_and_outside_clicks_do_not_activate_the_target(cx: &mut TestAppContext) {
        let (fixture, cx) = fixture(cx);
        draw(&fixture, cx);
        cx.simulate_mouse_down(
            point(px(30.0), px(30.0)),
            MouseButton::Right,
            Modifiers::default(),
        );
        draw(&fixture, cx);
        let entry = cx.debug_bounds("context-entry-0").unwrap();
        cx.simulate_click(entry.center(), Modifiers::default());
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().as_deref(),
            Some("first")
        );
        draw(&fixture, cx);
        cx.simulate_mouse_down(
            point(px(30.0), px(30.0)),
            MouseButton::Right,
            Modifiers::default(),
        );
        draw(&fixture, cx);
        cx.simulate_click(point(px(5.0), px(5.0)), Modifiers::default());
        cx.update(|_, cx| {
            assert_eq!(fixture.read(cx).activations, 0);
            assert!(!fixture.read(cx).host.read(cx).is_open());
        });
    }

    #[cfg(target_os = "macos")]
    #[gpui::test]
    fn menu_control_click_and_replacement_preserve_focus(cx: &mut TestAppContext) {
        let (fixture, cx) = fixture(cx);
        draw(&fixture, cx);
        cx.simulate_click(
            point(px(40.0), px(40.0)),
            Modifiers {
                control: true,
                ..Modifiers::default()
            },
        );
        draw(&fixture, cx);
        cx.simulate_mouse_down(
            point(px(5.0), px(5.0)),
            MouseButton::Right,
            Modifiers::default(),
        );
        draw(&fixture, cx);
        cx.update(|window, cx| {
            let fixture = fixture.read(cx);
            assert_eq!(fixture.builds, 2);
            assert_eq!(fixture.activations, 0);
            assert!(fixture.host.read(cx).is_open());
            assert!(fixture.host.read(cx).focus.is_focused(window));
        });
        cx.simulate_keystrokes("escape");
        cx.update(|window, cx| assert!(fixture.read(cx).focus.is_focused(window)));
    }

    #[gpui::test]
    fn menu_closes_on_scroll_resize_and_focus_loss(cx: &mut TestAppContext) {
        let (fixture, cx) = fixture(cx);
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        assert!(cx.update(|window, _| window.is_window_active()));
        for change in 0..4 {
            draw(&fixture, cx);
            cx.simulate_mouse_down(
                point(px(10.0), px(10.0)),
                MouseButton::Right,
                Modifiers::default(),
            );
            draw(&fixture, cx);
            match change {
                0 => cx.simulate_event(gpui::ScrollWheelEvent {
                    position: point(px(400.0), px(400.0)),
                    delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(-20.0))),
                    ..gpui::ScrollWheelEvent::default()
                }),
                1 => cx.simulate_resize(size(px(900.0), px(700.0))),
                2 => cx.update(|window, cx| {
                    let focus = fixture.read(cx).focus.clone();
                    focus.focus(window, cx);
                }),
                _ => cx.deactivate_window(),
            }
            draw(&fixture, cx);
            assert!(
                !cx.update(|_, cx| fixture.read(cx).host.read(cx).is_open()),
                "change {change}"
            );
        }
    }

    #[gpui::test]
    fn menu_is_clamped_at_every_window_corner(cx: &mut TestAppContext) {
        let (fixture, cx) = fixture(cx);
        let viewport = cx.update(|window, _| window.viewport_size());
        for position in [
            Point::default(),
            point(viewport.width, px(0.0)),
            point(px(0.0), viewport.height),
            point(viewport.width, viewport.height),
        ] {
            cx.update(|window, cx| {
                let host = fixture.read(cx).host.clone();
                host.update(cx, |host, cx| {
                    host.open(
                        position,
                        vec![MenuEntry::copy("Copy", "value")],
                        None,
                        window,
                        cx,
                    );
                });
            });
            draw(&fixture, cx);
            let bounds = cx.debug_bounds("context-menu").unwrap();
            assert!(bounds.left() >= px(8.0) && bounds.top() >= px(8.0));
            assert!(bounds.right() <= viewport.width - px(8.0));
            assert!(bounds.bottom() <= viewport.height - px(8.0));
        }
    }
}
