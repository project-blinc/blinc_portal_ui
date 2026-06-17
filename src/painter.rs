//! `PortalPainter` — the painting primitive every widget is built on.
//! Free-form draw + style-aware helpers. See `README.md`.

use crate::core::{ctrl_radius, PortalStyle, Response};
use blinc_core::draw::{Path, Stroke, TextStyle};
use blinc_core::layer::{Brush, CornerRadius, Point, Rect};
use blinc_core::{DrawContext, FontWeight, TextAlign, TextBaseline};

/// Drawable region scoped to a rect reserved via
/// [`crate::PortalUi::allocate_painter`].
///
/// Methods proxy to the underlying [`DrawContext`] in canvas-content
/// coords. The painter does NOT push its own clip stack frame by
/// default — `allocate_painter_clipped` opt-in handles that.
/// `local_to_world` / `world_to_local` convert between the painter's
/// (0..w, 0..h) frame and canvas-content space; useful for widgets
/// (and sketch surfaces) that prefer to think in local coords.
pub struct PortalPainter<'a> {
    pub(crate) ctx: &'a mut dyn DrawContext,
    pub(crate) rect: Rect,
    pub(crate) time_s: f32,
}

impl<'a> PortalPainter<'a> {
    /// Outer rect in canvas-content coordinates.
    pub fn rect(&self) -> Rect {
        self.rect
    }

    /// Painter size in pixels (width, height).
    pub fn size(&self) -> (f32, f32) {
        (self.rect.width(), self.rect.height())
    }

    /// Portal clock — monotonic seconds since the portal was created.
    /// Useful for time-based painting (sparklines with phase,
    /// pulsating decorations, etc.).
    pub fn time(&self) -> f32 {
        self.time_s
    }

    /// Convert painter-local `(0..w, 0..h)` to canvas-content coords.
    pub fn local_to_world(&self, local: Point) -> Point {
        Point::new(self.rect.x() + local.x, self.rect.y() + local.y)
    }

    /// Inverse of [`Self::local_to_world`].
    pub fn world_to_local(&self, world: Point) -> Point {
        Point::new(world.x - self.rect.x(), world.y - self.rect.y())
    }

    // ── Pass-through draw operations ──

    pub fn fill_rect(&mut self, rect: Rect, radius: CornerRadius, brush: Brush) {
        self.ctx.fill_rect(rect, radius, brush);
    }

    pub fn stroke_rect(&mut self, rect: Rect, radius: CornerRadius, stroke: &Stroke, brush: Brush) {
        self.ctx.stroke_rect(rect, radius, stroke, brush);
    }

    pub fn fill_circle(&mut self, c: Point, r: f32, brush: Brush) {
        self.ctx.fill_circle(c, r, brush);
    }

    pub fn stroke_circle(&mut self, c: Point, r: f32, stroke: &Stroke, brush: Brush) {
        self.ctx.stroke_circle(c, r, stroke, brush);
    }

    pub fn fill_path(&mut self, path: &Path, brush: Brush) {
        self.ctx.fill_path(path, brush);
    }

    pub fn stroke_path(&mut self, path: &Path, stroke: &Stroke, brush: Brush) {
        self.ctx.stroke_path(path, stroke, brush);
    }

    pub fn draw_text(&mut self, text: &str, style: &TextStyle, origin: Point) {
        self.ctx.draw_text(text, origin, style);
    }

    /// Draw text at the painter's centre using `style`. Convenience
    /// over `draw_text` + centre arithmetic.
    pub fn draw_text_centered(&mut self, text: &str, style: &TextStyle) {
        let cx = self.rect.x() + self.rect.width() * 0.5;
        let cy = self.rect.y() + self.rect.height() * 0.5;
        let mut s = style.clone();
        s.align = TextAlign::Center;
        s.baseline = TextBaseline::Middle;
        self.ctx.draw_text(text, Point::new(cx, cy), &s);
    }

    // ── Style-aware convenience helpers used by built-in widgets ──

    /// Filled rounded rect at the painter's full bounds, with control
    /// radius from `style`. The single most-painted shape in the
    /// portal — every button / field / slider track uses it.
    pub fn fill_self(&mut self, style: &PortalStyle, brush: Brush) {
        self.ctx.fill_rect(self.rect, ctrl_radius(style), brush);
    }

    /// Stroked rounded rect at the painter's full bounds.
    pub fn stroke_self(&mut self, style: &PortalStyle, stroke: &Stroke, brush: Brush) {
        self.ctx
            .stroke_rect(self.rect, ctrl_radius(style), stroke, brush);
    }

    /// Plain-text glyph stroke at the given origin in
    /// canvas-content coords, using the style's text colour + font.
    pub fn text(
        &mut self,
        text: &str,
        origin: Point,
        style: &PortalStyle,
        color: blinc_core::layer::Color,
    ) {
        let s = TextStyle {
            family: "system-ui".to_string(),
            size: style.font_size,
            weight: FontWeight::Regular,
            color,
            align: TextAlign::Left,
            baseline: TextBaseline::Alphabetic,
            line_height: style.line_height / style.font_size.max(1.0),
            letter_spacing: 0.0,
        };
        self.ctx.draw_text(text, origin, &s);
    }
}

/// Build the standard [`Response`] for a freshly-allocated painter
/// from the canvas kit's interaction state. The portal's render-loop
/// constructs this once for every `allocate_painter` call after the
/// rect is chosen.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_response(
    rect: Rect,
    region_hovered: bool,
    region_active: bool,
    region_clicked_this_frame: bool,
    pointer_content: Option<Point>,
    drag_delta_content: Point,
) -> Response {
    let pointer_local = pointer_content
        .filter(|_| region_hovered || region_active)
        .map(|p| Point::new(p.x - rect.x(), p.y - rect.y()));
    Response {
        rect,
        hovered: region_hovered,
        pressed: region_active,
        clicked: region_clicked_this_frame,
        changed: false,
        animating: false,
        pointer_local,
        drag_delta_local: drag_delta_content,
    }
}
