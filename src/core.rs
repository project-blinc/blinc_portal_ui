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

/// Re-export so callers can pick a widget shadow without `use
/// blinc_theme::tokens::ShadowToken` boilerplate.
pub use blinc_theme::tokens::ShadowToken;

/// Centralises the `.shadow(token)` + `.shadow_sm()` / `.shadow_md()`
/// / `.shadow_lg()` / `.shadow_xl()` / `.shadow_2xl()` /
/// `.shadow_inner()` / `.shadow_default()` / `.shadow_none()` fluent
/// surface every widget builder exposes. Implementors only have to
/// override `shadow(token)`; the typed shortcuts default to forward
/// into it.
///
/// Resolution at paint time mirrors the contract documented on
/// `ButtonBuilder`: disabled state always wins (empty stack), then
/// an explicit `Some(token)` resolves via
/// `ThemeState::get().shadows().get(token)`, then falls back to the
/// widget's per-variant default (`ShadowToken::None` for the
/// non-button widgets today — they paint flat per cn parity).
pub trait ShadowMix: Sized {
    fn shadow(self, token: ShadowToken) -> Self;
    fn shadow_sm(self) -> Self {
        self.shadow(ShadowToken::Sm)
    }
    fn shadow_default(self) -> Self {
        self.shadow(ShadowToken::Default)
    }
    fn shadow_md(self) -> Self {
        self.shadow(ShadowToken::Md)
    }
    fn shadow_lg(self) -> Self {
        self.shadow(ShadowToken::Lg)
    }
    fn shadow_xl(self) -> Self {
        self.shadow(ShadowToken::Xl)
    }
    fn shadow_2xl(self) -> Self {
        self.shadow(ShadowToken::Xxl)
    }
    fn shadow_inner(self) -> Self {
        self.shadow(ShadowToken::Inner)
    }
    fn shadow_none(self) -> Self {
        self.shadow(ShadowToken::None)
    }
}

/// Sealed entry-point trait used by widget builders that own a value
/// (Switch, Slider, TextInput). Implemented for both `&mut T` and
/// `&Signal<T>` so a single `ui.switch(...)` / `ui.slider(...)` /
/// `ui.text_input(...)` accepts either; the builder reads / writes
/// the bound source through the resulting [`ValueBinding`].
///
/// The "sealed" status is enforced by keeping `BindToken` private —
/// out-of-crate types can't accidentally satisfy the trait, which
/// prevents the `&mut Signal<T>` ambiguity the workflow design
/// flagged.
mod sealed {
    pub struct BindToken;
}

/// Carrier that the value-bearing widget builders read + write
/// through. Two variants — borrowed-mut for caller-owned state and
/// signal-bound for cross-portal reactive state.
pub enum ValueBinding<'b, T: 'b> {
    Mut(&'b mut T),
    Signal(&'b blinc_core::reactive::Signal<T>),
}

impl<'b, T: Clone + Send + Sync + 'static> ValueBinding<'b, T> {
    /// Read the current value. Cheap for `Mut` (clone of `T`); for
    /// `Signal` this also subscribes the portal for reactive
    /// re-paint when any external writer mutates the signal.
    pub fn get(&self) -> T
    where
        T: Default,
    {
        match self {
            ValueBinding::Mut(v) => (*v).clone(),
            ValueBinding::Signal(sig) => sig.get(),
        }
    }

    /// Write a new value back through the binding. For `Mut` this
    /// writes in place; for `Signal` it goes through `Signal::set`
    /// which fires the dirty bit for downstream subscribers.
    pub fn set(&mut self, new: T) {
        match self {
            ValueBinding::Mut(v) => **v = new,
            ValueBinding::Signal(sig) => sig.set(new),
        }
    }
}

/// Conversion trait for the value-bearing builders' entry points.
/// Single inherent method per widget on `PortalUi`:
///
///     pub fn switch<'b, V: PortalValue<'b, bool>>(&'b mut self, v: V)
///         -> SwitchBuilder<'a, 'b>
///
/// works with both `&mut bool` and `&Signal<bool>` callers, no
/// `switch_signal` overload needed. The deprecated `*_signal`
/// methods still exist as one-line shims for source compat.
#[allow(private_bounds)]
pub trait PortalValue<'b, T: 'b> {
    #[doc(hidden)]
    const _SEAL: sealed::BindToken = sealed::BindToken;
    fn into_binding(self) -> ValueBinding<'b, T>;
}

impl<'b, T: 'b> PortalValue<'b, T> for &'b mut T {
    fn into_binding(self) -> ValueBinding<'b, T> {
        ValueBinding::Mut(self)
    }
}

impl<'b, T: 'b> PortalValue<'b, T> for &'b blinc_core::reactive::Signal<T> {
    fn into_binding(self) -> ValueBinding<'b, T> {
        ValueBinding::Signal(self)
    }
}

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
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, Default)]
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
    /// Stable widget id assigned by [`PortalUi::allocate_painter`].
    /// Widgets that need to set / check focus (text_input, future
    /// number input) read this without having to call
    /// `make_widget_id` themselves (which would bump the per-frame
    /// call counter and shift every later widget's id). Defaults to
    /// `WidgetId::default()` on `Response::empty()`.
    pub widget_id: WidgetId,
    /// Set by widgets that paint a "picture-in-picture" corner icon
    /// (charts, pie, radar) when the user clicks it. The host's
    /// overlay-escape contract uses this to mount an expanded view
    /// of the widget in a popover. `false` for every widget that
    /// doesn't expose a PiP affordance.
    pub pip_clicked: bool,
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
            widget_id: WidgetId::default(),
            pip_clicked: false,
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

/// Which visual treatment a [`Button`](crate::widget) should paint.
///
/// Naming mirrors `blinc_cn::ButtonVariant` exactly so the two crates
/// share one vocabulary. Each variant is resolved into a
/// [`ButtonPalette`] at theme-load time (see
/// [`PortalStyle::from_active_theme`]).
///
/// The portal-widget DEFAULT is [`ButtonVariant::Ghost`], not
/// `Primary`. cn::button defaults to `Primary` because cn buttons
/// live in chrome (toolbars, dialogs) where a saturated accent fill
/// is the standard CTA look. portal_ui buttons live inside node
/// content slots where a Primary fill would dominate the canvas;
/// `Ghost` is the right inline default. Use `.primary()` explicitly
/// to opt into the saturated treatment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum ButtonVariant {
    /// Filled accent fill, inverse text. cn::ButtonVariant::Primary
    /// parity. Use for the bold call-to-action.
    Primary,
    /// Secondary brand fill — alternate action paired with a Primary.
    /// cn::ButtonVariant::Secondary parity.
    Secondary,
    /// Error-tinted fill for destructive / irreversible actions.
    /// cn::ButtonVariant::Destructive parity.
    Destructive,
    /// Transparent fill with a 1px border. cn::ButtonVariant::Outline
    /// parity.
    Outline,
    /// Transparent fill, text-primary glyph. Hovers / presses with a
    /// low-alpha text-over-bg wash. portal_ui's DEFAULT when no
    /// variant is selected.
    #[default]
    Ghost,
    /// Text-coloured, no chrome. cn::ButtonVariant::Link parity.
    Link,
}

/// Pre-resolved colour set for one [`ButtonVariant`] in one state.
///
/// Populated once per frame inside [`PortalStyle::from_active_theme`]
/// by reading [`blinc_theme::tokens::ColorToken`]s; widgets paint by
/// switching on `idle / hover / pressed / text / border` without
/// further token reads. Disabled state lives outside the per-variant
/// table — see [`ButtonPalettes::disabled`].
#[derive(Clone, Copy, Debug)]
pub struct ButtonPalette {
    pub idle: Color,
    pub hover: Color,
    pub pressed: Color,
    pub text: Color,
    /// `Some` only for variants that paint a border (Outline today,
    /// future Toggled / Selected states). Variants with `border:
    /// None` skip the border stroke.
    pub border: Option<Color>,
}

/// One [`ButtonPalette`] per [`ButtonVariant`], plus a single
/// `disabled` palette shared across variants. Mirrors
/// `cn::ButtonVariant::background / foreground / border` resolution
/// so portal_ui and cn paint identical pixels for the same variant +
/// state.
#[derive(Clone, Debug)]
pub struct ButtonPalettes {
    pub primary: ButtonPalette,
    pub secondary: ButtonPalette,
    pub destructive: ButtonPalette,
    pub outline: ButtonPalette,
    pub ghost: ButtonPalette,
    pub link: ButtonPalette,
    /// Cross-variant disabled look — cn::button collapses every
    /// variant's disabled treatment to a single (InputBgDisabled,
    /// TextTertiary, BorderSecondary) triple. portal_ui follows
    /// suit so a disabled button looks identical regardless of its
    /// pre-disable variant.
    pub disabled: ButtonPalette,
}

impl ButtonPalettes {
    /// Look up the palette for a given variant (excluding the
    /// cross-cutting `disabled` state — that is read directly via
    /// `self.disabled`).
    pub fn for_variant(&self, v: ButtonVariant) -> &ButtonPalette {
        match v {
            ButtonVariant::Primary => &self.primary,
            ButtonVariant::Secondary => &self.secondary,
            ButtonVariant::Destructive => &self.destructive,
            ButtonVariant::Outline => &self.outline,
            ButtonVariant::Ghost => &self.ghost,
            ButtonVariant::Link => &self.link,
        }
    }
}

/// Pre-resolved shadow stacks per [`ButtonVariant`], plus a shared
/// `disabled` slot. Mirrors [`ButtonPalettes`] so token resolution
/// happens once per frame in
/// [`PortalStyle::from_active_theme`] and widgets pay only a slice
/// read at paint time.
///
/// Each slot stores `Vec<blinc_core::layer::Shadow>` — already
/// lowered from the theme's `blinc_theme::Shadow` so the painter
/// can hand them to `DrawContext::draw_shadow` directly without
/// re-conversion.
#[derive(Clone, Debug)]
pub struct ButtonShadows {
    pub primary: Vec<blinc_core::layer::Shadow>,
    pub secondary: Vec<blinc_core::layer::Shadow>,
    pub destructive: Vec<blinc_core::layer::Shadow>,
    pub outline: Vec<blinc_core::layer::Shadow>,
    pub ghost: Vec<blinc_core::layer::Shadow>,
    pub link: Vec<blinc_core::layer::Shadow>,
    /// Cross-variant disabled shadow — always empty (cn::button
    /// parity: disabled buttons render flat).
    pub disabled: Vec<blinc_core::layer::Shadow>,
}

impl ButtonShadows {
    pub fn for_variant(&self, v: ButtonVariant) -> &[blinc_core::layer::Shadow] {
        match v {
            ButtonVariant::Primary => &self.primary,
            ButtonVariant::Secondary => &self.secondary,
            ButtonVariant::Destructive => &self.destructive,
            ButtonVariant::Outline => &self.outline,
            ButtonVariant::Ghost => &self.ghost,
            ButtonVariant::Link => &self.link,
        }
    }
}

/// Theme-derived visual constants the built-in widgets read from.
///
/// Hosts construct one from the active theme each frame (cheap — just
/// reads from theme tokens). Custom widgets can ignore `PortalStyle`
/// entirely and read theme tokens directly via
/// [`blinc_theme::ThemeState::get`].
///
/// ## Token-discipline policy
///
/// Every colour field on `PortalStyle` traces back to a single
/// [`ColorToken`](blinc_theme::tokens::ColorToken) read in
/// [`Self::from_active_theme`]. The few derived values
/// (`accent_pressed`, the destructive hover/pressed shades) are
/// algorithmic shifts on a token value, not freestanding literals
/// — calling out the known token gaps cn::button papers over:
///
/// - No `ErrorHover` / `ErrorActive` tokens — destructive hover /
///   pressed are `darken(Error, 0.10 / 0.15)`. Matches cn exactly.
/// - No overlay-alpha token — ghost / outline hover/pressed apply
///   alpha 0.05 / 0.10 on `TextPrimary`. Same magic alphas as cn.
/// - No `DisabledText` / `DisabledBorder` for non-input variants —
///   disabled buttons resolve to `(InputBgDisabled, TextTertiary,
///   BorderSecondary)` to stay aligned with cn.
///
/// Don't introduce parallel `ColorToken` entries for derived shades;
/// the algorithmic form is the cross-crate contract.
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

    /// Per-variant button palettes. The recommended access path is
    /// [`Self::buttons`].`for_variant(...)`; the legacy flat fields
    /// (`button_bg`, `button_hover`, `button_pressed`, `button_text`)
    /// continue to mirror the `Ghost` variant for source-compat with
    /// custom widgets that read them directly.
    pub buttons: ButtonPalettes,
    /// Per-variant shadow stacks. Resolved once per frame from
    /// `ThemeState::get().shadows()` so widgets only pay a slice
    /// borrow at paint time. cn::button parity: Primary / Secondary /
    /// Destructive default to `ShadowToken::Md`, Outline to
    /// `ShadowToken::Sm`, Ghost / Link / disabled to none.
    pub buttons_shadow: ButtonShadows,

    /// Deprecated — alias for `buttons.ghost.idle`. Kept for one
    /// release so custom widgets that read this field don't break.
    /// New code should call `style.buttons.for_variant(variant).idle`.
    #[doc(hidden)]
    pub button_bg: Color,
    #[doc(hidden)]
    pub button_hover: Color,
    #[doc(hidden)]
    pub button_pressed: Color,
    #[doc(hidden)]
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

        // 18 % text-over-bg is the contrast sweet spot for the slider
        // track: visible on every inset shade we paint widgets over
        // (white-ish through near-black) without competing with the
        // accent-filled portion of a slider or the thumb glyph.
        let track_alpha = 0.18_f32;
        // Ghost / Outline hover and pressed apply 5% / 10% text-over-
        // surface alphas — same magic numbers cn::button uses for the
        // equivalent variants. No theme token for the subtle-alpha
        // constant; the cross-crate agreement is the contract.
        let overlay_hover_alpha = 0.05_f32;
        let overlay_pressed_alpha = 0.10_f32;

        let error = c(ColorToken::Error);
        let primary = c(ColorToken::Primary);
        let primary_hover = c(ColorToken::PrimaryHover);
        let primary_active = c(ColorToken::PrimaryActive);
        let secondary = c(ColorToken::Secondary);
        let secondary_hover = c(ColorToken::SecondaryHover);
        let secondary_active = c(ColorToken::SecondaryActive);
        let text_inverse = c(ColorToken::TextInverse);
        let border = c(ColorToken::Border);
        let border_secondary = c(ColorToken::BorderSecondary);
        let text_tertiary = c(ColorToken::TextTertiary);
        let input_bg_disabled = c(ColorToken::InputBgDisabled);
        let transparent = Color::rgba(0.0, 0.0, 0.0, 0.0);

        // Pre-resolve per-variant shadow stacks. ThemeState::shadows
        // returns an owned ShadowTokens (clone of the RwLock-guarded
        // inner value); bind to a local so the slice borrows we take
        // off `.get(...)` outlive the temporary. Lower each
        // blinc_theme::Shadow into a blinc_core::layer::Shadow once
        // here so the painter doesn't re-convert per frame.
        let theme_shadows = blinc_theme::ThemeState::get().shadows();
        let lower_stack =
            |stack: &[blinc_theme::tokens::Shadow]| -> Vec<blinc_core::layer::Shadow> {
                stack
                    .iter()
                    .map(|s| blinc_core::layer::Shadow::from(s.clone()))
                    .collect()
            };
        let buttons_shadow = ButtonShadows {
            primary: lower_stack(theme_shadows.get(ShadowToken::Md)),
            secondary: lower_stack(theme_shadows.get(ShadowToken::Md)),
            destructive: lower_stack(theme_shadows.get(ShadowToken::Md)),
            outline: lower_stack(theme_shadows.get(ShadowToken::Sm)),
            ghost: Vec::new(),
            link: Vec::new(),
            // Disabled buttons render flat regardless of pre-disable
            // variant — cn::button parity.
            disabled: Vec::new(),
        };

        // MIRROR cn::ButtonVariant::background / foreground / border.
        // Any change to the destructive darken factors (0.10 / 0.15)
        // or the overlay alphas (0.05 / 0.10) MUST be reflected in
        // cn::button — and vice versa. The two crates paint the
        // exact same pixels for the same variant + state.
        let buttons = ButtonPalettes {
            primary: ButtonPalette {
                idle: primary,
                hover: primary_hover,
                pressed: primary_active,
                text: text_inverse,
                border: None,
            },
            secondary: ButtonPalette {
                idle: secondary,
                hover: secondary_hover,
                pressed: secondary_active,
                text: text_inverse,
                border: None,
            },
            destructive: ButtonPalette {
                idle: error,
                hover: darken(error, 0.10),
                pressed: darken(error, 0.15),
                text: text_inverse,
                border: None,
            },
            outline: ButtonPalette {
                idle: transparent,
                hover: text_primary.with_alpha(overlay_hover_alpha),
                pressed: text_primary.with_alpha(overlay_pressed_alpha),
                text: text_primary,
                border: Some(border),
            },
            ghost: ButtonPalette {
                idle: transparent,
                hover: text_primary.with_alpha(overlay_hover_alpha),
                pressed: text_primary.with_alpha(overlay_pressed_alpha),
                text: text_primary,
                border: None,
            },
            link: ButtonPalette {
                idle: transparent,
                hover: transparent,
                pressed: transparent,
                text: primary,
                border: None,
            },
            disabled: ButtonPalette {
                idle: input_bg_disabled,
                hover: input_bg_disabled,
                pressed: input_bg_disabled,
                text: text_tertiary,
                border: Some(border_secondary),
            },
        };

        Self {
            font_size: 12.0,
            line_height: 16.0,
            spacing: 6.0,
            control_height: 24.0,
            control_radius: 6.0,
            indent: 14.0,
            text_primary,
            text_secondary: c(ColorToken::TextSecondary),
            text_disabled: text_tertiary,
            background: c(ColorToken::SurfaceElevated),
            // Legacy flat fields mirror the Ghost variant so custom
            // widgets that read them directly keep painting the same
            // muted text-over-alpha treatment they always did.
            button_bg: buttons.ghost.idle,
            button_hover: buttons.ghost.hover,
            button_pressed: buttons.ghost.pressed,
            button_text: buttons.ghost.text,
            buttons,
            buttons_shadow,
            field_bg: c(ColorToken::InputBg),
            field_border: border,
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

    #[test]
    fn host_bridge_from_closures_round_trips_through_caller_transform() {
        // Translate canvas → screen by (+100, +200); inverse subtracts.
        let b = HostBridge::from_closures(
            |p| Point::new(p.x + 100.0, p.y + 200.0),
            |p| Point::new(p.x - 100.0, p.y - 200.0),
        );
        let r = Rect::new(10.0, 20.0, 30.0, 40.0);
        let s = b.rect_to_screen(r);
        assert_eq!(
            (s.x(), s.y(), s.width(), s.height()),
            (110.0, 220.0, 30.0, 40.0),
            "rect_to_screen applies canvas_to_screen to both corners"
        );
        let p = (b.screen_to_canvas)(Point::new(110.0, 220.0));
        assert_eq!((p.x, p.y), (10.0, 20.0), "screen_to_canvas inverts");
    }

    #[test]
    fn sense_default_is_none() {
        // Per-axis default for the `Sense` builder slot — paint-only
        // regions should not register a hit region. Default<Sense>
        // is what `allocate_painter(size, Sense::default())` lands on.
        assert_eq!(Sense::default(), Sense::None);
    }

    #[test]
    fn response_chainable_builder_overrides_each_field() {
        let r = Response::empty()
            .with_rect(Rect::new(5.0, 6.0, 7.0, 8.0))
            .with_clicked(true)
            .with_changed(true)
            .with_animating(true);
        assert_eq!(
            (r.rect.x(), r.rect.y(), r.rect.width(), r.rect.height()),
            (5.0, 6.0, 7.0, 8.0)
        );
        assert!(r.clicked);
        assert!(r.changed);
        assert!(r.animating);
        // Untouched fields stay at empty()'s defaults.
        assert!(!r.hovered);
        assert!(!r.pressed);
    }

    #[test]
    fn portal_id_from_hashed_is_deterministic() {
        let a = PortalId::from_hashed("stable_input");
        let b = PortalId::from_hashed("stable_input");
        assert_eq!(a, b, "same input → same id");
        let c = PortalId::from_hashed("different_input");
        assert_ne!(a, c, "different input → different id");
    }
}
