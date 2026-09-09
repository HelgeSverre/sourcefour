//! Static SVG previews from the exact text revisions already loaded by the diff.

use std::sync::{Arc, LazyLock};

use gpui::{Div, div, prelude::*, px};
use resvg::{tiny_skia, usvg};
use sourcefour_model::{DiffContent, RepoPath, TextDiff, TextSide};

use super::{
    SourcefourWindow,
    diff::{DiffView, render_image},
};

/// Each side uses at most 16 MiB for its output pixels, regardless of SVG dimensions.
const MAX_DIMENSION: u16 = 2048;
const MAX_SOURCE_BYTES: usize = 8 * 1024 * 1024;

pub(super) fn canvas() -> gpui::Rgba {
    gpui::rgb(0x00f0_f0f0)
}

pub(super) enum SvgPreview {
    Loading,
    Ready { before: SvgSide, after: SvgSide },
}

pub(super) enum SvgSide {
    Absent,
    Rendered(Arc<gpui::Image>),
    Unavailable(RenderFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RenderFailure {
    TooLarge,
    InvalidSvg,
    InvalidSize,
    InvalidEncoding,
    Encoding,
}

impl RenderFailure {
    fn message(self) -> &'static str {
        match self {
            Self::TooLarge => "SVG preview exceeds the 8 MiB source limit. Inspect it in Source.",
            Self::InvalidSvg => "This SVG could not be rendered. Inspect it in Source.",
            Self::InvalidSize => "This SVG has unsupported dimensions. Inspect it in Source.",
            Self::InvalidEncoding => "This SVG is not valid UTF-8. Inspect it in Source.",
            Self::Encoding => "The SVG preview image could not be created. Inspect it in Source.",
        }
    }
}

pub(super) fn is_svg(path: &RepoPath) -> bool {
    path.0
        .get(path.0.len().saturating_sub(4)..)
        .is_some_and(|extension| extension.eq_ignore_ascii_case(b".svg"))
}

pub(super) fn applies(view: &DiffView) -> bool {
    is_svg(view.origin.path()) && matches!(view.content, Some(DiffContent::Text(_)))
}

pub(super) fn showing(view: &DiffView) -> bool {
    view.show_preview && applies(view)
}

fn render_pair(diff: &TextDiff) -> SvgPreview {
    SvgPreview::Ready {
        before: render_side(diff.old.as_ref()),
        after: render_side(diff.new.as_ref()),
    }
}

fn render_side(side: Option<&TextSide>) -> SvgSide {
    let Some(side) = side else {
        return SvgSide::Absent;
    };
    if side.decoding() != sourcefour_model::TextDecoding::Utf8 {
        return SvgSide::Unavailable(RenderFailure::InvalidEncoding);
    }
    match render_svg(side.text()) {
        Ok(image) => SvgSide::Rendered(image),
        Err(error) => SvgSide::Unavailable(error),
    }
}

fn render_svg(source: &str) -> Result<Arc<gpui::Image>, RenderFailure> {
    let pixels = rasterize(source)?;
    let png = pixels.encode_png().map_err(|_| RenderFailure::Encoding)?;
    render_image(Some(&png), "png").ok_or(RenderFailure::Encoding)
}

/// The same bounded renderer for SVG images embedded in Markdown previews.
pub(super) fn image_from_bytes(bytes: &[u8]) -> Result<Arc<gpui::Image>, RenderFailure> {
    let source = std::str::from_utf8(bytes).map_err(|_| RenderFailure::InvalidEncoding)?;
    render_svg(source)
}

/// Parsing and rasterization happen once on a worker, never during a UI frame.
fn rasterize(source: &str) -> Result<tiny_skia::Pixmap, RenderFailure> {
    static FONTS: LazyLock<Arc<usvg::fontdb::Database>> = LazyLock::new(|| {
        let mut fonts = usvg::fontdb::Database::new();
        fonts.load_system_fonts();
        Arc::new(fonts)
    });
    if source.len() > MAX_SOURCE_BYTES {
        return Err(RenderFailure::TooLarge);
    }
    let select_font = usvg::FontResolver::default_font_selector();
    let options = usvg::Options {
        font_resolver: usvg::FontResolver {
            select_font: Box::new(move |font, database| {
                // Path-only icons need no font scan. Text loads the shared
                // system database lazily, on the same background worker.
                if database.is_empty() {
                    *database = Arc::clone(&FONTS);
                }
                select_font(font, database)
            }),
            select_fallback: usvg::FontResolver::default_fallback_selector(),
        },
        // Repository SVGs may embed data images, but cannot read arbitrary
        // filesystem paths or fetch network resources while being previewed.
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_string: Box::new(|_, _| None),
            ..usvg::ImageHrefResolver::default()
        },
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_str(source, &options).map_err(|_| RenderFailure::InvalidSvg)?;
    let size = tree.size();
    let scale = f32::from(MAX_DIMENSION) / size.width().max(size.height());
    if !scale.is_finite() || scale <= 0.0 {
        return Err(RenderFailure::InvalidSize);
    }
    let (width, height) = (
        pixel_dimension(size.width() * scale),
        pixel_dimension(size.height() * scale),
    );
    let mut pixels = tiny_skia::Pixmap::new(width, height).ok_or(RenderFailure::InvalidSize)?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixels.as_mut(),
    );
    Ok(pixels)
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "pixel dimensions are clamped to 1..=2048"
)]
fn pixel_dimension(value: f32) -> u32 {
    value.ceil().clamp(1.0, f32::from(MAX_DIMENSION)) as u32
}

impl SourcefourWindow {
    pub(super) fn load_svg_preview(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(view) = &mut self.diff_view else {
            return;
        };
        if !showing(view) || view.svg_preview.is_some() {
            return;
        }
        let Some(DiffContent::Text(diff)) = &view.content else {
            return;
        };
        let diff = Arc::clone(diff);
        view.svg_preview = Some(SvgPreview::Loading);
        let token = self.diff_request;
        cx.spawn(async move |this, cx| {
            let preview = cx
                .background_executor()
                .spawn(async move { render_pair(&diff) })
                .await;
            this.update(cx, |this, cx| {
                this.set_svg_preview(token, preview, cx);
            })
            .ok();
        })
        .detach();
    }

    fn set_svg_preview(&mut self, token: u64, preview: SvgPreview, cx: &mut gpui::Context<Self>) {
        if self.diff_request != token {
            return;
        }
        if let Some(view) = &mut self.diff_view {
            view.svg_preview = Some(preview);
        }
        cx.notify();
    }

    pub(super) fn svg_preview_body(&self, view: &DiffView) -> Div {
        match &view.svg_preview {
            None | Some(SvgPreview::Loading) => self.diff_notice("Rendering SVG preview…"),
            Some(SvgPreview::Ready { before, after }) => div()
                .size_full()
                .flex()
                .child(self.svg_side("Before", before))
                .child(div().w(px(1.0)).flex_none().bg(self.theme.border))
                .child(self.svg_side("After", after)),
        }
    }

    fn svg_side(&self, label: &'static str, side: &SvgSide) -> Div {
        let (image, message) = match side {
            SvgSide::Absent => (None, "This SVG is not present on this side."),
            SvgSide::Rendered(image) => (Some(Arc::clone(image)), ""),
            SvgSide::Unavailable(error) => (None, error.message()),
        };
        // A light canvas keeps the default black/currentColor of icons visible.
        let has_image = image.is_some();
        self.image_pane(label, image, message, None)
            .when(has_image, |pane| pane.bg(canvas()))
    }
}

#[cfg(test)]
mod tests {
    use gpui::{AppContext, Entity, Render, TestAppContext, VisualTestContext};
    use sourcefour_model::{ChangeKind, ChangedFile, DiffPaths, TextDecoding};
    use sourcefour_test_support::TempRepo;

    use super::*;
    use crate::{app::WindowLaunch, demo::Scene, ui_state::UiState, views::diff::DiffMode};

    const RED: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 10"><path fill="red" d="M0 0H10V10H0Z"/></svg>"#;
    const BLUE: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 10"><path fill="blue" d="M0 0H10V10H0Z"/></svg>"#;

    #[test]
    fn svg_rasterization_preserves_colour_transparency_and_viewbox_aspect_ratio() {
        let pixels = rasterize(RED).expect("valid SVG renders");
        assert_eq!((pixels.width(), pixels.height()), (2048, 1024));
        let red = pixels.pixel(100, 100).expect("inside filled path");
        assert_eq!(
            (red.red(), red.green(), red.blue(), red.alpha()),
            (255, 0, 0, 255)
        );
        assert_eq!(pixels.pixel(1900, 100).expect("outside path").alpha(), 0);
        let image = render_svg(RED).expect("PNG wrapper");
        assert_eq!(image.format, gpui::ImageFormat::Png);
        assert!(image.bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
    }

    #[test]
    fn svg_dimensions_are_bounded_and_bad_source_is_reported() {
        let pixels =
            rasterize(r#"<svg xmlns="http://www.w3.org/2000/svg" width="40000" height="20000"/>"#)
                .expect("large canvas is scaled down");
        assert_eq!((pixels.width(), pixels.height()), (2048, 1024));
        assert_eq!(rasterize("<svg>").err(), Some(RenderFailure::InvalidSvg));
        assert_eq!(rasterize("").err(), Some(RenderFailure::InvalidSvg));
        assert_eq!(
            rasterize(&" ".repeat(MAX_SOURCE_BYTES + 1)).err(),
            Some(RenderFailure::TooLarge)
        );
    }

    #[test]
    fn svg_absence_invalid_encoding_and_invalid_markup_are_distinct() {
        assert!(matches!(render_side(None), SvgSide::Absent));
        let invalid = TextSide::new(String::from("broken"), TextDecoding::Utf8);
        assert!(matches!(
            render_side(Some(&invalid)),
            SvgSide::Unavailable(RenderFailure::InvalidSvg)
        ));
        let lossy = TextSide::new(RED.to_owned(), TextDecoding::LossyUtf8);
        assert!(matches!(
            render_side(Some(&lossy)),
            SvgSide::Unavailable(RenderFailure::InvalidEncoding)
        ));
        let empty = TextSide::new(String::new(), TextDecoding::Utf8);
        assert!(matches!(
            render_side(Some(&empty)),
            SvgSide::Unavailable(RenderFailure::InvalidSvg)
        ));
    }

    #[test]
    fn svg_preview_uses_the_staged_revision_after_the_worktree_changes()
    -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        std::fs::write(repository.path().join("icon.svg"), RED)?;
        repository.git(&["add", "icon.svg"]);
        repository.commit("red icon");
        std::fs::write(repository.path().join("icon.svg"), BLUE)?;
        repository.git(&["add", "icon.svg"]);
        let paths = DiffPaths::for_file(&file("icon.svg")).expect("two paths");
        let content = sourcefour_git::worktree_file_diff(
            &sourcefour_git::discover(repository.path())?,
            &paths,
            true,
            None,
        )?;
        std::fs::write(repository.path().join("icon.svg"), "new unsaved SVG edit")?;
        let DiffContent::Text(diff) = content else {
            panic!("SVG retains its source diff");
        };
        assert_eq!(diff.old.as_ref().expect("old").text(), RED);
        assert_eq!(diff.new.as_ref().expect("new").text(), BLUE);
        let SvgPreview::Ready {
            before: SvgSide::Rendered(before),
            after: SvgSide::Rendered(after),
        } = render_pair(&diff)
        else {
            panic!("both staged revisions render");
        };
        assert_ne!(before.bytes, after.bytes);
        Ok(())
    }

    fn file(name: &str) -> ChangedFile {
        ChangedFile {
            old_path: Some(RepoPath(name.as_bytes().to_vec())),
            new_path: Some(RepoPath(name.as_bytes().to_vec())),
            status: ChangeKind::Modified,
            additions: None,
            deletions: None,
            is_binary: false,
        }
    }

    fn diff_view(name: &str) -> DiffView {
        let file = file(name);
        let mut view = DiffView::for_working_tree(
            DiffPaths::for_file(&file).expect("paths"),
            true,
            file.status,
            vec![file].into(),
            0,
            DiffMode::Unified,
        );
        view.replace_content(sourcefour_git::unified(
            RED.as_bytes(),
            BLUE.as_bytes(),
            sourcefour_git::DiffLimits::default(),
        ));
        view
    }

    #[test]
    fn svg_preview_is_default_and_the_source_diff_stays_available() {
        for name in ["icon.svg", "ICONS/ICON.SVG", "with spaces/icon.SvG"] {
            let mut view = diff_view(name);
            assert!(showing(&view));
            assert!(super::super::preview::applies(&view));
            view.show_preview = false;
            assert!(!showing(&view));
            assert!(matches!(view.content, Some(DiffContent::Text(_))));
        }
        for name in ["svg", "icon.svg.txt", "icon.svg/readme.md", "icon.png"] {
            assert!(!applies(&diff_view(name)));
        }
    }

    struct PreviewFixture(Entity<SourcefourWindow>);

    impl Render for PreviewFixture {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            self.0.update(cx, |view, cx| {
                let diff = view.diff_view.as_ref().expect("fixture diff");
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(view.preview_toggle(diff.show_preview, cx))
                    .child(div().flex_1().min_h(px(0.0)).child(view.diff_body(
                        diff,
                        view.diff_rows_len(),
                        cx,
                    )))
            })
        }
    }

    fn window(cx: &mut TestAppContext) -> (Entity<PreviewFixture>, &mut VisualTestContext) {
        cx.add_window_view(|window, cx| {
            PreviewFixture(cx.new(|cx| {
                let mut view = SourcefourWindow::new(
                    WindowLaunch {
                        demo: true,
                        name: String::from("SVG preview"),
                        path: String::new(),
                        location: None,
                        scene: Scene::Overview,
                    },
                    &UiState::default(),
                    window,
                    cx,
                );
                view.diff_view = Some(diff_view("icon.svg"));
                view
            }))
        })
    }

    fn draw(fixture: &Entity<PreviewFixture>, cx: &mut VisualTestContext) {
        cx.draw(
            gpui::Point::default(),
            gpui::size(px(1200.0), px(800.0)),
            |_, _| fixture.clone().into_any_element(),
        );
    }

    #[gpui::test]
    fn svg_preview_loads_off_thread_and_is_reused_when_toggling_source(cx: &mut TestAppContext) {
        let (fixture, cx) = window(cx);
        let view = cx.update(|_, cx| fixture.read(cx).0.clone());
        view.update(cx, SourcefourWindow::load_svg_preview);
        draw(&fixture, cx);
        cx.run_until_parked();
        draw(&fixture, cx);
        let image_id = view.update(cx, |view, cx| {
            let SvgPreview::Ready {
                after: SvgSide::Rendered(image),
                ..
            } = view
                .diff_view
                .as_ref()
                .expect("diff")
                .svg_preview
                .as_ref()
                .expect("loaded preview")
            else {
                panic!("SVG renders");
            };
            let id = image.id;
            view.toggle_preview(false, cx);
            assert!(view.diff_rows_len() > 0, "Source still has diff lines");
            id
        });
        draw(&fixture, cx);
        view.update(cx, |view, cx| {
            view.toggle_preview(true, cx);
            assert_eq!(view.diff_rows_len(), 0, "preview has no text scrollbar");
        });
        cx.run_until_parked();
        draw(&fixture, cx);
        view.update(cx, |view, _| {
            let SvgPreview::Ready {
                after: SvgSide::Rendered(image),
                ..
            } = view
                .diff_view
                .as_ref()
                .expect("diff")
                .svg_preview
                .as_ref()
                .expect("loaded preview")
            else {
                panic!("preview stays loaded");
            };
            assert_eq!(image.id, image_id, "toggle reuses the rendered image");
        });
    }

    #[gpui::test]
    fn svg_preview_from_a_previous_file_is_discarded(cx: &mut TestAppContext) {
        let (fixture, cx) = window(cx);
        let view = cx.update(|_, cx| fixture.read(cx).0.clone());
        view.update(cx, |view, cx| {
            view.diff_request = 2;
            view.set_svg_preview(
                1,
                SvgPreview::Ready {
                    before: SvgSide::Absent,
                    after: SvgSide::Absent,
                },
                cx,
            );
            assert!(view.diff_view.as_ref().expect("diff").svg_preview.is_none());
        });
    }
}
