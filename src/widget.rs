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
// chart — inline dataflow visualiser. Sparkline-style line + bar
// variants for node-content slots. Reads from a `PortalValue<Vec<f32>>`
// so both `&mut Vec<f32>` and `&Signal<Vec<f32>>` work. Decimates to
// the painter's column budget when `data.len()` exceeds it; auto-
// scales y by default with `.y_range(...)` override. Paint-only —
// `Response::changed` is always `false` (parity with the other
// builders).
// ─────────────────────────────────────────────────────────────────────

/// Chart render style.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ChartVariant {
    /// Connected line strip across the data points. Default.
    #[default]
    Line,
    /// One filled rect per sample, baseline-anchored.
    Bar,
    /// Line + fill under it down to the baseline.
    Area,
}

/// Decimation strategy when `data.len()` exceeds the painter's
/// column budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ChartDecimation {
    /// Index-stride pick — collapses to N evenly-spaced samples;
    /// the last sample is always preserved so a sparkline shows
    /// the current value. Cheap and visually fine for slow-moving
    /// telemetry.
    #[default]
    Stride,
    /// Min/max bucketing — each bucket emits two anchors (min then
    /// max) so peaks survive at any decimation ratio. Doubles the
    /// vertex count vs `Stride` but preserves the envelope of
    /// noisy signals.
    MinMax,
}

#[must_use = "ChartsBuilder is lazy — call .show() / .changed() to paint"]
pub struct ChartsBuilder<'a, 'b> {
    ui: &'b mut PortalUi<'a>,
    value: ValueBinding<'b, Vec<f32>>,
    variant: ChartVariant,
    width_override: Option<f32>,
    height_override: Option<f32>,
    y_range: Option<std::ops::Range<f32>>,
    line_width: f32,
    bar_gap: f32,
    show_baseline: bool,
    show_latest: bool,
    fill_area: bool,
    decimation: ChartDecimation,
    stroke_color: Option<Color>,
    fill_color: Option<Color>,
    baseline_color: Option<Color>,
    background: Option<Color>,
    disabled: bool,
    shadow_token: Option<ShadowToken>,
    /// Hover tooltip showing the data value at the cursor's x.
    /// Defaults `true` — a hover crosshair + value pill render
    /// when the pointer is over the plot area. Disable for
    /// strictly passive sparklines.
    tooltip: bool,
    /// Decimal precision for the tooltip's number formatting.
    /// `None` picks a sensible default from the data range
    /// (`< 1` → 3 dp, `< 100` → 2 dp, else 1 dp).
    tooltip_precision: Option<u8>,
    /// Optional unit suffix appended to the tooltip number
    /// ("ms", "px", "%").
    tooltip_unit: Option<String>,
    /// Show a "picture-in-picture" corner icon. Click sets
    /// `Response::pip_clicked = true` so the host can mount an
    /// expanded view (typically in an overlay popover). Opt-in
    /// because most sparklines are read-only.
    pip: bool,
}

impl<'a, 'b> ShadowMix for ChartsBuilder<'a, 'b> {
    fn shadow(mut self, token: ShadowToken) -> Self {
        self.shadow_token = Some(token);
        self
    }
}

impl<'a, 'b> ChartsBuilder<'a, 'b> {
    pub fn variant(mut self, v: ChartVariant) -> Self {
        self.variant = v;
        self
    }
    pub fn line(mut self) -> Self {
        self.variant = ChartVariant::Line;
        self
    }
    pub fn bar(mut self) -> Self {
        self.variant = ChartVariant::Bar;
        self
    }
    pub fn area(mut self) -> Self {
        self.variant = ChartVariant::Area;
        self.fill_area = true;
        self
    }
    pub fn width(mut self, w: f32) -> Self {
        self.width_override = Some(w);
        self
    }
    pub fn height(mut self, h: f32) -> Self {
        self.height_override = Some(h);
        self
    }
    pub fn y_range(mut self, r: std::ops::Range<f32>) -> Self {
        self.y_range = Some(r);
        self
    }
    pub fn line_width(mut self, w: f32) -> Self {
        self.line_width = w.max(0.5);
        self
    }
    pub fn bar_gap(mut self, g: f32) -> Self {
        self.bar_gap = g.max(0.0);
        self
    }
    pub fn fill_area(mut self, b: bool) -> Self {
        self.fill_area = b;
        self
    }
    pub fn show_baseline(mut self, b: bool) -> Self {
        self.show_baseline = b;
        self
    }
    pub fn show_latest(mut self, b: bool) -> Self {
        self.show_latest = b;
        self
    }
    pub fn decimation(mut self, d: ChartDecimation) -> Self {
        self.decimation = d;
        self
    }
    pub fn stroke_color(mut self, c: Color) -> Self {
        self.stroke_color = Some(c);
        self
    }
    pub fn fill_color(mut self, c: Color) -> Self {
        self.fill_color = Some(c);
        self
    }
    pub fn baseline_color(mut self, c: Color) -> Self {
        self.baseline_color = Some(c);
        self
    }
    pub fn background(mut self, c: Color) -> Self {
        self.background = Some(c);
        self
    }
    pub fn disabled(mut self, b: bool) -> Self {
        self.disabled = b;
        self
    }
    /// Toggle the hover-tooltip pass — a crosshair + value pill
    /// painted near the cursor when the pointer is over the
    /// chart. Defaults `true` for charts; `false` if the host
    /// wants a strictly passive sparkline.
    pub fn tooltip(mut self, b: bool) -> Self {
        self.tooltip = b;
        self
    }
    /// Decimal precision used to format the tooltip's value. By
    /// default the precision is chosen from the data range
    /// (3 dp below 1, 2 dp below 100, 1 dp above).
    pub fn tooltip_precision(mut self, p: u8) -> Self {
        self.tooltip_precision = Some(p);
        self
    }
    /// Trailing unit suffix shown in the tooltip ("ms", "px",
    /// "%"). The string is appended as-is; pad with a space if
    /// you want a gap.
    pub fn tooltip_unit(mut self, u: impl Into<String>) -> Self {
        self.tooltip_unit = Some(u.into());
        self
    }
    /// Enable a "picture-in-picture" corner icon. When the user
    /// clicks it, `Response::pip_clicked` flips to `true` so the
    /// host can mount an expanded popover (the chart never owns
    /// the overlay — same escape-the-canvas contract every other
    /// portal_ui widget uses). Opt-in.
    pub fn pip(mut self, b: bool) -> Self {
        self.pip = b;
        self
    }

    pub fn show(self) -> Response {
        use blinc_core::draw::{LineCap, LineJoin};
        use blinc_core::layer::{CornerRadius, Rect};

        let ChartsBuilder {
            ui,
            value,
            variant,
            width_override,
            height_override,
            y_range,
            line_width,
            bar_gap,
            show_baseline,
            show_latest,
            fill_area,
            decimation,
            stroke_color,
            fill_color,
            baseline_color,
            background,
            disabled,
            shadow_token,
            tooltip,
            tooltip_precision,
            tooltip_unit,
            pip,
        } = self;

        let style = ui.style.clone();
        let (avail_w, _) = ui.available_size();
        // Defaults — 240 × 80 is the inline-sparkline sweet spot.
        // Width follows the parent allocation when unset, clamped
        // to a sensible band; height pins to 80 px (20 grid units).
        let width = width_override.unwrap_or_else(|| avail_w.clamp(120.0, 240.0));
        let height = height_override.unwrap_or(80.0);

        let data: Vec<f32> = value.get();
        // PiP icon needs click detection; tooltip needs hover.
        // Click implies hover so PiP-enabled charts get both.
        let sense = if pip && !disabled {
            Sense::Click
        } else if tooltip && !disabled {
            Sense::Hover
        } else {
            Sense::None
        };
        // Reserve a 24 px top strip when PiP is enabled — the
        // button (20 px) + 4 px breathing room — so the icon
        // never overlaps the data series. The painter height
        // grows to absorb the strip so the visible plot area
        // matches what the caller asked for.
        let pip_strip = if pip && !disabled { 24.0 } else { 0.0 };
        let alloc_h = height + pip_strip;
        let (mut p, mut resp) = ui.allocate_painter((width, alloc_h), sense);

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
        if let Some(bg) = background {
            let bg = if disabled { bg.with_alpha(0.5) } else { bg };
            p.fill_self(&style, Brush::Solid(bg));
        }

        // 4 px inner padding so series strokes don't kiss the
        // painter rect edge. PiP-enabled charts add `pip_strip`
        // to the top so the icon button sits above the data.
        let pad = 4.0_f32;
        let plot_x = p.rect().x() + pad;
        let plot_y = p.rect().y() + pad + pip_strip;
        let plot_w = (p.rect().width() - 2.0 * pad).max(0.0);
        let plot_h = (p.rect().height() - 2.0 * pad - pip_strip).max(0.0);
        if plot_w < 2.0 || plot_h < 2.0 || data.is_empty() {
            return resp;
        }

        // NaN-safe lo/hi.
        let mut lo = f32::INFINITY;
        let mut hi = f32::NEG_INFINITY;
        for &v in &data {
            if v.is_finite() {
                lo = lo.min(v);
                hi = hi.max(v);
            }
        }
        let (lo, hi) = if !lo.is_finite() || !hi.is_finite() {
            (0.0, 1.0)
        } else if let Some(r) = y_range.clone() {
            (r.start, r.end)
        } else if (hi - lo).abs() < 1e-6 {
            (lo - 0.5, hi + 0.5)
        } else {
            (lo, hi)
        };
        let span = (hi - lo).max(1e-6);
        let y_of = |v: f32| -> f32 {
            let t = ((v - lo) / span).clamp(0.0, 1.0);
            plot_y + plot_h * (1.0 - t)
        };

        // Column budget — Line/Area get one vertex per pixel, Bar
        // gets one column per `(1 + bar_gap)` px.
        let col_budget: usize = match variant {
            ChartVariant::Bar => ((plot_w / (1.0 + bar_gap)).floor() as usize).max(1),
            _ => (plot_w.floor() as usize).max(2),
        };

        let samples: Vec<f32> = if data.len() <= col_budget {
            data.iter()
                .map(|v| if v.is_finite() { *v } else { lo })
                .collect()
        } else {
            match decimation {
                ChartDecimation::Stride => {
                    let n = col_budget;
                    (0..n)
                        .map(|i| {
                            let idx = (i * data.len()) / n;
                            let v = data[idx.min(data.len() - 1)];
                            if v.is_finite() {
                                v
                            } else {
                                lo
                            }
                        })
                        .collect()
                }
                ChartDecimation::MinMax => {
                    let buckets = (col_budget / 2).max(1);
                    let mut out: Vec<f32> = Vec::with_capacity(buckets * 2);
                    for b in 0..buckets {
                        let s = (b * data.len()) / buckets;
                        let e = ((b + 1) * data.len()) / buckets;
                        let end = e.max(s + 1).min(data.len());
                        let slice = &data[s..end];
                        let mut mn = f32::INFINITY;
                        let mut mx = f32::NEG_INFINITY;
                        for &v in slice {
                            if v.is_finite() {
                                mn = mn.min(v);
                                mx = mx.max(v);
                            }
                        }
                        if !mn.is_finite() {
                            mn = lo;
                            mx = lo;
                        }
                        out.push(mn);
                        out.push(mx);
                    }
                    out
                }
            }
        };

        // Theme palette resolution.
        let stroke_brush_base = stroke_color.unwrap_or(style.accent);
        let fill_brush_base = fill_color.unwrap_or(style.accent.with_alpha(0.15));
        let baseline_col_base =
            baseline_color.unwrap_or(style.text_secondary.with_alpha(0.3));
        let (stroke_brush, fill_brush, baseline_col) = if disabled {
            (
                stroke_brush_base.with_alpha(0.4),
                fill_brush_base.with_alpha(0.4),
                baseline_col_base.with_alpha(0.4),
            )
        } else {
            (stroke_brush_base, fill_brush_base, baseline_col_base)
        };

        if show_baseline {
            let by = if lo <= 0.0 && hi >= 0.0 {
                y_of(0.0)
            } else {
                plot_y + plot_h
            };
            p.fill_rect(
                Rect::new(plot_x, by - 0.5, plot_w, 1.0),
                CornerRadius::uniform(0.0),
                Brush::Solid(baseline_col),
            );
        }

        let n = samples.len();
        let x_of = |i: usize| -> f32 {
            if n <= 1 {
                plot_x
            } else {
                plot_x + (i as f32) * (plot_w / ((n - 1) as f32))
            }
        };

        match variant {
            ChartVariant::Bar => {
                let baseline_y = if lo <= 0.0 && hi >= 0.0 {
                    y_of(0.0)
                } else {
                    plot_y + plot_h
                };
                let col_w = plot_w / n.max(1) as f32;
                let bar_w = (col_w - bar_gap).max(1.0);
                for (i, &v) in samples.iter().enumerate() {
                    let cx = plot_x + (i as f32 + 0.5) * col_w;
                    let bar_x = cx - bar_w * 0.5;
                    let yv = y_of(v);
                    let (top, h) = if yv <= baseline_y {
                        (yv, baseline_y - yv)
                    } else {
                        (baseline_y, yv - baseline_y)
                    };
                    if h >= 0.5 {
                        p.fill_rect(
                            Rect::new(bar_x, top, bar_w, h),
                            CornerRadius::uniform((bar_w * 0.25).min(2.0)),
                            Brush::Solid(stroke_brush),
                        );
                    }
                }
            }
            ChartVariant::Line | ChartVariant::Area => {
                if n == 1 {
                    p.fill_circle(
                        Point::new(x_of(0), y_of(samples[0])),
                        line_width.max(1.5),
                        Brush::Solid(stroke_brush),
                    );
                } else {
                    let mut path = blinc_core::draw::Path::new();
                    path = path.move_to(x_of(0), y_of(samples[0]));
                    for i in 1..n {
                        path = path.line_to(x_of(i), y_of(samples[i]));
                    }
                    let area_on = fill_area || matches!(variant, ChartVariant::Area);
                    if area_on {
                        let base_y = if lo <= 0.0 && hi >= 0.0 {
                            y_of(0.0)
                        } else {
                            plot_y + plot_h
                        };
                        let mut area = blinc_core::draw::Path::new();
                        area = area.move_to(x_of(0), base_y);
                        for i in 0..n {
                            area = area.line_to(x_of(i), y_of(samples[i]));
                        }
                        area = area.line_to(x_of(n - 1), base_y).close();
                        p.fill_path(&area, Brush::Solid(fill_brush));
                    }
                    let stroke = Stroke::new(line_width)
                        .with_cap(LineCap::Round)
                        .with_join(LineJoin::Round);
                    p.stroke_path(&path, &stroke, Brush::Solid(stroke_brush));
                }
                if show_latest {
                    let li = n - 1;
                    p.fill_circle(
                        Point::new(x_of(li), y_of(samples[li])),
                        line_width + 1.0,
                        Brush::Solid(stroke_brush),
                    );
                }
            }
        }

        // ─── PiP corner button ────────────────────────────────
        // Top-right corner affordance that signals the host to
        // mount an expanded view of the chart. Wrapped in a 20-px
        // button-style rect (field_bg + 1 px border) so the
        // glyph reads as a distinct UI control even when the
        // underlying data series overlaps it. Click handoff:
        // transfer resp.clicked → resp.pip_clicked when the
        // cursor sits inside the button at click time. The
        // tooltip suppresses in that same region.
        let pointer_in_pip = if pip && !disabled {
            if let Some(local) = resp.pointer_local {
                let lx = p.rect().x() + local.x;
                let ly = p.rect().y() + local.y;
                let pad = 4.0_f32;
                let btn_size = 20.0_f32;
                let bx = p.rect().x() + p.rect().width() - btn_size - pad;
                let by = p.rect().y() + pad;
                lx >= bx && lx < bx + btn_size && ly >= by && ly < by + btn_size
            } else {
                false
            }
        } else {
            false
        };
        if pip && !disabled {
            let pad = 4.0_f32;
            let btn_size = 20.0_f32;
            let bx = p.rect().x() + p.rect().width() - btn_size - pad;
            let by = p.rect().y() + pad;
            let btn_rect = blinc_core::layer::Rect::new(bx, by, btn_size, btn_size);
            // Use SurfaceElevated (`style.background`) for the
            // chrome bg — `field_bg` blends into the chart's
            // surface; the elevated token is the same shade
            // popovers / lifted controls use, so the button
            // stands off the data.
            let (btn_bg, btn_border, icon_color) = if pointer_in_pip {
                (style.button_hover, style.field_border_focus, style.text_primary)
            } else {
                (style.background, style.field_border, style.text_secondary)
            };
            p.fill_rect(btn_rect, CornerRadius::uniform(4.0), Brush::Solid(btn_bg));
            p.stroke_rect(
                btn_rect,
                CornerRadius::uniform(4.0),
                &Stroke::new(1.0),
                Brush::Solid(btn_border),
            );
            // PiP glyph: 12 × 9 outlined frame + 5 × 4 inner fill
            // tucked into the bottom-right corner. Round the
            // per-axis offsets so the 1.5 px stroke lands on a
            // clean pixel grid; a half-pixel offset bleeds the
            // stroke into anti-alias rows and reads as "off".
            const GW: f32 = 12.0;
            const GH: f32 = 9.0;
            let gx = bx + ((btn_size - GW) * 0.5).round();
            let gy = by + ((btn_size - GH) * 0.5).round();
            p.stroke_rect(
                blinc_core::layer::Rect::new(gx, gy, GW, GH),
                CornerRadius::uniform(1.5),
                &Stroke::new(1.5),
                Brush::Solid(icon_color),
            );
            p.fill_rect(
                blinc_core::layer::Rect::new(gx + 6.0, gy + 5.0, 5.0, 4.0),
                CornerRadius::uniform(1.0),
                Brush::Solid(icon_color),
            );
            if resp.clicked && pointer_in_pip {
                resp.clicked = false;
                resp.pip_clicked = true;
            }
        }

        // ─── tooltip ──────────────────────────────────────────
        // Hover crosshair + value pill at the cursor's x. Maps the
        // cursor's local-x back into the sample index, paints a
        // 1 px crosshair line, dots the matched sample, then
        // renders a small pill near the cursor with the formatted
        // value. Skipped when disabled / no hover / data empty /
        // cursor over the PiP icon.
        if tooltip && !disabled && resp.hovered && !pointer_in_pip {
            if let Some(local) = resp.pointer_local {
                let cur_x = p.rect().x() + local.x;
                let cur_y = p.rect().y() + local.y;
                if cur_x >= plot_x && cur_x <= plot_x + plot_w && n > 0 {
                    let idx = if n == 1 {
                        0
                    } else {
                        let t = (cur_x - plot_x) / plot_w;
                        ((t.clamp(0.0, 1.0)) * (n - 1) as f32).round() as usize
                    };
                    let val = samples[idx];
                    let sample_x = x_of(idx);
                    let sample_y = y_of(val);

                    // Crosshair — 1 px vertical at the sample's x.
                    let crosshair_col = stroke_brush.with_alpha(0.4);
                    p.fill_rect(
                        Rect::new(sample_x - 0.5, plot_y, 1.0, plot_h),
                        CornerRadius::uniform(0.0),
                        Brush::Solid(crosshair_col),
                    );
                    // Sample dot.
                    p.fill_circle(
                        Point::new(sample_x, sample_y),
                        line_width + 1.0,
                        Brush::Solid(stroke_brush),
                    );

                    // Format value — auto precision picks from the
                    // data range when the caller didn't set one.
                    let precision = tooltip_precision.unwrap_or_else(|| {
                        if span.abs() < 1.0 {
                            3
                        } else if span.abs() < 100.0 {
                            2
                        } else {
                            1
                        }
                    });
                    let formatted = format!("{:.1$}", val, precision as usize);
                    let label_text = if let Some(ref u) = tooltip_unit {
                        format!("{}{}", formatted, u)
                    } else {
                        formatted
                    };

                    // Pill chrome — surface bg + 1 px border + 4 px
                    // pad each side. Width follows the text, height
                    // 18 px (snug). Position near the cursor, offset
                    // up + right; clamp to the plot rect.
                    let pad_x = 6.0_f32;
                    let pill_h = 18.0_f32;
                    let label_w = approx_text_width(&label_text, &style);
                    let pill_w = label_w + 2.0 * pad_x;
                    let mut px_pill = sample_x + 8.0;
                    let mut py_pill = cur_y - pill_h - 8.0;
                    if px_pill + pill_w > plot_x + plot_w {
                        px_pill = sample_x - 8.0 - pill_w;
                    }
                    if py_pill < plot_y {
                        py_pill = cur_y + 8.0;
                    }
                    p.fill_rect(
                        Rect::new(px_pill, py_pill, pill_w, pill_h),
                        CornerRadius::uniform(4.0),
                        Brush::Solid(style.field_bg),
                    );
                    p.stroke_rect(
                        Rect::new(px_pill, py_pill, pill_w, pill_h),
                        CornerRadius::uniform(4.0),
                        &Stroke::new(1.0),
                        Brush::Solid(style.field_border),
                    );
                    let mut ts = text_style(&style, style.text_primary);
                    ts.baseline = TextBaseline::Middle;
                    p.draw_text(
                        &label_text,
                        &ts,
                        Point::new(px_pill + pad_x, py_pill + pill_h * 0.5),
                    );
                }
            }
        }

        resp
    }

    pub fn changed(self) -> bool {
        self.show().changed
    }
}

impl<'a> PortalUi<'a> {
    /// Inline dataflow chart — line / bar / area variants bound to
    /// a `Vec<f32>` series. Use for sparklines in node-content
    /// slots, slow-moving telemetry, simple histograms. Chain
    /// `.line()` / `.bar()` / `.area()` / `.show_latest()` /
    /// `.show_baseline()` / `.y_range(...)` / `.width()` /
    /// `.height()` / `.shadow_*()` then `.show()` / `.changed()`.
    ///
    /// Decimates to the painter's column budget when `data.len()`
    /// exceeds it (`.decimation(...)` picks `Stride` vs `MinMax`).
    /// Reads theme tokens for stroke / fill / baseline; explicit
    /// `.stroke_color(...)` etc. overrides exist for series that
    /// want a non-accent palette (e.g. warning / error tones).
    pub fn chart<'b, V: PortalValue<'b, Vec<f32>>>(
        &'b mut self,
        data: V,
    ) -> ChartsBuilder<'a, 'b> {
        ChartsBuilder {
            ui: self,
            value: data.into_binding(),
            variant: ChartVariant::Line,
            width_override: None,
            height_override: None,
            y_range: None,
            line_width: 1.5,
            bar_gap: 2.0,
            show_baseline: false,
            show_latest: false,
            fill_area: false,
            decimation: ChartDecimation::Stride,
            stroke_color: None,
            fill_color: None,
            baseline_color: None,
            background: None,
            disabled: false,
            shadow_token: None,
            tooltip: true,
            tooltip_precision: None,
            tooltip_unit: None,
            pip: false,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// pie_chart — slice-weight visualiser. Distinct from `chart` because
// the data shape (weights vs time series) and the knobs (slice
// palette + inner radius) don't overlap. Use for portion / mix
// breakdowns: connection-state counts, port-kind distribution, etc.
// ─────────────────────────────────────────────────────────────────────

#[must_use = "PieChartBuilder is lazy — call .show() / .changed() to paint"]
pub struct PieChartBuilder<'a, 'b> {
    ui: &'b mut PortalUi<'a>,
    value: ValueBinding<'b, Vec<f32>>,
    diameter: Option<f32>,
    /// Hole radius as a fraction of outer radius. `0.0` = solid
    /// pie, `0.5` = donut, `0.85` = ring gauge.
    inner_ratio: f32,
    /// Explicit per-slice palette. `None` rotates through HSV
    /// variations of the theme accent so each slice gets a
    /// distinct, theme-coherent colour.
    palette: Option<Vec<Color>>,
    /// 4 px gap between slices (rendered as a thin radial wedge in
    /// the background colour). `0.0` = continuous fill.
    slice_gap: f32,
    background: Option<Color>,
    disabled: bool,
    shadow_token: Option<ShadowToken>,
    /// Centre the pie horizontally inside the portal's available
    /// width. Default `true` — the pie sits in the middle of the
    /// content slot rather than flush-left.
    center: bool,
    /// Hover tooltip showing the slice's weight + percentage.
    /// Default `true`.
    tooltip: bool,
    /// Optional unit suffix appended to the slice value in the
    /// tooltip ("ms", "px", "%").
    tooltip_unit: Option<String>,
    /// Show a "picture-in-picture" corner icon. Click flips
    /// `Response::pip_clicked` so the host can open an expanded
    /// popover. Opt-in.
    pip: bool,
}

impl<'a, 'b> ShadowMix for PieChartBuilder<'a, 'b> {
    fn shadow(mut self, token: ShadowToken) -> Self {
        self.shadow_token = Some(token);
        self
    }
}

impl<'a, 'b> PieChartBuilder<'a, 'b> {
    /// Outer diameter in pixels. Default 96 (24 grid units). The
    /// painter rect is `diameter × diameter`; the pie is centred.
    pub fn diameter(mut self, d: f32) -> Self {
        self.diameter = Some(d.max(8.0));
        self
    }
    /// Hole radius as a fraction of the outer radius.
    /// `0.0` (default) is a solid pie; `0.5` is a donut;
    /// `>= 0.95` clamps to a thin ring so the inner / outer arcs
    /// don't collapse on top of each other.
    pub fn inner_ratio(mut self, r: f32) -> Self {
        self.inner_ratio = r.clamp(0.0, 0.95);
        self
    }
    /// Convenience for `.inner_ratio(0.5)`.
    pub fn donut(mut self) -> Self {
        self.inner_ratio = 0.5;
        self
    }
    /// Explicit per-slice colour palette. Cycles when `palette.len() < data.len()`.
    /// Pass theme tokens (`ColorToken::Accent`, `::Success`, etc.)
    /// resolved at the call site to stay theme-coherent.
    pub fn palette(mut self, colors: Vec<Color>) -> Self {
        self.palette = Some(colors);
        self
    }
    /// Background colour painted under the pie. `None` = transparent.
    pub fn background(mut self, c: Color) -> Self {
        self.background = Some(c);
        self
    }
    /// Pixel gap between adjacent slices (rendered by painting a
    /// thin radial gap in the background colour over the join).
    /// Disabled when `background` is None — the gap needs a
    /// concrete colour to paint over the slice fills.
    pub fn slice_gap(mut self, px: f32) -> Self {
        self.slice_gap = px.max(0.0);
        self
    }
    pub fn disabled(mut self, b: bool) -> Self {
        self.disabled = b;
        self
    }
    /// Centre the pie horizontally inside the portal's available
    /// width. Default `true`. Disable for hosts that already
    /// position the pie via their own layout.
    pub fn center(mut self, b: bool) -> Self {
        self.center = b;
        self
    }
    /// Toggle the hover tooltip — slice weight + share-of-total
    /// pill painted near the cursor while pointing at a slice.
    pub fn tooltip(mut self, b: bool) -> Self {
        self.tooltip = b;
        self
    }
    /// Trailing unit suffix shown after the slice value in the
    /// tooltip ("ms", "px", "%").
    pub fn tooltip_unit(mut self, u: impl Into<String>) -> Self {
        self.tooltip_unit = Some(u.into());
        self
    }
    /// Enable the "picture-in-picture" corner icon. When the user
    /// clicks it, `Response::pip_clicked` flips to `true` so the
    /// host can open an expanded popover.
    pub fn pip(mut self, b: bool) -> Self {
        self.pip = b;
        self
    }

    pub fn show(self) -> Response {
        use blinc_core::layer::{CornerRadius, Rect};

        let PieChartBuilder {
            ui,
            value,
            diameter,
            inner_ratio,
            palette,
            slice_gap,
            background,
            disabled,
            shadow_token,
            center,
            tooltip,
            tooltip_unit,
            pip,
        } = self;

        let style = ui.style.clone();
        let d = diameter.unwrap_or(96.0);
        let weights: Vec<f32> = value.get();

        // When centring, allocate a painter as wide as the
        // portal's available width and paint the pie centred
        // inside it. Without this the pie sits flush-left in the
        // content slot.
        let (avail_w, _) = ui.available_size();
        let alloc_w = if center {
            avail_w.max(d).min(avail_w.max(d))
        } else {
            d
        };
        let sense = if pip && !disabled {
            Sense::Click
        } else if tooltip && !disabled {
            Sense::Hover
        } else {
            Sense::None
        };
        let (mut p, mut resp) = ui.allocate_painter((alloc_w, d), sense);

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
        if let Some(bg) = background {
            let bg = if disabled { bg.with_alpha(0.5) } else { bg };
            p.fill_self(&style, Brush::Solid(bg));
        }

        // Total + finite filter — NaN / negative weights ignored.
        let mut total = 0.0_f32;
        for &w in &weights {
            if w.is_finite() && w > 0.0 {
                total += w;
            }
        }
        if total <= 0.0 || d < 8.0 {
            return resp;
        }

        // Centre the pie within the allocated painter rect.
        let cx = p.rect().x() + alloc_w * 0.5;
        let cy = p.rect().y() + d * 0.5;
        let outer_r = (d * 0.5) - 2.0;
        let inner_r = outer_r * inner_ratio;
        let alpha = if disabled { 0.4 } else { 1.0 };
        let (h0, _s0, _v0, _a0) = style.accent.to_hsva();

        // Default palette — HSV-rotate the theme accent through
        // golden-angle steps so adjacent slices stay distinct.
        let default_color = |i: usize| -> Color {
            const GOLDEN: f32 = 137.508;
            let h = (h0 + GOLDEN * i as f32).rem_euclid(360.0);
            Color::from_hsva(h, 0.55, 0.92, alpha)
        };

        // Sweep slices CW from -PI/2 (top of pie at 12 o'clock).
        // First pass — collect each visible slice's (data_idx,
        // a0, a1, weight, color) so the paint loop has stable
        // colour state and the tooltip hit-test below can reuse
        // the same angular spans without re-running the
        // accumulation.
        let two_pi = std::f32::consts::TAU;
        let start_offset = -std::f32::consts::FRAC_PI_2;
        struct Slice {
            weight: f32,
            a0: f32,
            a1: f32,
            color: Color,
        }
        let mut spans: Vec<Slice> = Vec::with_capacity(weights.len());
        let mut acc = 0.0_f32;
        let mut slice_idx = 0;
        for (i, &w) in weights.iter().enumerate() {
            if !w.is_finite() || w <= 0.0 {
                continue;
            }
            let a0 = start_offset + acc / total * two_pi;
            acc += w;
            let a1 = start_offset + acc / total * two_pi;

            let color = palette
                .as_ref()
                .and_then(|p| p.get(i % p.len()).copied())
                .map(|c| if disabled { c.with_alpha(0.4) } else { c })
                .unwrap_or_else(|| default_color(slice_idx));
            slice_idx += 1;
            let _ = i;
            spans.push(Slice {
                weight: w,
                a0,
                a1,
                color,
            });
        }
        for slice in &spans {
            let a0 = slice.a0;
            let a1 = slice.a1;
            let color = slice.color;

            // Build slice path as a many-segment polygon — sample
            // the outer arc (and inner arc for donuts) at small
            // steps and emit `line_to` per vertex. Avoids
            // `arc_to`'s SVG center-resolution math which was
            // producing self-intersecting petal shapes at pie
            // scales (single arc spanning >> 90°); a polygon with
            // 1° steps reads as a smooth disk at common chart
            // sizes (96 × 96 px) without any tessellation surprises.
            let span = a1 - a0;
            let steps = ((span.abs().to_degrees() as i32).max(8) as usize).max(8);
            let step_da = span / steps as f32;
            let mut path = blinc_core::draw::Path::new();
            if inner_r > 0.0 {
                // Donut wedge — walk outer CW, inner CCW back.
                let a = a0;
                let inner_start =
                    Point::new(cx + inner_r * a.cos(), cy + inner_r * a.sin());
                path = path.move_to(inner_start.x, inner_start.y);
                // Outer arc (a0 → a1), step by step.
                for k in 0..=steps {
                    let ak = a0 + step_da * k as f32;
                    let x = cx + outer_r * ak.cos();
                    let y = cy + outer_r * ak.sin();
                    path = path.line_to(x, y);
                }
                // Inner arc back (a1 → a0), step by step.
                for k in 0..=steps {
                    let ak = a1 - step_da * k as f32;
                    let x = cx + inner_r * ak.cos();
                    let y = cy + inner_r * ak.sin();
                    path = path.line_to(x, y);
                }
                path = path.close();
            } else {
                // Solid-pie wedge — centre → outer arc → centre.
                path = path.move_to(cx, cy);
                for k in 0..=steps {
                    let ak = a0 + step_da * k as f32;
                    let x = cx + outer_r * ak.cos();
                    let y = cy + outer_r * ak.sin();
                    path = path.line_to(x, y);
                }
                path = path.close();
            }
            p.fill_path(&path, Brush::Solid(color));
        }

        // Slice-gap stroke pass — paint over the slice seams in
        // the background colour. Only meaningful when a
        // background was declared; transparent backgrounds skip
        // the gap (would punch a hole through to whatever sits
        // below).
        if slice_gap > 0.0 {
            if let Some(bg) = background {
                let bg = if disabled { bg.with_alpha(0.5) } else { bg };
                for slice in &spans {
                    let a = slice.a0;
                    let (sa, ca) = (a.sin(), a.cos());
                    let inner_pt =
                        Point::new(cx + inner_r.max(0.0) * ca, cy + inner_r.max(0.0) * sa);
                    let outer_pt = Point::new(cx + outer_r * ca, cy + outer_r * sa);
                    let gap_path = blinc_core::draw::Path::new()
                        .move_to(inner_pt.x, inner_pt.y)
                        .line_to(outer_pt.x, outer_pt.y);
                    p.stroke_path(&gap_path, &Stroke::new(slice_gap), Brush::Solid(bg));
                }
            }
        }

        // ─── PiP corner button ────────────────────────────────
        // Same shape as ChartsBuilder's PiP: 20 × 20 button rect
        // with field_bg + 1 px border so the glyph reads as a
        // distinct control. Slice fills underneath stay visible
        // around the corner; the button itself masks the slice
        // wedge it overlaps so the icon never looks "stuck on".
        let pointer_in_pip = if pip && !disabled {
            if let Some(local) = resp.pointer_local {
                let lx = p.rect().x() + local.x;
                let ly = p.rect().y() + local.y;
                let pad = 4.0_f32;
                let btn_size = 20.0_f32;
                let bx = p.rect().x() + p.rect().width() - btn_size - pad;
                let by = p.rect().y() + pad;
                lx >= bx && lx < bx + btn_size && ly >= by && ly < by + btn_size
            } else {
                false
            }
        } else {
            false
        };
        if pip && !disabled {
            let pad = 4.0_f32;
            let btn_size = 20.0_f32;
            let bx = p.rect().x() + p.rect().width() - btn_size - pad;
            let by = p.rect().y() + pad;
            let btn_rect = Rect::new(bx, by, btn_size, btn_size);
            let (btn_bg, btn_border, icon_color) = if pointer_in_pip {
                (style.button_hover, style.field_border_focus, style.text_primary)
            } else {
                (style.background, style.field_border, style.text_secondary)
            };
            p.fill_rect(btn_rect, CornerRadius::uniform(4.0), Brush::Solid(btn_bg));
            p.stroke_rect(
                btn_rect,
                CornerRadius::uniform(4.0),
                &Stroke::new(1.0),
                Brush::Solid(btn_border),
            );
            // PiP glyph: 12 × 9 BB centred per-axis. Round to
            // half-pixels so the 1.5 px stroke lands on a clean
            // grid; the previous mathematically-centred version
            // looked top-biased because the inner fill sits in
            // the BR quadrant and pulls the visual mass down.
            const GW: f32 = 12.0;
            const GH: f32 = 9.0;
            let gx = bx + ((btn_size - GW) * 0.5).round();
            let gy = by + ((btn_size - GH) * 0.5).round();
            p.stroke_rect(
                Rect::new(gx, gy, GW, GH),
                CornerRadius::uniform(1.5),
                &Stroke::new(1.5),
                Brush::Solid(icon_color),
            );
            p.fill_rect(
                Rect::new(gx + 6.0, gy + 5.0, 5.0, 4.0),
                CornerRadius::uniform(1.0),
                Brush::Solid(icon_color),
            );
            if resp.clicked && pointer_in_pip {
                resp.clicked = false;
                resp.pip_clicked = true;
            }
        }

        // ─── tooltip ──────────────────────────────────────────
        // Hit-test the cursor against each slice. Convert
        // `pointer_local` (relative to the painter rect) to
        // polar coords around (cx, cy); if the radius is in
        // `[inner_r, outer_r]` AND the angle is inside one of the
        // slice's `[a0, a1]` ranges (normalised against the
        // start_offset wrap), show a pill with the slice's
        // weight + percentage near the cursor. Skipped when the
        // cursor sits over the PiP icon to avoid two overlapping
        // affordances.
        if tooltip && !disabled && resp.hovered && !pointer_in_pip {
            if let Some(local) = resp.pointer_local {
                let px_pt = p.rect().x() + local.x;
                let py_pt = p.rect().y() + local.y;
                let dx = px_pt - cx;
                let dy = py_pt - cy;
                let r = (dx * dx + dy * dy).sqrt();
                if r >= inner_r && r <= outer_r + 0.5 {
                    // atan2 returns in `[-PI, PI]`. Pie angles
                    // start at `-PI/2` and increase by total
                    // 2 PI; normalise the hit angle into a
                    // monotonic sweep from the start_offset.
                    let mut ang = dy.atan2(dx);
                    while ang < start_offset {
                        ang += two_pi;
                    }
                    let mut hit: Option<&Slice> = None;
                    for slice in &spans {
                        if ang >= slice.a0 && ang <= slice.a1 {
                            hit = Some(slice);
                            break;
                        }
                    }
                    if let Some(slice) = hit {
                        let pct = slice.weight / total * 100.0;
                        let formatted = format!("{:.2}", slice.weight);
                        let label = if let Some(ref u) = tooltip_unit {
                            format!("{}{} ({:.0}%)", formatted, u, pct)
                        } else {
                            format!("{} ({:.0}%)", formatted, pct)
                        };
                        let pad_x = 6.0_f32;
                        let pill_h = 18.0_f32;
                        let label_w = approx_text_width(&label, &style);
                        let pill_w = label_w + 2.0 * pad_x;
                        let mut px_pill = px_pt + 12.0;
                        let mut py_pill = py_pt - pill_h - 8.0;
                        if px_pill + pill_w > p.rect().x() + p.rect().width() {
                            px_pill = px_pt - 12.0 - pill_w;
                        }
                        if py_pill < p.rect().y() {
                            py_pill = py_pt + 12.0;
                        }
                        p.fill_rect(
                            Rect::new(px_pill, py_pill, pill_w, pill_h),
                            CornerRadius::uniform(4.0),
                            Brush::Solid(style.field_bg),
                        );
                        p.stroke_rect(
                            Rect::new(px_pill, py_pill, pill_w, pill_h),
                            CornerRadius::uniform(4.0),
                            &Stroke::new(1.0),
                            Brush::Solid(style.field_border),
                        );
                        let mut ts = text_style(&style, style.text_primary);
                        ts.baseline = TextBaseline::Middle;
                        p.draw_text(
                            &label,
                            &ts,
                            Point::new(px_pill + pad_x, py_pill + pill_h * 0.5),
                        );
                    }
                }
            }
        }

        resp
    }

    pub fn changed(self) -> bool {
        self.show().changed
    }
}

impl<'a> PortalUi<'a> {
    /// Pie / donut chart bound to a `Vec<f32>` of slice weights.
    /// Weights are auto-normalised to their sum; NaN / Inf /
    /// non-positive entries are skipped. Each slice picks a colour
    /// from `.palette(...)` if supplied, otherwise rotates HSV
    /// hues from the theme accent via the golden-angle step so
    /// adjacent slices stay distinct.
    ///
    /// Chain `.diameter(...)` / `.donut()` / `.inner_ratio(...)` /
    /// `.palette(...)` / `.slice_gap(...)` / `.background(...)` /
    /// `.shadow_*()` then `.show()`.
    pub fn pie_chart<'b, V: PortalValue<'b, Vec<f32>>>(
        &'b mut self,
        weights: V,
    ) -> PieChartBuilder<'a, 'b> {
        PieChartBuilder {
            ui: self,
            value: weights.into_binding(),
            diameter: None,
            inner_ratio: 0.0,
            palette: None,
            slice_gap: 0.0,
            background: None,
            disabled: false,
            shadow_token: None,
            center: true,
            tooltip: true,
            tooltip_unit: None,
            pip: false,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// radar_chart — polar-axis visualiser. Equally-spaced axes radiate
// from a centre; data points sit along each axis proportional to
// the value's share of the y range, connected into a polygon.
// Distinct from the cartesian charts (line / bar / area / pie)
// because the data shape is "N values along N axes", not a time
// series and not slice weights.
// ─────────────────────────────────────────────────────────────────────

#[must_use = "RadarChartBuilder is lazy — call .show() / .changed() to paint"]
pub struct RadarChartBuilder<'a, 'b> {
    ui: &'b mut PortalUi<'a>,
    value: ValueBinding<'b, Vec<f32>>,
    /// Optional axis labels — same length as the data series; when
    /// shorter or `None`, no labels paint. Labels render outside
    /// the outer ring at each axis's angle.
    labels: Option<Vec<String>>,
    diameter: Option<f32>,
    /// Override the radial value range. `None` → autoscale to
    /// `[0, data.max()]`.
    y_range: Option<std::ops::Range<f32>>,
    /// Stroke width for the data polygon outline.
    line_width: f32,
    /// Concentric grid rings (`true` by default) — paint 4 evenly-
    /// spaced rings at 25 / 50 / 75 / 100 % of the outer radius so
    /// the eye can read approximate values along each axis.
    show_grid: bool,
    /// Axis spokes (`true` by default) — radial lines from centre
    /// to outer ring at each axis's angle.
    show_axes: bool,
    /// Fill the data polygon with a translucent wash under the
    /// stroke. Defaults `true`; turn off for an outline-only look.
    fill_area: bool,
    /// Show a small dot at each vertex of the data polygon.
    show_vertices: bool,
    /// Theme overrides — all default to derived theme tokens.
    stroke_color: Option<Color>,
    fill_color: Option<Color>,
    grid_color: Option<Color>,
    axis_color: Option<Color>,
    background: Option<Color>,
    disabled: bool,
    shadow_token: Option<ShadowToken>,
    /// Centre the chart horizontally within the portal's
    /// available width. Default `true`.
    center: bool,
    /// Hover tooltip — shows the value at the axis nearest the
    /// cursor. Default `true`.
    tooltip: bool,
    /// Optional unit suffix in the tooltip.
    tooltip_unit: Option<String>,
    /// Picture-in-picture corner button. Default `false`.
    pip: bool,
}

impl<'a, 'b> ShadowMix for RadarChartBuilder<'a, 'b> {
    fn shadow(mut self, token: ShadowToken) -> Self {
        self.shadow_token = Some(token);
        self
    }
}

impl<'a, 'b> RadarChartBuilder<'a, 'b> {
    pub fn diameter(mut self, d: f32) -> Self {
        self.diameter = Some(d.max(32.0));
        self
    }
    pub fn labels(mut self, labels: Vec<String>) -> Self {
        self.labels = Some(labels);
        self
    }
    pub fn y_range(mut self, r: std::ops::Range<f32>) -> Self {
        self.y_range = Some(r);
        self
    }
    pub fn line_width(mut self, w: f32) -> Self {
        self.line_width = w.max(0.5);
        self
    }
    pub fn show_grid(mut self, b: bool) -> Self {
        self.show_grid = b;
        self
    }
    pub fn show_axes(mut self, b: bool) -> Self {
        self.show_axes = b;
        self
    }
    pub fn fill_area(mut self, b: bool) -> Self {
        self.fill_area = b;
        self
    }
    pub fn show_vertices(mut self, b: bool) -> Self {
        self.show_vertices = b;
        self
    }
    pub fn stroke_color(mut self, c: Color) -> Self {
        self.stroke_color = Some(c);
        self
    }
    pub fn fill_color(mut self, c: Color) -> Self {
        self.fill_color = Some(c);
        self
    }
    pub fn grid_color(mut self, c: Color) -> Self {
        self.grid_color = Some(c);
        self
    }
    pub fn axis_color(mut self, c: Color) -> Self {
        self.axis_color = Some(c);
        self
    }
    pub fn background(mut self, c: Color) -> Self {
        self.background = Some(c);
        self
    }
    pub fn disabled(mut self, b: bool) -> Self {
        self.disabled = b;
        self
    }
    pub fn center(mut self, b: bool) -> Self {
        self.center = b;
        self
    }
    pub fn tooltip(mut self, b: bool) -> Self {
        self.tooltip = b;
        self
    }
    pub fn tooltip_unit(mut self, u: impl Into<String>) -> Self {
        self.tooltip_unit = Some(u.into());
        self
    }
    pub fn pip(mut self, b: bool) -> Self {
        self.pip = b;
        self
    }

    pub fn show(self) -> Response {
        use blinc_core::draw::{LineCap, LineJoin};
        use blinc_core::layer::{CornerRadius, Rect};

        let RadarChartBuilder {
            ui,
            value,
            labels,
            diameter,
            y_range,
            line_width,
            show_grid,
            show_axes,
            fill_area,
            show_vertices,
            stroke_color,
            fill_color,
            grid_color,
            axis_color,
            background,
            disabled,
            shadow_token,
            center,
            tooltip,
            tooltip_unit,
            pip,
        } = self;

        let style = ui.style.clone();
        let d = diameter.unwrap_or(112.0);
        let data: Vec<f32> = value.get();
        let n = data.len();

        let (avail_w, _) = ui.available_size();
        let alloc_w = if center { avail_w.max(d) } else { d };
        let sense = if pip && !disabled {
            Sense::Click
        } else if tooltip && !disabled {
            Sense::Hover
        } else {
            Sense::None
        };
        let (mut p, mut resp) = ui.allocate_painter((alloc_w, d), sense);

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
        if let Some(bg) = background {
            let bg = if disabled { bg.with_alpha(0.5) } else { bg };
            p.fill_self(&style, Brush::Solid(bg));
        }

        // Need at least 3 axes for a polygon to read. Empty / 1 /
        // 2 falls through to an empty paint (chart slot still
        // occupies its rect for layout stability).
        if n < 3 || d < 16.0 {
            return resp;
        }

        // Geometry: leave a margin for axis labels if any. Without
        // labels, leave a 6 px halo so the outer ring doesn't kiss
        // the painter edge.
        let label_margin = if labels.as_ref().map(|l| !l.is_empty()).unwrap_or(false) {
            18.0
        } else {
            6.0
        };
        let cx = p.rect().x() + alloc_w * 0.5;
        let cy = p.rect().y() + d * 0.5;
        let outer_r = (d * 0.5) - label_margin;
        if outer_r < 8.0 {
            return resp;
        }

        // y-range autoscale — NaN-safe; default min = 0 (radial
        // values typically start at the centre).
        let mut hi = f32::NEG_INFINITY;
        for &v in &data {
            if v.is_finite() {
                hi = hi.max(v);
            }
        }
        let (lo, hi) = if !hi.is_finite() {
            (0.0, 1.0)
        } else if let Some(r) = y_range.clone() {
            (r.start, r.end)
        } else if hi.abs() < 1e-6 {
            (0.0, 1.0)
        } else {
            (0.0, hi)
        };
        let span = (hi - lo).max(1e-6);

        // Theme palette resolution.
        let alpha = if disabled { 0.4 } else { 1.0 };
        let stroke = stroke_color.unwrap_or(style.accent).with_alpha(alpha);
        let fill = if let Some(fc) = fill_color {
            fc.with_alpha(alpha)
        } else {
            style.accent.with_alpha(alpha * 0.18)
        };
        let grid_col = grid_color
            .unwrap_or_else(|| style.field_border.with_alpha(alpha * 0.7));
        let axis_col = axis_color
            .unwrap_or_else(|| style.text_secondary.with_alpha(alpha * 0.35));

        let two_pi = std::f32::consts::TAU;
        let start_offset = -std::f32::consts::FRAC_PI_2;
        let axis_angle = |i: usize| -> f32 {
            start_offset + (i as f32) * two_pi / (n as f32)
        };

        // Grid rings — 4 concentric circles at 25/50/75/100 %.
        if show_grid {
            for k in 1..=4 {
                let r = outer_r * (k as f32) * 0.25;
                p.stroke_circle(
                    Point::new(cx, cy),
                    r,
                    &Stroke::new(1.0),
                    Brush::Solid(grid_col),
                );
            }
        }

        // Axis spokes.
        if show_axes {
            for i in 0..n {
                let ang = axis_angle(i);
                let ex = cx + outer_r * ang.cos();
                let ey = cy + outer_r * ang.sin();
                let spoke = blinc_core::draw::Path::new()
                    .move_to(cx, cy)
                    .line_to(ex, ey);
                p.stroke_path(&spoke, &Stroke::new(1.0), Brush::Solid(axis_col));
            }
        }

        // Data polygon — compute vertex per axis, walk path.
        let mut vertices: Vec<Point> = Vec::with_capacity(n);
        for i in 0..n {
            let v = data[i];
            let t = if v.is_finite() {
                ((v - lo) / span).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let r = outer_r * t;
            let ang = axis_angle(i);
            vertices.push(Point::new(cx + r * ang.cos(), cy + r * ang.sin()));
        }

        // Fill area (closed polygon) painted first; stroke on top.
        if fill_area {
            let mut area = blinc_core::draw::Path::new();
            area = area.move_to(vertices[0].x, vertices[0].y);
            for v in vertices.iter().skip(1) {
                area = area.line_to(v.x, v.y);
            }
            area = area.close();
            p.fill_path(&area, Brush::Solid(fill));
        }

        // Stroke the polygon outline.
        let mut poly = blinc_core::draw::Path::new();
        poly = poly.move_to(vertices[0].x, vertices[0].y);
        for v in vertices.iter().skip(1) {
            poly = poly.line_to(v.x, v.y);
        }
        poly = poly.line_to(vertices[0].x, vertices[0].y); // explicit close stroke
        let stroke_def = Stroke::new(line_width)
            .with_cap(LineCap::Round)
            .with_join(LineJoin::Round);
        p.stroke_path(&poly, &stroke_def, Brush::Solid(stroke));

        // Vertex markers.
        if show_vertices {
            for v in &vertices {
                p.fill_circle(*v, line_width + 1.0, Brush::Solid(stroke));
            }
        }

        // Axis labels — outside the outer ring at each axis angle.
        if let Some(ref labels) = labels {
            let mut ts = text_style(&style, style.text_secondary.with_alpha(alpha));
            ts.baseline = TextBaseline::Middle;
            for (i, label) in labels.iter().enumerate().take(n) {
                if label.is_empty() {
                    continue;
                }
                let ang = axis_angle(i);
                let lr = outer_r + 8.0;
                let lx = cx + lr * ang.cos();
                let ly = cy + lr * ang.sin();
                // Right-align labels on the left half, left-align
                // on the right half so they don't dive into the
                // chart body.
                let lw = approx_text_width(label, &style);
                let lx = if ang.cos() < -0.2 {
                    lx - lw
                } else if ang.cos() < 0.2 {
                    lx - lw * 0.5
                } else {
                    lx
                };
                p.draw_text(label, &ts, Point::new(lx, ly));
            }
        }

        // ─── PiP corner button ────────────────────────────────
        let pointer_in_pip = if pip && !disabled {
            if let Some(local) = resp.pointer_local {
                let lx = p.rect().x() + local.x;
                let ly = p.rect().y() + local.y;
                let pad = 4.0_f32;
                let btn_size = 20.0_f32;
                let bx = p.rect().x() + p.rect().width() - btn_size - pad;
                let by = p.rect().y() + pad;
                lx >= bx && lx < bx + btn_size && ly >= by && ly < by + btn_size
            } else {
                false
            }
        } else {
            false
        };
        if pip && !disabled {
            let pad = 4.0_f32;
            let btn_size = 20.0_f32;
            let bx = p.rect().x() + p.rect().width() - btn_size - pad;
            let by = p.rect().y() + pad;
            let btn_rect = Rect::new(bx, by, btn_size, btn_size);
            let (btn_bg, btn_border, icon_color) = if pointer_in_pip {
                (style.button_hover, style.field_border_focus, style.text_primary)
            } else {
                (style.background, style.field_border, style.text_secondary)
            };
            p.fill_rect(btn_rect, CornerRadius::uniform(4.0), Brush::Solid(btn_bg));
            p.stroke_rect(
                btn_rect,
                CornerRadius::uniform(4.0),
                &Stroke::new(1.0),
                Brush::Solid(btn_border),
            );
            const GW: f32 = 12.0;
            const GH: f32 = 9.0;
            let gx = bx + ((btn_size - GW) * 0.5).round();
            let gy = by + ((btn_size - GH) * 0.5).round();
            p.stroke_rect(
                Rect::new(gx, gy, GW, GH),
                CornerRadius::uniform(1.5),
                &Stroke::new(1.5),
                Brush::Solid(icon_color),
            );
            p.fill_rect(
                Rect::new(gx + 6.0, gy + 5.0, 5.0, 4.0),
                CornerRadius::uniform(1.0),
                Brush::Solid(icon_color),
            );
            if resp.clicked && pointer_in_pip {
                resp.clicked = false;
                resp.pip_clicked = true;
            }
        }

        // ─── tooltip ──────────────────────────────────────────
        // Pick the axis whose direction is closest to the cursor's
        // direction-from-centre; show the value pill at the
        // cursor.
        if tooltip && !disabled && resp.hovered && !pointer_in_pip {
            if let Some(local) = resp.pointer_local {
                let px_pt = p.rect().x() + local.x;
                let py_pt = p.rect().y() + local.y;
                let dx = px_pt - cx;
                let dy = py_pt - cy;
                let r = (dx * dx + dy * dy).sqrt();
                if r >= 1.0 && r <= outer_r + 8.0 {
                    let cursor_ang = dy.atan2(dx);
                    let mut best_i = 0usize;
                    let mut best_d = f32::INFINITY;
                    for i in 0..n {
                        let ang = axis_angle(i);
                        // Smallest unsigned angular distance.
                        let diff = ((cursor_ang - ang).sin().abs()).atan2(
                            (cursor_ang - ang).cos(),
                        );
                        let d = diff.abs();
                        if d < best_d {
                            best_d = d;
                            best_i = i;
                        }
                    }
                    let val = data[best_i];
                    let label_pref = labels
                        .as_ref()
                        .and_then(|ls| ls.get(best_i).cloned())
                        .filter(|s| !s.is_empty());
                    let formatted = format!("{:.2}", val);
                    let body = if let Some(ref u) = tooltip_unit {
                        format!("{}{}", formatted, u)
                    } else {
                        formatted
                    };
                    let label_text = match label_pref {
                        Some(name) => format!("{}: {}", name, body),
                        None => body,
                    };
                    let pad_x = 6.0_f32;
                    let pill_h = 18.0_f32;
                    let label_w = approx_text_width(&label_text, &style);
                    let pill_w = label_w + 2.0 * pad_x;
                    let mut px_pill = px_pt + 12.0;
                    let mut py_pill = py_pt - pill_h - 8.0;
                    if px_pill + pill_w > p.rect().x() + p.rect().width() {
                        px_pill = px_pt - 12.0 - pill_w;
                    }
                    if py_pill < p.rect().y() {
                        py_pill = py_pt + 12.0;
                    }
                    p.fill_rect(
                        Rect::new(px_pill, py_pill, pill_w, pill_h),
                        CornerRadius::uniform(4.0),
                        Brush::Solid(style.field_bg),
                    );
                    p.stroke_rect(
                        Rect::new(px_pill, py_pill, pill_w, pill_h),
                        CornerRadius::uniform(4.0),
                        &Stroke::new(1.0),
                        Brush::Solid(style.field_border),
                    );
                    let mut ts = text_style(&style, style.text_primary);
                    ts.baseline = TextBaseline::Middle;
                    p.draw_text(
                        &label_text,
                        &ts,
                        Point::new(px_pill + pad_x, py_pill + pill_h * 0.5),
                    );
                }
            }
        }

        resp
    }

    pub fn changed(self) -> bool {
        self.show().changed
    }
}

impl<'a> PortalUi<'a> {
    /// Radar / spider chart — N values on N equally-spaced axes
    /// radiating from a centre, connected into a polygon. Useful
    /// for multi-dimensional metric comparisons (skills profile,
    /// node-graph health vector, audio analyser bands).
    ///
    /// Chain `.diameter(...)` / `.labels(...)` / `.y_range(...)` /
    /// `.fill_area(false)` / `.show_vertices(true)` / colour
    /// overrides / `.tooltip(...)` / `.pip(...)` then `.show()`.
    pub fn radar_chart<'b, V: PortalValue<'b, Vec<f32>>>(
        &'b mut self,
        values: V,
    ) -> RadarChartBuilder<'a, 'b> {
        RadarChartBuilder {
            ui: self,
            value: values.into_binding(),
            labels: None,
            diameter: None,
            y_range: None,
            line_width: 1.5,
            show_grid: true,
            show_axes: true,
            fill_area: true,
            show_vertices: false,
            stroke_color: None,
            fill_color: None,
            grid_color: None,
            axis_color: None,
            background: None,
            disabled: false,
            shadow_token: None,
            center: true,
            tooltip: true,
            tooltip_unit: None,
            pip: false,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// noise — procedural 2D pattern visualiser. Hash-based Perlin /
// Worley / Voronoi all reduced to a Vec<u8> RGBA buffer uploaded
// via `draw_rgba_pixels`. Useful for game-tooling shaders, audio
// noise floors, terrain previews.
// ─────────────────────────────────────────────────────────────────────

/// Choice of procedural noise function.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Hash)]
pub enum NoiseVariant {
    /// Smooth gradient noise — Perlin-style, value is interpolated
    /// dot-products of pseudo-random gradient vectors at a unit
    /// grid. Default.
    #[default]
    Perlin,
    /// Cellular distance noise — distance from each pixel to its
    /// nearest feature point. Produces a "cracked-stone" pattern;
    /// useful for tile / mosaic effects.
    Worley,
    /// Voronoi cells — same feature points as Worley, but the
    /// value is the cell id (each cell flat-shaded). Produces
    /// a stained-glass / cellular look.
    Voronoi,
}

#[must_use = "NoiseBuilder is lazy — call .show() to paint"]
pub struct NoiseBuilder<'a, 'b> {
    ui: &'b mut PortalUi<'a>,
    variant: NoiseVariant,
    /// Pseudo-random seed for the noise hash. Same seed +
    /// variant + scale gives the same pattern every paint.
    seed: u32,
    /// Scale — pixels per noise unit. Smaller = coarser
    /// pattern; larger = finer. Default 32 (one cell every
    /// 32 px for Worley / Voronoi).
    scale: f32,
    /// Octaves for Perlin (1 = single-octave, 4 = fbm-style).
    /// Ignored for Worley / Voronoi.
    octaves: u32,
    width_override: Option<f32>,
    height_override: Option<f32>,
    /// Optional colour ramp for Perlin — `(low, high)`. Each
    /// pixel's noise value (mapped to 0..1) interpolates between
    /// the two colours. `None` uses `style.field_bg` →
    /// `style.text_primary`.
    color_low: Option<Color>,
    color_high: Option<Color>,
    /// Optional border around the painter rect — same field
    /// border colour as other portal_ui widgets.
    show_border: bool,
    disabled: bool,
    shadow_token: Option<ShadowToken>,
    pip: bool,
}

impl<'a, 'b> ShadowMix for NoiseBuilder<'a, 'b> {
    fn shadow(mut self, token: ShadowToken) -> Self {
        self.shadow_token = Some(token);
        self
    }
}

impl<'a, 'b> NoiseBuilder<'a, 'b> {
    pub fn variant(mut self, v: NoiseVariant) -> Self {
        self.variant = v;
        self
    }
    pub fn perlin(mut self) -> Self {
        self.variant = NoiseVariant::Perlin;
        self
    }
    pub fn worley(mut self) -> Self {
        self.variant = NoiseVariant::Worley;
        self
    }
    pub fn voronoi(mut self) -> Self {
        self.variant = NoiseVariant::Voronoi;
        self
    }
    pub fn seed(mut self, s: u32) -> Self {
        self.seed = s;
        self
    }
    pub fn scale(mut self, s: f32) -> Self {
        self.scale = s.max(1.0);
        self
    }
    pub fn octaves(mut self, o: u32) -> Self {
        self.octaves = o.clamp(1, 6);
        self
    }
    pub fn width(mut self, w: f32) -> Self {
        self.width_override = Some(w);
        self
    }
    pub fn height(mut self, h: f32) -> Self {
        self.height_override = Some(h);
        self
    }
    pub fn color_ramp(mut self, low: Color, high: Color) -> Self {
        self.color_low = Some(low);
        self.color_high = Some(high);
        self
    }
    pub fn show_border(mut self, b: bool) -> Self {
        self.show_border = b;
        self
    }
    pub fn disabled(mut self, b: bool) -> Self {
        self.disabled = b;
        self
    }
    pub fn pip(mut self, b: bool) -> Self {
        self.pip = b;
        self
    }

    pub fn show(self) -> Response {
        use blinc_core::layer::{CornerRadius, Rect};

        let NoiseBuilder {
            ui,
            variant,
            seed,
            scale,
            octaves,
            width_override,
            height_override,
            color_low,
            color_high,
            show_border,
            disabled,
            shadow_token,
            pip,
        } = self;

        let style = ui.style.clone();
        let (avail_w, _) = ui.available_size();
        let width = width_override.unwrap_or_else(|| avail_w.clamp(120.0, 240.0));
        let height = height_override.unwrap_or(80.0);

        let sense = if pip && !disabled {
            Sense::Click
        } else {
            Sense::None
        };
        let pip_strip = if pip && !disabled { 24.0 } else { 0.0 };
        let alloc_h = height + pip_strip;
        let (mut p, mut resp) = ui.allocate_painter((width, alloc_h), sense);

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

        // Image rect — the painter region below the PiP strip.
        let img_x = p.rect().x();
        let img_y = p.rect().y() + pip_strip;
        let img_w = width;
        let img_h = height;
        // Quantise to integer pixels so the GPU image upload
        // matches the texture sampling grid.
        let tex_w = img_w.max(8.0).round() as u32;
        let tex_h = img_h.max(8.0).round() as u32;

        let (low, high) = (
            color_low.unwrap_or_else(|| style.field_bg),
            color_high.unwrap_or_else(|| style.text_primary),
        );
        let buf = generate_noise(variant, tex_w, tex_h, scale, seed, octaves, low, high);
        let dest = Rect::new(img_x, img_y, img_w, img_h);
        p.draw_rgba_pixels(&buf, tex_w, tex_h, dest);

        if show_border {
            p.stroke_rect(
                dest,
                CornerRadius::uniform(4.0),
                &Stroke::new(1.0),
                Brush::Solid(style.field_border),
            );
        }

        // PiP corner button — same shape as the chart builders.
        let pointer_in_pip = if pip && !disabled {
            if let Some(local) = resp.pointer_local {
                let lx = p.rect().x() + local.x;
                let ly = p.rect().y() + local.y;
                let pad = 4.0_f32;
                let btn_size = 20.0_f32;
                let bx = p.rect().x() + p.rect().width() - btn_size - pad;
                let by = p.rect().y() + pad;
                lx >= bx && lx < bx + btn_size && ly >= by && ly < by + btn_size
            } else {
                false
            }
        } else {
            false
        };
        if pip && !disabled {
            let pad = 4.0_f32;
            let btn_size = 20.0_f32;
            let bx = p.rect().x() + p.rect().width() - btn_size - pad;
            let by = p.rect().y() + pad;
            let btn_rect = Rect::new(bx, by, btn_size, btn_size);
            let (btn_bg, btn_border, icon_color) = if pointer_in_pip {
                (style.button_hover, style.field_border_focus, style.text_primary)
            } else {
                (style.background, style.field_border, style.text_secondary)
            };
            p.fill_rect(btn_rect, CornerRadius::uniform(4.0), Brush::Solid(btn_bg));
            p.stroke_rect(
                btn_rect,
                CornerRadius::uniform(4.0),
                &Stroke::new(1.0),
                Brush::Solid(btn_border),
            );
            const GW: f32 = 12.0;
            const GH: f32 = 9.0;
            let gx = bx + ((btn_size - GW) * 0.5).round();
            let gy = by + ((btn_size - GH) * 0.5).round();
            p.stroke_rect(
                Rect::new(gx, gy, GW, GH),
                CornerRadius::uniform(1.5),
                &Stroke::new(1.5),
                Brush::Solid(icon_color),
            );
            p.fill_rect(
                Rect::new(gx + 6.0, gy + 5.0, 5.0, 4.0),
                CornerRadius::uniform(1.0),
                Brush::Solid(icon_color),
            );
            if resp.clicked && pointer_in_pip {
                resp.clicked = false;
                resp.pip_clicked = true;
            }
        }

        resp
    }
}

impl<'a> PortalUi<'a> {
    /// Procedural 2D noise pattern — Perlin / Worley / Voronoi.
    /// Bakes a pixel buffer each frame from the variant + seed +
    /// scale and uploads it via `draw_rgba_pixels`. Useful for
    /// game-tool noise previews, terrain editors, audio noise
    /// floors. Chain `.perlin()` / `.worley()` / `.voronoi()` to
    /// pick the kernel; `.seed(...)` / `.scale(...)` /
    /// `.octaves(...)` / `.color_ramp(low, high)` to configure
    /// the look; `.width(...)` / `.height(...)` for sizing.
    pub fn noise<'b>(&'b mut self) -> NoiseBuilder<'a, 'b> {
        NoiseBuilder {
            ui: self,
            variant: NoiseVariant::default(),
            seed: 0,
            scale: 32.0,
            octaves: 1,
            width_override: None,
            height_override: None,
            color_low: None,
            color_high: None,
            show_border: true,
            disabled: false,
            shadow_token: None,
            pip: false,
        }
    }
}

// Hash-based noise generator. Single Vec<u8> RGBA output for the
// painter to upload via `draw_rgba_pixels`. Pure CPU; called once
// per paint. At default 240 × 80 sizes (~19 200 pixels) the cost
// is well under a frame at 60 Hz.
fn generate_noise(
    variant: NoiseVariant,
    w: u32,
    h: u32,
    scale: f32,
    seed: u32,
    octaves: u32,
    low: Color,
    high: Color,
) -> Vec<u8> {
    let mut buf = vec![0u8; (w as usize) * (h as usize) * 4];
    for y in 0..h {
        for x in 0..w {
            let t = match variant {
                NoiseVariant::Perlin => perlin_fbm(x as f32 / scale, y as f32 / scale, seed, octaves),
                NoiseVariant::Worley => worley(x as f32 / scale, y as f32 / scale, seed),
                NoiseVariant::Voronoi => voronoi(x as f32 / scale, y as f32 / scale, seed),
            };
            let t = t.clamp(0.0, 1.0);
            let r = (low.r + (high.r - low.r) * t).clamp(0.0, 1.0);
            let g = (low.g + (high.g - low.g) * t).clamp(0.0, 1.0);
            let b = (low.b + (high.b - low.b) * t).clamp(0.0, 1.0);
            let i = ((y * w + x) * 4) as usize;
            buf[i] = (r * 255.0) as u8;
            buf[i + 1] = (g * 255.0) as u8;
            buf[i + 2] = (b * 255.0) as u8;
            buf[i + 3] = 255;
        }
    }
    buf
}

fn hash2(x: i32, y: i32, seed: u32) -> u32 {
    let mut h = (x as u32)
        .wrapping_mul(374761393)
        .wrapping_add((y as u32).wrapping_mul(668265263))
        .wrapping_add(seed.wrapping_mul(2246822519));
    h ^= h >> 13;
    h = h.wrapping_mul(1274126177);
    h ^= h >> 16;
    h
}

fn rand_unit(h: u32) -> f32 {
    ((h >> 8) as f32) / ((1u32 << 24) as f32)
}

fn smoothstep(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

// 2D value-grid Perlin-like — gradient interpolation between
// pseudo-random unit vectors at lattice corners. Single octave;
// fbm wrapper combines multiple octaves with falling amplitude.
fn perlin_single(x: f32, y: f32, seed: u32) -> f32 {
    let xi = x.floor() as i32;
    let yi = y.floor() as i32;
    let xf = x - xi as f32;
    let yf = y - yi as f32;
    let u = smoothstep(xf);
    let v = smoothstep(yf);

    let grad = |gx: i32, gy: i32, px: f32, py: f32| -> f32 {
        let h = hash2(gx, gy, seed);
        let ang = rand_unit(h) * std::f32::consts::TAU;
        let dx = ang.cos();
        let dy = ang.sin();
        dx * px + dy * py
    };

    let a = grad(xi, yi, xf, yf);
    let b = grad(xi + 1, yi, xf - 1.0, yf);
    let c = grad(xi, yi + 1, xf, yf - 1.0);
    let d = grad(xi + 1, yi + 1, xf - 1.0, yf - 1.0);

    let ab = a + (b - a) * u;
    let cd = c + (d - c) * u;
    let v = ab + (cd - ab) * v;
    // Gradient noise lands in roughly [-0.707, 0.707]; remap.
    (v + 0.707) / 1.414
}

fn perlin_fbm(x: f32, y: f32, seed: u32, octaves: u32) -> f32 {
    let mut amp = 1.0_f32;
    let mut freq = 1.0_f32;
    let mut sum = 0.0_f32;
    let mut norm = 0.0_f32;
    for o in 0..octaves {
        sum += perlin_single(x * freq, y * freq, seed.wrapping_add(o.wrapping_mul(1013))) * amp;
        norm += amp;
        amp *= 0.5;
        freq *= 2.0;
    }
    (sum / norm).clamp(0.0, 1.0)
}

// Worley / cellular distance — distance from (x, y) to the
// nearest feature point. Feature points sit one-per-cell in the
// integer lattice; we scan the 3 × 3 neighbourhood around the
// query cell so the nearest point is always within the window.
fn worley(x: f32, y: f32, seed: u32) -> f32 {
    let xi = x.floor() as i32;
    let yi = y.floor() as i32;
    let mut best = f32::INFINITY;
    for ny in -1..=1 {
        for nx in -1..=1 {
            let cx = xi + nx;
            let cy = yi + ny;
            let h = hash2(cx, cy, seed);
            let fx = (cx as f32) + rand_unit(h);
            let fy = (cy as f32) + rand_unit(h.wrapping_mul(2654435761));
            let dx = fx - x;
            let dy = fy - y;
            let d = (dx * dx + dy * dy).sqrt();
            if d < best {
                best = d;
            }
        }
    }
    best.clamp(0.0, 1.0)
}

// Voronoi cell colouring — same feature points as Worley, but
// the value is a hash of the nearest cell's id (each cell renders
// at a single flat shade).
fn voronoi(x: f32, y: f32, seed: u32) -> f32 {
    let xi = x.floor() as i32;
    let yi = y.floor() as i32;
    let mut best = f32::INFINITY;
    let mut best_h = 0u32;
    for ny in -1..=1 {
        for nx in -1..=1 {
            let cx = xi + nx;
            let cy = yi + ny;
            let h = hash2(cx, cy, seed);
            let fx = (cx as f32) + rand_unit(h);
            let fy = (cy as f32) + rand_unit(h.wrapping_mul(2654435761));
            let dx = fx - x;
            let dy = fy - y;
            let d = dx * dx + dy * dy;
            if d < best {
                best = d;
                best_h = h;
            }
        }
    }
    rand_unit(best_h)
}

// ─────────────────────────────────────────────────────────────────────
// texture — generic image / texture visualiser. Accepts either a
// host-loaded `ImageId` (cheap; the GPU keeps the texture in its
// asset cache) or a raw RGBA byte slice (re-uploaded each frame
// via `draw_rgba_pixels`). Mirrors CSS `object-fit` for stretch /
// contain / cover behaviour against the painter rect.
// ─────────────────────────────────────────────────────────────────────

/// How the image fills the painter rect when the source aspect
/// ratio doesn't match. Variants follow CSS `object-fit`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TextureFit {
    /// Stretch the source to exactly fill the rect; aspect
    /// ratio may distort.
    #[default]
    Fill,
    /// Scale uniformly so the source fits entirely inside the
    /// rect; letterbox margins appear when aspect mismatches.
    Contain,
    /// Scale uniformly so the source covers the rect; crop
    /// when aspect mismatches.
    Cover,
    /// Same as `Contain` but never up-scale beyond the source's
    /// native size — small images paint at 1:1 inside the rect.
    ScaleDown,
}

/// Source data for the texture widget.
///
/// The lifetime `'src` ties the borrowed RGBA buffer to its owner
/// so the painter can copy without lifetime gymnastics; ImageId
/// variants are `Copy`-cheap.
pub enum TextureSource<'src> {
    /// Host-loaded asset — `ImageId` from `blinc_image` /
    /// `blinc_layout::ImageLoader`. The GPU keeps the texture
    /// cached across frames; cheap to repaint.
    Id(blinc_core::draw::ImageId),
    /// Raw RGBA byte buffer + dimensions. Re-uploaded every
    /// frame via `draw_rgba_pixels` (the GPU clones the slice
    /// each call). Use for procedural / dynamic textures; for
    /// static images prefer the host loader + `Id`.
    Rgba {
        data: &'src [u8],
        width: u32,
        height: u32,
    },
}

#[must_use = "TextureBuilder is lazy — call .show() to paint"]
pub struct TextureBuilder<'a, 'b, 'src> {
    ui: &'b mut PortalUi<'a>,
    source: TextureSource<'src>,
    width_override: Option<f32>,
    height_override: Option<f32>,
    fit: TextureFit,
    tint: Option<Color>,
    opacity: f32,
    show_border: bool,
    background: Option<Color>,
    disabled: bool,
    shadow_token: Option<ShadowToken>,
    pip: bool,
}

impl<'a, 'b, 'src> ShadowMix for TextureBuilder<'a, 'b, 'src> {
    fn shadow(mut self, token: ShadowToken) -> Self {
        self.shadow_token = Some(token);
        self
    }
}

impl<'a, 'b, 'src> TextureBuilder<'a, 'b, 'src> {
    pub fn fit(mut self, f: TextureFit) -> Self {
        self.fit = f;
        self
    }
    pub fn contain(mut self) -> Self {
        self.fit = TextureFit::Contain;
        self
    }
    pub fn cover(mut self) -> Self {
        self.fit = TextureFit::Cover;
        self
    }
    pub fn fill(mut self) -> Self {
        self.fit = TextureFit::Fill;
        self
    }
    pub fn scale_down(mut self) -> Self {
        self.fit = TextureFit::ScaleDown;
        self
    }
    pub fn width(mut self, w: f32) -> Self {
        self.width_override = Some(w);
        self
    }
    pub fn height(mut self, h: f32) -> Self {
        self.height_override = Some(h);
        self
    }
    pub fn tint(mut self, c: Color) -> Self {
        self.tint = Some(c);
        self
    }
    pub fn opacity(mut self, o: f32) -> Self {
        self.opacity = o.clamp(0.0, 1.0);
        self
    }
    pub fn show_border(mut self, b: bool) -> Self {
        self.show_border = b;
        self
    }
    pub fn background(mut self, c: Color) -> Self {
        self.background = Some(c);
        self
    }
    pub fn disabled(mut self, b: bool) -> Self {
        self.disabled = b;
        self
    }
    pub fn pip(mut self, b: bool) -> Self {
        self.pip = b;
        self
    }

    pub fn show(self) -> Response {
        use blinc_core::draw::ImageOptions;
        use blinc_core::layer::{CornerRadius, Rect};

        let TextureBuilder {
            ui,
            source,
            width_override,
            height_override,
            fit,
            tint,
            opacity,
            show_border,
            background,
            disabled,
            shadow_token,
            pip,
        } = self;

        let style = ui.style.clone();
        let (avail_w, _) = ui.available_size();
        let width = width_override.unwrap_or_else(|| avail_w.clamp(96.0, 240.0));
        let height = height_override.unwrap_or(80.0);

        let (src_w, src_h) = match &source {
            TextureSource::Rgba { width, height, .. } => (*width as f32, *height as f32),
            TextureSource::Id(_) => (width, height),
        };

        let sense = if pip && !disabled {
            Sense::Click
        } else {
            Sense::None
        };
        let pip_strip = if pip && !disabled { 24.0 } else { 0.0 };
        let alloc_h = height + pip_strip;
        let (mut p, mut resp) = ui.allocate_painter((width, alloc_h), sense);

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

        let img_x = p.rect().x();
        let img_y = p.rect().y() + pip_strip;
        let outer = Rect::new(img_x, img_y, width, height);
        if let Some(bg) = background {
            let bg = if disabled { bg.with_alpha(0.5) } else { bg };
            p.fill_rect(outer, CornerRadius::uniform(4.0), Brush::Solid(bg));
        }

        // Compute dest rect from `fit`. Pure aspect-ratio math.
        let aspect_src = if src_h > 0.0 { src_w / src_h } else { 1.0 };
        let aspect_dst = if height > 0.0 { width / height } else { 1.0 };
        let dest = match fit {
            TextureFit::Fill => outer,
            TextureFit::Contain => {
                if aspect_src > aspect_dst {
                    // Source wider than dst — letterbox top + bottom.
                    let dw = width;
                    let dh = width / aspect_src;
                    Rect::new(img_x, img_y + (height - dh) * 0.5, dw, dh)
                } else {
                    let dh = height;
                    let dw = height * aspect_src;
                    Rect::new(img_x + (width - dw) * 0.5, img_y, dw, dh)
                }
            }
            TextureFit::Cover => {
                if aspect_src > aspect_dst {
                    // Source wider than dst — fit height, crop sides.
                    let dh = height;
                    let dw = height * aspect_src;
                    Rect::new(img_x + (width - dw) * 0.5, img_y, dw, dh)
                } else {
                    let dw = width;
                    let dh = width / aspect_src;
                    Rect::new(img_x, img_y + (height - dh) * 0.5, dw, dh)
                }
            }
            TextureFit::ScaleDown => {
                // Contain when src exceeds dst on either axis;
                // 1:1 centered otherwise.
                if src_w <= width && src_h <= height {
                    Rect::new(
                        img_x + (width - src_w) * 0.5,
                        img_y + (height - src_h) * 0.5,
                        src_w,
                        src_h,
                    )
                } else if aspect_src > aspect_dst {
                    let dw = width;
                    let dh = width / aspect_src;
                    Rect::new(img_x, img_y + (height - dh) * 0.5, dw, dh)
                } else {
                    let dh = height;
                    let dw = height * aspect_src;
                    Rect::new(img_x + (width - dw) * 0.5, img_y, dw, dh)
                }
            }
        };

        let effective_opacity = if disabled { opacity * 0.5 } else { opacity };
        match source {
            TextureSource::Rgba {
                data,
                width: w,
                height: h,
            } => {
                p.draw_rgba_pixels(data, w, h, dest);
            }
            TextureSource::Id(id) => {
                let mut opts = ImageOptions::new().with_opacity(effective_opacity);
                if let Some(c) = tint {
                    opts = opts.with_tint(c);
                }
                p.draw_image(id, dest, &opts);
            }
        }

        if show_border {
            p.stroke_rect(
                outer,
                CornerRadius::uniform(4.0),
                &Stroke::new(1.0),
                Brush::Solid(style.field_border),
            );
        }

        // PiP corner button — same shape as the chart / noise widgets.
        let pointer_in_pip = if pip && !disabled {
            if let Some(local) = resp.pointer_local {
                let lx = p.rect().x() + local.x;
                let ly = p.rect().y() + local.y;
                let pad = 4.0_f32;
                let btn_size = 20.0_f32;
                let bx = p.rect().x() + p.rect().width() - btn_size - pad;
                let by = p.rect().y() + pad;
                lx >= bx && lx < bx + btn_size && ly >= by && ly < by + btn_size
            } else {
                false
            }
        } else {
            false
        };
        if pip && !disabled {
            let pad = 4.0_f32;
            let btn_size = 20.0_f32;
            let bx = p.rect().x() + p.rect().width() - btn_size - pad;
            let by = p.rect().y() + pad;
            let btn_rect = Rect::new(bx, by, btn_size, btn_size);
            let (btn_bg, btn_border, icon_color) = if pointer_in_pip {
                (style.button_hover, style.field_border_focus, style.text_primary)
            } else {
                (style.background, style.field_border, style.text_secondary)
            };
            p.fill_rect(btn_rect, CornerRadius::uniform(4.0), Brush::Solid(btn_bg));
            p.stroke_rect(
                btn_rect,
                CornerRadius::uniform(4.0),
                &Stroke::new(1.0),
                Brush::Solid(btn_border),
            );
            const GW: f32 = 12.0;
            const GH: f32 = 9.0;
            let gx = bx + ((btn_size - GW) * 0.5).round();
            let gy = by + ((btn_size - GH) * 0.5).round();
            p.stroke_rect(
                Rect::new(gx, gy, GW, GH),
                CornerRadius::uniform(1.5),
                &Stroke::new(1.5),
                Brush::Solid(icon_color),
            );
            p.fill_rect(
                Rect::new(gx + 6.0, gy + 5.0, 5.0, 4.0),
                CornerRadius::uniform(1.0),
                Brush::Solid(icon_color),
            );
            if resp.clicked && pointer_in_pip {
                resp.clicked = false;
                resp.pip_clicked = true;
            }
        }

        resp
    }
}

impl<'a> PortalUi<'a> {
    /// Inline texture / image viewer. Accepts a host-loaded
    /// `ImageId` (cheap; cached by the GPU asset registry) or a
    /// raw RGBA byte slice (re-uploaded each frame). Chain
    /// `.contain()` / `.cover()` / `.fill()` / `.scale_down()`
    /// for aspect-ratio behaviour, `.tint(...)` /
    /// `.opacity(...)` (Id source only), `.show_border(false)` /
    /// `.background(c)` for chrome, `.width()` / `.height()` for
    /// sizing. PiP corner button via `.pip(true)`.
    pub fn texture<'b, 'src>(
        &'b mut self,
        source: TextureSource<'src>,
    ) -> TextureBuilder<'a, 'b, 'src> {
        TextureBuilder {
            ui: self,
            source,
            width_override: None,
            height_override: None,
            fit: TextureFit::default(),
            tint: None,
            opacity: 1.0,
            show_border: true,
            background: None,
            disabled: false,
            shadow_token: None,
            pip: false,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// sdf_shape — single-shape SDF visualiser. 2D variants paint
// directly via fill_circle / fill_rect; 3D variants set the
// `DrawContext`'s 3D state (perspective + shape kind + light)
// then emit a fill_rect whose fragment shader raymarches the
// chosen SDF. State is reset after the paint so downstream
// widgets don't inherit the 3D shading.
// ─────────────────────────────────────────────────────────────────────

/// SDF shape variants — 2D and 3D. The 3D-variant integer
/// values match the `shape_type` field consumed by
/// `DrawContext::set_3d_shape` (1 = box, 2 = sphere, …).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SdfShape {
    /// 2D — flat-filled circle.
    Circle2D,
    /// 2D — flat-filled rounded rect.
    RoundedBox2D,
    /// 2D — capsule (rect with 50 % corner radius).
    Capsule2D,
    /// 3D — extruded box with Blinn-Phong shading.
    Box3D,
    /// 3D — sphere with Blinn-Phong shading.
    Sphere3D,
    /// 3D — cylinder with Blinn-Phong shading.
    Cylinder3D,
    /// 3D — torus with Blinn-Phong shading.
    Torus3D,
    /// 3D — capsule (rounded-end pill) with Blinn-Phong shading.
    Capsule3D,
}

impl SdfShape {
    fn is_3d(self) -> bool {
        matches!(
            self,
            SdfShape::Box3D
                | SdfShape::Sphere3D
                | SdfShape::Cylinder3D
                | SdfShape::Torus3D
                | SdfShape::Capsule3D
        )
    }

    fn shape_type_3d(self) -> f32 {
        // Mirrors blinc_gpu's shape_type encoding: 1 box, 2 sphere,
        // 3 cylinder, 4 torus, 5 capsule, 6 group (unused here).
        match self {
            SdfShape::Box3D => 1.0,
            SdfShape::Sphere3D => 2.0,
            SdfShape::Cylinder3D => 3.0,
            SdfShape::Torus3D => 4.0,
            SdfShape::Capsule3D => 5.0,
            _ => 0.0,
        }
    }
}

#[must_use = "SdfShapeBuilder is lazy — call .show() to paint"]
pub struct SdfShapeBuilder<'a, 'b> {
    ui: &'b mut PortalUi<'a>,
    shape: SdfShape,
    width_override: Option<f32>,
    height_override: Option<f32>,
    fill: Option<Color>,
    stroke: Option<Color>,
    stroke_width: f32,
    /// 3D depth (extrusion magnitude) as a fraction of the
    /// painter's min side. Default `1.0` — one full min-side
    /// worth of z-extrusion. Curved SDFs (sphere / torus /
    /// cylinder) need at least this for the silhouette to read
    /// crisp; values < 0.5 make the raymarcher hit the back
    /// face inside the anti-alias band and edges go soft.
    depth: f32,
    /// 3D rotation around the X axis in radians. Default `0.35`
    /// (~20°) for a gentle tilt that exposes top + front faces
    /// on a box.
    rotate_x: f32,
    /// 3D rotation around the Y axis in radians. Default `0.7`
    /// (~40°) so a box reveals 3 faces.
    rotate_y: f32,
    /// Ambient lighting contribution (0..1). Default `0.3`.
    ambient: f32,
    /// Specular highlight sharpness. Default `32.0`.
    specular: f32,
    /// 3D light direction (normalised in the shader). Default
    /// `[-0.5, -1.0, 0.5]` for a soft top-front key.
    light_dir: [f32; 3],
    /// 3D light intensity multiplier. Default `0.8`.
    light_intensity: f32,
    /// Corner radius for the 2D RoundedBox2D variant.
    corner_radius: f32,
    background: Option<Color>,
    show_border: bool,
    disabled: bool,
    shadow_token: Option<ShadowToken>,
    pip: bool,
}

impl<'a, 'b> ShadowMix for SdfShapeBuilder<'a, 'b> {
    fn shadow(mut self, token: ShadowToken) -> Self {
        self.shadow_token = Some(token);
        self
    }
}

impl<'a, 'b> SdfShapeBuilder<'a, 'b> {
    pub fn shape(mut self, s: SdfShape) -> Self {
        self.shape = s;
        self
    }
    pub fn width(mut self, w: f32) -> Self {
        self.width_override = Some(w);
        self
    }
    pub fn height(mut self, h: f32) -> Self {
        self.height_override = Some(h);
        self
    }
    pub fn fill(mut self, c: Color) -> Self {
        self.fill = Some(c);
        self
    }
    pub fn stroke(mut self, c: Color) -> Self {
        self.stroke = Some(c);
        self
    }
    pub fn stroke_width(mut self, w: f32) -> Self {
        self.stroke_width = w.max(0.0);
        self
    }
    pub fn depth(mut self, d: f32) -> Self {
        self.depth = d.clamp(0.0, 2.0);
        self
    }
    pub fn rotate_x(mut self, r: f32) -> Self {
        self.rotate_x = r;
        self
    }
    pub fn rotate_y(mut self, r: f32) -> Self {
        self.rotate_y = r;
        self
    }
    pub fn ambient(mut self, a: f32) -> Self {
        self.ambient = a.clamp(0.0, 1.0);
        self
    }
    pub fn specular(mut self, s: f32) -> Self {
        self.specular = s.max(0.0);
        self
    }
    pub fn light(mut self, direction: [f32; 3], intensity: f32) -> Self {
        self.light_dir = direction;
        self.light_intensity = intensity.max(0.0);
        self
    }
    pub fn corner_radius(mut self, r: f32) -> Self {
        self.corner_radius = r.max(0.0);
        self
    }
    pub fn background(mut self, c: Color) -> Self {
        self.background = Some(c);
        self
    }
    pub fn show_border(mut self, b: bool) -> Self {
        self.show_border = b;
        self
    }
    pub fn disabled(mut self, b: bool) -> Self {
        self.disabled = b;
        self
    }
    pub fn pip(mut self, b: bool) -> Self {
        self.pip = b;
        self
    }

    pub fn show(self) -> Response {
        use blinc_core::layer::{ClipShape, CornerRadius, Rect};

        let SdfShapeBuilder {
            ui,
            shape,
            width_override,
            height_override,
            fill,
            stroke,
            stroke_width,
            depth,
            rotate_x,
            rotate_y,
            ambient,
            specular,
            light_dir,
            light_intensity,
            corner_radius,
            background,
            show_border,
            disabled,
            shadow_token,
            pip,
        } = self;

        let style = ui.style.clone();
        let (avail_w, _) = ui.available_size();
        let width = width_override.unwrap_or_else(|| avail_w.clamp(96.0, 160.0));
        let height = height_override.unwrap_or(96.0);

        let sense = if pip && !disabled {
            Sense::Click
        } else {
            Sense::None
        };
        let pip_strip = if pip && !disabled { 24.0 } else { 0.0 };
        let alloc_h = height + pip_strip;
        let (mut p, mut resp) = ui.allocate_painter((width, alloc_h), sense);

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

        let img_x = p.rect().x();
        let img_y = p.rect().y() + pip_strip;
        let outer = Rect::new(img_x, img_y, width, height);
        if let Some(bg) = background {
            let bg = if disabled { bg.with_alpha(0.5) } else { bg };
            p.fill_rect(outer, CornerRadius::uniform(8.0), Brush::Solid(bg));
        }
        if show_border {
            p.stroke_rect(
                outer,
                CornerRadius::uniform(8.0),
                &Stroke::new(1.0),
                Brush::Solid(style.field_border),
            );
        }

        // Inner content rect — leave a small inset so the shape
        // doesn't kiss the border.
        let pad = 8.0_f32;
        let inner = Rect::new(
            img_x + pad,
            img_y + pad,
            (width - 2.0 * pad).max(0.0),
            (height - 2.0 * pad).max(0.0),
        );
        let min_side = inner.width().min(inner.height());
        let alpha = if disabled { 0.5 } else { 1.0 };
        let fill_brush = fill.unwrap_or(style.accent).with_alpha(alpha);

        // ─── 2D variants ──────────────────────────────────────
        match shape {
            SdfShape::Circle2D => {
                let r = min_side * 0.5;
                let cx = inner.x() + inner.width() * 0.5;
                let cy = inner.y() + inner.height() * 0.5;
                p.fill_circle(Point::new(cx, cy), r, Brush::Solid(fill_brush));
                if let Some(sc) = stroke {
                    p.stroke_circle(
                        Point::new(cx, cy),
                        r,
                        &Stroke::new(stroke_width.max(1.0)),
                        Brush::Solid(sc.with_alpha(alpha)),
                    );
                }
            }
            SdfShape::RoundedBox2D => {
                p.fill_rect(
                    inner,
                    CornerRadius::uniform(corner_radius),
                    Brush::Solid(fill_brush),
                );
                if let Some(sc) = stroke {
                    p.stroke_rect(
                        inner,
                        CornerRadius::uniform(corner_radius),
                        &Stroke::new(stroke_width.max(1.0)),
                        Brush::Solid(sc.with_alpha(alpha)),
                    );
                }
            }
            SdfShape::Capsule2D => {
                let r = inner.height() * 0.5;
                p.fill_rect(
                    inner,
                    CornerRadius::uniform(r),
                    Brush::Solid(fill_brush),
                );
                if let Some(sc) = stroke {
                    p.stroke_rect(
                        inner,
                        CornerRadius::uniform(r),
                        &Stroke::new(stroke_width.max(1.0)),
                        Brush::Solid(sc.with_alpha(alpha)),
                    );
                }
            }
            _ if shape.is_3d() => {
                // ─── 3D variants ──────────────────────────────
                // Push a clip at the widget's inner rect so the
                // SDF's rotated 3D projection can't bleed past
                // the border (sphere / torus rotated cylinder
                // would otherwise have their projected silhouette
                // extend outside the painter rect at small
                // widget sizes).
                let ctx = &mut *p.ctx;
                ctx.push_clip(ClipShape::rect(inner));
                // perspective_d pulls the virtual camera back.
                // Box / cylinder have square / rectangular
                // silhouettes that span the full rotated
                // bounding box — they need more distance to fit
                // (~2000 instead of 1400). Sphere / torus /
                // capsule have rounder projections that already
                // fit at 1400.
                let persp = match shape {
                    SdfShape::Box3D | SdfShape::Cylinder3D => 2000.0,
                    _ => 1400.0,
                };
                ctx.set_3d_transform(rotate_x, rotate_y, persp);
                ctx.set_3d_shape(
                    shape.shape_type_3d(),
                    depth * min_side,
                    ambient,
                    specular,
                );
                ctx.set_3d_light(light_dir, light_intensity);

                // For a sphere / cylinder / torus the shader uses
                // the rect's inscribed circle; for box / capsule
                // it uses the rect bounds. Either way we square
                // the inner rect so the proportions read right.
                let sq = min_side;
                let shape_rect = Rect::new(
                    inner.x() + (inner.width() - sq) * 0.5,
                    inner.y() + (inner.height() - sq) * 0.5,
                    sq,
                    sq,
                );
                p.fill_rect(
                    shape_rect,
                    CornerRadius::uniform(0.0),
                    Brush::Solid(fill_brush),
                );

                // Reset 3D state + pop the clip we pushed above.
                let ctx = &mut *p.ctx;
                ctx.set_3d_shape(0.0, 0.0, 0.3, 32.0);
                ctx.set_3d_transform(0.0, 0.0, 0.0);
                ctx.set_3d_light([0.0, 0.0, 0.0], 0.0);
                ctx.set_3d_translate_z(0.0);
                ctx.pop_clip();
            }
            _ => {}
        }

        // PiP corner button (same shape as every chart / noise /
        // texture widget).
        let pointer_in_pip = if pip && !disabled {
            if let Some(local) = resp.pointer_local {
                let lx = p.rect().x() + local.x;
                let ly = p.rect().y() + local.y;
                let pad = 4.0_f32;
                let btn_size = 20.0_f32;
                let bx = p.rect().x() + p.rect().width() - btn_size - pad;
                let by = p.rect().y() + pad;
                lx >= bx && lx < bx + btn_size && ly >= by && ly < by + btn_size
            } else {
                false
            }
        } else {
            false
        };
        if pip && !disabled {
            let pad = 4.0_f32;
            let btn_size = 20.0_f32;
            let bx = p.rect().x() + p.rect().width() - btn_size - pad;
            let by = p.rect().y() + pad;
            let btn_rect = Rect::new(bx, by, btn_size, btn_size);
            let (btn_bg, btn_border, icon_color) = if pointer_in_pip {
                (style.button_hover, style.field_border_focus, style.text_primary)
            } else {
                (style.background, style.field_border, style.text_secondary)
            };
            p.fill_rect(btn_rect, CornerRadius::uniform(4.0), Brush::Solid(btn_bg));
            p.stroke_rect(
                btn_rect,
                CornerRadius::uniform(4.0),
                &Stroke::new(1.0),
                Brush::Solid(btn_border),
            );
            const GW: f32 = 12.0;
            const GH: f32 = 9.0;
            let gx = bx + ((btn_size - GW) * 0.5).round();
            let gy = by + ((btn_size - GH) * 0.5).round();
            p.stroke_rect(
                Rect::new(gx, gy, GW, GH),
                CornerRadius::uniform(1.5),
                &Stroke::new(1.5),
                Brush::Solid(icon_color),
            );
            p.fill_rect(
                Rect::new(gx + 6.0, gy + 5.0, 5.0, 4.0),
                CornerRadius::uniform(1.0),
                Brush::Solid(icon_color),
            );
            if resp.clicked && pointer_in_pip {
                resp.clicked = false;
                resp.pip_clicked = true;
            }
        }

        resp
    }
}

impl<'a> PortalUi<'a> {
    /// SDF shape preview — single shape painted into a small
    /// rect. 2D variants use direct fill primitives;
    /// 3D variants drive the GPU's per-element raymarching SDF
    /// (box / sphere / cylinder / torus / capsule) via the
    /// `set_3d_*` state on `DrawContext`. Chain `.shape(...)` /
    /// `.depth(...)` / `.rotate_x(...)` / `.rotate_y(...)` /
    /// `.light(...)` for 3D variants; `.fill(c)` / `.stroke(c)` /
    /// `.corner_radius(r)` for 2D; `.show_border(false)` /
    /// `.background(c)` / `.shadow_*()` for chrome; `.pip(true)`
    /// for the corner popover button.
    pub fn sdf_shape<'b>(&'b mut self, shape: SdfShape) -> SdfShapeBuilder<'a, 'b> {
        SdfShapeBuilder {
            ui: self,
            shape,
            width_override: None,
            height_override: None,
            fill: None,
            stroke: None,
            stroke_width: 1.5,
            // depth=1.0 == one full min-side worth of z-extrusion.
            // Curved SDFs (sphere / torus / cylinder) need at
            // least this much depth for the silhouette to read
            // crisp at small widget sizes — too-shallow z makes
            // the raymarcher hit the back face inside the
            // anti-alias band and the edges go soft. Box reads
            // fine at any depth.
            depth: 1.0,
            rotate_x: 0.35,
            rotate_y: 0.7,
            ambient: 0.3,
            specular: 32.0,
            light_dir: [-0.5, -1.0, 0.5],
            light_intensity: 0.8,
            corner_radius: 12.0,
            background: None,
            show_border: true,
            disabled: false,
            shadow_token: None,
            pip: false,
        }
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
