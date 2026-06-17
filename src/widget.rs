//! Built-in widgets implemented as [`PortalUi`] inherent-method
//! extensions: [`label`](PortalUi::label), [`button`](PortalUi::button),
//! [`switch`](PortalUi::switch), [`slider`](PortalUi::slider),
//! [`text_input`](PortalUi::text_input).
//!
//! Every widget has two forms. The plain form (`label`, `button`,
//! `switch`, `slider`, `text_input`) takes a `&mut value` the caller
//! owns and reports mutation via `Response::changed`. The `_signal`
//! form reads (and on input widgets, writes) a
//! [`Signal`](blinc_core::reactive::Signal) — reading inside the portal
//! frame auto-subscribes via [`crate::PortalSubscriptions`], so any
//! external `Signal::set` flips the portal's dirty flag and the host
//! repaints. Use signal forms when the value is shared with other
//! parts of the app; the plain forms are for self-contained widget
//! state.
//!
//! Widgets read sizes and colours from [`PortalStyle`] (theme-derived;
//! constructed once per frame by the host) so re-styling the active
//! theme automatically re-themes every portal on the next paint.

use crate::core::{ctrl_radius, PortalStyle, Response, Sense};
use crate::ui::PortalUi;
use blinc_core::draw::{Stroke, TextStyle};
use blinc_core::layer::{Brush, Color, Point};
use blinc_core::reactive::Signal;
use blinc_core::{FontWeight, TextAlign, TextBaseline};

// ─────────────────────────────────────────────────────────────────────
// Sizing helpers
// ─────────────────────────────────────────────────────────────────────

/// Approximate text width for layout — uses a 0.55em advance, which
/// is close enough for proportional UI fonts at portal sizes (the
/// portal doesn't have the real font metrics handy and the cost of
/// querying them per widget per frame isn't worth the precision).
/// Widgets stretch elastically anyway via `available_size`.
fn approx_text_width(text: &str, style: &PortalStyle) -> f32 {
    text.chars().count() as f32 * style.font_size * 0.55
}

fn text_style(style: &PortalStyle, color: Color) -> TextStyle {
    TextStyle {
        family: "system-ui".to_string(),
        size: style.font_size,
        weight: FontWeight::Regular,
        color,
        align: TextAlign::Left,
        baseline: TextBaseline::Alphabetic,
        line_height: style.line_height / style.font_size.max(1.0),
        letter_spacing: 0.0,
    }
}

// ─────────────────────────────────────────────────────────────────────
// label
// ─────────────────────────────────────────────────────────────────────

impl<'a> PortalUi<'a> {
    /// Static text. Does not register a hit region. Returns the rect
    /// it allocated so callers can chain layout decisions on it.
    pub fn label(&mut self, text: &str) -> Response {
        let style = self.style.clone();
        let width = approx_text_width(text, &style).max(1.0);
        let height = style.line_height;
        let (mut p, resp) = self.allocate_painter((width, height), Sense::None);
        let color = style.text_primary;
        let mut ts = text_style(&style, color);
        // Baseline-down: position so the alphabetic baseline sits
        // near the bottom of the rect.
        ts.baseline = TextBaseline::Alphabetic;
        let origin = Point::new(p.rect().x(), p.rect().y() + style.font_size);
        p.draw_text(text, &ts, origin);
        let _ = color; // silence linter; baked into ts above
        resp
    }

    /// Label that re-reads a signal and re-paints when the signal
    /// changes from anywhere. Reading the signal inside the
    /// portal's render closure registers the portal as a subscriber
    /// for the rest of the frame.
    pub fn label_signal<T: Clone + std::fmt::Display + Send + 'static + Default>(
        &mut self,
        sig: &Signal<T>,
    ) -> Response {
        let value = sig.get();
        self.label(&value.to_string())
    }
}

// ─────────────────────────────────────────────────────────────────────
// button
// ─────────────────────────────────────────────────────────────────────

impl<'a> PortalUi<'a> {
    /// Push button with a text label. Returns a [`Response`] whose
    /// `clicked` field is true on the frame the user releases inside
    /// the button.
    pub fn button(&mut self, label: &str) -> Response {
        let style = self.style.clone();
        let pad_x = 10.0_f32;
        let width = approx_text_width(label, &style) + pad_x * 2.0;
        let height = style.control_height;
        let (mut p, resp) = self.allocate_painter((width, height), Sense::Click);

        let fill = if resp.pressed {
            style.button_pressed
        } else if resp.hovered {
            style.button_hover
        } else {
            style.button_bg
        };
        p.fill_self(&style, Brush::Solid(fill));

        let mut ts = text_style(&style, style.button_text);
        ts.align = TextAlign::Center;
        ts.baseline = TextBaseline::Middle;
        let centre_x = p.rect().x() + p.rect().width() * 0.5;
        let centre_y = p.rect().y() + p.rect().height() * 0.5;
        p.draw_text(label, &ts, Point::new(centre_x, centre_y));

        resp
    }
}

// ─────────────────────────────────────────────────────────────────────
// switch
// ─────────────────────────────────────────────────────────────────────

impl<'a> PortalUi<'a> {
    /// Compact on/off toggle. Returns a response whose `changed`
    /// fires only on the frame the user toggled.
    pub fn switch(&mut self, value: &mut bool) -> Response {
        let style = self.style.clone();
        let width = 32.0_f32;
        let height = 18.0_f32;
        let (mut p, mut resp) = self.allocate_painter((width, height), Sense::Click);
        if resp.clicked {
            *value = !*value;
            resp.changed = true;
        }

        // Track
        let track_color = if *value { style.accent } else { style.track };
        let radius = blinc_core::layer::CornerRadius::uniform(height * 0.5);
        p.fill_rect(p.rect(), radius, Brush::Solid(track_color));

        // Thumb
        let thumb_r = height * 0.5 - 2.0;
        let thumb_cx = if *value {
            p.rect().x() + p.rect().width() - thumb_r - 2.0
        } else {
            p.rect().x() + thumb_r + 2.0
        };
        let thumb_cy = p.rect().y() + p.rect().height() * 0.5;
        p.fill_circle(
            Point::new(thumb_cx, thumb_cy),
            thumb_r,
            Brush::Solid(style.thumb),
        );

        resp
    }

    /// Signal-bound `switch`. Toggles the signal value on click.
    pub fn switch_signal(&mut self, sig: &Signal<bool>) -> Response {
        let mut v = sig.get();
        let resp = self.switch(&mut v);
        if resp.changed {
            sig.set(v);
        }
        resp
    }
}

// ─────────────────────────────────────────────────────────────────────
// slider — horizontal, fixed width follows available size
// ─────────────────────────────────────────────────────────────────────

impl<'a> PortalUi<'a> {
    /// Horizontal slider. Width fills the remaining row up to a
    /// reasonable cap; height matches the style's control height.
    /// The user drags the thumb or clicks the track to set a value.
    pub fn slider(&mut self, value: &mut f32, range: std::ops::Range<f32>) -> Response {
        let style = self.style.clone();
        let (avail_w, _) = self.available_size();
        let width = avail_w.clamp(80.0, 220.0);
        let height = style.control_height;
        let (mut p, mut resp) = self.allocate_painter((width, height), Sense::Drag);

        let track_h = 4.0_f32;
        let track_y = p.rect().y() + (height - track_h) * 0.5;
        let track_rect = blinc_core::layer::Rect::new(
            p.rect().x() + 8.0,
            track_y,
            p.rect().width() - 16.0,
            track_h,
        );
        let radius = blinc_core::layer::CornerRadius::uniform(track_h * 0.5);

        // Compute thumb position.
        let span = (range.end - range.start).max(1e-6);
        let mut t = ((*value - range.start) / span).clamp(0.0, 1.0);

        // Pointer interaction — when active, map pointer.x → t.
        if resp.pressed {
            if let Some(local) = resp.pointer_local {
                let track_x_local = local.x - 8.0;
                let new_t = (track_x_local / track_rect.width()).clamp(0.0, 1.0);
                if (new_t - t).abs() > 1e-4 {
                    t = new_t;
                    *value = range.start + t * span;
                    resp.changed = true;
                }
            }
        }

        // Track (unfilled).
        p.fill_rect(track_rect, radius, Brush::Solid(style.track));
        // Filled portion up to thumb.
        let filled_rect = blinc_core::layer::Rect::new(
            track_rect.x(),
            track_rect.y(),
            track_rect.width() * t,
            track_rect.height(),
        );
        p.fill_rect(filled_rect, radius, Brush::Solid(style.track_filled));

        // Thumb.
        let thumb_cx = track_rect.x() + track_rect.width() * t;
        let thumb_cy = track_rect.y() + track_rect.height() * 0.5;
        let thumb_r = if resp.pressed { 8.0 } else { 7.0 };
        p.fill_circle(
            Point::new(thumb_cx, thumb_cy),
            thumb_r,
            Brush::Solid(style.thumb),
        );

        resp
    }

    /// Signal-bound `slider`.
    pub fn slider_signal(&mut self, sig: &Signal<f32>, range: std::ops::Range<f32>) -> Response {
        let mut v = sig.get();
        let resp = self.slider(&mut v, range);
        if resp.changed {
            sig.set(v);
        }
        resp
    }
}

// ─────────────────────────────────────────────────────────────────────
// text_input — read-only display in v0.1 (typing requires keyboard
// focus dispatch which isn't wired through the canvas kit yet; the
// kit caches modifier state but not character input). Behaves as a
// styled value chip the caller can click to take focus elsewhere
// (overlay-anchored editor, host-side popup, etc.).
// ─────────────────────────────────────────────────────────────────────

impl<'a> PortalUi<'a> {
    /// Boxed text display. Renders `value` inside a bordered field;
    /// the response's `clicked` fires when the user clicks the box —
    /// hosts route that to whatever editor they prefer (cn::input in
    /// a popover is the standard pattern). Typing inside the box
    /// itself is deferred until canvas-kit grows a text-input
    /// dispatch path.
    pub fn text_input(&mut self, value: &mut String) -> Response {
        let style = self.style.clone();
        let (avail_w, _) = self.available_size();
        let width = avail_w.clamp(80.0, 240.0);
        let height = style.control_height;
        let (mut p, resp) = self.allocate_painter((width, height), Sense::Click);

        // Field background + border.
        let bg = style.field_bg;
        let border = if resp.hovered {
            style.field_border_focus
        } else {
            style.field_border
        };
        p.fill_self(&style, Brush::Solid(bg));
        p.stroke_self(&style, &Stroke::new(1.0), Brush::Solid(border));

        // Text.
        let mut ts = text_style(&style, style.text_primary);
        ts.baseline = TextBaseline::Middle;
        let origin = Point::new(p.rect().x() + 8.0, p.rect().y() + height * 0.5);
        p.draw_text(value, &ts, origin);

        // `changed` stays false — see the doc note above. The plain
        // form exists so callers can hook click + edit-by-other-means.
        let _ = value;
        resp
    }

    /// Signal-bound `text_input`. Reads the signal each frame so
    /// external mutations show through; writes are deferred until
    /// the text-input editing path is wired.
    pub fn text_input_signal(&mut self, sig: &Signal<String>) -> Response {
        let mut v = sig.get();
        self.text_input(&mut v)
    }
}

// ─────────────────────────────────────────────────────────────────────
// Unused-import suppressions for items only referenced behind cfg
// ─────────────────────────────────────────────────────────────────────

#[allow(dead_code)]
fn _ctrl_radius_marker(style: &PortalStyle) -> blinc_core::layer::CornerRadius {
    ctrl_radius(style)
}
