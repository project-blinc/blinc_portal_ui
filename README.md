# blinc_portal_ui

Immediate-mode widgets for canvas closures. Renders interactive
widgets and free-form paint inside a
[`blinc_canvas_kit::CanvasKit`] element: node bodies in
[`blinc_node_editor`], tool palettes, in-canvas mini UIs.

```rust
let outcome = portal
    .begin(ctx, &kit, body_rect)
    .style(&PortalStyle::from_active_theme())
    .host(&host_bridge)
    .run(|ui| {
        ui.label("Threshold");
        ui.slider(&threshold).range(0.0..1.0).show();
        let resp = ui.chart(&samples).area().pip(true).show();
        if resp.pip_clicked {
            open_chart_popover(ui.host().rect_to_screen(resp.rect));
        }
    });
```

## Documentation

- **[Widget reference](docs/widgets.md)** — every widget with a
  usage example. Covers input widgets (label, button, switch,
  slider, numeric_input, text_input, color_picker, select) and
  the square-aspect display-portal family (chart, pie_chart,
  radar_chart, noise, texture, sdf_shape).
- **[Host integration](docs/integration.md)** — module map, the
  frame contract, click + keyboard hook installation, overlay
  anchoring, fit-content sizing via
  `FrameOutcome::natural_width/height`, PiP popovers,
  [theme integration](docs/integration.md#theme-integration)
  (per-frame `PortalStyle::from_active_theme` + per-portal
  overrides + ShadowToken resolution), signal lifecycle, custom
  widgets.

## Imgui, not retained mode

Canvas closures re-run every frame. `had_canvas_painted` already
cache-busts the canvas interior, so an imgui rhythm (re-emit every
frame) drops in cleanly: no offscreen texture, no diffable tree,
no second event router. Widget calls are `draw + hit_rect`; the
kit provides both.

## State

Two layers:

1. **Widget-local** (slider drag offset, hover ease, cursor) lives
   in `PortalStorage`, a `HashMap<WidgetId, Box<dyn Any>>`.
   `WidgetId` hashes the portal id + the id stack at the call site
   + an optional caller key, so re-emitting the same call sequence
   produces stable ids. Untouched entries drop at frame end.
2. **Semantic** (the values widgets edit) lives in
   [`Signal<T>`](blinc_core::reactive::Signal). Every built-in
   value-bearing widget reads + auto-subscribes via the reactive
   tracker and writes on edit. Mutations from outside the portal
   (FSM, animation, retained-mode `cn::input` on the same signal)
   flip the portal dirty bit and the next composite repaints.

## What works, what doesn't

Built-in widget catalog: `label`, `label_signal`, `button` (+ six
variants), `switch`, `slider`, `numeric_input`, `text_input`,
`textarea`, `file_picker`, `color_picker`,
`select_trigger` / `select` / `select_signal`,
`chart` (line / bar / area), `pie_chart`, `radar_chart`,
`wave_graph` (oscilloscope / audio waveform), `noise` (Perlin /
Worley / Voronoi), `texture` (ImageId + inline RGBA),
`sdf_shape` (2D + 3D, eight variants). Layout primitives:
`horizontal`, `spacing`, `push_id`, `allocate_painter`,
`request_animation`. Custom widgets are one-liners over
`allocate_painter`.

Gaps:
- No flexbox. Layout combinators are vertical / horizontal stack +
  indent.
- No Tab traversal between widgets — focus moves only on pointer
  click.
- Selection (drag-to-select), clipboard (Cmd+C / V / X), and IME
  composition for `text_input`.

## Modules

See [docs/integration.md](docs/integration.md) for the per-module
breakdown and host-side wiring patterns.
