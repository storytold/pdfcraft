//! Tile rendering, completion delivery, and pixel hand-off.

use pdfcraft_render::{PageRenderer, ReadingColors, ReadingParams, RenderConfig, RenderRequest, Tile};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// Rectangular tile region in device pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct FfiTile {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Reading colors preset mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FfiReadingColorsMode {
    Off,
    BlackOnWhite,
    WhiteOnBlack,
    YellowOnBlack,
    GreenOnBlack,
    Custom,
}

/// Reading colors settings for accessible / night reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct FfiReadingColors {
    pub mode: FfiReadingColorsMode,
    /// Custom text color in 0x00RRGGBB format.
    pub custom_text_rgb: Option<u32>,
    /// Custom background color in 0x00RRGGBB format.
    pub custom_background_rgb: Option<u32>,
    pub only_black: bool,
    pub line_art: bool,
}

/// Request to render a full page or a specific tile.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FfiRenderRequest {
    /// 0-based page index.
    pub page: u32,
    /// Sub-region in device pixels. If `None`, the full page is rendered.
    pub tile: Option<FfiTile>,
    /// Device pixels per PDF point (e.g., 2.0 for Retina @2x).
    pub scale: f32,
    /// Caller-defined generation tag, echoed back in the result for cancellation checks.
    pub tag: u64,
    /// Low-detail preview flag (for rapid scrolling/thumbnails).
    pub preview: bool,
    /// Reading colors override (e.g. night reading / dark mode).
    pub reading_colors: Option<FfiReadingColors>,
    /// Hide markup comments/annotations while keeping form fields and links.
    pub hide_comments: bool,
}

impl Default for FfiRenderRequest {
    fn default() -> Self {
        Self { page: 0, tile: None, scale: 1.0, tag: 0, preview: false, reading_colors: None, hide_comments: false }
    }
}

/// Completed render raster.
#[derive(Debug, Clone, uniffi::Record)]
pub struct FfiRenderResult {
    /// 0-based page index.
    pub page: u32,
    /// Pixel width of the returned bitmap.
    pub width: u32,
    /// Pixel height of the returned bitmap.
    pub height: u32,
    /// Echoed tag from the request.
    pub tag: u64,
    /// Premultiplied RGBA8 pixel bytes, row-major (length = width * height * 4).
    pub pixels: Vec<u8>,
    /// Error message if rendering failed.
    pub error: Option<String>,
    /// Time taken to render in milliseconds.
    pub render_time_ms: u32,
}

/// Callback interface implemented by native shells (Swift/Kotlin) for async completions.
#[uniffi::export(callback_interface)]
pub trait RenderCallback: Send + Sync + 'static {
    fn on_render_complete(&self, result: FfiRenderResult);
}

fn map_reading_colors(ffi: &FfiReadingColors) -> ReadingColors {
    match ffi.mode {
        FfiReadingColorsMode::Off => ReadingColors::default(),
        FfiReadingColorsMode::BlackOnWhite => ReadingColors::BLACK_ON_WHITE,
        FfiReadingColorsMode::WhiteOnBlack => ReadingColors::WHITE_ON_BLACK,
        FfiReadingColorsMode::YellowOnBlack => ReadingColors::YELLOW_ON_BLACK,
        FfiReadingColorsMode::GreenOnBlack => ReadingColors::GREEN_ON_BLACK,
        FfiReadingColorsMode::Custom => {
            let text_rgb = ffi.custom_text_rgb.unwrap_or(0x000000);
            let bg_rgb = ffi.custom_background_rgb.unwrap_or(0xffffff);
            let text = [((text_rgb >> 16) & 0xff) as u8, ((text_rgb >> 8) & 0xff) as u8, (text_rgb & 0xff) as u8];
            let background = [((bg_rgb >> 16) & 0xff) as u8, ((bg_rgb >> 8) & 0xff) as u8, (bg_rgb & 0xff) as u8];
            ReadingColors { params: Some(ReadingParams { text, background, only_black: ffi.only_black, line_art: ffi.line_art }) }
        }
    }
}

pub fn render_tile_sync(bytes: Arc<Vec<u8>>, password: Option<Arc<str>>, req: &FfiRenderRequest) -> FfiRenderResult {
    let start = Instant::now();
    let reading_colors = req.reading_colors.as_ref().map(map_reading_colors).unwrap_or_default();
    let config = RenderConfig { password, hide_comments: req.hide_comments, reading_colors, ..Default::default() };

    let mut renderer = PageRenderer::new(bytes, config);
    let internal_req = RenderRequest {
        page: req.page as usize,
        kind: pdfcraft_render::RequestKind::Pixels,
        tile: req.tile.map(|t| Tile { x: t.x, y: t.y, w: t.width, h: t.height }),
        scale: req.scale,
        tag: req.tag,
        preview: req.preview,
    };

    let rendered = renderer.render(internal_req);
    let elapsed = start.elapsed().as_millis().min(u32::MAX as u128) as u32;

    FfiRenderResult {
        page: req.page,
        width: rendered.width,
        height: rendered.height,
        tag: req.tag,
        pixels: rendered.rgba.to_vec(),
        error: rendered.error,
        render_time_ms: elapsed,
    }
}

/// State for tracking tag cancellation.
#[derive(Debug, Default)]
pub struct CancellationTracker {
    retired_before_tag: AtomicU64,
}

impl CancellationTracker {
    pub fn new() -> Self {
        Self { retired_before_tag: AtomicU64::new(0) }
    }

    pub fn retire_older_than(&self, tag: u64) {
        self.retired_before_tag.fetch_max(tag, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self, tag: u64) -> bool {
        tag < self.retired_before_tag.load(Ordering::SeqCst)
    }
}
