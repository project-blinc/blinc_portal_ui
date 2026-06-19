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

use crate::core::{
    ctrl_radius, ButtonVariant, PortalStyle, PortalValue, Response, Sense, ShadowMix, ShadowToken,
    ValueBinding,
};
use crate::ui::PortalUi;
use blinc_core::draw::{Stroke, TextStyle};
use blinc_core::layer::{Brush, Color, Point};
use blinc_core::reactive::Signal;
use blinc_core::{FontWeight, TextAlign, TextBaseline};

// ─────────────────────────────────────────────────────────────────────
// Sizing helpers
// ─────────────────────────────────────────────────────────────────────

/// Text width in pixels at the portal style's font size, routed
/// through `blinc_layout::measure_text` so the value matches the
/// pixels the renderer actually paints. Hosts that registered a
/// real font measurer (every Blinc app does) get accurate metrics;
/// pure unit tests fall back to the 0.55em estimator.
fn text_width(text: &str, style: &PortalStyle) -> f32 {
    blinc_layout::measure_text(text, style.font_size).width
}

/// Back-compat shim — kept while the rest of the file migrates to
/// the renderer-accurate `text_width`. Equivalent behaviour now.
#[inline]
fn approx_text_width(text: &str, style: &PortalStyle) -> f32 {
    text_width(text, style)
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
    ///
    /// Row-aware vertical sizing: in a horizontal layout the rect
    /// allocates `control_height` so the label vertically centres
    /// against sibling widgets (switch / numeric_input / slider /
    /// select / button), which all allocate the same height. In
    /// vertical layout the rect stays at `line_height` for compact
    /// stacking. The text origin lands at the rect's vertical
    /// midpoint either way.
    pub fn label(&mut self, text: &str) -> Response {
        let style = self.style.clone();
        let width = approx_text_width(text, &style).max(1.0);
        let in_row = self.layout == crate::ui::LayoutDirection::Horizontal;
        let height = if in_row {
            style.control_height
        } else {
            style.line_height
        };
        let (mut p, resp) = self.allocate_painter((width, height), Sense::None);
        let mut ts = text_style(&style, style.text_primary);
        ts.baseline = TextBaseline::Middle;
        let origin = Point::new(p.rect().x(), p.rect().y() + height * 0.5);
        p.draw_text(text, &ts, origin);
        resp
    }

    /// Label that re-reads a signal and re-paints when the signal
    /// changes from anywhere. Reading the signal inside the
    /// portal's render closure registers the portal as a subscriber
    /// for the rest of the frame.
    ///
    /// Uses `Signal::try_get` so types without a `Default` impl
    /// (`NonZeroU32`, non_exhaustive enums, user newtypes that lack
    /// a meaningful zero value) work without a workaround. Signals
    /// that don't resolve (graph rebuild mid-frame) render an empty
    /// label rather than panic.
    pub fn label_signal<T: Clone + std::fmt::Display + Send + 'static>(
        &mut self,
        sig: &Signal<T>,
    ) -> Response {
        match sig.try_get() {
            Some(v) => self.label(&v.to_string()),
            None => self.label(""),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// button
// ─────────────────────────────────────────────────────────────────────

/// Deferred-paint builder for [`PortalUi::button`].
///
/// `button(label)` returns this builder rather than painting
/// immediately so callers can attach a variant + disabled state
/// before paint without growing the method surface to six
/// `button_primary` / `button_destructive` / … helpers. Borrows the
/// `&mut PortalUi` until consumed via [`Self::show`] (the only
/// terminal that returns a [`Response`]), so the borrow checker
/// statically prevents a half-configured builder from sitting
/// alongside another `ui.*` call.
///
/// Variant defaults to [`ButtonVariant::Ghost`] — see
/// [`ButtonVariant`]'s doc for the rationale for picking Ghost
/// over Primary at this layer.
#[must_use = "ButtonBuilder is lazy — call .show() (or .clicked() / .changed() / .hovered() / .pressed()) to paint"]
pub struct ButtonBuilder<'a, 'b> {
    ui: &'b mut PortalUi<'a>,
    label: String,
    variant: ButtonVariant,
    disabled: bool,
    /// Optional per-call shadow override. `None` resolves to the
    /// variant's default from `PortalStyle::buttons_shadow`; setting
    /// `Some(ShadowToken::Xx)` forces that token. Disabled wins
    /// over the override regardless.
    shadow_token: Option<ShadowToken>,
    /// Optional leading glyph painted at the left of the label
    /// (e.g. `"{ }"` for a code button, `"+"` for an add action).
    /// Set via [`Self::icon`]. When present the label left-aligns
    /// inside the chip so the row reads `icon  label`; an absent
    /// icon falls back to the centred-label layout.
    icon: Option<String>,
}

impl<'a, 'b> ShadowMix for ButtonBuilder<'a, 'b> {
    /// Override the variant's default shadow stack with an explicit
    /// [`ShadowToken`]. Pass `ShadowToken::None` to suppress shadow
    /// for a normally-elevated variant. Disabled buttons emit no
    /// shadow regardless of this setting. Shortcut methods
    /// (`shadow_sm` / `shadow_md` / ...) come from the
    /// [`ShadowMix`] trait's default impls.
    fn shadow(mut self, token: ShadowToken) -> Self {
        self.shadow_token = Some(token);
        self
    }
}

impl<'a, 'b> ButtonBuilder<'a, 'b> {
    /// Switch to `ButtonVariant::Primary` (saturated accent fill).
    pub fn primary(mut self) -> Self {
        self.variant = ButtonVariant::Primary;
        self
    }
    /// Switch to `ButtonVariant::Secondary`.
    pub fn secondary(mut self) -> Self {
        self.variant = ButtonVariant::Secondary;
        self
    }
    /// Switch to `ButtonVariant::Destructive` (error-tinted fill).
    pub fn destructive(mut self) -> Self {
        self.variant = ButtonVariant::Destructive;
        self
    }
    /// Switch to `ButtonVariant::Outline` (bordered, transparent fill).
    pub fn outline(mut self) -> Self {
        self.variant = ButtonVariant::Outline;
        self
    }
    /// Switch to `ButtonVariant::Ghost` (transparent fill — portal_ui
    /// default; explicit form is here for source clarity).
    pub fn ghost(mut self) -> Self {
        self.variant = ButtonVariant::Ghost;
        self
    }
    /// Switch to `ButtonVariant::Link` (text-coloured, no chrome).
    pub fn link(mut self) -> Self {
        self.variant = ButtonVariant::Link;
        self
    }
    /// Programmatic variant selection — matches cn::button's
    /// `.variant(v)` setter for hosts that store the variant in
    /// config rather than at the call site.
    pub fn variant(mut self, v: ButtonVariant) -> Self {
        self.variant = v;
        self
    }
    /// Toggle the disabled state. When `true`, the button paints
    /// with the cross-variant disabled palette (cn::button parity)
    /// and `Response::clicked` is suppressed.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Attach a leading glyph (any short string — typically an
    /// inline code-style glyph like `"{ }"`, a unicode arrow, or a
    /// tabler icon's pre-rendered char) that paints at the left of
    /// the label. With an icon set the chip switches from centred
    /// label to a left-aligned row: `4 px pad | icon | 4 px gap |
    /// label | 8 px pad`. portal_ui takes a flat `impl Into<String>`
    /// rather than an SVG handle so it stays free of an icon-set
    /// dependency; hosts that ship SVG icons can use the cn::button
    /// surface instead.
    pub fn icon(mut self, glyph: impl Into<String>) -> Self {
        self.icon = Some(glyph.into());
        self
    }

    /// Paint the button and return the full [`Response`]. Most call
    /// sites only need one flag — see [`Self::clicked`] etc. for
    /// boolean shortcuts.
    pub fn show(self) -> Response {
        let ButtonBuilder {
            ui,
            label,
            variant,
            disabled,
            shadow_token,
            icon,
        } = self;
        let style = ui.style.clone();
        let pad_x = 10.0_f32;
        // 4 px grid: with an icon, layout is `pad_left(4) | icon |
        // gap(4) | label | pad_right(8)`. Without one, fall back to
        // the symmetric `pad_x` padding used by every other portal
        // button.
        let icon_w = icon
            .as_ref()
            .map(|g| approx_text_width(g, &style))
            .unwrap_or(0.0);
        let label_w = approx_text_width(&label, &style);
        let width = if icon.is_some() {
            4.0 + icon_w + 4.0 + label_w + 8.0
        } else {
            label_w + pad_x * 2.0
        };
        let height = style.control_height;
        let sense = if disabled { Sense::None } else { Sense::Click };
        let (mut p, mut resp) = ui.allocate_painter((width, height), sense);

        let palette = if disabled {
            &style.buttons.disabled
        } else {
            style.buttons.for_variant(variant)
        };

        // Shadow stack: disabled wins (always empty); explicit
        // `.shadow(token)` override wins next; falls back to the
        // variant's pre-resolved default in `PortalStyle`. Emitted
        // BEFORE the fill so the GPU sorts it behind the body.
        let shadow_override_stack: Vec<blinc_core::layer::Shadow>;
        let shadow_stack: &[blinc_core::layer::Shadow] = if disabled {
            &[]
        } else if let Some(tok) = shadow_token {
            let theme_shadows = blinc_theme::ThemeState::get().shadows();
            shadow_override_stack = theme_shadows
                .get(tok)
                .iter()
                .map(|s| blinc_core::layer::Shadow::from(s.clone()))
                .collect();
            &shadow_override_stack
        } else {
            style.buttons_shadow.for_variant(variant)
        };
        p.shadow_self(&style, shadow_stack);

        let fill = if disabled {
            palette.idle
        } else if resp.pressed {
            palette.pressed
        } else if resp.hovered {
            palette.hover
        } else {
            palette.idle
        };
        p.fill_self(&style, Brush::Solid(fill));

        // Outline / disabled variants paint a 1px border on top of
        // the fill. Other variants leave the field unbordered.
        if let Some(border_color) = palette.border {
            p.stroke_self(&style, &Stroke::new(1.0), Brush::Solid(border_color));
        }

        let mut ts = text_style(&style, palette.text);
        ts.baseline = TextBaseline::Middle;
        let centre_y = p.rect().y() + p.rect().height() * 0.5;
        if let Some(ref glyph) = icon {
            // Left-aligned row: `pad(4) | icon | gap(4) | label`.
            let icon_x = p.rect().x() + 4.0;
            p.draw_text(glyph, &ts, Point::new(icon_x, centre_y));
            let label_x = icon_x + icon_w + 4.0;
            p.draw_text(&label, &ts, Point::new(label_x, centre_y));
        } else {
            ts.align = TextAlign::Center;
            let centre_x = p.rect().x() + p.rect().width() * 0.5;
            p.draw_text(&label, &ts, Point::new(centre_x, centre_y));
        }

        // Disabled buttons swallow clicks at the paint layer — the
        // hit region was registered as Sense::None so resp.clicked is
        // already false, but be explicit.
        if disabled {
            resp.clicked = false;
            resp.pressed = false;
        }

        resp
    }

    /// Paint the button and report whether it was clicked this frame.
    /// Convenience over `self.show().clicked`.
    pub fn clicked(self) -> bool {
        self.show().clicked
    }
    /// Paint and report `Response::hovered`.
    pub fn hovered(self) -> bool {
        self.show().hovered
    }
    /// Paint and report `Response::pressed`.
    pub fn pressed(self) -> bool {
        self.show().pressed
    }
    /// Paint and report `Response::changed`. (Buttons report click
    /// edges via `clicked`; `changed` is included for API symmetry
    /// with the other widgets' builders if they grow one later.)
    pub fn changed(self) -> bool {
        self.show().changed
    }
}

impl<'a> PortalUi<'a> {
    /// Push button. Returns a [`ButtonBuilder`] that paints when
    /// terminated by [`ButtonBuilder::show`], [`ButtonBuilder::clicked`],
    /// or any of the other `.<state>()` accessors.
    ///
    /// Defaults to [`ButtonVariant::Ghost`]. Call `.primary()` /
    /// `.destructive()` / `.outline()` / `.secondary()` / `.link()`
    /// before the terminal to switch variant. See
    /// [`ButtonVariant`] for the full vocabulary (mirrors
    /// `cn::ButtonVariant`).
    ///
    /// ```ignore
    /// // Default Ghost variant — transparent until hovered.
    /// if ui.button("Reset").clicked() { reset_state(); }
    ///
    /// // Destructive — error-tinted fill.
    /// if ui.button("Delete").destructive().clicked() { delete_node(); }
    ///
    /// // Primary action with disabled state from form validity.
    /// ui.button("Save").primary().disabled(!is_valid).show();
    /// ```
    pub fn button<'b>(&'b mut self, label: &str) -> ButtonBuilder<'a, 'b> {
        ButtonBuilder {
            ui: self,
            label: label.to_string(),
            variant: ButtonVariant::default(),
            disabled: false,
            shadow_token: None,
            icon: None,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// switch — compact on/off toggle.
// ─────────────────────────────────────────────────────────────────────

/// Deferred-paint builder returned by [`PortalUi::switch`]. Carries
/// the bound value (mutable borrow OR signal) and the fluent axes
/// (size, disabled, palette overrides, shadow). Paint via
/// [`Self::show`] / [`Self::clicked`] / [`Self::changed`].
#[must_use = "SwitchBuilder is lazy — call .show() / .clicked() / .changed() to paint"]
pub struct SwitchBuilder<'a, 'b> {
    ui: &'b mut PortalUi<'a>,
    value: ValueBinding<'b, bool>,
    disabled: bool,
    on_color: Option<Color>,
    off_color: Option<Color>,
    thumb_color: Option<Color>,
    shadow_token: Option<ShadowToken>,
    /// Override the inset shadow drawn into the track (recessed
    /// look). `None` resolves to the variant default
    /// [`ShadowToken::Inner`]; `Some(ShadowToken::None)` suppresses
    /// the inset entirely.
    track_inner_shadow_token: Option<ShadowToken>,
    /// Override the drop shadow under the thumb (raises it off the
    /// track). `None` resolves to [`ShadowToken::Sm`].
    thumb_shadow_token: Option<ShadowToken>,
}

impl<'a, 'b> ShadowMix for SwitchBuilder<'a, 'b> {
    fn shadow(mut self, token: ShadowToken) -> Self {
        self.shadow_token = Some(token);
        self
    }
}

impl<'a, 'b> SwitchBuilder<'a, 'b> {
    pub fn disabled(mut self, b: bool) -> Self {
        self.disabled = b;
        self
    }
    /// Override the active-track fill (default = `style.accent`).
    pub fn on_color(mut self, c: Color) -> Self {
        self.on_color = Some(c);
        self
    }
    /// Override the inactive-track fill (default = `style.track`).
    pub fn off_color(mut self, c: Color) -> Self {
        self.off_color = Some(c);
        self
    }
    /// Override the thumb fill (default = `style.thumb`).
    pub fn thumb_color(mut self, c: Color) -> Self {
        self.thumb_color = Some(c);
        self
    }

    /// Override the inset shadow drawn into the track surface.
    /// Default is [`ShadowToken::Inner`] which gives the recessed
    /// look most form switches want; pass
    /// `ShadowToken::None` to suppress.
    pub fn track_inner_shadow(mut self, token: ShadowToken) -> Self {
        self.track_inner_shadow_token = Some(token);
        self
    }

    /// Override the drop shadow under the thumb. Default is
    /// [`ShadowToken::Sm`] (subtle lift); pass `ShadowToken::None`
    /// to make the thumb sit flat on the track.
    pub fn thumb_shadow(mut self, token: ShadowToken) -> Self {
        self.thumb_shadow_token = Some(token);
        self
    }

    pub fn show(self) -> Response {
        let SwitchBuilder {
            ui,
            mut value,
            disabled,
            on_color,
            off_color,
            thumb_color,
            shadow_token,
            track_inner_shadow_token,
            thumb_shadow_token,
        } = self;
        let style = ui.style.clone();
        let width = 32.0_f32;
        let height = 18.0_f32;
        let sense = if disabled { Sense::None } else { Sense::Click };
        let (mut p, mut resp) = ui.allocate_painter((width, height), sense);
        let mut current = value.get();
        if !disabled && resp.clicked {
            current = !current;
            value.set(current);
            resp.changed = true;
        }

        let theme_shadows = blinc_theme::ThemeState::get().shadows();
        let lower_stack = |tok: ShadowToken| -> Vec<blinc_core::layer::Shadow> {
            theme_shadows
                .get(tok)
                .iter()
                .map(|s| blinc_core::layer::Shadow::from(s.clone()))
                .collect()
        };

        // Optional drop shadow under the whole track (off by
        // default; honours the ShadowMix `.shadow(token)` override).
        if let Some(tok) = shadow_token {
            if !disabled {
                let stack = lower_stack(tok);
                p.shadow_self(&style, &stack);
            }
        }

        let track_color = if current {
            on_color.unwrap_or(style.accent)
        } else {
            off_color.unwrap_or(style.track)
        };
        let track_color = if disabled {
            track_color.with_alpha(0.4)
        } else {
            track_color
        };
        let radius = blinc_core::layer::CornerRadius::uniform(height * 0.5);
        p.fill_rect(p.rect(), radius, Brush::Solid(track_color));

        // 1px outline around the track. Reads cleanly across
        // themes and avoids the dark-bar artefact that an inset
        // shadow produced at switch sizes. The
        // `.track_inner_shadow(...)` builder method is still
        // available for hosts that want the recessed-track look
        // explicitly; default behaviour ships the outline.
        if let Some(inner_tok) = track_inner_shadow_token {
            if !disabled && !matches!(inner_tok, ShadowToken::None) {
                let stack = lower_stack(inner_tok);
                p.inner_shadow_rect(p.rect(), radius, &stack);
            }
        } else {
            let border_color = if disabled {
                style.field_border.with_alpha(0.4)
            } else {
                style.field_border
            };
            p.stroke_rect(p.rect(), radius, &Stroke::new(1.0), Brush::Solid(border_color));
        }

        let thumb_r = height * 0.5 - 2.0;
        let thumb_cx = if current {
            p.rect().x() + p.rect().width() - thumb_r - 2.0
        } else {
            p.rect().x() + thumb_r + 2.0
        };
        let thumb_cy = p.rect().y() + p.rect().height() * 0.5;

        // Drop shadow under the thumb. Use the rounded-rect shadow
        // primitive (radius = thumb_r so it's effectively circular)
        // instead of `draw_circle_shadow` because at switch sizes
        // the circle-specific path emits too faint a falloff to
        // read against a saturated track. Default `Md` (6px blur);
        // override via `.thumb_shadow(...)` / `.shadow_*()` on the
        // builder.
        if !disabled {
            let thumb_tok = thumb_shadow_token.unwrap_or(ShadowToken::Md);
            if !matches!(thumb_tok, ShadowToken::None) {
                let stack = lower_stack(thumb_tok);
                let thumb_rect = blinc_core::layer::Rect::new(
                    thumb_cx - thumb_r,
                    thumb_cy - thumb_r,
                    thumb_r * 2.0,
                    thumb_r * 2.0,
                );
                let thumb_radius = blinc_core::layer::CornerRadius::uniform(thumb_r);
                p.shadow_rect(thumb_rect, thumb_radius, &stack);
            }
        }

        let thumb_brush = thumb_color.unwrap_or(style.thumb);
        let thumb_brush = if disabled {
            thumb_brush.with_alpha(0.6)
        } else {
            thumb_brush
        };
        p.fill_circle(Point::new(thumb_cx, thumb_cy), thumb_r, Brush::Solid(thumb_brush));

        resp
    }
    pub fn changed(self) -> bool {
        self.show().changed
    }
    pub fn clicked(self) -> bool {
        self.show().clicked
    }
}

impl<'a> PortalUi<'a> {
    /// On/off toggle. Accepts either `&mut bool` or `&Signal<bool>`
    /// via [`PortalValue`]. Returns a [`SwitchBuilder`]; chain
    /// `.disabled(...)` / `.shadow_*()` / palette overrides, then
    /// terminate with `.show()` (full Response), `.changed()`
    /// (`bool`, toggled this frame), or `.clicked()`.
    pub fn switch<'b, V: PortalValue<'b, bool>>(&'b mut self, value: V) -> SwitchBuilder<'a, 'b> {
        SwitchBuilder {
            ui: self,
            value: value.into_binding(),
            disabled: false,
            on_color: None,
            off_color: None,
            thumb_color: None,
            shadow_token: None,
            track_inner_shadow_token: None,
            thumb_shadow_token: None,
        }
    }

    /// Deprecated: use `ui.switch(&sig).show()` — the unified
    /// builder accepts both `&mut bool` and `&Signal<bool>`.
    #[deprecated(note = "Use `ui.switch(&sig).show()` instead — the builder accepts both &mut bool and &Signal<bool>.")]
    pub fn switch_signal(&mut self, sig: &Signal<bool>) -> Response {
        self.switch(sig).show()
    }
}

// ─────────────────────────────────────────────────────────────────────
// slider — horizontal, fixed width follows available size
// ─────────────────────────────────────────────────────────────────────

#[must_use = "SliderBuilder is lazy — call .show() / .changed() to paint"]
pub struct SliderBuilder<'a, 'b> {
    ui: &'b mut PortalUi<'a>,
    value: ValueBinding<'b, f32>,
    range: std::ops::Range<f32>,
    step: f32,
    disabled: bool,
    track_color: Option<Color>,
    fill_color: Option<Color>,
    thumb_color: Option<Color>,
    width_override: Option<f32>,
    shadow_token: Option<ShadowToken>,
}

impl<'a, 'b> ShadowMix for SliderBuilder<'a, 'b> {
    fn shadow(mut self, token: ShadowToken) -> Self {
        self.shadow_token = Some(token);
        self
    }
}

impl<'a, 'b> SliderBuilder<'a, 'b> {
    /// Snap the value to multiples of `step`. `0.0` (default) keeps
    /// the slider continuous.
    pub fn step(mut self, step: f32) -> Self {
        self.step = step.max(0.0);
        self
    }
    pub fn disabled(mut self, b: bool) -> Self {
        self.disabled = b;
        self
    }
    pub fn track_color(mut self, c: Color) -> Self {
        self.track_color = Some(c);
        self
    }
    pub fn fill_color(mut self, c: Color) -> Self {
        self.fill_color = Some(c);
        self
    }
    pub fn thumb_color(mut self, c: Color) -> Self {
        self.thumb_color = Some(c);
        self
    }
    pub fn width(mut self, w: f32) -> Self {
        self.width_override = Some(w);
        self
    }

    pub fn show(self) -> Response {
        let SliderBuilder {
            ui,
            mut value,
            range,
            step,
            disabled,
            track_color,
            fill_color,
            thumb_color,
            width_override,
            shadow_token,
        } = self;
        let style = ui.style.clone();
        let (avail_w, _) = ui.available_size();
        let width = width_override.unwrap_or_else(|| avail_w.clamp(80.0, 220.0));
        let height = style.control_height;
        let sense = if disabled { Sense::None } else { Sense::Drag };
        let (mut p, mut resp) = ui.allocate_painter((width, height), sense);

        let track_h = 4.0_f32;
        let track_y = p.rect().y() + (height - track_h) * 0.5;
        let track_rect = blinc_core::layer::Rect::new(
            p.rect().x() + 8.0,
            track_y,
            p.rect().width() - 16.0,
            track_h,
        );
        let radius = blinc_core::layer::CornerRadius::uniform(track_h * 0.5);

        let span = (range.end - range.start).max(1e-6);
        let mut current = value.get();
        let mut t = ((current - range.start) / span).clamp(0.0, 1.0);

        if !disabled && resp.pressed {
            if let Some(local) = resp.pointer_local {
                let track_x_local = local.x - 8.0;
                let mut new_t = (track_x_local / track_rect.width()).clamp(0.0, 1.0);
                if step > 0.0 {
                    // Quantise the value side so equal-spaced ticks
                    // map to identical t bands regardless of width.
                    let raw_value = range.start + new_t * span;
                    let snapped = (raw_value / step).round() * step;
                    new_t = ((snapped - range.start) / span).clamp(0.0, 1.0);
                }
                if (new_t - t).abs() > 1e-4 {
                    t = new_t;
                    current = range.start + t * span;
                    value.set(current);
                    resp.changed = true;
                }
            }
        }

        if let Some(tok) = shadow_token {
            if !disabled {
                let theme_shadows = blinc_theme::ThemeState::get().shadows();
                let stack: Vec<blinc_core::layer::Shadow> = theme_shadows
                    .get(tok)
                    .iter()
                    .map(|s| blinc_core::layer::Shadow::from(s.clone()))
                    .collect();
                p.shadow_self(&style, &stack);
            }
        }

        let track_brush = track_color.unwrap_or(style.track);
        let fill_brush = fill_color.unwrap_or(style.track_filled);
        let thumb_brush = thumb_color.unwrap_or(style.thumb);
        let (track_brush, fill_brush, thumb_brush) = if disabled {
            (
                track_brush.with_alpha(0.4),
                fill_brush.with_alpha(0.4),
                thumb_brush.with_alpha(0.6),
            )
        } else {
            (track_brush, fill_brush, thumb_brush)
        };

        p.fill_rect(track_rect, radius, Brush::Solid(track_brush));
        let filled_rect = blinc_core::layer::Rect::new(
            track_rect.x(),
            track_rect.y(),
            track_rect.width() * t,
            track_rect.height(),
        );
        p.fill_rect(filled_rect, radius, Brush::Solid(fill_brush));

        let thumb_cx = track_rect.x() + track_rect.width() * t;
        let thumb_cy = track_rect.y() + track_rect.height() * 0.5;
        let thumb_r = if resp.pressed && !disabled { 8.0 } else { 7.0 };
        p.fill_circle(
            Point::new(thumb_cx, thumb_cy),
            thumb_r,
            Brush::Solid(thumb_brush),
        );

        resp
    }
    pub fn changed(self) -> bool {
        self.show().changed
    }
}

impl<'a> PortalUi<'a> {
    /// Horizontal slider. Accepts either `&mut f32` or
    /// `&Signal<f32>` via [`PortalValue`]; the range is always the
    /// second argument. Returns a [`SliderBuilder`] for fluent
    /// configuration.
    pub fn slider<'b, V: PortalValue<'b, f32>>(
        &'b mut self,
        value: V,
        range: std::ops::Range<f32>,
    ) -> SliderBuilder<'a, 'b> {
        SliderBuilder {
            ui: self,
            value: value.into_binding(),
            range,
            step: 0.0,
            disabled: false,
            track_color: None,
            fill_color: None,
            thumb_color: None,
            width_override: None,
            shadow_token: None,
        }
    }

    #[deprecated(note = "Use `ui.slider(&sig, range).show()` instead — the builder accepts both &mut f32 and &Signal<f32>.")]
    pub fn slider_signal(&mut self, sig: &Signal<f32>, range: std::ops::Range<f32>) -> Response {
        self.slider(sig, range).show()
    }
}

// ─────────────────────────────────────────────────────────────────────
// numeric_input — dual-mode scrub-or-edit numeric field.
//
// Idle state: label-shaped chip showing the formatted number; the
// user drags horizontally to scrub (Sense::Drag). Click without drag
// enters edit mode where the chip becomes an inline text field for
// typing a number; Enter / blur commits, Esc cancels. State (mode,
// caret, scratch buffer, sub-pixel drag accumulator) lives in
// PortalStorage keyed by a single stable WidgetId reused across
// both modes so the cursor / hit region never reshuffles when the
// user switches modes.
// ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Default, Clone)]
struct NumericInputState {
    /// `true` while the field is in inline-edit (typed-character)
    /// mode. Toggles from idle on a no-drag click; clears on Enter
    /// / Esc / blur.
    editing: bool,
    /// Caret byte offset within `scratch` while editing.
    caret: usize,
    /// Edit-mode scratch buffer. Reseeded from the bound value at
    /// the moment the user clicks-into-edit and after every
    /// arrow-key nudge so the displayed text stays in sync.
    scratch: String,
    /// Sub-pixel drag accumulator (pixels). Drag-to-scrub
    /// integrates `drag_delta_local.x` here until a full
    /// `step * sensitivity` worth has accumulated; the integer
    /// multiple flushes into the bound value, the fractional
    /// remainder stays in the accumulator so smooth slow drags
    /// still advance.
    drag_accum_pixels: f32,
    /// Tracks the prior frame's `pressed` state so the rising-edge
    /// drag-start can reset the accumulator. Without this, a
    /// new drag inherits leftover sub-pixel from the prior gesture.
    was_pressed_last_frame: bool,
    /// Timestamp (`ui.time()` seconds, portal-monotonic) of the
    /// most recent no-drag click. Set on each click; a second
    /// click within `DOUBLE_CLICK_WINDOW_S` is treated as the
    /// double-click that enters edit mode. Cleared when edit
    /// mode is committed / cancelled.
    last_click_time: f32,
}

#[must_use = "NumericInputBuilder is lazy — call .show() / .changed() to paint"]
pub struct NumericInputBuilder<'a, 'b> {
    ui: &'b mut PortalUi<'a>,
    value: ValueBinding<'b, f32>,
    min: Option<f32>,
    max: Option<f32>,
    /// Step magnitude per scrub-pixel * sensitivity. `0.0` means
    /// "no quantise" — value is only clamped to min/max.
    step: f32,
    integer: bool,
    precision: u8,
    unit: Option<String>,
    placeholder: Option<String>,
    drag_sensitivity: f32,
    width_override: Option<f32>,
    disabled: bool,
    shadow_token: Option<ShadowToken>,
}

impl<'a, 'b> ShadowMix for NumericInputBuilder<'a, 'b> {
    fn shadow(mut self, token: ShadowToken) -> Self {
        self.shadow_token = Some(token);
        self
    }
}

impl<'a, 'b> NumericInputBuilder<'a, 'b> {
    pub fn min(mut self, v: f32) -> Self {
        self.min = Some(v);
        self
    }
    pub fn max(mut self, v: f32) -> Self {
        self.max = Some(v);
        self
    }
    /// Set both `min` and `max` at once — mirrors `SliderBuilder`'s
    /// range entry style.
    pub fn range(mut self, range: std::ops::Range<f32>) -> Self {
        self.min = Some(range.start);
        self.max = Some(range.end);
        self
    }
    /// Step magnitude per scrub-pixel * sensitivity. `0.0` disables
    /// quantisation; the value still clamps to `min..=max`.
    pub fn step(mut self, step: f32) -> Self {
        self.step = step.max(0.0);
        self
    }
    /// Integer-only mode. Defaults step to `1.0` when step was the
    /// builder default (still 0.0 from construction); does NOT
    /// override an explicit `.step(...)` set earlier in the chain.
    /// On commit / scrub / arrow the value is rounded to the
    /// nearest integer.
    pub fn integer(mut self) -> Self {
        self.integer = true;
        if self.step == 0.0 {
            self.step = 1.0;
        }
        self
    }
    /// Decimal precision used by `format_value` in idle paint and
    /// the edit-mode reseed. Ignored when `integer` is `true`.
    pub fn precision(mut self, p: u8) -> Self {
        self.precision = p;
        self
    }
    /// Trailing unit suffix shown in idle mode only ("ms", "px",
    /// "%"). Hidden in edit mode so the user types just the number.
    pub fn unit(mut self, u: impl Into<String>) -> Self {
        self.unit = Some(u.into());
        self
    }
    pub fn placeholder(mut self, t: impl Into<String>) -> Self {
        self.placeholder = Some(t.into());
        self
    }
    /// Multiplier on `(drag_delta_local.x * step)` per pixel.
    /// Default `1.0` — one pixel of drag = one step.
    pub fn drag_sensitivity(mut self, s: f32) -> Self {
        self.drag_sensitivity = s.max(0.0);
        self
    }
    pub fn width(mut self, w: f32) -> Self {
        self.width_override = Some(w);
        self
    }
    pub fn disabled(mut self, b: bool) -> Self {
        self.disabled = b;
        self
    }

    pub fn show(self) -> Response {
        use blinc_core::events::KeyCode;

        let NumericInputBuilder {
            ui,
            mut value,
            min,
            max,
            step,
            integer,
            precision,
            unit,
            placeholder,
            drag_sensitivity,
            width_override,
            disabled,
            shadow_token,
        } = self;
        let style = ui.style.clone();
        let height = style.control_height;

        let widget_id = ui.make_widget_id(None);
        let portal_id = ui.portal_id;

        // Width: 3-character floor so a single-digit value still
        // reads as a clearly editable field. When `.width(...)` is
        // set, it's still clamped UP to the 3-char floor — passing
        // a too-narrow override doesn't crush the chip.
        let three_char_w = text_width("000", &style) + 16.0;
        let unit_w = unit
            .as_deref()
            .filter(|u| !u.is_empty())
            .map(|u| text_width(u, &style) + 4.0)
            .unwrap_or(0.0);
        let content_w = text_width(
            &format_numeric(value.get(), precision, integer),
            &style,
        ) + unit_w
            + 16.0;
        let width = match width_override {
            Some(w) => w.max(three_char_w),
            None => content_w.max(three_char_w),
        };

        // Phase 1: snapshot current state + bound value.
        let current_raw = value.get();
        let current = if current_raw.is_finite() {
            current_raw
        } else {
            min.unwrap_or(0.0)
        };
        let clamp_value = |v: f32| -> f32 {
            let v = match (min, max) {
                (Some(lo), Some(hi)) => v.clamp(lo, hi),
                (Some(lo), None) => v.max(lo),
                (None, Some(hi)) => v.min(hi),
                (None, None) => v,
            };
            if integer {
                v.round()
            } else if step > 0.0 {
                let base = min.unwrap_or(0.0);
                ((v - base) / step).round() * step + base
            } else {
                v
            }
        };

        let mut state = ui
            .storage
            .get_or_insert_with::<NumericInputState, _>(widget_id, NumericInputState::default)
            .clone();
        state.caret = state.caret.min(state.scratch.len());

        let focused_before = !disabled && ui.is_focused(widget_id);
        // Click-outside auto-commit: if we were editing last frame
        // but focus has since moved (POINTER_DOWN cleared it), parse
        // and commit before painting this frame.
        if state.editing && !focused_before {
            if let Ok(parsed) = state.scratch.parse::<f32>() {
                let new_val = clamp_value(parsed);
                if (new_val - current).abs() > f32::EPSILON {
                    value.set(new_val);
                }
            }
            state.editing = false;
            state.scratch.clear();
            state.caret = 0;
        }

        let mut changed = false;

        // Phase 2: keyboard while focused. Arrow keys nudge in BOTH
        // modes; Enter / Esc only matter in edit mode.
        if focused_before {
            let keys: Vec<crate::ui::KbdKey> = ui.kbd_keys_frame.to_vec();
            let chars: Vec<char> = ui.kbd_chars_frame.to_vec();
            let mut commit_blur = false;
            let mut cancel_blur = false;
            for k in &keys {
                let key = KeyCode(k.key_code);
                if key == KeyCode::ESCAPE {
                    cancel_blur = true;
                    break;
                } else if key == KeyCode::ENTER {
                    if state.editing {
                        commit_blur = true;
                    }
                } else if key == KeyCode::UP {
                    let nudge = if step > 0.0 { step } else { 1.0 };
                    let new_val = clamp_value(current + nudge);
                    if (new_val - current).abs() > f32::EPSILON {
                        value.set(new_val);
                        changed = true;
                        if state.editing {
                            state.scratch = format_numeric(new_val, precision, integer);
                            state.caret = state.scratch.len();
                        }
                    }
                } else if key == KeyCode::DOWN {
                    let nudge = if step > 0.0 { step } else { 1.0 };
                    let new_val = clamp_value(current - nudge);
                    if (new_val - current).abs() > f32::EPSILON {
                        value.set(new_val);
                        changed = true;
                        if state.editing {
                            state.scratch = format_numeric(new_val, precision, integer);
                            state.caret = state.scratch.len();
                        }
                    }
                } else if state.editing {
                    if key == KeyCode::BACKSPACE {
                        if state.caret > 0 {
                            let new_caret = prev_char_boundary(&state.scratch, state.caret);
                            state.scratch.replace_range(new_caret..state.caret, "");
                            state.caret = new_caret;
                        }
                    } else if key == KeyCode::DELETE {
                        if state.caret < state.scratch.len() {
                            let next = next_char_boundary(&state.scratch, state.caret);
                            state.scratch.replace_range(state.caret..next, "");
                        }
                    } else if key == KeyCode::LEFT {
                        state.caret = prev_char_boundary(&state.scratch, state.caret);
                    } else if key == KeyCode::RIGHT {
                        state.caret = next_char_boundary(&state.scratch, state.caret);
                    } else if key == KeyCode::HOME {
                        state.caret = 0;
                    } else if key == KeyCode::END {
                        state.caret = state.scratch.len();
                    }
                }
            }
            if state.editing {
                for ch in chars {
                    if !ch.is_control() {
                        // Restrict to numeric input — digits, sign,
                        // decimal separator. Anything else is
                        // silently dropped. Paste of non-numeric is
                        // not attempted here.
                        let allow = ch.is_ascii_digit()
                            || ch == '-'
                            || ch == '+'
                            || (ch == '.' && !integer);
                        if allow {
                            let mut buf = [0u8; 4];
                            let s = ch.encode_utf8(&mut buf);
                            state
                                .scratch
                                .insert_str(state.caret.min(state.scratch.len()), s);
                            state.caret += s.len();
                        }
                    }
                }
            }

            if commit_blur {
                if let Ok(parsed) = state.scratch.parse::<f32>() {
                    let new_val = clamp_value(parsed);
                    if (new_val - current).abs() > f32::EPSILON {
                        value.set(new_val);
                        changed = true;
                    }
                }
                state.editing = false;
                state.scratch.clear();
                state.caret = 0;
                crate::ui::set_focused_region(None);
            } else if cancel_blur {
                state.editing = false;
                state.scratch.clear();
                state.caret = 0;
                crate::ui::set_focused_region(None);
            }
        }

        // Phase 3: paint. Allocate the painter with the stable
        // widget_id so storage / focus / hit-region key the same
        // entry across mode flips.
        let resp_clicked;
        let resp_hovered;
        let resp_pressed;
        let resp_rect;
        let drag_delta_x;
        {
            let sense = if disabled { Sense::Hover } else { Sense::Drag };
            let (mut p, r) = ui.allocate_painter_for_id((width, height), sense, widget_id);
            resp_clicked = r.clicked;
            resp_hovered = r.hovered;
            resp_pressed = r.pressed;
            resp_rect = r.rect;
            drag_delta_x = r.drag_delta_local.x;

            if let Some(tok) = shadow_token {
                if !disabled {
                    let theme_shadows = blinc_theme::ThemeState::get().shadows();
                    let stack: Vec<blinc_core::layer::Shadow> = theme_shadows
                        .get(tok)
                        .iter()
                        .map(|s| blinc_core::layer::Shadow::from(s.clone()))
                        .collect();
                    p.shadow_self(&style, &stack);
                }
            }

            let bg = if disabled {
                style.field_bg.with_alpha(0.5)
            } else {
                style.field_bg
            };
            let border = if focused_before || resp_hovered {
                style.field_border_focus
            } else {
                style.field_border
            };
            p.fill_self(&style, Brush::Solid(bg));

            // Progress affordance — a subtle accent-tinted fill whose
            // width tracks the current value's position inside the
            // `[min, max]` range. Acts as a fuel gauge so users
            // scrubbing without watching the digits still see motion.
            // Only rendered when BOTH bounds are set (no scale without
            // a range) and the value is inside the band. Left-rounded
            // to match the chip silhouette; right-rounded only at the
            // 100% edge so the partial-fill terminator stays sharp.
            if let (Some(lo), Some(hi)) = (min, max) {
                let span = hi - lo;
                if span > f32::EPSILON {
                    let frac = ((current - lo) / span).clamp(0.0, 1.0);
                    if frac > 0.0 {
                        let chip = p.rect();
                        let fill_w = chip.width() * frac;
                        let fill_rect = blinc_core::layer::Rect::new(
                            chip.x(),
                            chip.y(),
                            fill_w,
                            chip.height(),
                        );
                        let r = style.control_radius;
                        let right_r = if frac >= 0.9999 { r } else { 0.0 };
                        let radius =
                            blinc_core::layer::CornerRadius::new(r, right_r, right_r, r);
                        let tint_alpha = if disabled { 0.06 } else { 0.18 };
                        let tint = style.accent.with_alpha(tint_alpha);
                        p.fill_rect(fill_rect, radius, Brush::Solid(tint));
                    }
                }
            }

            p.stroke_self(&style, &Stroke::new(1.0), Brush::Solid(border));

            let text_color = if disabled {
                style.text_disabled
            } else {
                style.text_primary
            };
            let mut ts = text_style(&style, text_color);
            ts.baseline = TextBaseline::Middle;
            let text_x = p.rect().x() + 8.0;
            let text_y = p.rect().y() + height * 0.5;

            if state.editing {
                // Edit mode — left-aligned so the caret can sit at
                // a stable x while the user types. Right-edge
                // padding mirrors `text_input`.
                if state.scratch.is_empty() {
                    if let Some(ph) = &placeholder {
                        let mut pts = ts.clone();
                        pts.color = style.text_disabled;
                        p.draw_text(ph, &pts, Point::new(text_x, text_y));
                    }
                } else {
                    p.draw_text(&state.scratch, &ts, Point::new(text_x, text_y));
                }

                let safe_caret = state.caret.min(state.scratch.len());
                let before_caret = &state.scratch[..safe_caret];
                let caret_x = text_x + text_width(before_caret, &style);
                let caret_top = p.rect().y() + 4.0;
                let caret_bot = p.rect().y() + p.rect().height() - 4.0;
                let caret_path = blinc_core::draw::Path::new()
                    .move_to(caret_x, caret_top)
                    .line_to(caret_x, caret_bot);
                p.stroke_path(
                    &caret_path,
                    &Stroke::new(1.0),
                    Brush::Solid(style.text_primary),
                );
            } else {
                // Idle / scrub mode — centred text. Use the
                // renderer's TextAlign::Center via the TextStyle
                // (passing the centre x as the origin) rather than
                // computing a left-edge offset by hand — the
                // approx-glyph-advance estimate the latter relied
                // on drifts at small char counts and reads
                // visibly off-centre. Centre vs the chip rect
                // minus half the unit-block so the number sits
                // optically centred when a `.unit(...)` suffix is
                // present.
                let formatted = format_numeric(current, precision, integer);
                let unit_text = unit.as_deref().filter(|u| !u.is_empty());
                let unit_w = unit_text
                    .map(|u| text_width(u, &style) + 4.0)
                    .unwrap_or(0.0);
                let centre_x = p.rect().x() + (p.rect().width() - unit_w) * 0.5;
                let mut centred = ts.clone();
                centred.align = blinc_core::TextAlign::Center;
                p.draw_text(&formatted, &centred, Point::new(centre_x, text_y));
                if let Some(u) = unit_text {
                    let num_w_half = text_width(&formatted, &style) * 0.5;
                    let unit_x = centre_x + num_w_half + 4.0;
                    let mut uts = ts.clone();
                    uts.color = style.text_disabled;
                    p.draw_text(u, &uts, Point::new(unit_x, text_y));
                }
            }
        } // painter dropped

        // Phase 4: post-paint scrub + click-to-edit. Drag-scrub
        // runs only in idle mode; editing eats the drag (text
        // selection wins once that lands).
        //
        // SCRUB MATH. The pixel accumulator integrates per-frame
        // drag delta; once it crosses `pixels_per_step`, that many
        // integer step-units flush into the value and the
        // sub-pixel remainder stays in the accumulator so slow
        // drags still respond. `pixels_per_step` defaults to
        // `BASE_PIXELS_PER_STEP / drag_sensitivity` — at the
        // default sensitivity of 1.0 a single integer step costs
        // 4 pixels of drag (one step every ~quarter inch on a
        // typical trackpad). For continuous mode (step == 0.0) we
        // synthesise a step from the range: range/200 so a full
        // drag across the chip covers a reasonable spread.
        if !disabled && !state.editing {
            let pressed_now = resp_pressed;
            if pressed_now && !state.was_pressed_last_frame {
                state.drag_accum_pixels = 0.0;
            }
            if pressed_now && drag_delta_x.abs() > 0.0 {
                state.drag_accum_pixels += drag_delta_x;
                let step_value = if step > 0.0 {
                    step
                } else {
                    // Continuous-mode synthesised step.
                    let span = match (min, max) {
                        (Some(lo), Some(hi)) => (hi - lo).abs().max(1.0),
                        _ => 1.0,
                    };
                    span / 200.0
                };
                const BASE_PIXELS_PER_STEP: f32 = 4.0;
                let pixels_per_step =
                    (BASE_PIXELS_PER_STEP / drag_sensitivity.max(0.01)).max(0.5);
                let raw_steps = state.drag_accum_pixels / pixels_per_step;
                let integer_steps = raw_steps.trunc();
                if integer_steps.abs() >= 1.0 {
                    let new_val = clamp_value(current + integer_steps * step_value);
                    if (new_val - current).abs() > f32::EPSILON {
                        value.set(new_val);
                        changed = true;
                    }
                    state.drag_accum_pixels -= integer_steps * pixels_per_step;
                }
            }
            state.was_pressed_last_frame = pressed_now;
        }

        if !disabled && resp_clicked {
            // Click semantics: SINGLE click is a no-op (the scrub
            // affordance owns it — drag-to-scrub is the primary
            // interaction). DOUBLE click within
            // `DOUBLE_CLICK_WINDOW_S` enters edit mode where the
            // user can type a value. Avoids the alignment shift
            // single-click-to-edit produced (idle = centred,
            // edit = left-aligned) on accidental clicks.
            const DOUBLE_CLICK_WINDOW_S: f32 = 0.3;
            let now = ui.time();
            if now - state.last_click_time < DOUBLE_CLICK_WINDOW_S
                && state.last_click_time > 0.0
            {
                state.editing = true;
                state.scratch = format_numeric(current, precision, integer);
                state.caret = state.scratch.len();
                state.drag_accum_pixels = 0.0;
                state.last_click_time = 0.0;
                crate::ui::set_focused_region(Some(widget_id.to_region_id(portal_id)));
            } else {
                state.last_click_time = now;
            }
        }

        // Persist state back to storage.
        *ui.storage
            .get_or_insert_with::<NumericInputState, _>(widget_id, NumericInputState::default) =
            state;

        if focused_before || resp_pressed {
            ui.request_animation();
        }

        let mut resp = Response::empty();
        resp.rect = resp_rect;
        resp.hovered = resp_hovered;
        resp.pressed = resp_pressed;
        resp.clicked = resp_clicked;
        resp.changed = changed;
        resp.widget_id = widget_id;
        resp
    }

    pub fn changed(self) -> bool {
        self.show().changed
    }
}

impl<'a> PortalUi<'a> {
    /// Dual-mode numeric input: drag-to-scrub on the chip OR
    /// click-to-edit inline. Accepts either `&mut f32` or
    /// `&Signal<f32>` via [`PortalValue`]. Configure via
    /// `.range(...)` / `.step(...)` / `.integer()` / `.precision()`
    /// / `.unit(...)` / `.drag_sensitivity(...)` / `.disabled(...)`
    /// / `.shadow_*()`. Terminate with `.show()` or `.changed()`.
    pub fn numeric_input<'b, V: PortalValue<'b, f32>>(
        &'b mut self,
        value: V,
    ) -> NumericInputBuilder<'a, 'b> {
        NumericInputBuilder {
            ui: self,
            value: value.into_binding(),
            min: None,
            max: None,
            step: 0.0,
            integer: false,
            precision: 2,
            unit: None,
            placeholder: None,
            drag_sensitivity: 1.0,
            width_override: None,
            disabled: false,
            shadow_token: None,
        }
    }
}

/// Format a numeric value for display. Integer mode rounds
/// half-to-even via `as i64`; float mode formats with the requested
/// precision and no trailing-zero trimming so the user sees a stable
/// width per zoom step.
fn format_numeric(value: f32, precision: u8, integer: bool) -> String {
    if integer {
        format!("{}", value.round() as i64)
    } else {
        format!("{value:.precision$}", precision = precision as usize)
    }
}

// ─────────────────────────────────────────────────────────────────────
// text_input — inline editable single-line field.
//
// Typing is wired through [`install_kbd_hook`]: the outer canvas div
// captures KEY_DOWN + TEXT_INPUT events into per-frame buffers that
// the focused widget drains on its next paint. Click sets focus,
// Escape / click-outside releases it. The widget owns its cursor
// position in [`PortalStorage`]; the caller owns the value.
//
// Supported: typing, Backspace, Delete, Home, End, Left/Right
// arrows, Escape (blur). Selection / clipboard / IME are future
// concerns — single-shot ASCII-level edits are enough for the
// node-editor inspector path that drives this work.
// ─────────────────────────────────────────────────────────────────────

/// Per-widget cursor state stored in [`PortalStorage`].
#[derive(Debug, Default, Clone, Copy)]
struct TextInputState {
    /// Byte offset of the caret in the owning `String`. Clamped on
    /// every read to `value.len()` so external mutations that
    /// shorten the string don't leave a dangling cursor.
    caret: usize,
}

#[must_use = "TextInputBuilder is lazy — call .show() / .changed() to paint"]
pub struct TextInputBuilder<'a, 'b> {
    ui: &'b mut PortalUi<'a>,
    value: ValueBinding<'b, String>,
    disabled: bool,
    placeholder: Option<String>,
    width_override: Option<f32>,
    shadow_token: Option<ShadowToken>,
}

impl<'a, 'b> ShadowMix for TextInputBuilder<'a, 'b> {
    fn shadow(mut self, token: ShadowToken) -> Self {
        self.shadow_token = Some(token);
        self
    }
}

impl<'a, 'b> TextInputBuilder<'a, 'b> {
    pub fn disabled(mut self, b: bool) -> Self {
        self.disabled = b;
        self
    }
    pub fn placeholder(mut self, t: impl Into<String>) -> Self {
        self.placeholder = Some(t.into());
        self
    }
    pub fn width(mut self, w: f32) -> Self {
        self.width_override = Some(w);
        self
    }

    pub fn show(self) -> Response {
        use blinc_core::events::KeyCode;

        let TextInputBuilder {
            ui,
            mut value,
            disabled,
            placeholder,
            width_override,
            shadow_token,
        } = self;
        let style = ui.style.clone();
        let (avail_w, _) = ui.available_size();
        let width = width_override.unwrap_or_else(|| avail_w.clamp(80.0, 240.0));
        let height = style.control_height;

        let widget_id = ui.make_widget_id(None);
        let portal_id = ui.portal_id;
        let focused = !disabled && ui.is_focused(widget_id);

        let mut current = value.get();
        let mut caret = ui
            .storage
            .get_or_insert_with::<TextInputState, _>(widget_id, TextInputState::default)
            .caret
            .min(current.len());
        let mut changed = false;
        let mut release_focus = false;

        if focused {
            let keys: Vec<crate::ui::KbdKey> = ui.kbd_keys_frame.to_vec();
            let chars: Vec<char> = ui.kbd_chars_frame.to_vec();
            for k in &keys {
                let key = KeyCode(k.key_code);
                if key == KeyCode::ESCAPE {
                    release_focus = true;
                    break;
                } else if key == KeyCode::BACKSPACE {
                    if caret > 0 {
                        let new_caret = prev_char_boundary(&current, caret);
                        current.replace_range(new_caret..caret, "");
                        caret = new_caret;
                        changed = true;
                    }
                } else if key == KeyCode::DELETE {
                    if caret < current.len() {
                        let next = next_char_boundary(&current, caret);
                        current.replace_range(caret..next, "");
                        changed = true;
                    }
                } else if key == KeyCode::LEFT {
                    caret = prev_char_boundary(&current, caret);
                } else if key == KeyCode::RIGHT {
                    caret = next_char_boundary(&current, caret);
                } else if key == KeyCode::HOME {
                    caret = 0;
                } else if key == KeyCode::END {
                    caret = current.len();
                }
            }
            for ch in chars {
                if !ch.is_control() {
                    let mut buf = [0u8; 4];
                    let s = ch.encode_utf8(&mut buf);
                    current.insert_str(caret.min(current.len()), s);
                    caret += s.len();
                    changed = true;
                }
            }
        }

        caret = caret.min(current.len());
        ui.storage
            .get_or_insert_with::<TextInputState, _>(widget_id, TextInputState::default)
            .caret = caret;

        if release_focus {
            crate::ui::set_focused_region(None);
        }

        let resp_clicked;
        let resp_hovered;
        let resp_rect;
        let click_local: Option<Point>;
        {
            let sense = if disabled { Sense::Hover } else { Sense::Click };
            let (mut p, r) = ui.allocate_painter_for_id((width, height), sense, widget_id);
            resp_clicked = r.clicked;
            resp_hovered = r.hovered;
            resp_rect = r.rect;
            click_local = if r.clicked { r.pointer_local } else { None };

            if let Some(tok) = shadow_token {
                if !disabled {
                    let theme_shadows = blinc_theme::ThemeState::get().shadows();
                    let stack: Vec<blinc_core::layer::Shadow> = theme_shadows
                        .get(tok)
                        .iter()
                        .map(|s| blinc_core::layer::Shadow::from(s.clone()))
                        .collect();
                    p.shadow_self(&style, &stack);
                }
            }

            let bg = if disabled {
                style.field_bg.with_alpha(0.5)
            } else {
                style.field_bg
            };
            let border = if focused || r.hovered {
                style.field_border_focus
            } else {
                style.field_border
            };
            p.fill_self(&style, Brush::Solid(bg));
            p.stroke_self(&style, &Stroke::new(1.0), Brush::Solid(border));

            let text_color = if disabled {
                style.text_disabled
            } else {
                style.text_primary
            };
            let mut ts = text_style(&style, text_color);
            ts.baseline = TextBaseline::Middle;
            let text_x = p.rect().x() + 8.0;
            let text_y = p.rect().y() + height * 0.5;

            // Placeholder when empty + not focused; same call shape
            // as the actual text path so the renderer doesn't
            // measure two glyphs when only one is needed.
            if current.is_empty() && !focused {
                if let Some(ph) = &placeholder {
                    let mut pts = ts.clone();
                    pts.color = style.text_disabled;
                    p.draw_text(ph, &pts, Point::new(text_x, text_y));
                }
            } else {
                p.draw_text(&current, &ts, Point::new(text_x, text_y));
            }

            if focused {
                let safe_caret = caret.min(current.len());
                let before_caret = &current[..safe_caret];
                let caret_x = text_x + text_width(before_caret, &style);
                let caret_top = p.rect().y() + 4.0;
                let caret_bot = p.rect().y() + p.rect().height() - 4.0;
                let caret_path = blinc_core::draw::Path::new()
                    .move_to(caret_x, caret_top)
                    .line_to(caret_x, caret_bot);
                p.stroke_path(
                    &caret_path,
                    &Stroke::new(1.0),
                    Brush::Solid(style.text_primary),
                );
            }
        }

        if !disabled && resp_clicked {
            crate::ui::set_focused_region(Some(widget_id.to_region_id(portal_id)));
            if let Some(local) = click_local {
                let click_x_in_text = local.x - 8.0;
                let new_caret = caret_from_click_x(&current, &style, click_x_in_text);
                ui.storage
                    .get_or_insert_with::<TextInputState, _>(widget_id, TextInputState::default)
                    .caret = new_caret;
            }
        }
        if focused {
            ui.request_animation();
        }
        if changed {
            value.set(current);
        }

        let mut resp = Response::empty();
        resp.rect = resp_rect;
        resp.hovered = resp_hovered;
        resp.clicked = resp_clicked;
        resp.changed = changed;
        resp.widget_id = widget_id;
        resp
    }
    pub fn changed(self) -> bool {
        self.show().changed
    }
}

impl<'a> PortalUi<'a> {
    /// Editable single-line text field. Accepts `&mut String` or
    /// `&Signal<String>` via [`PortalValue`]; the builder takes
    /// `.placeholder(...)`, `.disabled(...)`, `.width(...)`,
    /// `.shadow_*()` etc. Terminate with `.show()` (full Response)
    /// or `.changed()` (`bool`).
    pub fn text_input<'b, V: PortalValue<'b, String>>(
        &'b mut self,
        value: V,
    ) -> TextInputBuilder<'a, 'b> {
        TextInputBuilder {
            ui: self,
            value: value.into_binding(),
            disabled: false,
            placeholder: None,
            width_override: None,
            shadow_token: None,
        }
    }

    #[deprecated(note = "Use `ui.text_input(&sig).show()` instead — the builder accepts both &mut String and &Signal<String>.")]
    pub fn text_input_signal(&mut self, sig: &Signal<String>) -> Response {
        self.text_input(sig).show()
    }
}

/// Walk `value` char-by-char and return the byte offset where a
/// click at `click_x` (text-relative pixels, i.e. with the 8px
/// left-pad already subtracted) should drop the caret. The caret
/// lands at the closest grapheme boundary — left of a char's
/// midpoint snaps to its left edge; right of it snaps to its right.
/// O(n²) on string length because each iteration re-measures the
/// growing prefix; acceptable for single-line inputs in the
/// hundreds-of-chars regime.
fn caret_from_click_x(value: &str, style: &PortalStyle, click_x: f32) -> usize {
    if click_x <= 0.0 || value.is_empty() {
        return 0;
    }
    let mut last_width = 0.0_f32;
    for (i, c) in value.char_indices() {
        let next_end = i + c.len_utf8();
        let prefix = &value[..next_end];
        let width = text_width(prefix, style);
        let midpoint = (last_width + width) * 0.5;
        if click_x < midpoint {
            return i;
        }
        last_width = width;
    }
    value.len()
}

/// UTF-8 safe step back from a byte offset to the previous char
/// boundary. Returns `offset` unchanged when already at zero.
fn prev_char_boundary(s: &str, offset: usize) -> usize {
    if offset == 0 {
        return 0;
    }
    let mut i = offset - 1;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// UTF-8 safe step forward from a byte offset to the next char
/// boundary. Returns `offset` unchanged when already at end.
fn next_char_boundary(s: &str, offset: usize) -> usize {
    if offset >= s.len() {
        return s.len();
    }
    let mut i = offset + 1;
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

// ─────────────────────────────────────────────────────────────────────
// color_picker — hex-bound trigger chip with an inline preview swatch
// and the hex string label. Same overlay-escape contract as
// `select_trigger`: the widget paints the chip and reports clicks; the
// host opens the wheel popover anchored to `Response::rect` and writes
// the picked hex back into the bound value (typically a
// `Signal<String>`). The host-side popover content is built via
// [`crate::color_wheel_panel`] which returns a `Div` ready to drop
// into `OverlayBuilder::popover().content(...)`.
// ─────────────────────────────────────────────────────────────────────

#[must_use = "ColorPickerBuilder is lazy — call .show() / .clicked() to paint"]
pub struct ColorPickerBuilder<'a, 'b> {
    ui: &'b mut PortalUi<'a>,
    value: ValueBinding<'b, String>,
    width_override: Option<f32>,
    disabled: bool,
    shadow_token: Option<ShadowToken>,
}

impl<'a, 'b> ShadowMix for ColorPickerBuilder<'a, 'b> {
    fn shadow(mut self, token: ShadowToken) -> Self {
        self.shadow_token = Some(token);
        self
    }
}

impl<'a, 'b> ColorPickerBuilder<'a, 'b> {
    pub fn disabled(mut self, b: bool) -> Self {
        self.disabled = b;
        self
    }
    pub fn width(mut self, w: f32) -> Self {
        self.width_override = Some(w);
        self
    }

    pub fn show(self) -> Response {
        let ColorPickerBuilder {
            ui,
            value,
            width_override,
            disabled,
            shadow_token,
        } = self;
        let style = ui.style.clone();
        let height = style.control_height;
        let hex = value.get();
        // Width: hex label + swatch + padding. 6-digit hex is the
        // common case so we size around `#ffffff` worst case (7 chars
        // including hash). Wider when the bound value is the 8-digit
        // alpha-included form.
        let hex_w = text_width(&hex, &style).max(text_width("#ffffff", &style));
        // 4 px grid: swatch padding 4 px, swatch→label gap 4 px,
        // label→right-edge gap 12 px (3 units of breathing room).
        let swatch_size = (height - 8.0).max(12.0);
        let default_w = swatch_size + 4.0 + hex_w + 12.0;
        let width = width_override.unwrap_or(default_w);

        let sense = if disabled { Sense::Hover } else { Sense::Click };
        let (mut p, mut resp) = ui.allocate_painter((width, height), sense);

        if let Some(tok) = shadow_token {
            if !disabled {
                let theme_shadows = blinc_theme::ThemeState::get().shadows();
                let stack: Vec<blinc_core::layer::Shadow> = theme_shadows
                    .get(tok)
                    .iter()
                    .map(|s| blinc_core::layer::Shadow::from(s.clone()))
                    .collect();
                p.shadow_self(&style, &stack);
            }
        }

        let bg = if disabled {
            style.field_bg.with_alpha(0.5)
        } else if resp.pressed {
            style.button_pressed
        } else if resp.hovered {
            style.button_hover
        } else {
            style.field_bg
        };
        let border = if resp.hovered && !disabled {
            style.field_border_focus
        } else {
            style.field_border
        };
        p.fill_self(&style, Brush::Solid(bg));
        p.stroke_self(&style, &Stroke::new(1.0), Brush::Solid(border));

        // Swatch — left-anchored, vertically centred. Parses the
        // bound hex; falls back to `field_bg` when the string isn't a
        // valid colour so the chip never goes blank on a typo.
        // 4 px left pad on the swatch, 4 px between swatch and label.
        let swatch_x = p.rect().x() + 4.0;
        let swatch_y = p.rect().y() + (height - swatch_size) * 0.5;
        let swatch_rect =
            blinc_core::layer::Rect::new(swatch_x, swatch_y, swatch_size, swatch_size);
        let swatch_color = Color::from_hex_str(&hex).unwrap_or(style.field_bg);
        let swatch_radius = blinc_core::layer::CornerRadius::uniform(4.0);
        p.fill_rect(swatch_rect, swatch_radius, Brush::Solid(swatch_color));
        p.stroke_rect(
            swatch_rect,
            swatch_radius,
            &Stroke::new(1.0),
            Brush::Solid(style.field_border),
        );

        // Hex label.
        let label_color = if disabled {
            style.text_disabled
        } else {
            style.text_primary
        };
        let mut ts = text_style(&style, label_color);
        ts.baseline = TextBaseline::Middle;
        let label_x = swatch_x + swatch_size + 4.0;
        let label_y = p.rect().y() + height * 0.5;
        if !hex.is_empty() {
            p.draw_text(&hex, &ts, Point::new(label_x, label_y));
        }

        if disabled {
            resp.clicked = false;
            resp.pressed = false;
        }
        resp
    }

    pub fn clicked(self) -> bool {
        self.show().clicked
    }
}

impl<'a> PortalUi<'a> {
    /// Paint a colour-picker trigger chip bound to a hex
    /// `String`. The chip shows a small swatch at the bound colour, the
    /// canonical hex label, and a chevron. Returns a
    /// [`ColorPickerBuilder`]; chain `.disabled(...)` / `.width(...)` /
    /// `.shadow_*()` and terminate with `.show()` (full Response) or
    /// `.clicked()` (`bool`).
    ///
    /// The host opens the wheel popover when `resp.clicked` is true by
    /// anchoring against `resp.rect` through
    /// [`crate::core::HostBridge::rect_to_screen`] and mounting
    /// [`crate::color_wheel_panel(bound_signal)`] as the overlay's
    /// content. The widget never reaches into the overlay manager
    /// itself — same contract as [`Self::select_trigger`].
    pub fn color_picker<'b, V: PortalValue<'b, String>>(
        &'b mut self,
        value: V,
    ) -> ColorPickerBuilder<'a, 'b> {
        ColorPickerBuilder {
            ui: self,
            value: value.into_binding(),
            width_override: None,
            disabled: false,
            shadow_token: None,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// Inline script-editor trigger. Originally a bespoke
// `ScriptEditorBuilder`, but per user feedback (\"reuse the button
// widget and add a script icon\") it collapsed into
// `ui.button(&preview).icon(\"{ }\")`. The host helper that mounts the
// code-editor popover (see `node_editor_demo::open_script_editor_popover`)
// owns the preview-string composition (`first non-blank line` +
// optional `+N more` suffix) and the host language → SyntaxConfig
// mapping. portal_ui stays out of the syntax module entirely.
// ─────────────────────────────────────────────────────────────────────

// ─────────────────────────────────────────────────────────────────────
// select_trigger — dropdown chip rendered as a button-styled field.
// The widget paints the trigger and reports clicks; opening the
// menu is the host's job (overlay-escape contract per the README's
// "Overlay escape" section). Two-line summary of usage:
//
//   if ui.select_trigger("Mode: Linear").clicked {
//       let anchor = host_bridge.rect_to_screen(resp.rect);
//       open_select_overlay(options, anchor, |picked| current.set(picked));
//   }
//
// `display_text` is the label of the currently-selected option;
// callers typically resolve it once before the paint. Keeping the
// API trigger-only means portal_ui never reaches into the overlay
// manager — same z-order, dismiss rules, and theming as every
// other host popover.
// ─────────────────────────────────────────────────────────────────────

#[must_use = "SelectBuilder is lazy — call .show() / .clicked() to paint"]
pub struct SelectBuilder<'a, 'b> {
    ui: &'b mut PortalUi<'a>,
    display_text: String,
    disabled: bool,
    placeholder: Option<String>,
    width_override: Option<f32>,
    shadow_token: Option<ShadowToken>,
}

impl<'a, 'b> ShadowMix for SelectBuilder<'a, 'b> {
    fn shadow(mut self, token: ShadowToken) -> Self {
        self.shadow_token = Some(token);
        self
    }
}

impl<'a, 'b> SelectBuilder<'a, 'b> {
    pub fn disabled(mut self, b: bool) -> Self {
        self.disabled = b;
        self
    }
    pub fn placeholder(mut self, t: impl Into<String>) -> Self {
        self.placeholder = Some(t.into());
        self
    }
    pub fn width(mut self, w: f32) -> Self {
        self.width_override = Some(w);
        self
    }

    pub fn show(self) -> Response {
        let SelectBuilder {
            ui,
            display_text,
            disabled,
            placeholder,
            width_override,
            shadow_token,
        } = self;
        let style = ui.style.clone();
        let (avail_w, _) = ui.available_size();
        let width = width_override.unwrap_or_else(|| avail_w.clamp(80.0, 240.0));
        let height = style.control_height;
        let sense = if disabled { Sense::Hover } else { Sense::Click };
        let (mut p, mut resp) = ui.allocate_painter((width, height), sense);

        if let Some(tok) = shadow_token {
            if !disabled {
                let theme_shadows = blinc_theme::ThemeState::get().shadows();
                let stack: Vec<blinc_core::layer::Shadow> = theme_shadows
                    .get(tok)
                    .iter()
                    .map(|s| blinc_core::layer::Shadow::from(s.clone()))
                    .collect();
                p.shadow_self(&style, &stack);
            }
        }

        let bg = if disabled {
            style.field_bg.with_alpha(0.5)
        } else if resp.pressed {
            style.button_pressed
        } else if resp.hovered {
            style.button_hover
        } else {
            style.field_bg
        };
        let border = if resp.hovered && !disabled {
            style.field_border_focus
        } else {
            style.field_border
        };
        p.fill_self(&style, Brush::Solid(bg));
        p.stroke_self(&style, &Stroke::new(1.0), Brush::Solid(border));

        let (label_text, label_color) = if display_text.is_empty() {
            match placeholder {
                Some(ph) => (ph, style.text_disabled),
                None => (String::new(), style.text_primary),
            }
        } else if disabled {
            (display_text, style.text_disabled)
        } else {
            (display_text, style.text_primary)
        };
        let mut ts = text_style(&style, label_color);
        ts.baseline = TextBaseline::Middle;
        let origin = Point::new(p.rect().x() + 8.0, p.rect().y() + height * 0.5);
        if !label_text.is_empty() {
            p.draw_text(&label_text, &ts, origin);
        }

        let chev_cx = p.rect().x() + p.rect().width() - 12.0;
        let chev_cy = p.rect().y() + p.rect().height() * 0.5;
        let chev_half_w = 4.0_f32;
        let chev_half_h = 2.5_f32;
        let path = blinc_core::draw::Path::new()
            .move_to(chev_cx - chev_half_w, chev_cy - chev_half_h)
            .line_to(chev_cx, chev_cy + chev_half_h)
            .line_to(chev_cx + chev_half_w, chev_cy - chev_half_h);
        let chev_color = if disabled {
            style.text_disabled
        } else {
            style.text_secondary
        };
        p.stroke_path(&path, &Stroke::new(1.5), Brush::Solid(chev_color));

        if disabled {
            resp.clicked = false;
            resp.pressed = false;
        }
        resp
    }
    pub fn clicked(self) -> bool {
        self.show().clicked
    }
}

impl<'a> PortalUi<'a> {
    /// Paint a select trigger showing `display_text` on the left and
    /// a downward chevron on the right. Returns a [`SelectBuilder`]
    /// — chain `.disabled(...)` / `.placeholder(...)` /
    /// `.width(...)` / `.shadow_*()`, then terminate with `.show()`
    /// (full Response) or `.clicked()` (`bool`). The host opens the
    /// dropdown menu by anchoring against `Response::rect` via
    /// [`crate::core::HostBridge::rect_to_screen`].
    pub fn select_trigger<'b>(&'b mut self, display_text: &str) -> SelectBuilder<'a, 'b> {
        SelectBuilder {
            ui: self,
            display_text: display_text.to_string(),
            disabled: false,
            placeholder: None,
            width_override: None,
            shadow_token: None,
        }
    }

    /// Convenience: resolve the label for `current_value` against
    /// `(value, label)` pairs and build a [`SelectBuilder`]. If no
    /// option matches `current_value`, the value string itself is
    /// shown so the trigger doesn't render blank when the host's
    /// stored value drifts out of the option set.
    pub fn select<'b, S: AsRef<str>>(
        &'b mut self,
        current_value: &str,
        options: &[(S, S)],
    ) -> SelectBuilder<'a, 'b> {
        let label = options
            .iter()
            .find(|(v, _)| v.as_ref() == current_value)
            .map(|(_, l)| l.as_ref())
            .unwrap_or(current_value)
            .to_string();
        SelectBuilder {
            ui: self,
            display_text: label,
            disabled: false,
            placeholder: None,
            width_override: None,
            shadow_token: None,
        }
    }

    /// Signal-bound `select`. Reads the signal each frame so external
    /// mutations (overlay item-click writes, FSM updates) re-paint
    /// the trigger automatically. Writes do NOT happen here — the
    /// host's menu closure calls `sig.set(picked)` from its own
    /// click handler.
    pub fn select_signal<'b, S: AsRef<str>>(
        &'b mut self,
        sig: &Signal<String>,
        options: &[(S, S)],
    ) -> SelectBuilder<'a, 'b> {
        let current = sig.get();
        self.select(&current, options)
    }
}

// ─────────────────────────────────────────────────────────────────────
// Unused-import suppressions for items only referenced behind cfg
// ─────────────────────────────────────────────────────────────────────

#[allow(dead_code)]
fn _ctrl_radius_marker(style: &PortalStyle) -> blinc_core::layer::CornerRadius {
    ctrl_radius(style)
}
