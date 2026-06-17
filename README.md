# blinc_portal_ui

Immediate-mode widget toolkit for canvas closures. Lets you render
real interactive widgets and free-form painting INSIDE a
[`blinc_canvas_kit::CanvasKit`] element — typically a node body in
[`blinc_node_editor`], a tool palette in a custom canvas-kit app, or
any other region of a canvas closure that wants to host an inline
mini UI.

## Why imgui (and not retained mode)

Canvas closures already run every frame — the `had_canvas_painted`
cache-bust gate means the canvas's interior isn't cached frame-to-
frame. That makes imgui's "re-emit every frame" rhythm a perfect
fit: no offscreen texture to manage, no retained tree to diff
against, no second event router to keep in sync with the host. The
portal's widget calls are pure draw + pure hit-region registration,
and the kit you're already using does both.

## State model

Two categories, kept distinct:

1. **Widget-local interaction state** — slider drag offset, text
   input cursor, hover ease t. Lives in `PortalStorage`, a
   `HashMap<WidgetId, Box<dyn Any>>` the portal owns. `WidgetId` is
   a hash chain through the portal id + the id stack at the call
   site + an optional caller key, so issuing the same widget
   sequence frame-to-frame produces stable ids. Entries that no
   widget touched this frame are dropped at frame end.

2. **Semantic state** — the values widgets edit. Use Blinc
   [`Signal<T>`](blinc_core::reactive::Signal). Every built-in
   widget exposes a `_signal` form that reads the signal at frame
   time, auto-subscribes the portal to it via the reactive read
   tracker, and writes back on user edits. Signal mutations from
   ANYWHERE (FSM transitions, animations, async tasks, retained-
   mode `cn::input` bound to the same signal) flip the portal's
   dirty flag and the host's next composite repaints.

## The frame contract

Inside your canvas closure:

```rust
// Once per portal-per-frame:
portal.frame(ctx, &kit, body_rect, &PortalStyle::from_active_theme(), &host_bridge,
    |ui| {
        ui.label("Threshold");
        ui.slider_signal(&threshold, 0.0..1.0);
        if ui.button(if running.get() { "Stop" } else { "Run" }).clicked {
            running.toggle();
        }
        ui.label_signal(&display_pct);

        // Free-form painting — sparklines, sketches, custom viz.
        let (mut p, _) = ui.allocate_painter((180.0, 32.0), Sense::Hover);
        draw_sparkline(&mut p, &history.get());
    });
```

Before any portal renders, install the click-recorder hook ONCE on
the kit so widget `Response::clicked` works:

```rust
let mut kit = CanvasKit::new("graph");
blinc_portal_ui::ui::install_click_hook(&mut kit);
```

## Overlay escape

Popovers / dropdowns / tooltips opened from a portal widget should
escape the portal and live in the host's overlay manager — they
paint in screen space above everything, dismiss via the host's
click-outside rules, and share z-order with overlays opened
elsewhere in the app. The portal contributes the anchor coordinate
(transformed from widget-local → canvas-content → screen via
`HostBridge::canvas_to_screen`) and otherwise hands off completely.

```rust
if ui.button("Open").clicked {
    let anchor = host_bridge.rect_to_screen(response.rect);
    OverlayManager::show(my_popover_content(), anchor);
}
```

## Free-form painting

Every widget is implemented in terms of `allocate_painter`:

```rust
let (mut p, resp) = ui.allocate_painter((180.0, 120.0), Sense::Drag);
if resp.pressed {
    if let Some(local) = resp.pointer_local {
        state.add_point(local);
    }
}
for stroke in &state.strokes {
    p.stroke_path(stroke, &Stroke::new(2.0), Brush::Solid(theme.text_primary()));
}
```

Or take the whole region: `ui.allocate_painter(ui.available_size(), Sense::Drag)`.

## Built-in widgets

`label`, `label_signal`, `button`, `slider`, `slider_signal`,
`switch`, `switch_signal`, `text_input`, `text_input_signal`, plus
`spacing`, `horizontal`, `push_id`.

Custom widgets are a one-liner over `allocate_painter`:

```rust
fn star_button(ui: &mut PortalUi, rating: u8) -> u8 {
    let (mut p, resp) = ui.allocate_painter((100.0, 24.0), Sense::Click);
    // ... paint stars, return updated rating
    rating
}
```

## Things that aren't built yet

- Text input is read-only display in v0.1 (canvas-kit doesn't have
  a typed-character dispatch path yet). Click-to-open-overlay is
  the workaround.
- No flexbox; the layout combinators are vertical / horizontal
  stack + indent. Node bodies don't need more.
- No focus traversal (Tab key). When canvas-kit grows keyboard
  routing, focus handling lands here too.

## Crate layout

- `core` — `PortalId`, `WidgetId`, `Response`, `Sense`,
  `PortalStorage`, `PortalStyle`, `HostBridge`.
- `painter` — `PortalPainter`.
- `signal` — `PortalSubscriptions` + the global notifier installer.
- `ui` — `Portal`, `PortalUi`, `install_click_hook`.
- `widget` — built-in widget impls (all live as inherent methods on
  `PortalUi`).
