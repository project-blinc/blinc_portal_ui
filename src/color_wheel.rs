//! Free-form colour wheel panel — a `Div` ready to drop into an
//! `OverlayBuilder::popover().content(...)` host overlay.
//!
//! The panel is two-way bound to a `Signal<String>` holding the
//! `#rrggbb[aa]` hex string. Pointer interaction reads the live HSVA
//! out of an internal companion `(f32, f32, f32, f32)` signal so a
//! drag never round-trips through the hex quantiser (the round-trip
//! would ratchet hue by 1–2° over a long drag). The hex signal still
//! updates every commit so downstream consumers reading the bound
//! `Signal<String>` see the canonical lowercase representation.
//!
//! ## Layout
//!
//! One wrapper `div().w(W).h(H).on_mouse_down().on_drag()` with a
//! `canvas()` child. The wrapping div owns the pointer handlers
//! because [`blinc_layout::Canvas`] does not expose `.on_*` event
//! hooks. The trigger chip painted by
//! [`crate::PortalUi::color_picker`] gives users the swatch + hex
//! readout; this panel is just the wheel.
//!
//! ## Paint
//!
//! - Hue ring: 48 wedge `fill_path` calls each with a 2-stop
//!   `Gradient::linear` between adjacent hues. (Multi-stop gradients
//!   are silently truncated to 2 stops by the current GPU path
//!   layer, so this is the only practical way to paint a hue arc.)
//! - SV square: two stacked `fill_rect` calls — horizontal
//!   white-to-pure-hue, vertical transparent-to-black.
//! - Markers: stroke_circle on the ring + on the SV square.
//!
//! All draw calls go through `DrawContext` so the wheel renders
//! correctly inside the overlay-canvas dispatch pass. The closure
//! does NOT call `draw_text` — that primitive is invisible inside a
//! canvas closure per the long-standing PRIM_TEXT-in-canvas-overlay
//! gotcha.

use blinc_core::draw::Path;
use blinc_core::layer::{Brush, Color, CornerRadius, Gradient, Point, Rect, Vec2};
use blinc_core::reactive::{signal, Signal};
use blinc_layout::{canvas, div, Div};

// Sizes align to a 4 px grid (1 unit = 4 px). WHEEL_PX = 56 units,
// RING_THICKNESS = 6 units. Markers (paint helpers) are exempt —
// circle radii follow the visual weight needed, not the layout grid.
const WHEEL_PX: f32 = 224.0;
const RING_THICKNESS: f32 = 24.0;
const HUE_SEGMENTS: usize = 48;

/// Build the host-side overlay panel for the colour-wheel popover.
///
/// `hex` is the user-facing two-way binding: callers read the
/// canonical lowercase hex from it, and writes from outside the
/// picker flow into the wheel marker because the canvas closure
/// reads `hex.get()` every paint.
///
/// Pair with [`crate::PortalUi::color_picker`] — the trigger chip
/// returns `Response.clicked`; on click the host calls
/// `HostBridge::rect_to_screen(resp.rect)` and mounts the result of
/// this function as the overlay's content.
pub fn color_wheel_panel(hex: Signal<String>) -> Div {
    // Companion HSVA signal — drag commits write here first, then
    // serialise to the hex string. Reading HSVA from the parsed hex
    // every paint would ratchet hue over a long drag (parse + format
    // round-trip is u8-quantised). We keep the float-precision HSVA
    // for the duration of the gesture and let the hex form be the
    // string-typed output channel.
    let (h0, s0, v0, a0) = Color::from_hex_str(&hex.get())
        .unwrap_or(Color::WHITE)
        .to_hsva();
    let hsva = signal((h0, s0, v0, a0));
    let drag_mode = signal(0u8); // 0=idle, 1=ring, 2=sv
    // Bounds captured at mouse_down so on_drag has a stable origin
    // even when `EventContext::local_x/y` semantics differ between
    // the initial hit and subsequent drag dispatches. Mirrors the
    // pattern in `cn::slider` which uses absolute `mouse_x` + a
    // stored start.
    let drag_origin = signal((0.0_f32, 0.0_f32));

    // Snapshot constants for closures — atan2 axis convention is
    // (lx).atan2(-ly) so 0° sits at 12 o'clock (matches Photoshop).
    let cx = WHEEL_PX * 0.5;
    let cy = WHEEL_PX * 0.5;
    let outer_r = (WHEEL_PX * 0.5) - 8.0;
    let inner_r = outer_r - RING_THICKNESS;
    let sv_side = inner_r * 1.35;
    let sv_x = cx - sv_side * 0.5;
    let sv_y = cy - sv_side * 0.5;

    // Wheel paint closure — reads hex_for_canvas, paints ring + SV
    // square + both markers. Pure read; no Signal writes here.
    let hex_for_canvas = hex.clone();
    let hsva_for_canvas = hsva;
    let wheel_canvas = canvas(move |dc, _bounds| {
        // Prefer the live HSVA companion (in-flight drag precision)
        // when its hue / sv matches the hex-parsed value within
        // quantisation tolerance. Otherwise re-parse from hex so
        // external writes track.
        let (h_hex, s_hex, v_hex, a_hex) = Color::from_hex_str(&hex_for_canvas.get())
            .unwrap_or(Color::WHITE)
            .to_hsva();
        let (h_live, s_live, v_live, _) = hsva_for_canvas.get();
        // 8-bit drift tolerance — anything tighter than 1/255 is
        // below quantisation, anything looser drops the live HSVA.
        let drift = (h_live - h_hex).abs() > 0.5
            || (s_live - s_hex).abs() > 0.004
            || (v_live - v_hex).abs() > 0.004;
        let (h, s, v, _a) = if drift {
            (h_hex, s_hex, v_hex, a_hex)
        } else {
            (h_live, s_live, v_live, a_hex)
        };

        paint_hue_ring(dc, cx, cy, inner_r, outer_r);
        paint_sv_square(dc, sv_x, sv_y, sv_side, h);
        paint_hue_marker(dc, cx, cy, (inner_r + outer_r) * 0.5, h);
        paint_sv_marker(dc, sv_x, sv_y, sv_side, s, v, h);
    })
    .w(WHEEL_PX)
    .h(WHEEL_PX);

    // Pointer handlers — write the live HSVA + the hex string.
    // Drag mode is latched on press so a drag that starts in the
    // ring keeps editing hue even if the cursor wanders into the
    // square (matches Photoshop UX).
    let hex_on_down = hex.clone();
    let hsva_on_down = hsva;
    let drag_on_down = drag_mode;
    let origin_on_down = drag_origin;
    let on_mouse_down = move |ctx: &blinc_layout::EventContext| {
        // Cache the panel's absolute top-left so `on_drag` can map
        // its `mouse_x/mouse_y` back into wheel-local space without
        // relying on `local_x/y` (which may not refresh for drag
        // dispatches on every platform).
        origin_on_down.set((ctx.bounds_x, ctx.bounds_y));
        let lx = ctx.local_x - cx;
        let ly = ctx.local_y - cy;
        let r = (lx * lx + ly * ly).sqrt();
        let (_, s, v, a) = hsva_on_down.get();
        if r >= inner_r && r <= outer_r {
            drag_on_down.set(1);
            let h_new = lx.atan2(-ly).to_degrees().rem_euclid(360.0);
            commit_hsva(h_new, s, v, a, hsva_on_down, &hex_on_down);
        } else if ctx.local_x >= sv_x
            && ctx.local_x <= sv_x + sv_side
            && ctx.local_y >= sv_y
            && ctx.local_y <= sv_y + sv_side
        {
            drag_on_down.set(2);
            let (h_kept, _, _, _) = hsva_on_down.get();
            let s_new = ((ctx.local_x - sv_x) / sv_side).clamp(0.0, 1.0);
            let v_new = 1.0 - ((ctx.local_y - sv_y) / sv_side).clamp(0.0, 1.0);
            commit_hsva(h_kept, s_new, v_new, a, hsva_on_down, &hex_on_down);
        }
    };

    let hex_on_drag = hex.clone();
    let hsva_on_drag = hsva;
    let drag_on_drag = drag_mode;
    let origin_on_drag = drag_origin;
    let on_drag = move |ctx: &blinc_layout::EventContext| {
        let mode = drag_on_drag.get();
        let (bx, by) = origin_on_drag.get();
        // `mouse_x/y` are absolute window coords (`cn::slider` uses
        // these for its drag delta), so subtracting the captured
        // panel origin gives the wheel-local position regardless of
        // whether `ctx.local_x/y` refreshed for this dispatch.
        let lx = (ctx.mouse_x - bx) - cx;
        let ly = (ctx.mouse_y - by) - cy;
        let local_x = ctx.mouse_x - bx;
        let local_y = ctx.mouse_y - by;
        let (h_kept, s_kept, v_kept, a) = hsva_on_drag.get();
        match mode {
            1 => {
                let h_new = lx.atan2(-ly).to_degrees().rem_euclid(360.0);
                commit_hsva(h_new, s_kept, v_kept, a, hsva_on_drag, &hex_on_drag);
            }
            2 => {
                let s_new = ((local_x - sv_x) / sv_side).clamp(0.0, 1.0);
                let v_new = 1.0 - ((local_y - sv_y) / sv_side).clamp(0.0, 1.0);
                commit_hsva(h_kept, s_new, v_new, a, hsva_on_drag, &hex_on_drag);
            }
            _ => {}
        }
    };

    let drag_on_up = drag_mode;
    let on_mouse_up = move |_ctx: &blinc_layout::EventContext| {
        drag_on_up.set(0);
    };

    div()
        .w(WHEEL_PX)
        .h(WHEEL_PX)
        .on_mouse_down(on_mouse_down)
        .on_drag(on_drag)
        .on_mouse_up(on_mouse_up)
        .child(wheel_canvas)
}

// ─── Paint helpers ──────────────────────────────────────────────────

fn paint_hue_ring(
    dc: &mut dyn blinc_core::draw::DrawContext,
    cx: f32,
    cy: f32,
    inner_r: f32,
    outer_r: f32,
) {
    let step = std::f32::consts::TAU / HUE_SEGMENTS as f32;
    let outer_radii = Vec2::new(outer_r, outer_r);
    let inner_radii = Vec2::new(inner_r, inner_r);
    for i in 0..HUE_SEGMENTS {
        let a0 = i as f32 * step;
        let a1 = (i + 1) as f32 * step;
        let p0i = polar(cx, cy, inner_r, a0);
        let p0o = polar(cx, cy, outer_r, a0);
        let p1i = polar(cx, cy, inner_r, a1);
        let p1o = polar(cx, cy, outer_r, a1);

        // Closed wedge: inner-start -> outer-start -> outer arc to
        // outer-end -> inner-end -> inner arc back to inner-start.
        // SVG sweep flag: outer arc CW (sweep=true), inner arc CCW
        // (sweep=false) so the polygon stays simple.
        let path = Path::new()
            .move_to(p0i.x, p0i.y)
            .line_to(p0o.x, p0o.y)
            .arc_to(outer_radii, 0.0, false, true, p1o.x, p1o.y)
            .line_to(p1i.x, p1i.y)
            .arc_to(inner_radii, 0.0, false, false, p0i.x, p0i.y)
            .close();

        let h0_deg = a0.to_degrees();
        let h1_deg = a1.to_degrees();
        let c0 = Color::from_hsva(h0_deg, 1.0, 1.0, 1.0);
        let c1 = Color::from_hsva(h1_deg, 1.0, 1.0, 1.0);
        // Gradient runs along the outer-edge midpoint vector so the
        // two stops sit at the wedge's angular endpoints. The 2-stop
        // GPU path truncates anything richer; per-wedge linear is
        // the largest faithful gradient on offer.
        let r_mid = (inner_r + outer_r) * 0.5;
        let g_start = polar(cx, cy, r_mid, a0);
        let g_end = polar(cx, cy, r_mid, a1);
        let gradient = Gradient::linear(g_start, g_end, c0, c1);
        dc.fill_path(&path, Brush::Gradient(gradient));
    }
}

fn paint_sv_square(
    dc: &mut dyn blinc_core::draw::DrawContext,
    x: f32,
    y: f32,
    side: f32,
    hue_deg: f32,
) {
    let rect = Rect::new(x, y, side, side);
    let radius = CornerRadius::uniform(4.0);
    let pure_hue = Color::from_hsva(hue_deg, 1.0, 1.0, 1.0);

    // Horizontal: white to pure hue.
    let g_h = Gradient::linear(
        Point::new(x, y + side * 0.5),
        Point::new(x + side, y + side * 0.5),
        Color::WHITE,
        pure_hue,
    );
    dc.fill_rect(rect, radius, Brush::Gradient(g_h));

    // Vertical overlay: transparent black to opaque black.
    let g_v = Gradient::linear(
        Point::new(x + side * 0.5, y),
        Point::new(x + side * 0.5, y + side),
        Color::rgba(0.0, 0.0, 0.0, 0.0),
        Color::rgba(0.0, 0.0, 0.0, 1.0),
    );
    dc.fill_rect(rect, radius, Brush::Gradient(g_v));
}

fn paint_hue_marker(
    dc: &mut dyn blinc_core::draw::DrawContext,
    cx: f32,
    cy: f32,
    r_mid: f32,
    hue_deg: f32,
) {
    let a = hue_deg.to_radians();
    let p = polar(cx, cy, r_mid, a);
    // Two stacked SDF fill_circles — white outer ring + pure-hue
    // inner disk. `stroke_circle` inside canvas closures composites
    // poorly against the path-tessellated hue ring (the stroke
    // primitive emits axis-aligned bounds that read as a faceted
    // outline at small radii), so we fall through to two solid
    // disks. Both go through the SDF circle pipeline, which
    // smoothsteps the edge fragment perfectly at any radius.
    let pure_hue = Color::from_hsva(hue_deg, 1.0, 1.0, 1.0);
    dc.fill_circle(p, 9.0, Brush::Solid(Color::WHITE));
    dc.fill_circle(p, 6.5, Brush::Solid(pure_hue));
}

fn paint_sv_marker(
    dc: &mut dyn blinc_core::draw::DrawContext,
    sv_x: f32,
    sv_y: f32,
    sv_side: f32,
    s: f32,
    v: f32,
    hue_deg: f32,
) {
    let px = sv_x + s * sv_side;
    let py = sv_y + (1.0 - v) * sv_side;
    let p = Point::new(px, py);
    // Outer white disk, inner disk at the picked colour so the
    // marker doubles as a preview swatch at the exact pick point.
    let picked = Color::from_hsva(hue_deg, s, v, 1.0);
    dc.fill_circle(p, 8.0, Brush::Solid(Color::WHITE));
    dc.fill_circle(p, 5.5, Brush::Solid(picked));
}

// ─── Math helpers ───────────────────────────────────────────────────

/// Polar-to-cartesian in the wheel's convention: angle 0 = top
/// (12 o'clock), positive CW. atan2 ≡ `lx.atan2(-ly)` in hit-test
/// uses the same convention so the marker tracks the cursor.
fn polar(cx: f32, cy: f32, r: f32, angle_rad: f32) -> Point {
    let x = cx + r * angle_rad.sin();
    let y = cy - r * angle_rad.cos();
    Point::new(x, y)
}

fn commit_hsva(
    h: f32,
    s: f32,
    v: f32,
    a: f32,
    hsva: Signal<(f32, f32, f32, f32)>,
    hex: &Signal<String>,
) {
    hsva.set((h, s, v, a));
    let c = Color::from_hsva(h, s, v, a);
    hex.set(c.to_hex_string(a < 1.0));
}
