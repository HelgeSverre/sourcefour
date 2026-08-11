//! The palette, and the map of where each color belongs.
//!
//! Every color in the app comes from here — either a `Theme` field (a token)
//! or a semantic helper on `Theme` (a derived modifier). Views never invent
//! their own `opacity(..)` math for a recurring role; if a new role recurs,
//! it gets a named helper here first. The website's `brand.css` mirrors these
//! values — change one, change the other.
//!
//! # Where each token paints
//!
//! ```text
//! the window                                  an overlay (diff, settings,
//! ┌─────────────────────────────────────┐     actions, branch dialog)
//! │ titlebar                  bg_chrome │
//! │ toolbar                   bg_chrome │     ░░░░░░ scrim() ░░░░░░░░░░░
//! ├──────────┬──────┬───────────────────┤     ░┌────────────────────┐░
//! │ sidebar  │ graph│ history   bg_list │     ░│ header   bg_chrome │░
//! │ bg_panel │      │ hover    bg_hover │     ░├────┬───────────────┤░
//! │          │      │ selected          │     ░│ 12 │ code  bg_list │░
//! │          │      │       bg_selected │     ░│ 13 │+ added line   │░
//! │          │      ├───────────────────┤     ░│    │- removed line │░
//! │          │      │ details  bg_panel │     ░└────┴───────────────┘░
//! ├──────────┴──────┴───────────────────┤     ░░░░░░░░│░░░░░░░░░░░░░░░
//! │ status bar                bg_chrome │       gutter_rule() divider,
//! └─────────────────────────────────────┘       tint_added()/removed()
//! ```
//!
//! # The ladders
//!
//! Surfaces, recessed to raised — pick by how far forward a region sits:
//! `bg_page` (the void behind everything) → `bg_chrome` (window furniture:
//! bars, headers) → `bg_panel` (framed regions: sidebar, details, overlay
//! frames) → `bg_list` (content being read: history, diffs, logs) →
//! `bg_hover` (transient emphasis) → `bg_selected` (the current row).
//!
//! Borders: `border` between regions that already differ in surface;
//! `border_strong` when the two sides share a surface and the line itself
//! must carry the separation (input outlines, the diff gutter rule).
//!
//! Ink: `text_primary` for what the user came to read, `text_secondary`
//! for supporting matter, `text_faint` for furniture (line numbers, hints),
//! `text_on_accent` on accent-filled controls.
//!
//! Status colors: `green` success/additions · `red` failure/deletions ·
//! `orange` in-progress/warnings · `purple` tags/detached · `cyan` the
//! hue nothing else claims, spent on a graph lane and on what an Actions
//! log's escapes call cyan · `accent` interaction and selection.
//! `graph_lanes` recycles them for lanes.
//!
//! # Derived modifiers
//!
//! | helper                      | use it for                                |
//! |-----------------------------|-------------------------------------------|
//! | `scrim()`                   | the backdrop behind every overlay         |
//! | `recessed()`                | inset surfaces: plot wells, code fences   |
//! | `gutter_rule()`             | line-number ↔ code divider in diffs       |
//! | `tint_added()`/`removed()`  | washes behind diff lines                  |
//! | `chip_border()`/`chip_fill()` | tinted badges: CURRENT, refs, PRs, chips |
//! | `hud()`                     | floating labels over media                |
//! | `grab_active()`/`grab_hover()` | splitters under drag / pointer         |
//!
//! The modal shell itself — backdrop, panel, and the true-click guard —
//! lives in `views::modal_backdrop` / `views::modal_panel` /
//! `views::is_true_click`, so every overlay closes and occludes the same way.

use gpui::{Hsla, rgb};
use sourcefour_model::GRAPH_COLOR_COUNT;

pub(crate) const INITIAL_WIDTH: f32 = 1280.0;
pub(crate) const INITIAL_HEIGHT: f32 = 800.0;
pub(crate) const MINIMUM_WIDTH: f32 = 900.0;
pub(crate) const MINIMUM_HEIGHT: f32 = 560.0;
pub(crate) const TITLEBAR_HEIGHT: f32 = 38.0;
pub(crate) const TOOLBAR_HEIGHT: f32 = 46.0;
pub(crate) const SIDEBAR_WIDTH: f32 = 236.0;
pub(crate) const HEADER_HEIGHT: f32 = 26.0;
/// A cozy history row; `history.density` picks between this and compact,
/// and [`crate::settings::HistorySettings::row_height`] is what the list
/// reads.
pub(crate) const HISTORY_ROW_HEIGHT: f32 = 30.0;
pub(crate) const GRAPH_WIDTH: f32 = 76.0;
pub(crate) const DETAILS_HEIGHT: f32 = 268.0;
pub(crate) const STATUS_HEIGHT: f32 = 26.0;
/// The monospace family used for diff content, hashes and logs, unless
/// `appearance.mono_font` names another one. Each platform's is the one it
/// ships, so the app never depends on a font it did not install.
pub(crate) const MONO_FONT: &str = if cfg!(target_os = "macos") {
    "Menlo"
} else if cfg!(target_os = "windows") {
    "Consolas"
} else {
    "monospace"
};
/// Thickness of every draggable panel divider.
pub(crate) const SPLITTER_WIDTH: f32 = 3.0;
pub(crate) const ERROR_WIDTH: f32 = 460.0;
pub(crate) const ERROR_HEIGHT: f32 = 264.0;

#[derive(Clone, Copy)]
pub(crate) struct Theme {
    pub(crate) bg_page: Hsla,
    pub(crate) bg_chrome: Hsla,
    pub(crate) bg_panel: Hsla,
    pub(crate) bg_list: Hsla,
    pub(crate) bg_hover: Hsla,
    pub(crate) bg_selected: Hsla,
    pub(crate) border: Hsla,
    pub(crate) border_strong: Hsla,
    pub(crate) text_primary: Hsla,
    pub(crate) text_secondary: Hsla,
    pub(crate) text_faint: Hsla,
    /// Ink on accent-filled controls. Dark, like the website's buttons —
    /// white on the accent blue fails contrast.
    pub(crate) text_on_accent: Hsla,
    pub(crate) accent: Hsla,
    pub(crate) green: Hsla,
    pub(crate) orange: Hsla,
    pub(crate) purple: Hsla,
    pub(crate) red: Hsla,
    pub(crate) cyan: Hsla,
    /// Sized by the model so a graph color index can never fall outside it.
    pub(crate) graph_lanes: [Hsla; GRAPH_COLOR_COUNT as usize],
}

impl Theme {
    /// The modal backdrop behind every overlay: dims the window without
    /// hiding it. Used by the diff, settings, actions and branch dialogs.
    #[expect(clippy::unused_self, reason = "a palette role, kept on Theme")]
    pub(crate) fn scrim(&self) -> Hsla {
        gpui::black().opacity(0.55)
    }

    /// A well sunk into whatever surface it sits on: the Actions timeline
    /// plot, a preview's code fence. Black at 18% rather than a surface of
    /// its own, so one value recesses equally over chrome, panel, and list.
    #[expect(clippy::unused_self, reason = "a palette role, kept on Theme")]
    pub(crate) fn recessed(&self) -> Hsla {
        gpui::black().opacity(0.18)
    }

    /// The divider between line numbers and code in the diff overlay —
    /// `border_strong`, so it stays visible over the add/del washes.
    pub(crate) fn gutter_rule(&self) -> Hsla {
        self.border_strong
    }

    /// Persistent outline for the hunk selected by diff navigation.
    pub(crate) fn active_hunk_border(&self) -> Hsla {
        self.accent.opacity(0.4)
    }

    /// The wash behind an added diff line. A tint over `bg_list`, never a
    /// surface of its own.
    pub(crate) fn tint_added(&self) -> Hsla {
        self.green.opacity(0.08)
    }

    /// The wash behind a removed diff line.
    pub(crate) fn tint_removed(&self) -> Hsla {
        self.red.opacity(0.08)
    }

    /// Border of a tinted badge (CURRENT, ref chips, PR chips, run chips):
    /// the badge's own color at 45%, over `chip_fill` of the same color.
    #[expect(clippy::unused_self, reason = "a palette role, kept on Theme")]
    pub(crate) fn chip_border(&self, color: Hsla) -> Hsla {
        color.opacity(0.45)
    }

    /// Fill of a tinted badge: the badge's own color at 10%.
    #[expect(clippy::unused_self, reason = "a palette role, kept on Theme")]
    pub(crate) fn chip_fill(&self, color: Hsla) -> Hsla {
        color.opacity(0.1)
    }

    /// Floating labels over media, like the image diff's Before/After
    /// chips: chrome at 85% so the picture shows through.
    pub(crate) fn hud(&self) -> Hsla {
        self.bg_chrome.opacity(0.85)
    }

    /// A splitter being dragged.
    pub(crate) fn grab_active(&self) -> Hsla {
        self.accent.opacity(0.55)
    }

    /// A splitter under the pointer.
    pub(crate) fn grab_hover(&self) -> Hsla {
        self.accent.opacity(0.35)
    }

    pub(crate) fn dark() -> Self {
        let accent = rgb(0x5b_9d_ff).into();
        let green = rgb(0x7e_c9_6f).into();
        let orange = rgb(0xe0_a4_58).into();
        let purple = rgb(0xb3_92_f0).into();
        let red = rgb(0xe5_65_5e).into();
        let cyan = rgb(0x64_c7_d6).into();
        Self {
            bg_page: rgb(0x0b_0c_0f).into(),
            bg_chrome: rgb(0x15_17_1c).into(),
            bg_panel: rgb(0x19_1c_22).into(),
            bg_list: rgb(0x1d_21_29).into(),
            bg_hover: rgb(0x22_27_34).into(),
            bg_selected: rgb(0x26_30_42).into(),
            border: rgb(0x26_2a_33).into(),
            border_strong: rgb(0x2f_35_42).into(),
            text_primary: rgb(0xdf_e3_ec).into(),
            text_secondary: rgb(0x9a_a2_b3).into(),
            text_faint: rgb(0x5f_67_7a).into(),
            text_on_accent: rgb(0x08_10_1f).into(),
            accent,
            green,
            orange,
            purple,
            red,
            cyan,
            graph_lanes: [accent, green, orange, purple, red, cyan],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn geometry_matches_the_visual_contract() {
        assert_eq!(
            (
                INITIAL_WIDTH,
                INITIAL_HEIGHT,
                SIDEBAR_WIDTH,
                HISTORY_ROW_HEIGHT,
                DETAILS_HEIGHT
            ),
            (1280.0, 800.0, 236.0, 30.0, 268.0)
        );
    }
}
