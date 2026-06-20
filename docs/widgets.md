# Widget reference

Every method shown is an inherent method on `&mut PortalUi` and may
be called from inside the closure passed to `Portal::begin(...).run(|ui| ...)`.
Value-bearing widgets accept either `&mut value` or `&Signal<value>`
via the `PortalValue` trait — both forms are shown for the first
widget, then `&Signal` is used throughout for brevity.

Every builder terminates with `.show() -> Response`; the most-used
boolean shortcuts (`.clicked()`, `.changed()`, `.hovered()`,
`.pressed()`) chain directly off the builder so trivial reads stay
one-liners.

## label / label_signal

Static text emitter. Layout-aware: paints at the row baseline when
called inside `ui.horizontal(...)`, at the line baseline otherwise.

```rust
ui.label("Threshold");
ui.label(&format!("count = {}", count));
```

`label_signal` auto-subscribes to a signal and re-paints when it
fires:

```rust
let pct: Signal<f32> = ctx.use_state(|| 0.5);
ui.label_signal(&pct);            // renders "0.5", "0.51", ...
```

## button

Default variant is `Ghost` (low-chroma, fits inline). All
`cn::ButtonVariant` values are available as fluent setters and via
`.variant(ButtonVariant::...)` for programmatic selection.

```rust
if ui.button("Reset").clicked() { reset(); }                 // Ghost
if ui.button("Save").primary().shadow_md().clicked() { save(); }
if ui.button("Delete").destructive().disabled(!can_delete).clicked() {
    delete();
}

// Icon prefix — single glyph, plain string (Tabler / Lucide work).
if ui.button("Edit").icon("✎").outline().clicked() {
    open_editor();
}
```

## switch

Toggle on/off. Bound to `bool`.

```rust
let running = ctx.use_state(|| false);
ui.switch(&running).show();           // signal binding
ui.switch(&mut local_bool).show();    // direct binding
```

## slider

Horizontal value scrubber bound to `f32`. Range required; step is
optional.

```rust
let threshold = ctx.use_state(|| 0.5_f32);
ui.slider(&threshold).range(0.0..1.0).step(0.01).show();
```

## numeric_input

Dual-mode chip: drag-to-scrub OR click-to-edit inline. A faint
accent gauge fills the chip proportionally to the bound value when
a finite range is declared. Arrow Up/Down nudge by `step` while
focused; Enter commits, Esc cancels.

```rust
let n = ctx.use_state(|| 3.0_f32);
ui.numeric_input(&n)
    .integer()                       // round on commit + display as Int
    .range(0.0..8.0)
    .step(1.0)
    .unit(" px")
    .show();
```

## text_input

Single-line text editor bound to `String`. Click to focus, type to
edit, Esc / click elsewhere to release. Backspace, Delete, Home,
End, arrow keys all behave. **Selection**: drag to select, shift+click
to extend, shift+arrow / shift+Home/End to extend by keyboard; typing
or Backspace/Delete replaces the selection. Hosts must install
`install_kbd_hook` on the canvas div to feed key events into the portal.
Clipboard (Cmd+C/V/X) and IME composition are not yet wired.

```rust
let label = ctx.use_state(|| String::new());
ui.text_input(&label).placeholder("Label…").show();
```

## textarea

Multi-line text editor bound to `String`. Like `text_input` but
Enter inserts a newline (doesn't blur), Up/Down move the caret
between lines preserving the horizontal position, and Home/End act
on the current line. `.rows(n)` sets the visible height (default 4);
content beyond `rows` scrolls with the caret. Drag-to-select spans
lines (shift+click and shift+Up/Down extend); selection-replace on
edit. Same `install_kbd_hook` requirement as `text_input`.

```rust
let notes = ctx.use_state(|| String::new());
ui.textarea(&notes).rows(6).placeholder("Notes…").show();
```

## file_picker

Trigger chip bound to a path `String` — shows a folder glyph + the
path's base name (or a placeholder). Like `color_picker` /
`select`, the widget only signals intent; the host opens the file
dialog (native or an overlay browser) on `resp.clicked` and writes
the chosen path back.

```rust
let path = ctx.use_state(|| String::new());
let resp = ui.file_picker(&path).placeholder("Choose model…").show();
if resp.clicked {
    let anchor = ui.host().rect_to_screen(resp.rect);
    open_file_dialog(anchor, path.clone()); // host-owned
}
```

## color_picker

Trigger chip for a colour value (RGBA hex string e.g. `"#ff8800ff"`).
Paints a square swatch + the hex string. Clicking does NOT open a
picker on its own — the host anchors a colour-wheel overlay against
the response rect.

```rust
let fill = ctx.use_state(|| String::from("#ff8800ff"));
let resp = ui.color_picker(&fill).show();
if resp.clicked {
    let anchor = ui.host().rect_to_screen(resp.rect);
    open_color_wheel_popover(anchor, fill.clone());
}
```

The bundled `color_wheel_panel(value_signal)` returns a content
closure suitable for `blinc_cn::popover` or any other host overlay.

## select_trigger / select / select_signal

Dropdown trigger. The trigger paints; opening the menu is the
host's job. Same recipe as `color_picker`.

```rust
const FORMATS: &[(&str, &str)] = &[("text", "Plain"), ("json", "JSON")];
let fmt = ctx.use_state(|| String::from("text"));
let resp = ui.select_signal(&fmt, FORMATS).show();
if resp.clicked {
    let anchor = ui.host().rect_to_screen(resp.rect);
    let mut menu = blinc_cn::context_menu()
        .at(anchor.x(), anchor.y() + anchor.height() + 4.0);
    for (value, label) in FORMATS {
        let s = fmt;
        let v = value.to_string();
        menu = menu.item(*label, move || s.set(v.clone()));
    }
    let _ = menu.show();
}
```

## chart

Line / bar / area chart over `&Signal<Vec<f32>>`. Square aspect by
default. Optional tooltip on hover, optional baseline + latest
marker, optional PiP corner button for opening an expanded view.

```rust
let samples = ctx.use_state(|| vec![0.0; 64]);

// Area chart with baseline + latest dot + PiP button + tooltip
let resp = ui.chart(&samples)
    .area()
    .show_baseline(true)
    .show_latest(true)
    .tooltip(true)
    .tooltip_precision(2)
    .pip(true)
    .show();

if resp.pip_clicked {
    let anchor = ui.host().rect_to_screen(resp.rect);
    open_expanded_chart(anchor, samples.clone());
}

// Bar histogram
ui.chart(&buckets).bar().y_range(0.0..1.0).bar_gap(2.0).show();

// Plain line
ui.chart(&series).show();
```

## pie_chart

Slice breakdown over `&Signal<Vec<f32>>`. Weights normalise
automatically; `.donut()` gives a doughnut hole. Centered by
default.

```rust
let weights = ctx.use_state(|| vec![3.0, 2.0, 5.0]);
let resp = ui.pie_chart(&weights)
    .donut()
    .diameter(96.0)
    .tooltip(true)
    .pip(true)
    .show();
```

## radar_chart

Spider/radar plot over an N-axis vector. `.labels(...)` provides
axis names; `.y_range(...)` clamps each axis's [min, max].

```rust
let axes = ctx.use_state(|| vec![0.7, 0.4, 0.9, 0.3, 0.6, 0.5]);
ui.radar_chart(&axes)
    .labels(vec![
        "cpu".into(), "mem".into(), "io".into(),
        "net".into(), "err".into(), "lat".into(),
    ])
    .y_range(0.0..1.0)
    .show_grid(true)
    .show_axes(true)
    .fill_area(true)
    .show();
```

## wave_graph

Oscilloscope-style signed-amplitude plot over `&Signal<Vec<f32>>`.
Same data shape as `chart`, but centered on zero with optional
horizontal grid divisions and a centerline. Square aspect by
default. Use for audio waveforms, LFO traces, signed control
signals.

```rust
let buffer = ctx.use_state(|| vec![0.0_f32; 256]);

// Default: stroke style, ±1 range, grid + centerline visible.
ui.wave_graph(&buffer).show();

// Filled audio-editor style with tighter range + tooltip + PiP.
ui.wave_graph(&buffer)
    .filled()
    .y_range(-1.0..1.0)
    .grid_divisions(3)
    .line_width(1.0)
    .tooltip(true)
    .pip(true)
    .show();

// Mirrored — abs(sample) reflected above + below the centerline.
ui.wave_graph(&buffer).mirrored().show();
```

Builder methods: `.stroke()` / `.filled()` / `.mirrored()`
(or `.style(WaveStyle::...)`), `.y_range(Range<f32>)`,
`.line_width(f32)`, `.grid_divisions(u32)`,
`.show_centerline(bool)`, `.show_grid(bool)`, `.stroke_color(c)` /
`.fill_color(c)` / `.grid_color(c)` / `.centerline_color(c)`,
`.background(c)`, `.tooltip(bool)`, `.tooltip_precision(u8)`,
`.tooltip_unit("V")`, `.pip(bool)`, `.disabled(bool)`.

## noise

Procedural pattern preview — Perlin fbm, Worley cells, or Voronoi
cells. Square by default. Per-seed deterministic.

```rust
ui.noise()
    .variant(NoiseVariant::Perlin)   // or Worley / Voronoi
    .seed(42)
    .scale(28.0)
    .octaves(3)
    .pip(true)
    .show();
```

## texture

Image preview. Accepts either an `ImageId` (host-loaded asset) or
an inline `&[u8]` RGBA buffer. Fit modes mirror CSS object-fit:
`.cover()` / `.contain()` / `.scale_down()` / default-stretch.

```rust
// Host-loaded asset
ui.texture(TextureSource::Id(image_id)).contain().show();

// Inline RGBA buffer
let buf: Vec<u8> = bake_pattern(128, 128);
ui.texture(TextureSource::Rgba {
        data: &buf,
        width: 128,
        height: 128,
    })
    .cover()
    .pip(true)
    .show();
```

## sdf_shape

Per-element 2D / 3D SDF preview. 2D variants paint via the
standard SDF pipeline; 3D variants raymarch with Blinn-Phong
lighting and a scissor clip on the widget border. Square by
default.

```rust
// 2D
ui.sdf_shape(SdfShape::RoundedBox2D).show();
ui.sdf_shape(SdfShape::Circle2D).stroke(theme.text_primary, 2.0).show();

// 3D — rotate to taste; the default light direction is overhead.
ui.sdf_shape(SdfShape::Sphere3D).show();
ui.sdf_shape(SdfShape::Box3D)
    .rotate_x(0.35)
    .rotate_y(0.7)
    .depth(1.0)                      // depth × inscribed-side, scale 1.0
    .light([-0.3, -0.7, 0.6], 1.0)
    .show();
```

Available shapes: `Circle2D`, `RoundedBox2D`, `Capsule2D`, `Box3D`,
`Sphere3D`, `Cylinder3D`, `Torus3D`, `Capsule3D`.

## Layout primitives

`ui.horizontal(|ui| ...)` lays out subsequent widgets left-to-
right inside the closure; nested `horizontal` blocks nest.

```rust
ui.horizontal(|ui| {
    ui.label("running");
    ui.switch(&running).show();
});
```

`ui.spacing(N)` inserts N pixels of empty advance.
`ui.push_id(key, |ui| ...)` derives a sub-id stack so duplicate
widgets emitted from a loop have stable ids:

```rust
for (i, item) in items.iter().enumerate() {
    ui.push_id(i, |ui| {
        ui.text_input(&item.label).show();
    });
}
```

## allocate_painter

The escape hatch every built-in widget builds on. Returns
`(PortalPainter, Response)` for a region of the portal; the
painter wraps the underlying `DrawContext` with widget-friendly
primitives (`fill_rect`, `stroke_path`, `fill_circle`,
`draw_text`, `draw_rgba_pixels`). Response carries pointer state
+ hit-test results.

```rust
let (mut p, resp) = ui.allocate_painter((180.0, 32.0), Sense::Drag);
if let Some(local) = resp.pointer_local {
    if resp.pressed {
        state.add_point(local);
    }
}
for stroke in &state.strokes {
    p.stroke_path(stroke, &Stroke::new(2.0), Brush::Solid(theme.text_primary));
}
```

Use `ui.allocate_painter(ui.available_size(), Sense::Drag)` for
the whole remaining region.

## Shared builder methods

Every value-bearing builder supports:

- `.disabled(bool)` — paint at the shared disabled palette and
  ignore input.
- The `ShadowMix` shortcuts: `.shadow_sm()`, `.shadow_md()`,
  `.shadow_lg()`, `.shadow_xl()`, `.shadow_2xl()`,
  `.shadow_inner()`, `.shadow_default()`, `.shadow_none()`, plus
  the explicit `.shadow(ShadowToken)`.
- `.show() -> Response` — the universal terminal. Direct boolean
  shortcuts (`.clicked()`, `.changed()`, `.hovered()`,
  `.pressed()`) chain off the builder where they make sense.

Display portals additionally support `.pip(bool)` to enable the
PiP corner button — the response's `pip_clicked` field reports
the click.
