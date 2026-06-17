# blinc_portal_ui

Immediate-mode widgets for canvas closures. Renders interactive
widgets and free-form paint inside a
[`blinc_canvas_kit::CanvasKit`] element: node bodies in
[`blinc_node_editor`], tool palettes, in-canvas mini UIs.

## Imgui, not retained mode

Canvas closures re-run every frame. `had_canvas_painted` already
cache-busts the canvas interior, so an imgui rhythm (re-emit every
frame) drops in cleanly: no offscreen texture, no diffable tree, no
second event router. Widget calls are `draw + hit_rect`; the kit
provides both.

## State

Two layers:

1. **Widget-local** (slider drag offset, hover ease, cursor) lives
   in `PortalStorage`, a `HashMap<WidgetId, Box<dyn Any>>`.
   `WidgetId` hashes the portal id + the id stack at the call site
   + an optional caller key, so re-emitting the same call sequence
   produces stable ids. Untouched entries drop at frame end.
2. **Semantic** (the values widgets edit) lives in
   [`Signal<T>`](blinc_core::reactive::Signal). Every built-in
   widget has a `_signal` form that reads + auto-subscribes via
   the reactive tracker and writes on edit. Mutations from outside
   the portal (FSM, animation, retained-mode `cn::input` on the
   same signal) flip the portal dirty bit and the next composite
   repaints.

## Frame contract

Inside the canvas closure:

```rust
portal.frame(ctx, &kit, body_rect, &PortalStyle::from_active_theme(), &host_bridge,
    |ui| {
        ui.label("Threshold");
        ui.slider_signal(&threshold, 0.0..1.0);
        if ui.button(if running.get() { "Stop" } else { "Run" }).clicked {
            running.toggle();
        }
        ui.label_signal(&display_pct);

        let (mut p, _) = ui.allocate_painter((180.0, 32.0), Sense::Hover);
        draw_sparkline(&mut p, &history.get());
    });
```

Install the click-recorder hook once per kit so `Response::clicked`
fires:

```rust
let mut kit = CanvasKit::new("graph");
blinc_portal_ui::ui::install_click_hook(&mut kit);
```

## Overlay escape

Popovers, dropdowns, and tooltips opened from a widget should live
in the host overlay manager: they paint in screen space, dismiss
via the host's click-outside rules, and share z-order with other
overlays. The portal hands off an anchor coordinate (widget-local
to canvas-content to screen, via `HostBridge::canvas_to_screen`)
and does nothing else.

```rust
if ui.button("Open").clicked {
    let anchor = host_bridge.rect_to_screen(response.rect);
    OverlayManager::show(my_popover_content(), anchor);
}
```

## Free-form paint

Every widget builds on `allocate_painter`:

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

Use `ui.allocate_painter(ui.available_size(), Sense::Drag)` for the
whole region.

## Built-in widgets

`label`, `label_signal`, `button`, `slider`, `slider_signal`,
`switch`, `switch_signal`, `text_input`, `text_input_signal`,
`spacing`, `horizontal`, `push_id`.

Custom widgets are one-liners over `allocate_painter`:

```rust
fn star_button(ui: &mut PortalUi, rating: u8) -> u8 {
    let (mut p, resp) = ui.allocate_painter((100.0, 24.0), Sense::Click);
    // paint stars, return updated rating
    rating
}
```

## Not yet built

- Text input renders as read-only display until canvas-kit
  surfaces a typed-character dispatch path. Click-to-open-overlay
  is the workaround.
- No flexbox. Layout combinators are vertical / horizontal stack +
  indent.
- No focus traversal (Tab). Lands when canvas-kit grows keyboard
  routing.

## Modules

- `core`: `PortalId`, `WidgetId`, `Response`, `Sense`,
  `PortalStorage`, `PortalStyle`, `HostBridge`.
- `painter`: `PortalPainter`.
- `signal`: `PortalSubscriptions` + global notifier installer.
- `ui`: `Portal`, `PortalUi`, `install_click_hook`.
- `widget`: built-in widget impls (inherent methods on `PortalUi`).
