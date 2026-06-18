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
        if ui.button(if running.get() { "Stop" } else { "Run" }).clicked() {
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
if ui.button("Open").clicked() {
    let anchor = host_bridge.rect_to_screen(response.rect);
    OverlayManager::show(my_popover_content(), anchor);
}
```

## Button variants

`button` returns a deferred `ButtonBuilder`. Fluent variant setters
match `cn::ButtonVariant` one-to-one — `Primary`, `Secondary`,
`Destructive`, `Outline`, `Ghost`, `Link` — and the cross-variant
`.disabled(bool)` toggle resolves to a single shared disabled
palette regardless of variant (`cn::button` parity). The terminal
call is `.show() -> Response` or one of the boolean shortcuts
(`.clicked()`, `.hovered()`, `.pressed()`, `.changed()`).

```rust
if ui.button("Reset").clicked() { reset(); }                 // default Ghost
if ui.button("Delete").destructive().clicked() { delete(); }
ui.button("Save").primary().disabled(!is_valid).show();
let resp = ui.button(label).variant(cfg.variant).show();      // programmatic
```

**Default variant is `Ghost`, not `Primary`** — deliberate
divergence from `cn::button`. Portal buttons live inside node
content slots where a Primary fill would dominate the canvas;
Ghost is the right inline default. Use `.primary()` explicitly to
opt into the saturated treatment.

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

`label`, `label_signal`, `button` (+ `ButtonVariant`: Primary /
Secondary / Destructive / Outline / Ghost / Link, default Ghost),
`switch`, `slider`, `numeric_input`, `text_input`, `select_trigger`,
`select`, `select_signal`, `spacing`, `horizontal`, `push_id`.

`numeric_input` is dual-mode: drag-to-scrub on the chip OR
click-to-edit inline. Supports `.min(f32)` / `.max(f32)` /
`.range(Range<f32>)` / `.step(f32)` / `.integer()` / `.precision(u8)`
/ `.unit(...)` / `.drag_sensitivity(...)` / `.disabled(b)` plus the
shared `ShadowMix` shortcuts. Arrow Up/Down nudge by step while
focused; Enter commits the edit, Esc cancels.

The value-bearing widgets (`switch`, `slider`, `text_input`) accept
either `&mut value` or `&Signal<value>` via the [`PortalValue`]
trait — one entry-method works for both. Each returns a Builder
(`SwitchBuilder` / `SliderBuilder` / `TextInputBuilder` /
`SelectBuilder`) that supports `.disabled(b)`, palette overrides,
and the [`ShadowMix`] shadow surface (`.shadow_sm/.shadow_md/.shadow_lg/
.shadow_xl/.shadow_2xl/.shadow_inner/.shadow_default/.shadow_none`
plus `.shadow(ShadowToken)`). Terminate with `.show()` for the
full [`Response`] or `.clicked()` / `.changed()` for booleans.

The pre-cascade `*_signal` free methods stay as `#[deprecated]`
shims so existing call sites compile; new code should use
`ui.switch(&sig).show()` etc.

`select_trigger` / `select` / `select_signal` paint a dropdown
trigger only. Opening the menu is the host's job — the trigger
returns a [`Response`] whose `rect` field can be transformed via
`ui.host().rect_to_screen(...)` and anchored against any overlay
the host provides (`blinc_cn::context_menu` is the standard for
Blinc apps). See `node_editor_demo`'s Sink node for the canonical
recipe.

Custom widgets are one-liners over `allocate_painter`:

```rust
fn star_button(ui: &mut PortalUi, rating: u8) -> u8 {
    let (mut p, resp) = ui.allocate_painter((100.0, 24.0), Sense::Click);
    // paint stars, return updated rating
    rating
}
```

## Inline text editing

`text_input` / `text_input_signal` accept typed characters directly
when focused: click to focus, type to edit, Esc / click elsewhere
to release. Backspace / Delete / Home / End / arrow keys all
behave as expected; selection / clipboard / IME are not yet wired.

Hosts opt in by wrapping the outer canvas div with
[`install_kbd_hook`]:

```rust
let outer = ldiv().w_full().h_full().child(canvas);
let outer = blinc_portal_ui::ui::install_kbd_hook(outer);
```

The hook attaches additive `on_key_down` + `on_text_input` handlers
that push events into process-global buffers; `Portal::frame`
drains them at the start of every paint and routes them to the
focused widget. Global focus state is exposed via
[`current_focused_region`] / [`set_focused_region`] for hosts that
want to coordinate focus with their own retained-mode widgets.

## Not yet built

- No flexbox. Layout combinators are vertical / horizontal stack +
  indent.
- No Tab traversal between portal_ui widgets — focus moves only on
  pointer click.
- Selection (drag-to-select), clipboard (Cmd+C / V / X), and IME
  composition for `text_input`.

## Modules

- `core`: `PortalId`, `WidgetId`, `Response`, `Sense`,
  `PortalStorage`, `PortalStyle`, `HostBridge`.
- `painter`: `PortalPainter`.
- `signal`: `PortalSubscriptions` + global notifier installer.
- `ui`: `Portal`, `PortalUi`, `install_click_hook`.
- `widget`: built-in widget impls (inherent methods on `PortalUi`).
