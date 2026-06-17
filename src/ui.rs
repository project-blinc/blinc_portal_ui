//! `Portal` (persistent host-owned runtime) and `PortalUi`
//! (per-frame borrow passed to the closure). See `README.md`.

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
        self.subs.dirty_flag().store(true, std::sync::atomic::Ordering::Release);
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
    /// paint when true.
    #[allow(clippy::too_many_arguments)] // 8 args are intrinsic to the frame contract.
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
        // Flag dirty when the closure's footprint changes so the
        // host's next frame re-grows the slot to fit — even when
        // nothing else (signal / animation) would otherwise have
        // requested a repaint.
        if (consumed - prev).abs() > 0.5 {
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
    #[allow(dead_code)]
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

    /// Remaining space inside the portal's bounds, measured from the
    /// cursor in the layout's primary direction.
    pub fn available_size(&self) -> (f32, f32) {
        let w = (self.bounds.x() + self.bounds.width() - self.cursor.x - self.style.spacing).max(0.0);
        let h = (self.bounds.y() + self.bounds.height() - self.cursor.y - self.style.spacing).max(0.0);
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
    pub fn allocate_painter(&mut self, size: (f32, f32), sense: Sense) -> (PortalPainter<'_>, Response) {
        self.allocate_painter_with_key(size, sense, None)
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
                let pointer_world = if hovered || active {
                    // Approximate pointer position: derive from
                    // active drag_start when pressed, else hovered
                    // region centre. Better: kit could expose a
                    // `current_content_point`, but absent that this is
                    // a reasonable read for hover-driven widgets.
                    if active {
                        self.interaction.drag_start
                    } else {
                        None
                    }
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
    /// advance the cursor's x. After `f` returns, the parent cursor's
    /// y advances by the row's tallest child + spacing.
    pub fn horizontal<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        let saved_layout = self.layout;
        let saved_cursor = self.cursor;
        let saved_row_height = self.horizontal_row_height;
        self.layout = LayoutDirection::Horizontal;
        self.horizontal_row_height = 0.0;
        let result = f(self);
        let row_height = self.horizontal_row_height;
        // Restore parent context; advance parent cursor vertically by
        // the row height plus a spacing gap.
        self.layout = saved_layout;
        self.cursor = saved_cursor;
        self.horizontal_row_height = saved_row_height;
        if row_height > 0.0 {
            match self.layout {
                LayoutDirection::Vertical => self.cursor.y += row_height + self.style.spacing,
                LayoutDirection::Horizontal => self.cursor.x += row_height + self.style.spacing,
            }
        }
        result
    }
}
