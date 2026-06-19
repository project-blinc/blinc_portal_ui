//! `PortalPainter` — the painting primitive every widget is built on.
//!
//! Holds a `&mut DrawContext`, the painter's reserved rect, and the
//! portal's wall-clock time. Free-form draw methods proxy through to
//! the underlying context in canvas-content coords; style-aware
//! helpers (`fill_self`, `stroke_self`, `text`) read [`PortalStyle`]
//! so widgets stay theme-coherent without re-deriving colours and
//! metrics each call.
//!
//! Reserved via [`crate::PortalUi::allocate_painter`] (which also
//! returns a [`Response`]) or [`crate::PortalUi::allocate_paint`]
//! (painter only, for decorative fills).

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

    /// Emit one [`blinc_core::layer::Shadow`] per layer in the stack
    /// at the painter's full bounds. MUST be called BEFORE
    /// `fill_self` so the shadow primitive sorts behind the fill.
    /// Layers paint back-to-front (deepest first) — same order as
    /// the renderer's box-shadow walker. Empty stack short-circuits
    /// so Ghost / Link / disabled variants emit no primitives.
    pub fn shadow_self(&mut self, style: &PortalStyle, stack: &[blinc_core::layer::Shadow]) {
        if stack.is_empty() {
            return;
        }
        let radius = ctrl_radius(style);
        for s in stack.iter().rev() {
            self.ctx.draw_shadow(self.rect, radius, s.clone());
        }
    }

    /// Drop shadow under a specific rect (not the painter's full
    /// bounds). Useful when a widget paints a sub-rect that's
    /// smaller than its hit region — e.g. a slider's track sitting
    /// inside a wider painter. Same paint-order rule: emit before
    /// the fill.
    pub fn shadow_rect(
        &mut self,
        rect: blinc_core::layer::Rect,
        radius: blinc_core::layer::CornerRadius,
        stack: &[blinc_core::layer::Shadow],
    ) {
        if stack.is_empty() {
            return;
        }
        for s in stack.iter().rev() {
            self.ctx.draw_shadow(rect, radius, s.clone());
        }
    }

    /// Inset (inner) shadow under the painter's full bounds. Same
    /// stack semantics as `shadow_self` but the renderer projects
    /// the shadow INTO the rect — produces a recessed-surface look
    /// (sunken track, pressed-in field).
    pub fn inner_shadow_self(
        &mut self,
        style: &PortalStyle,
        stack: &[blinc_core::layer::Shadow],
    ) {
        if stack.is_empty() {
            return;
        }
        let radius = ctrl_radius(style);
        for s in stack.iter().rev() {
            self.ctx.draw_inner_shadow(self.rect, radius, s.clone());
        }
    }

    /// Inset shadow under a specific rect with an explicit corner
    /// radius — companion to `shadow_rect` for inset.
    pub fn inner_shadow_rect(
        &mut self,
        rect: blinc_core::layer::Rect,
        radius: blinc_core::layer::CornerRadius,
        stack: &[blinc_core::layer::Shadow],
    ) {
        if stack.is_empty() {
            return;
        }
        for s in stack.iter().rev() {
            self.ctx.draw_inner_shadow(rect, radius, s.clone());
        }
    }

    /// Radially symmetric drop shadow under a circle. Sequence
    /// rule: emit BEFORE the circle fill so the shadow sorts
    /// behind. Used by switch thumbs / slider thumbs that want a
    /// subtle lift off the track surface.
    pub fn circle_shadow(
        &mut self,
        center: Point,
        radius: f32,
        stack: &[blinc_core::layer::Shadow],
    ) {
        if stack.is_empty() {
            return;
        }
        for s in stack.iter().rev() {
            self.ctx.draw_circle_shadow(center, radius, s.clone());
        }
    }

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
/// from the canvas kit's interaction state.
///
/// Public so custom widget authors who replicate
/// [`crate::PortalUi::allocate_painter`]'s response shape (for instance
/// to feed in a synthesized hover state from a non-pointer source) can
/// reuse the exact construction the built-in widgets get.
#[allow(clippy::too_many_arguments)]
pub fn build_response(
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
        // Filled in by `allocate_painter_with_key` after the kit's
        // hit-region is registered; callers using `build_response`
        // directly get the default.
        widget_id: Default::default(),
        pip_clicked: false,
    }
}
