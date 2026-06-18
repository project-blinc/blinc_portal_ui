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
        } = self;
        let style = ui.style.clone();
        let pad_x = 10.0_f32;
        let width = approx_text_width(&label, &style) + pad_x * 2.0;
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
        ts.align = TextAlign::Center;
        ts.baseline = TextBaseline::Middle;
        let centre_x = p.rect().x() + p.rect().width() * 0.5;
        let centre_y = p.rect().y() + p.rect().height() * 0.5;
        p.draw_text(&label, &ts, Point::new(centre_x, centre_y));

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
