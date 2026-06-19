//! Immediate-mode widget toolkit for canvas closures.
//!
//! A `Portal` is a host-owned runtime keyed by a stable id (a node id,
//! a panel id) that drives one rectangular slice of an existing canvas
//! every frame. The closure handed to [`Portal::frame`] paints widgets
//! into the slice via a per-frame [`PortalUi`], registers hit regions
//! with the host's `CanvasKit`, and reads back interaction state — all
//! synchronously, no retained tree.
//!
//! Three pieces compose: [`PortalStyle`] supplies theme-derived colours
//! and metrics, [`HostBridge`] supplies coordinate transforms so widgets
//! can anchor overlays outside the canvas, and [`PortalStorage`] holds
//! the per-widget scratch state (slider drag offset, hover ease t) that
//! frame-to-frame continuity needs. Built-in widgets ([`widget`] module)
//! consume all three; custom widgets can ignore any of them.

pub mod color_wheel;
pub mod core;
pub mod painter;
pub mod signal;
pub mod ui;
pub mod widget;

pub use crate::color_wheel::color_wheel_panel;
pub use crate::core::{
    ButtonPalette, ButtonPalettes, ButtonShadows, ButtonVariant, HostBridge, PortalId,
    PortalStorage, PortalStyle, PortalValue, Response, Sense, ShadowMix, ShadowToken,
    ValueBinding, WidgetId,
};
pub use crate::widget::{
    ButtonBuilder, ChartDecimation, ChartVariant, ChartsBuilder, ColorPickerBuilder,
    NumericInputBuilder, SelectBuilder, SliderBuilder, SwitchBuilder, TextInputBuilder,
};
pub use crate::painter::PortalPainter;
pub use crate::signal::PortalSubscriptions;
pub use crate::ui::{
    install_click_hook, FrameOutcome, LayoutDirection, Portal, PortalFrame, PortalManager, PortalUi,
};
