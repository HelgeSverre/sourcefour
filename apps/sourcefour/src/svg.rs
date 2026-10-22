//! Bounded SVG rasterization shared by standalone and embedded previews.

use resvg::{tiny_skia, usvg};
use std::sync::{Arc, LazyLock};

const MAX_DIMENSION: u16 = 2048;
pub(crate) const MAX_SOURCE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RenderFailure {
    TooLarge,
    InvalidSvg,
    InvalidSize,
    InvalidEncoding,
    Encoding,
}

impl RenderFailure {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::TooLarge => "SVG preview exceeds the 8 MiB source limit. Inspect it in Source.",
            Self::InvalidSvg => "This SVG could not be rendered. Inspect it in Source.",
            Self::InvalidSize => "This SVG has unsupported dimensions. Inspect it in Source.",
            Self::InvalidEncoding => "This SVG is not valid UTF-8. Inspect it in Source.",
            Self::Encoding => "The SVG preview image could not be created. Inspect it in Source.",
        }
    }
}

pub(crate) fn render_svg(source: &str) -> Result<Arc<gpui::Image>, RenderFailure> {
    let pixels = rasterize(source)?;
    let png = pixels.encode_png().map_err(|_| RenderFailure::Encoding)?;
    Ok(Arc::new(gpui::Image::from_bytes(
        gpui::ImageFormat::Png,
        png,
    )))
}

/// Encoded pixels and the SVG's logical size are independent: a 20px badge
/// gets crisp pixels at 2x without growing into a full-width illustration.
pub(crate) struct EmbeddedSvg {
    pub(crate) image: Arc<gpui::Image>,
    pub(crate) size: gpui::Size<f32>,
}

pub(crate) fn image_from_bytes(bytes: &[u8]) -> Result<EmbeddedSvg, RenderFailure> {
    let source = std::str::from_utf8(bytes).map_err(|_| RenderFailure::InvalidEncoding)?;
    let (pixels, size) = rasterize_at_scale(source, true)?;
    let png = pixels.encode_png().map_err(|_| RenderFailure::Encoding)?;
    Ok(EmbeddedSvg {
        image: Arc::new(gpui::Image::from_bytes(gpui::ImageFormat::Png, png)),
        size,
    })
}

pub(crate) fn rasterize(source: &str) -> Result<tiny_skia::Pixmap, RenderFailure> {
    rasterize_at_scale(source, false).map(|(pixels, _)| pixels)
}

/// Parsing and rasterization happen once on a worker, never during a UI frame.
fn rasterize_at_scale(
    source: &str,
    embedded: bool,
) -> Result<(tiny_skia::Pixmap, gpui::Size<f32>), RenderFailure> {
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
    let limit = f32::from(MAX_DIMENSION) / size.width().max(size.height());
    let scale = if embedded { limit.min(2.0) } else { limit };
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
    Ok((pixels, gpui::size(size.width(), size.height())))
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "pixel dimensions are clamped to 1..=2048"
)]
fn pixel_dimension(value: f32) -> u32 {
    value.ceil().clamp(1.0, f32::from(MAX_DIMENSION)) as u32
}
