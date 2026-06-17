//! Immediate-mode widget toolkit for canvas closures.
//!
//! See `README.md` for the architecture overview, state model,
//! overlay-escape story, and painter usage. This crate's items are
//! the practical surface; the README is the explanation.

pub mod core;
pub mod painter;
pub mod signal;
pub mod ui;
pub mod widget;

pub use crate::core::{
    HostBridge, PortalId, PortalStorage, PortalStyle, Response, Sense, WidgetId,
};
pub use crate::painter::PortalPainter;
pub use crate::signal::PortalSubscriptions;
pub use crate::ui::{Portal, PortalUi};
