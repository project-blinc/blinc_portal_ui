//! Immediate-mode widget toolkit for canvas closures.
//!
//! A [`Portal`] is a host-owned runtime keyed by a stable id (a node
//! id, a panel id) that drives one rectangular slice of an existing
//! canvas every frame. Open a frame with the typed-builder
//! [`Portal::begin`] (`.style(...).host(...).clip_radius(...).run(|ui|
//! ...)`); the closure paints widgets into the slice via a per-frame
//! [`PortalUi`], registers hit regions with the host's `CanvasKit`,
//! and reads back interaction state — all synchronously, no retained
//! tree. The terminal `.run(...)` returns a [`FrameOutcome`] with
//! `needs_redraw()` + `natural_width()` / `natural_height()` for
//! hosts driving fit-content sizing.
//!
//! Three pieces compose: [`PortalStyle`] supplies theme-derived colours
//! and metrics, [`HostBridge`] supplies coordinate transforms so widgets
//! can anchor overlays outside the canvas, and [`PortalStorage`] holds
//! the per-widget scratch state (slider drag offset, hover ease t) that
//! frame-to-frame continuity needs. Built-in widgets ([`widget`] module)
//! consume all three; custom widgets can ignore any of them.
//!
//! The built-in catalog covers input widgets (label, button, switch,
//! slider, numeric_input, text_input, color_picker, select) plus a
//! square-aspect display-portal family (chart / pie_chart /
//! radar_chart / noise / texture / sdf_shape) that all support a
//! `.pip(true)` corner button — the host wires `Response.pip_clicked`
//! to an overlay popover for the expanded view. See the crate
//! `README.md` for the widget catalog, PiP recipe, and fit-content
//! sizing pattern.

pub mod color_wheel;
pub mod core;
pub mod painter;
pub mod signal;
pub mod ui;
pub mod widget;

pub use crate::color_wheel::color_wheel_panel;
pub use crate::core::{
    ButtonPalette, ButtonPalettes, ButtonShadows, ButtonVariant, HostBridge, PortalId,
    PortalStorage, PortalStyle, PortalValue, Response, Sense, ShadowMix, ShadowToken, ValueBinding,
    WidgetId,
};
pub use crate::painter::PortalPainter;
pub use crate::signal::PortalSubscriptions;
pub use crate::ui::{
    install_click_hook, FrameOutcome, LayoutDirection, Portal, PortalFrame, PortalManager, PortalUi,
};
pub use crate::widget::{
    ButtonBuilder, ChartDecimation, ChartVariant, ChartsBuilder, ColorPickerBuilder, NoiseBuilder,
    NoiseVariant, NumericInputBuilder, PieChartBuilder, RadarChartBuilder, SdfShape,
    SdfShapeBuilder, SelectBuilder, SliderBuilder, SwitchBuilder, TextInputBuilder, TextureBuilder,
    TextureFit, TextureSource,
};
