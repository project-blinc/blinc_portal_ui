//! Core value types every portal frame moves through.
//!
//! - [`PortalId`] / [`WidgetId`] — stable hash-derived identifiers that
//!   correlate widget state across frames and namespace canvas-kit hit
//!   regions so multiple portals never alias on shared keys.
//! - [`Response`] — what the host gets back from each widget /
//!   `allocate_painter` call: rect + hover / press / click / drag /
//!   pointer-local / animating bits. Built once per allocation; custom
//!   widgets compose it via [`Response::with_clicked`] etc.
//! - [`Sense`] — the kind of pointer interaction a rect wants, defaulting
//!   to [`Sense::None`] for paint-only regions.
//! - [`PortalStorage`] — typed-`Any` scratch cells keyed by [`WidgetId`],
//!   GC'd at the end of each frame so dropped widgets don't leak.
//! - [`PortalStyle`] — theme-derived constants (colours, metrics) the
//!   built-in widgets read each frame.
//! - [`HostBridge`] — coordinate transforms (canvas ↔ screen) so widgets
//!   can anchor overlays outside the canvas surface.

use blinc_core::layer::{Color, CornerRadius, Point, Rect};
use std::any::Any;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

// ─────────────────────────────────────────────────────────────────────
// Ids
// ─────────────────────────────────────────────────────────────────────

/// Stable identifier for a portal instance. Used as the prefix for
/// every hit-region id the portal registers with
/// [`blinc_canvas_kit::CanvasKit`], so different portals never collide
/// on shared widget keys (every node in a node-editor host gets its
/// own `PortalId`).
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct PortalId(pub u64);

impl PortalId {
    /// Derive a `PortalId` from any hashable host key. Hosts typically
    /// pass their stable node / instance id; the portal manager hashes
    /// it once and stores the result.
    pub fn from_hashed<H: Hash + ?Sized>(key: &H) -> Self {
        let mut h = ahash::AHasher::default();
        key.hash(&mut h);
        PortalId(h.finish())
    }
}

/// Per-frame identifier for a widget. Derived from the portal's
/// `id_stack` (a hash chain through every layout / group enter, plus
/// the widget's caller-supplied key when provided) — stable across
/// frames as long as the closure issues widget calls in the same
/// order. Used both as a [`PortalStorage`] key (so a slider keeps its
/// drag offset between frames) and as the suffix on the hit-region id
/// registered with the canvas kit.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct WidgetId(pub u64);

impl WidgetId {
    /// Build a region id string the canvas kit can ingest. Format:
    /// `portal_<portal_id_hex>_w_<widget_id_hex>`. Stable so consecutive
    /// frames can correlate `kit.interaction()` results back to widgets.
    pub fn to_region_id(self, portal: PortalId) -> String {
        format!("portal_{:016x}_w_{:016x}", portal.0, self.0)
    }
}

// ─────────────────────────────────────────────────────────────────────
// Response
// ─────────────────────────────────────────────────────────────────────

/// What the host wants to know about how the user interacted with the
/// region a widget (or [`crate::PortalPainter`]) reserved this frame.
///
/// All fields are derived from the canvas kit's
/// [`InteractionState`](blinc_canvas_kit::InteractionState) and the
/// portal's per-frame "was clicked this frame" cache; widgets compose
/// them into their own visuals (hover ease, pressed scale) and may
/// also drive caller logic (`if resp.clicked { … }`).
#[derive(Clone, Debug)]
pub struct Response {
    /// Outer rect of the widget in canvas-content coordinates. Useful
    /// for anchoring popovers (transform via
    /// [`HostBridge::canvas_to_screen`] before passing to the overlay
    /// manager).
    pub rect: Rect,
    /// Cursor is currently over the widget's rect.
    pub hovered: bool,
    /// Cursor went down on the widget and hasn't released yet (i.e.
    /// the widget is the kit's `active` region).
    pub pressed: bool,
    /// The user completed a click (down + up over the widget) DURING
    /// THIS FRAME. Only true for one frame per click.
    pub clicked: bool,
    /// The widget mutated the value it owns this frame (typed text,
    /// dragged the slider, toggled the switch). Read by callers using
    /// the plain `&mut value` widget forms to know when to persist
    /// state.
    pub changed: bool,
    /// The widget is mid-animation and the portal should request
    /// another frame. Hover ease, press spring, etc. The portal ORs
    /// the per-frame `animating` set; if any is true, the portal
    /// flags itself dirty.
    pub animating: bool,
    /// Cursor position in widget-local coords (origin = `rect.top_left`)
    /// when `hovered` is true. Useful for painters: a sketch surface
    /// reads this to know where the user is drawing.
    pub pointer_local: Option<Point>,
    /// Drag delta in widget-local coords since the previous frame.
    /// Zero when `pressed` is false. Sliders consume this to advance
    /// the value; painters consume it for free-form draw strokes.
    pub drag_delta_local: Point,
}

impl Response {
    /// Used internally by widgets that don't actually reserve a region
    /// (e.g. `spacing`). Returns an empty rect at `(0, 0)` with all
    /// flags false.
    pub fn empty() -> Self {
        Self {
            rect: Rect::new(0.0, 0.0, 0.0, 0.0),
            hovered: false,
            pressed: false,
            clicked: false,
            changed: false,
            animating: false,
            pointer_local: None,
            drag_delta_local: Point::new(0.0, 0.0),
        }
    }

    /// Override the rect — useful when a custom widget allocates a
    /// painter then narrows the hit / response region to a subset
    /// (e.g. circular thumbs inside square painters).
    pub fn with_rect(mut self, rect: Rect) -> Self {
        self.rect = rect;
        self
    }

    /// Override the `clicked` bit — for synthetic clicks (keyboard-
    /// driven activation, scripted UI tests) or to suppress the bit
    /// when a widget consumed the click internally.
    pub fn with_clicked(mut self, clicked: bool) -> Self {
        self.clicked = clicked;
        self
    }

    /// Override the `changed` bit — for widgets that own their value
    /// and want to report mutation back to the caller without going
    /// through the kit's interaction state.
    pub fn with_changed(mut self, changed: bool) -> Self {
        self.changed = changed;
        self
    }

    /// Override the `animating` bit — useful when a widget knows its
    /// internal animation hasn't settled and needs another paint.
    pub fn with_animating(mut self, animating: bool) -> Self {
        self.animating = animating;
        self
    }
}

/// What kind of pointer interaction a region wants. Used by
/// [`PortalUi::allocate_painter`] and indirectly by every widget.
///
/// - `None` — paint-only, no hit-region registered. Backgrounds and
///   decorations use this so they don't intercept clicks on widgets
///   layered above.
/// - `Hover` — register the rect so the kit reports hover state, but
///   don't bother tracking press / drag.
/// - `Click` — register + track press; `Response::clicked` fires on
///   release inside the rect.
/// - `Drag` — same as `Click` plus `Response::drag_delta_local`
///   reports per-frame movement while pressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Sense {
    /// Paint-only: no hit region is registered. Default for decorative
    /// fills, dividers, and any painter the caller doesn't need a
    /// response from.
    #[default]
    None,
    Hover,
    Click,
    Drag,
}

// ─────────────────────────────────────────────────────────────────────
// Storage
// ─────────────────────────────────────────────────────────────────────

/// Per-portal scratch storage for widget-local state — drag offsets,
/// hover ease t, text input cursor / selection, combo open state.
///
/// Keyed by [`WidgetId`]. Cleared NEVER on a per-frame basis; entries
/// live as long as the portal does. To garbage-collect entries for
/// widgets that disappeared, the portal records every `get_or_default`
/// id it issued this frame and `retain`s only those at the end of
/// the frame — handled inside [`crate::ui::Portal`].
#[derive(Default)]
pub struct PortalStorage {
    pub(crate) cells: HashMap<WidgetId, Box<dyn Any + Send>>,
    /// Set of ids touched this frame. Snapshotted by
    /// [`crate::ui::Portal::end_frame`] which then drops everything not
    /// in the set.
    pub(crate) used_this_frame: std::collections::HashSet<WidgetId>,
}

impl PortalStorage {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert-or-fetch a typed cell. Records `id` as used-this-frame
    /// (so GC keeps it). If a previous value exists with a DIFFERENT
    /// concrete type, it's replaced — widget ids should never collide
    /// across types, but if a host changes a widget at the same call
    /// site we'd rather start fresh than panic.
    pub fn get_or_insert_with<T, F>(&mut self, id: WidgetId, init: F) -> &mut T
    where
        T: Any + Send + 'static,
        F: FnOnce() -> T,
    {
        self.used_this_frame.insert(id);
        // Use the entry API only after the type check — `init` can be
        // called at most once across both branches.
        let needs_reinit = self
            .cells
            .get(&id)
            .map(|c| c.downcast_ref::<T>().is_none())
            .unwrap_or(true);
        if needs_reinit {
            self.cells.insert(id, Box::new(init()));
        }
        self.cells
            .get_mut(&id)
            .expect("just inserted")
            .downcast_mut::<T>()
            .expect("type just inserted")
    }

    /// Drop entries that no widget touched this frame.
    pub(crate) fn gc_frame(&mut self) {
        let used = std::mem::take(&mut self.used_this_frame);
        self.cells.retain(|id, _| used.contains(id));
    }

    /// Cell count — for tests and diagnostics.
    pub fn len(&self) -> usize {
        self.cells.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }
}

// ─────────────────────────────────────────────────────────────────────
// Style — theme-derived constants for built-in widgets
// ─────────────────────────────────────────────────────────────────────

/// Theme-derived visual constants the built-in widgets read from.
///
/// Hosts construct one from the active theme each frame (cheap — just
/// reads from theme tokens). Custom widgets can ignore `PortalStyle`
/// entirely and read theme tokens directly via
/// [`blinc_theme::ThemeState::get`].
#[derive(Clone, Debug)]
pub struct PortalStyle {
    pub font_size: f32,
    pub line_height: f32,
    pub spacing: f32,
    pub control_height: f32,
    pub control_radius: f32,
    pub indent: f32,

    pub text_primary: Color,
    pub text_secondary: Color,
    pub text_disabled: Color,

    pub background: Color,

    pub button_bg: Color,
    pub button_hover: Color,
    pub button_pressed: Color,
    pub button_text: Color,

    pub field_bg: Color,
    pub field_border: Color,
    pub field_border_focus: Color,

    pub track: Color,
    pub track_filled: Color,
    pub thumb: Color,

    pub accent: Color,
    pub accent_pressed: Color,
}

impl PortalStyle {
    /// Build a default style from the active [`blinc_theme::ThemeState`].
    /// Tracks + button surfaces use a SEMI-TRANSPARENT text colour
    /// (`text_primary` with low alpha) instead of mixing against the
    /// theme's `Surface` token. Mixing against `Surface` collapses
    /// against light-mode portals: the inset background is
    /// `darken(Surface, 0.20)` (mid-grey) and a `mix(Surface, text,
    /// 0.22)` track is the same mid-grey — track vanishes. Alpha
    /// over whatever bg the widget actually sits on stays visible
    /// on both schemes.
    pub fn from_active_theme() -> Self {
        use blinc_theme::{tokens::ColorToken, ThemeState};
        let st = ThemeState::get();
        let c = |tok: ColorToken| st.color(tok);

        let text_primary = c(ColorToken::TextPrimary);
        let accent = c(ColorToken::Accent);

        // 18 % text-over-bg is the contrast sweet spot: visible on
        // every inset shade we paint widgets over (white-ish through
        // near-black) without competing with the accent-filled
        // portion of a slider or the thumb glyph.
        let track_alpha = 0.18_f32;
        let button_bg_alpha = 0.08_f32;
        let button_hover_alpha = 0.14_f32;
        let button_pressed_alpha = 0.20_f32;

        Self {
            font_size: 12.0,
            line_height: 16.0,
            spacing: 6.0,
            control_height: 24.0,
            control_radius: 6.0,
            indent: 14.0,
            text_primary,
            text_secondary: c(ColorToken::TextSecondary),
            text_disabled: c(ColorToken::TextTertiary),
            background: c(ColorToken::SurfaceElevated),
            button_bg: text_primary.with_alpha(button_bg_alpha),
            button_hover: text_primary.with_alpha(button_hover_alpha),
            button_pressed: text_primary.with_alpha(button_pressed_alpha),
            button_text: text_primary,
            field_bg: c(ColorToken::InputBg),
            field_border: c(ColorToken::Border),
            field_border_focus: c(ColorToken::BorderFocus),
            track: text_primary.with_alpha(track_alpha),
            track_filled: accent,
            thumb: text_primary,
            accent,
            accent_pressed: darken(accent, 0.12),
        }
    }
}

fn darken(c: Color, t: f32) -> Color {
    Color::rgba(
        (c.r * (1.0 - t)).max(0.0),
        (c.g * (1.0 - t)).max(0.0),
        (c.b * (1.0 - t)).max(0.0),
        c.a,
    )
}

/// Round-rect convenience — every widget paints into a corner-radius
/// rect at `style.control_radius`.
pub fn ctrl_radius(style: &PortalStyle) -> CornerRadius {
    CornerRadius::uniform(style.control_radius)
}

// ─────────────────────────────────────────────────────────────────────
// HostBridge
// ─────────────────────────────────────────────────────────────────────

/// Plumbing the host provides to each portal each frame: how to
/// transform widget-local coords to host-screen-space for overlay
/// anchoring, and (optionally) the host-side overlay / toast
/// managers. The portal does NOT own this — it borrows it during
/// `render_into` and `dispatch_event`.
///
/// Coordinate transforms are stored as closures so the host can plug
/// in any pan / zoom / 3D matrix it likes — the canvas-kit case is a
/// simple affine but a 3D scene-kit could project through a camera.
#[derive(Clone)]
pub struct HostBridge {
    /// `canvas-content (x, y) → host-screen (x, y)`. Used to anchor
    /// overlays opened from portal widgets; the host paints overlays
    /// in screen space.
    pub canvas_to_screen: Arc<dyn Fn(Point) -> Point + Send + Sync>,
    /// Inverse — used to interpret raw screen-space input events
    /// (e.g. drag deltas from a global pointer-move dispatch) back
    /// into the portal's coordinate frame. Currently unused by the
    /// built-in widgets (they read deltas from the kit's content-
    /// space drag stream) but exposed for custom widgets.
    pub screen_to_canvas: Arc<dyn Fn(Point) -> Point + Send + Sync>,
}

impl HostBridge {
    /// Trivial bridge — coords pass through unchanged. Useful for
    /// tests and for hosts that put the portal in a non-transformed
    /// canvas.
    pub fn identity() -> Self {
        Self {
            canvas_to_screen: Arc::new(|p| p),
            screen_to_canvas: Arc::new(|p| p),
        }
    }

    /// Build a bridge from a pair of plain closures. The host writes
    /// the closures using whatever transform it has on hand (an affine
    /// from canvas-kit, a 3D projection from a scene kit) and this
    /// constructor handles the `Arc::new` boxing so caller code stays
    /// terse.
    pub fn from_closures<C, S>(canvas_to_screen: C, screen_to_canvas: S) -> Self
    where
        C: Fn(Point) -> Point + Send + Sync + 'static,
        S: Fn(Point) -> Point + Send + Sync + 'static,
    {
        Self {
            canvas_to_screen: Arc::new(canvas_to_screen),
            screen_to_canvas: Arc::new(screen_to_canvas),
        }
    }

    /// Transform a widget rect (canvas-content coords) into screen
    /// space — convenience for overlay anchoring.
    pub fn rect_to_screen(&self, r: Rect) -> Rect {
        let tl = (self.canvas_to_screen)(Point::new(r.x(), r.y()));
        let br = (self.canvas_to_screen)(Point::new(r.x() + r.width(), r.y() + r.height()));
        Rect::new(tl.x, tl.y, br.x - tl.x, br.y - tl.y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widget_id_region_format_is_stable() {
        let p = PortalId(0xdead_beef_cafe_babe);
        let w = WidgetId(0x1234_5678_90ab_cdef);
        let s = w.to_region_id(p);
        assert_eq!(s, "portal_deadbeefcafebabe_w_1234567890abcdef");
    }

    #[test]
    fn storage_gc_drops_unused() {
        let mut s = PortalStorage::new();
        let a = WidgetId(1);
        let b = WidgetId(2);
        *s.get_or_insert_with::<i32, _>(a, || 10) += 1;
        *s.get_or_insert_with::<i32, _>(b, || 20) += 1;
        assert_eq!(s.len(), 2);
        // Frame ends — both touched this frame.
        s.gc_frame();
        assert_eq!(s.len(), 2);
        // Next frame: only `a` touched.
        *s.get_or_insert_with::<i32, _>(a, || 99) += 1;
        s.gc_frame();
        assert_eq!(s.len(), 1);
        assert!(s.cells.contains_key(&a));
        assert!(!s.cells.contains_key(&b));
    }

    #[test]
    fn host_bridge_identity_rect_passthrough() {
        let b = HostBridge::identity();
        let r = Rect::new(10.0, 20.0, 30.0, 40.0);
        let s = b.rect_to_screen(r);
        assert_eq!(
            (s.x(), s.y(), s.width(), s.height()),
            (10.0, 20.0, 30.0, 40.0)
        );
    }
}
