//! Runtime: [`Portal`], [`PortalUi`], [`PortalManager`].
//!
//! [`Portal`] is the persistent runtime the host owns — one per UI
//! region keyed by a stable host id (a node id, a panel id). The host
//! constructs it once and calls [`Portal::frame`] every paint pass.
//! [`PortalUi`] is the per-frame builder handed to the closure; it
//! holds the cursor / id-stack / layout direction / style and is what
//! widgets mutate as they paint.
//!
//! [`PortalManager`] keeps a `K → Portal` map and the typical
//! get-or-create / retain-live boilerplate so hosts don't re-implement
//! it. The clicked-this-frame cache + the scheduler bridge live here
//! too; both are global side effects scoped to whatever
//! [`install_click_hook`] sees and what
//! [`blinc_animation::try_get_scheduler`] returns.

use crate::core::{HostBridge, PortalId, PortalStorage, PortalStyle, Response, Sense, WidgetId};
use crate::painter::{build_response, PortalPainter};
use crate::signal::PortalSubscriptions;

use blinc_canvas_kit::{CanvasEvent, CanvasKit, InteractionState};
use blinc_core::layer::{ClipShape, CornerRadius, Point, Rect};
use blinc_core::reactive::with_read_tracking;
use blinc_core::DrawContext;

use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};
use web_time::Instant;

// ─────────────────────────────────────────────────────────────────────
// Clicked-this-frame cache — single global, keyed by region id
// ─────────────────────────────────────────────────────────────────────

/// Region-id -> click timestamp.
///
/// CanvasKit dispatches clicks through `on_element_click` callbacks
/// (fire-and-forget); imgui needs to know "did this region get
/// clicked since I last looked?". We hook `on_element_click` once
/// per kit (`install_click_hook`) and insert into this map; each
/// portal frame consumes its entry on read (`map.remove(...)`),
/// so a click fires exactly once for the next widget paint that
/// observes it.
static CLICKS: OnceLock<Mutex<ahash::AHashMap<String, Instant>>> = OnceLock::new();

fn clicks() -> &'static Mutex<ahash::AHashMap<String, Instant>> {
    CLICKS.get_or_init(|| Mutex::new(ahash::AHashMap::default()))
}

/// Drop click-stamp entries older than this duration. Bounds the
/// CLICKS map size in long-running apps: entries from widgets that
/// disappeared before being observed (a popover dismisses mid-click,
/// a tree branch collapses between dispatch and paint) would otherwise
/// stay in the map indefinitely until a fresh widget with the same
/// region id happens to collide. A click that fires within this
/// window of being recorded is still observed exactly once on read.
const CLICK_STAMP_MAX_AGE_MS: u128 = 500;

fn sweep_stale_clicks() {
    let mut map = clicks().lock().unwrap();
    if map.is_empty() {
        return;
    }
    let now = Instant::now();
    map.retain(|_, stamped| now.duration_since(*stamped).as_millis() < CLICK_STAMP_MAX_AGE_MS);
}

/// Install the kit-side click recorder. Hosts call this ONCE after
/// constructing the kit (before any portal runs). The recorder
/// timestamps click events by region id; portal frames consume
/// the entry on read to drive `Response.clicked`.
///
/// Uses [`CanvasKit::add_click_listener`] (additive) rather than
/// [`CanvasKit::on_element_click`] (single-owner / replacing) so
/// the host's own click handler (e.g. node-editor's selection
/// logic) is left intact — both fire on every click.
pub fn install_click_hook(kit: &mut CanvasKit) {
    kit.add_click_listener(|evt: &CanvasEvent| {
        if let Some(id) = &evt.region_id {
            clicks().lock().unwrap().insert(id.clone(), Instant::now());
        }
    });
}

// ─────────────────────────────────────────────────────────────────────
// Portal — persistent runtime owned by the host
// ─────────────────────────────────────────────────────────────────────

/// One portal = one immediate-mode UI region owned by the host
/// (typically tied to a stable host id like a node id or panel id).
///
/// Lifecycle: construct once, call [`Self::frame`] every paint pass
/// (inside the canvas closure), drop when the host removes the
/// associated entity. Drop unsubscribes any signal subscriptions.
pub struct Portal {
    id: PortalId,
    storage: PortalStorage,
    subs: PortalSubscriptions,
    created_at: Instant,
    /// Per-widget previous-frame drag positions, keyed by region id.
    /// `Response::drag_delta_local` reads (current - prev) for the
    /// region under the kit's `active` cursor.
    last_drag_pos: ahash::AHashMap<String, Point>,
    /// Pixels of vertical content the closure produced last frame —
    /// `(cursor.y - bounds.y) + trailing spacing`. The host reads
    /// this via [`Self::consumed_height`] and feeds it back as the
    /// portal's bounds on the next frame so the slot grows to fit
    /// whatever the closure painted (the closure has full
    /// `available_size` control over how MUCH it paints; the portal
    /// just makes sure the slot accommodates what it asked for).
    last_consumed_height: f32,
    /// Scheduler tick-callback id while the portal is animating.
    /// `Some` when the closure called `ui.request_animation()` last
    /// frame; the callback is a no-op (the portal repaints itself
    /// via its own dirty path) but its presence forces
    /// `AnimationScheduler::has_active = true`, so the bg thread
    /// ticks at full `target_fps` even when a focused text input is
    /// otherwise driving `wants_continuous` (which would otherwise
    /// downscale the wake cadence to half rate and drag every
    /// portal-driven animation along with it). Removed when the
    /// closure settles (animating goes false). `None` while idle so
    /// idle pages don't keep a stale callback in the scheduler.
    anim_tick_handle: Option<blinc_animation::scheduler::TickCallbackId>,
}

impl Portal {
    /// Build a new portal tied to a stable host key (typically a node
    /// id or panel id). The key is hashed once; later changes to the
    /// underlying value don't affect this portal's identity.
    pub fn new<K: Hash + ?Sized>(host_key: &K) -> Self {
        let id = PortalId::from_hashed(host_key);
        let now = Instant::now();
        Self {
            id,
            storage: PortalStorage::new(),
            subs: PortalSubscriptions::new(id),
            created_at: now,
            last_drag_pos: ahash::AHashMap::default(),
            last_consumed_height: 0.0,
            anim_tick_handle: None,
        }
    }

    /// Pixels of vertical content the closure produced last frame.
    /// The host should feed this back as the portal's bounds height
    /// on the next frame (clamped against the template's declared
    /// minimum) so the slot grows to fit. Returns `0.0` before the
    /// first frame runs.
    pub fn consumed_height(&self) -> f32 {
        self.last_consumed_height
    }

    /// Portal id — used to derive region ids and as the public handle
    /// the host uses to look up the portal in its own map.
    pub fn id(&self) -> PortalId {
        self.id
    }

    /// True if any tracked signal fired since the last frame. Portals
    /// inside canvas closures don't usually need to check this (the
    /// canvas already runs every frame), but a host that gates canvas
    /// repaints on activity can read this to know when to wake.
    pub fn is_dirty(&self) -> bool {
        self.subs.is_dirty()
    }

    /// Manually flag the portal as needing a repaint — useful for
    /// animations driven outside the signal system.
    pub fn mark_dirty(&self) {
        self.subs
            .dirty_flag()
            .store(true, std::sync::atomic::Ordering::Release);
    }

    /// Run a single frame's UI closure. Call inside the canvas
    /// closure once per visible portal region.
    ///
    /// `bounds` is the region the portal occupies, in canvas-content
    /// coordinates. The closure receives a [`PortalUi`] bound to that
    /// rect; widgets paint into `ctx`, register hit-regions with
    /// `kit`, and read interaction state for the frame.
    ///
    /// `clip_radius` is the corner radius the rounded-rect clip uses
    /// — pass the same value the host paints the portal's visible
    /// background with so widget paint clips to that exact curve.
    /// Pass `0.0` for a pure axis-aligned rect clip.
    ///
    /// Returns true if any widget is mid-animation or any read
    /// signal fired this frame — the host should request another
    /// paint when true. The host is expected to feed this into its
    /// own redraw / animation-tick request path; dropping the result
    /// silently is almost always a bug, hence `#[must_use]`.
    #[allow(clippy::too_many_arguments)] // 8 args are intrinsic to the frame contract.
    #[must_use = "Portal::frame returns whether the closure needs another paint; the host should OR this into its redraw / animation_tick request"]
    pub fn frame<F>(
        &mut self,
        ctx: &mut dyn DrawContext,
        kit: &CanvasKit,
        bounds: Rect,
        clip_radius: f32,
        style: &PortalStyle,
        host: &HostBridge,
        ui_closure: F,
    ) -> bool
    where
        F: FnOnce(&mut PortalUi<'_>),
    {
        // Frame bookkeeping
        let frame_start = Instant::now();
        self.subs.clear_dirty();
        self.storage.used_this_frame.clear();
        // Drop click stamps from widgets that disappeared before any
        // paint observed them. Cheap (~O(map size)), runs once per
        // portal-frame; the per-widget read still does its own remove.
        sweep_stale_clicks();
        let interaction = kit.interaction();

        // Push the clip frame BEFORE building the UI so every
        // primitive a widget emits (and any free-form `allocate_painter`
        // draw) is bound to the portal rect. Without this, a too-wide
        // label / tall slider / sparkline that overshoots its painter
        // bounds bleeds past the inset background — visible at the
        // node's rounded corners and bottom edge. Popped after the
        // closure returns, before the frame helper exits.
        let clip = if clip_radius > 0.0 {
            ClipShape::rounded_rect(bounds, CornerRadius::uniform(clip_radius))
        } else {
            ClipShape::rect(bounds)
        };
        ctx.push_clip(clip);

        // Build the PortalUi — borrows everything for the lifetime of
        // this call.
        let mut any_animating = false;
        let portal_id = self.id;
        let time_s = frame_start.duration_since(self.created_at).as_secs_f32();
        // Captured by the closure; read after to compute consumed
        // height. Using a Cell so we don't have to thread `&mut`
        // out of the read-tracking closure.
        let final_cursor_y = std::cell::Cell::new(bounds.y());

        // Run the user closure inside reactive read-tracking so we
        // capture the signal set it actually touched.
        let (_unit, read_signals) = with_read_tracking(|| {
            let mut ui = PortalUi {
                ctx,
                kit,
                bounds,
                cursor: Point::new(bounds.x() + style.spacing, bounds.y() + style.spacing),
                layout: LayoutDirection::Vertical,
                horizontal_row_height: 0.0,
                style,
                storage: &mut self.storage,
                interaction: &interaction,
                last_drag_pos: &mut self.last_drag_pos,
                host,
                id_stack: vec![portal_id.0],
                portal_id,
                time_s,
                animating: &mut any_animating,
                // Reset every frame so the same call-site sequence
                // produces the same WidgetId across frames.
                call_counter: 0,
            };
            ui_closure(&mut ui);
            final_cursor_y.set(ui.cursor.y);
        });

        // Consumed height = how far the cursor advanced from the
        // portal's top edge + a trailing spacing pad. The host uses
        // this on the NEXT frame to size the slot so the closure's
        // output fits. Saturates at 0.0 — defensive for weird
        // closures that move the cursor backward.
        let consumed = (final_cursor_y.get() - bounds.y() + style.spacing).max(0.0);
        let prev = self.last_consumed_height;
        self.last_consumed_height = consumed;
        // Asymmetric feedback gate to keep the slot from pumping a
        // dirty flag every frame on animating children.
        //
        // - Grow: fire immediately — a closure that expanded between
        //   frames is being clipped right now, so the host needs the
        //   next frame at full size.
        // - Shrink: only fire when nothing is mid-animation. Springy
        //   widgets (press scale, hover ease) ripple consumed_height
        //   downward by sub-pixel amounts every frame while settling;
        //   the animating set already keeps the host repainting, so a
        //   second dirty hint just doubles the wake rate. Once the
        //   animations settle (any_animating goes false) the shrink
        //   fires once to re-tighten the slot.
        let dh = consumed - prev;
        let grew = dh > 0.5;
        let shrank_idle = dh < -0.5 && !any_animating;
        if grew || shrank_idle {
            self.mark_dirty();
        }

        // Pop the clip frame pushed at the top — pairs with the
        // `push_clip` above so callers' subsequent draws (other
        // nodes, edges, group chrome) paint unrestricted.
        ctx.pop_clip();

        // Sync subscriptions to the new read set.
        self.subs.update_from_read_set(read_signals);
        // GC storage cells that no widget touched this frame.
        self.storage.gc_frame();

        // Bridge to the animation scheduler so a focused text input
        // (which sets `wants_continuous = true`) doesn't downscale
        // the wake cadence to half rate while this portal has a
        // live animation. The scheduler's `has_active` only sees
        // springs / keyframes / timelines / tick_callbacks — it
        // doesn't know about portal-driven per-frame animation
        // (sparkline tickers, ease-in widgets, etc.) unless we
        // register a tick callback. Empty closure: the portal
        // repaints itself via its own dirty path; we just need a
        // tick_callback present so `!tick_callbacks.is_empty()`
        // contributes to `has_active`. Toggle every frame:
        // register when newly animating, remove when settled.
        if any_animating && self.anim_tick_handle.is_none() {
            if let Some(scheduler) = blinc_animation::try_get_scheduler() {
                self.anim_tick_handle = scheduler.register_tick_callback(|_dt| {});
            }
        } else if !any_animating && self.anim_tick_handle.is_some() {
            if let Some(id) = self.anim_tick_handle.take() {
                if let Some(scheduler) = blinc_animation::try_get_scheduler() {
                    scheduler.remove_tick_callback(id);
                }
            }
        }

        let _ = frame_start;
        any_animating || self.subs.is_dirty()
    }
}

impl Drop for Portal {
    fn drop(&mut self) {
        // A portal dropped mid-animation must hand its tick callback
        // back to the scheduler — otherwise the global has_active
        // count stays artificially elevated and the scheduler keeps
        // ticking at full rate forever (cn_demo regression shape).
        if let Some(id) = self.anim_tick_handle.take() {
            if let Some(scheduler) = blinc_animation::try_get_scheduler() {
                scheduler.remove_tick_callback(id);
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// Portal::begin — chainable builder for frame(...)
// ─────────────────────────────────────────────────────────────────────

/// Chainable alternative to [`Portal::frame`]: configure the frame
/// piece by piece, then `.run(|ui| ...)` to dispatch the closure. The
/// 7 positional arguments to `frame()` are easy to mix up; the
/// builder form makes the wiring self-documenting.
///
/// Constructed via [`Portal::begin`]. `style` and `host` are required;
/// `clip_radius` defaults to `0.0` (axis-aligned rect clip). Calling
/// [`Self::run`] without setting `style` / `host` panics with a clear
/// message rather than producing a runtime miscompute.
pub struct PortalFrame<'a> {
    portal: &'a mut Portal,
    ctx: &'a mut dyn DrawContext,
    kit: &'a CanvasKit,
    bounds: Rect,
    clip_radius: f32,
    style: Option<&'a PortalStyle>,
    host: Option<&'a HostBridge>,
}

impl<'a> PortalFrame<'a> {
    /// Rounded-rect clip radius for the portal viewport. Default `0.0`
    /// (sharp rect clip). Pass the same radius the host paints the
    /// portal's visible background with so widget paint clips to the
    /// exact curve.
    pub fn clip_radius(mut self, r: f32) -> Self {
        self.clip_radius = r;
        self
    }

    /// Active style for the frame. Built-in widgets read colours and
    /// metrics from this; custom widgets can ignore it. Required.
    pub fn style(mut self, style: &'a PortalStyle) -> Self {
        self.style = Some(style);
        self
    }

    /// Coordinate-transform bridge for widgets that need to anchor
    /// host-screen overlays. Pass [`HostBridge::identity`] when the
    /// portal lives in a non-transformed canvas. Required.
    pub fn host(mut self, host: &'a HostBridge) -> Self {
        self.host = Some(host);
        self
    }

    /// Dispatch the UI closure. Returns a [`FrameOutcome`] whose
    /// `needs_redraw()` the host ORs into its repaint / animation-tick
    /// request path. Marked `#[must_use]` on the outcome type so
    /// dropping the result without consulting it warns.
    pub fn run<F>(self, ui_closure: F) -> FrameOutcome
    where
        F: FnOnce(&mut PortalUi<'_>),
    {
        let style = self
            .style
            .expect("PortalFrame::run: missing .style(&PortalStyle)");
        let host = self
            .host
            .expect("PortalFrame::run: missing .host(&HostBridge)");
        let any = self.portal.frame(
            self.ctx,
            self.kit,
            self.bounds,
            self.clip_radius,
            style,
            host,
            ui_closure,
        );
        FrameOutcome {
            any_animating_or_dirty: any,
        }
    }
}

/// Typed return from [`PortalFrame::run`]. The host should call
/// [`Self::needs_redraw`] and feed it into its repaint /
/// animation-tick request path.
#[must_use = "FrameOutcome reports whether the portal needs another paint; consult `.needs_redraw()`"]
pub struct FrameOutcome {
    any_animating_or_dirty: bool,
}

impl FrameOutcome {
    /// True if any widget is mid-animation or any read signal fired
    /// this frame — the host should request another paint when true.
    pub fn needs_redraw(&self) -> bool {
        self.any_animating_or_dirty
    }
}

impl Portal {
    /// Open a [`PortalFrame`] builder bound to `ctx` / `kit` / `bounds`.
    /// Set `.style(...)` / `.host(...)` (and optionally `.clip_radius(...)`)
    /// then call `.run(|ui| ...)`. Equivalent to [`Self::frame`] but
    /// the wiring is self-documenting.
    pub fn begin<'a>(
        &'a mut self,
        ctx: &'a mut dyn DrawContext,
        kit: &'a CanvasKit,
        bounds: Rect,
    ) -> PortalFrame<'a> {
        PortalFrame {
            portal: self,
            ctx,
            kit,
            bounds,
            clip_radius: 0.0,
            style: None,
            host: None,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// PortalUi — per-frame builder
// ─────────────────────────────────────────────────────────────────────

/// Layout direction for the current group. Cursor advances along this
/// axis after each widget; `available_size` measures the
/// perpendicular axis as remaining space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutDirection {
    Vertical,
    Horizontal,
}

/// Per-frame builder handed to the portal's UI closure. Holds the
/// cursor / id stack / layout direction in the active scope; widgets
/// mutate it as they paint.
pub struct PortalUi<'a> {
    pub(crate) ctx: &'a mut dyn DrawContext,
    pub(crate) kit: &'a CanvasKit,
    pub(crate) bounds: Rect,
    pub(crate) cursor: Point,
    pub(crate) layout: LayoutDirection,
    /// Tallest widget allocated in the current horizontal row;
    /// horizontal() advances the parent cursor by this after the
    /// closure returns.
    pub(crate) horizontal_row_height: f32,
    pub(crate) style: &'a PortalStyle,
    /// Per-widget scratch state. Widgets that hold local state
    /// (slider drag offset, text-input cursor, hover ease t) borrow
    /// this via `get_or_insert_with`; widgets without local state
    /// (button, label) don't touch it. Frame-end GC keeps it pruned.
    #[allow(dead_code)] // exposed for widget impls; not all built-ins use it yet
    pub(crate) storage: &'a mut PortalStorage,
    pub(crate) interaction: &'a InteractionState,
    pub(crate) last_drag_pos: &'a mut ahash::AHashMap<String, Point>,
    pub(crate) host: &'a HostBridge,
    pub(crate) id_stack: Vec<u64>,
    pub(crate) portal_id: PortalId,
    pub(crate) time_s: f32,
    pub(crate) animating: &'a mut bool,
    /// Monotonic per-frame counter. Bumps on every widget allocation
    /// (`make_widget_id` increments + reads). Mixed into the widget
    /// id hash so two no-keyed widgets at different call sites get
    /// distinct ids — without it every label / button / switch would
    /// collide under `hash(id_stack)`. Stable across frames as long
    /// as the closure issues widget calls in the same order; for
    /// loops / conditionals, use [`Self::push_id`] / a caller key.
    pub(crate) call_counter: u32,
}

impl<'a> PortalUi<'a> {
    /// Active style (clone-cheap; widgets read this each call).
    pub fn style(&self) -> &PortalStyle {
        self.style
    }

    /// Portal clock — monotonic seconds since the portal was created.
    pub fn time(&self) -> f32 {
        self.time_s
    }

    /// Region the portal is painting into.
    pub fn bounds(&self) -> Rect {
        self.bounds
    }

    /// Current cursor position in canvas-content coordinates — the
    /// origin the next `allocate_painter` will use. Lets custom
    /// layout helpers read where the cursor sits without reaching
    /// into the type's internals.
    pub fn cursor(&self) -> Point {
        self.cursor
    }

    /// Active layout direction (vertical or horizontal). Useful for
    /// widgets that adapt their secondary-axis sizing based on which
    /// way the cursor is advancing.
    pub fn layout(&self) -> LayoutDirection {
        self.layout
    }

    /// Borrow the host's coordinate-transform bridge. Widgets that
    /// need to anchor an overlay outside the canvas (a host-native
    /// popover for typing, a colour picker, a tooltip) call
    /// `ui.host().canvas_to_screen(point)` or
    /// `ui.host().rect_to_screen(rect)` to convert their widget-local
    /// rect into the screen-space coords the overlay manager expects.
    pub fn host(&self) -> &HostBridge {
        self.host
    }

    /// Remaining space the next widget can grow into.
    ///
    /// - `w` is the remaining horizontal room from the cursor to the
    ///   right edge of the portal bounds (minus trailing spacing pad).
    ///   In horizontal layout this is what's left of the current row;
    ///   in vertical it's the full row width.
    /// - `h` semantics depend on layout direction:
    ///   - **Vertical**: remaining vertical space from the cursor down
    ///     to the bottom edge of the portal bounds.
    ///   - **Horizontal**: a *row height budget* equal to
    ///     `style.control_height`. Reporting the full remaining
    ///     vertical extent would over-promise — a tall widget that
    ///     consumed it would paint past the row's intended footprint
    ///     and overlap anything stacked below the row.
    ///
    /// Widgets that need taller content should compose vertically.
    pub fn available_size(&self) -> (f32, f32) {
        let w =
            (self.bounds.x() + self.bounds.width() - self.cursor.x - self.style.spacing).max(0.0);
        let h = match self.layout {
            LayoutDirection::Vertical => {
                (self.bounds.y() + self.bounds.height() - self.cursor.y - self.style.spacing)
                    .max(0.0)
            }
            LayoutDirection::Horizontal => self.style.control_height,
        };
        (w, h)
    }

    /// Advance the cursor by `px` in the layout direction.
    pub fn spacing(&mut self, px: f32) {
        match self.layout {
            LayoutDirection::Vertical => self.cursor.y += px,
            LayoutDirection::Horizontal => self.cursor.x += px,
        }
    }

    /// Push an extra hashable key onto the id stack for the lifetime
    /// of the closure. Use when emitting a widget inside a loop so
    /// each iteration gets a distinct [`WidgetId`].
    pub fn push_id<R, K: Hash>(&mut self, key: K, f: impl FnOnce(&mut Self) -> R) -> R {
        let mut h = ahash::AHasher::default();
        key.hash(&mut h);
        self.id_stack.push(h.finish());
        let r = f(self);
        self.id_stack.pop();
        r
    }

    /// Mark a widget id used this frame and return its hash. Public
    /// so custom widget implementations can stamp themselves.
    ///
    /// Mixes the per-frame call counter into the hash + bumps it, so
    /// two consecutive no-keyed widget calls get distinct ids. Stable
    /// across frames when the call sequence is stable; pair with
    /// [`Self::push_id`] inside loops / conditional branches to keep
    /// ids correlated.
    pub fn make_widget_id(&mut self, caller_key: Option<&str>) -> WidgetId {
        let mut h = ahash::AHasher::default();
        for layer in &self.id_stack {
            layer.hash(&mut h);
        }
        self.call_counter.hash(&mut h);
        self.call_counter = self.call_counter.wrapping_add(1);
        if let Some(k) = caller_key {
            k.hash(&mut h);
        }
        WidgetId(h.finish())
    }

    /// Tell the portal an animation is in flight so the next frame
    /// is requested. Widgets that ease over time (hover, press) flip
    /// this when their interpolation hasn't settled.
    pub fn request_animation(&mut self) {
        *self.animating = true;
    }

    /// Reserve a rect of `size` at the cursor and return a painter
    /// over it plus a `Response` built from the kit's interaction
    /// state. The cursor advances along the layout direction by the
    /// rect's primary-axis dimension + `style.spacing`; in
    /// horizontal layouts the row's tallest widget governs the
    /// parent's eventual vertical advance.
    pub fn allocate_painter(
        &mut self,
        size: (f32, f32),
        sense: Sense,
    ) -> (PortalPainter<'_>, Response) {
        self.allocate_painter_with_key(size, sense, None)
    }

    /// Reserve a rect of `size` at the cursor and return ONLY the
    /// painter — for decorative fills, separators, and anything else
    /// that doesn't need a hit region. Equivalent to
    /// `allocate_painter(size, Sense::None).0` but skips the response
    /// build entirely and saves the caller a discarded `_` binding.
    /// The cursor advances the same way as [`Self::allocate_painter`].
    pub fn allocate_paint(&mut self, size: (f32, f32)) -> PortalPainter<'_> {
        self.allocate_painter(size, Sense::None).0
    }

    /// `allocate_painter` with an explicit caller key. Useful when
    /// two widgets at the same call site need distinct ids (e.g. a
    /// row of buttons inside a `for` loop).
    pub fn allocate_painter_with_key(
        &mut self,
        size: (f32, f32),
        sense: Sense,
        caller_key: Option<&str>,
    ) -> (PortalPainter<'_>, Response) {
        let rect = Rect::new(self.cursor.x, self.cursor.y, size.0, size.1);
        // Advance the cursor for the next widget.
        match self.layout {
            LayoutDirection::Vertical => self.cursor.y += size.1 + self.style.spacing,
            LayoutDirection::Horizontal => {
                self.cursor.x += size.0 + self.style.spacing;
                if size.1 > self.horizontal_row_height {
                    self.horizontal_row_height = size.1;
                }
            }
        }

        // Build a response from the kit's interaction state.
        let widget_id = self.make_widget_id(caller_key);
        let region_id = widget_id.to_region_id(self.portal_id);

        let mut response = Response::empty();
        response.rect = rect;

        match sense {
            Sense::None => {
                // No hit-region; visual decoration only.
            }
            Sense::Hover | Sense::Click | Sense::Drag => {
                self.kit.hit_rect(region_id.clone(), rect);
                let hovered = self.interaction.hovered.as_deref() == Some(region_id.as_str());
                let active = self.interaction.active.as_deref() == Some(region_id.as_str());
                let pointer_world = if active || hovered {
                    // Use the kit's live cursor position when present.
                    // Falls back to drag_start for the active case
                    // (only fires for the first POINTER_DOWN before
                    // the kit's continuous tracking populates) and to
                    // the rect centre for hover when the kit hasn't
                    // produced any pointer event yet. Both fallbacks
                    // exist for pre-event paint passes; in steady
                    // state every drag widget reads the live cursor.
                    self.interaction.current_content_point.or_else(|| {
                        if active {
                            self.interaction.drag_start
                        } else {
                            Some(Point::new(
                                rect.x() + rect.width() * 0.5,
                                rect.y() + rect.height() * 0.5,
                            ))
                        }
                    })
                } else {
                    None
                };
                let drag_delta_local = if active && matches!(sense, Sense::Drag) {
                    let cur = pointer_world.unwrap_or(Point::new(rect.x(), rect.y()));
                    let prev = self.last_drag_pos.insert(region_id.clone(), cur);
                    if let Some(p) = prev {
                        Point::new(cur.x - p.x, cur.y - p.y)
                    } else {
                        Point::new(0.0, 0.0)
                    }
                } else {
                    self.last_drag_pos.remove(&region_id);
                    Point::new(0.0, 0.0)
                };

                // Clicked-this-frame: CONSUME the stamp on read.
                // imgui-style: a click fires exactly once for the
                // first widget frame that observes it, then the
                // entry is dropped from the map. More robust than a
                // timestamp compare across frame boundaries — works
                // even when the canvas-kit's input dispatch and our
                // paint phase race (the stamp accumulates until the
                // next paint observes it, then it's gone). Also
                // self-pruning: clicks for widgets that disappeared
                // between frames stay in the map until something
                // with the same id re-renders, which is the
                // imgui-norm "if you don't ask for it this frame,
                // you don't get it" semantics.
                let clicked = matches!(sense, Sense::Click | Sense::Drag)
                    && clicks().lock().unwrap().remove(&region_id).is_some();

                response = build_response(
                    rect,
                    hovered,
                    active,
                    clicked,
                    pointer_world,
                    drag_delta_local,
                );
            }
        }

        let painter = PortalPainter {
            ctx: self.ctx,
            rect,
            time_s: self.time_s,
        };
        (painter, response)
    }

    /// Run `f` with the cursor laying out horizontally — children
    /// advance the cursor's x while sharing the same `cursor.y`.
    ///
    /// After `f` returns, the inner row is treated as a single
    /// "compound widget" whose width is how far the inner widgets
    /// advanced the x cursor and whose height is the row's tallest
    /// child. The parent cursor is then advanced along the parent's
    /// own primary axis: down by the row height when the parent is
    /// vertical, right by the row width when the parent is horizontal
    /// (nested horizontals stack along a single row). In the nested-
    /// horizontal case the inner row's height also feeds into the
    /// outer row's `horizontal_row_height` so the outer row sizes its
    /// own vertical extent correctly.
    pub fn horizontal<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        let saved_layout = self.layout;
        let saved_cursor = self.cursor;
        let saved_row_height = self.horizontal_row_height;
        self.layout = LayoutDirection::Horizontal;
        self.horizontal_row_height = 0.0;

        let result = f(self);

        // Capture the inner row's bounding box BEFORE restoring the
        // saved cursor. Width = how far the inner widgets advanced x;
        // height = the tallest child's height in this row.
        let inner_width = (self.cursor.x - saved_cursor.x).max(0.0);
        let inner_height = self.horizontal_row_height;

        // Restore parent context.
        self.layout = saved_layout;
        self.cursor = saved_cursor;
        self.horizontal_row_height = saved_row_height;

        // Advance the parent cursor in the parent's primary axis.
        // For a horizontal-in-vertical, that's down by row height.
        // For a horizontal-in-horizontal, the inner row is one tall
        // "block" inside the outer row — advance outer x by the inner
        // row's WIDTH (not height — the previous code added row
        // height to x, which gave wildly wrong layouts), and bubble
        // the inner row's height up so the outer row's tallest-child
        // tracking includes it.
        if inner_height > 0.0 || inner_width > 0.0 {
            match self.layout {
                LayoutDirection::Vertical => {
                    self.cursor.y += inner_height + self.style.spacing;
                }
                LayoutDirection::Horizontal => {
                    self.cursor.x += inner_width + self.style.spacing;
                    if inner_height > self.horizontal_row_height {
                        self.horizontal_row_height = inner_height;
                    }
                }
            }
        }

        result
    }
}

// ─────────────────────────────────────────────────────────────────────
// PortalManager — convenience map for hosts that own a portal per key
// ─────────────────────────────────────────────────────────────────────

/// `K → Portal` map plus the typical get-or-create / retain-live
/// plumbing every host writes anyway.
///
/// Hosts that own multiple portals (one per node in a node editor,
/// one per pane in a multi-pane layout, etc.) typically write the
/// same `HashMap<K, Portal>` + retain-by-live-set + lookup logic in
/// every implementation. This struct owns that pattern. Construct
/// once, call [`Self::get_or_make`] inside the paint loop, and call
/// [`Self::retain`] once per frame to drop portals for keys that no
/// longer exist.
///
/// The map is keyed by `K` (not by `PortalId`) so [`Self::retain`]
/// can compare against the host's live-key set without round-tripping
/// through the hash.
pub struct PortalManager<K: std::hash::Hash + Eq + Clone> {
    portals: std::collections::HashMap<K, Portal>,
}

impl<K: std::hash::Hash + Eq + Clone> Default for PortalManager<K> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: std::hash::Hash + Eq + Clone> PortalManager<K> {
    /// Empty manager. No allocation until the first
    /// [`Self::get_or_make`] call.
    pub fn new() -> Self {
        Self {
            portals: std::collections::HashMap::new(),
        }
    }

    /// Look up the portal bound to `key`, constructing a fresh one
    /// (with `Portal::new(&key)`) if none exists. Returned reference
    /// is valid for the lifetime of `self`; the caller threads it
    /// through [`Portal::frame`] and discards the borrow.
    pub fn get_or_make(&mut self, key: K) -> &mut Portal {
        self.portals
            .entry(key.clone())
            .or_insert_with(|| Portal::new(&key))
    }

    /// Drop portals whose key is no longer "live" per `keep`. Hosts
    /// pass a predicate that consults their own live-set (e.g.
    /// `|k| graph.nodes.contains_key(k)`) and the manager drops the
    /// portals that fall out. Each dropped portal's `Drop` cleans up
    /// its scheduler tick callback and signal subscriptions.
    pub fn retain<F: FnMut(&K) -> bool>(&mut self, mut keep: F) {
        self.portals.retain(|k, _| keep(k));
    }

    /// Last-frame consumed-height for the portal under `key`, or
    /// `0.0` if no portal exists yet. The host typically feeds this
    /// back as next-frame bounds so the slot grows with content.
    pub fn consumed_height(&self, key: &K) -> f32 {
        self.portals.get(key).map_or(0.0, Portal::consumed_height)
    }

    /// Number of live portals tracked.
    pub fn len(&self) -> usize {
        self.portals.len()
    }

    /// True when no portals are tracked.
    pub fn is_empty(&self) -> bool {
        self.portals.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portal_manager_get_or_make_constructs_once_per_key() {
        let mut mgr = PortalManager::<&'static str>::new();
        assert!(mgr.is_empty());
        let id_a = mgr.get_or_make("a").id();
        let id_a_again = mgr.get_or_make("a").id();
        assert_eq!(id_a, id_a_again, "same key returns same portal id");
        let id_b = mgr.get_or_make("b").id();
        assert_ne!(id_a, id_b, "different keys produce distinct portals");
        assert_eq!(mgr.len(), 2);
    }

    #[test]
    fn portal_manager_retain_drops_missing_keys() {
        let mut mgr = PortalManager::<i32>::new();
        let _ = mgr.get_or_make(1);
        let _ = mgr.get_or_make(2);
        let _ = mgr.get_or_make(3);
        assert_eq!(mgr.len(), 3);
        let live = [1_i32, 3_i32];
        mgr.retain(|k| live.contains(k));
        assert_eq!(mgr.len(), 2);
        assert!(mgr.consumed_height(&1) >= 0.0); // 1 still present
        assert_eq!(mgr.consumed_height(&2), 0.0); // dropped → default
    }
}
