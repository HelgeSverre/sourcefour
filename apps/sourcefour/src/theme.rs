use gpui::{Hsla, rgb};

pub(crate) const INITIAL_WIDTH: f32 = 1280.0;
pub(crate) const INITIAL_HEIGHT: f32 = 800.0;
pub(crate) const MINIMUM_WIDTH: f32 = 900.0;
pub(crate) const MINIMUM_HEIGHT: f32 = 560.0;
pub(crate) const TITLEBAR_HEIGHT: f32 = 38.0;
pub(crate) const TOOLBAR_HEIGHT: f32 = 46.0;
pub(crate) const SIDEBAR_WIDTH: f32 = 236.0;
pub(crate) const HEADER_HEIGHT: f32 = 26.0;
pub(crate) const HISTORY_ROW_HEIGHT: f32 = 30.0;
pub(crate) const GRAPH_WIDTH: f32 = 76.0;
pub(crate) const DETAILS_HEIGHT: f32 = 268.0;
pub(crate) const STATUS_HEIGHT: f32 = 26.0;

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
    pub(crate) accent: Hsla,
    pub(crate) green: Hsla,
    pub(crate) orange: Hsla,
    pub(crate) purple: Hsla,
    pub(crate) red: Hsla,
    pub(crate) cyan: Hsla,
    pub(crate) graph_lanes: [Hsla; 6],
}

impl Theme {
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
