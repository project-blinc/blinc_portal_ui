# Host integration

How to wire portal-UI into a host's canvas + overlay + event loop.

## Module map

- **`core`** — the type vocabulary: `PortalId`, `WidgetId`,
  `Response` (including `pip_clicked`), `Sense`, `PortalStorage`,
  `PortalStyle`, `HostBridge`, `PortalValue`, `ShadowMix`,
  `ShadowToken`, `ButtonVariant`.
- **`painter`** — `PortalPainter`, the per-widget paint surface
  built on top of a `DrawContext`. Custom widgets use this
  directly via `ui.allocate_painter(...)`.
- **`signal`** — `PortalSubscriptions` + global notifier installer.
  Hooks Blinc reactive signal writes to "dirty this portal" so the
  next frame repaints.
- **`ui`** — runtime types: `Portal`, `PortalManager`, `PortalUi`,
  `PortalFrame`, `FrameOutcome`, `LayoutDirection`, plus
  `install_click_hook` and `install_kbd_hook`.
- **`widget`** — built-in widget impls (inherent methods on
  `PortalUi`) and the public Builder types
  (`ButtonBuilder`, `SliderBuilder`, `NumericInputBuilder`,
  `TextInputBuilder`, `SwitchBuilder`, `SelectBuilder`,
  `ColorPickerBuilder`, `ChartsBuilder`, `PieChartBuilder`,
  `RadarChartBuilder`, `WaveGraphBuilder`, `NoiseBuilder`,
  `TextureBuilder`, `SdfShapeBuilder`) plus supporting enums
  (`ChartVariant`, `ChartDecimation`, `WaveStyle`, `NoiseVariant`,
  `TextureSource`, `TextureFit`, `SdfShape`).
- **`color_wheel`** — `color_wheel_panel(value_signal)` returns a
  content closure suitable for `blinc_cn::popover` or any other
  host overlay. Anchored against a `color_picker` chip's
  response rect.

## One-time setup

Install the click + keyboard hooks once per `CanvasKit`. The click
hook records canvas-content-space click stamps so `Response::clicked`
fires the same frame; the keyboard hook attaches additive
`on_key_down` + `on_text_input` handlers to the outer canvas div so
focused widgets see typing events.

```rust
let mut kit = CanvasKit::new("graph");
blinc_portal_ui::ui::install_click_hook(&mut kit);

let outer = ldiv().w_full().h_full().child(canvas);
let outer = blinc_portal_ui::ui::install_kbd_hook(outer);
```

## Frame contract

Hosts keep one `Portal` per region (typically one per node or one
per panel). Each frame, inside the host's canvas closure:

```rust
let outcome = portal
    .begin(ctx, &kit, body_rect)
    .style(&PortalStyle::from_active_theme())
    .host(&host_bridge)
    .clip_radius(8.0)                // 0.0 for axis-aligned clip
    .run(|ui| {
        ui.label("Threshold");
        ui.slider(&threshold).range(0.0..1.0).show();
        // ... more widgets ...
    });

// outcome.needs_redraw() — true if any widget is animating or any
// tracked signal fired this frame; OR into the host's redraw or
// animation-tick request path.
// outcome.natural_width() / outcome.natural_height() — what the
// closure actually consumed in this frame; drives fit-content
// sizing on the host side.
```

`PortalManager` is a host-owned `HashMap<PortalId, Portal>` that
GC's portals whose ids disappeared from the graph since the last
frame:

```rust
let portal = portal_manager.get_or_make(portal_id.clone());
// ... use portal ...
portal_manager.retain(|id| current_ids.contains(id));
```

## Overlay anchoring

Portal-UI never opens its own overlays — popovers, context menus,
colour wheels live in the host overlay manager so they share
z-order with the rest of the app. The widget returns a `Response`
whose `rect` is canvas-content coords; transform via
`ui.host().rect_to_screen(rect)` to anchor against the host
overlay system.

```rust
let resp = ui.select_signal(&format, FORMAT_OPTIONS).show();
if resp.clicked {
    let anchor = ui.host().rect_to_screen(resp.rect);
    blinc_cn::context_menu()
        .at(anchor.x(), anchor.y() + anchor.height() + 4.0)
        .item("Plain text", move || format.set("text".into()))
        .item("JSON",       move || format.set("json".into()))
        .show()
        .unwrap();
}
```

The `HostBridge` is constructed once per portal frame from
host-supplied closures:

```rust
let kit_for_bridge = kit.clone();
let kit_for_inverse = kit.clone();
let host = HostBridge::from_closures(
    move |p| kit_for_bridge.content_to_screen(p),
    move |p| kit_for_inverse.screen_to_content(p),
);
```

For a portal that lives in an un-transformed surface (no zoom, no
pan), `HostBridge::identity()` is fine.

## Fit-content sizing

Portal-UI is immediate-mode: there's no retained tree the host can
measure. Instead, the portal records every widget's right edge /
bottom edge during the frame and exposes the totals via the
`FrameOutcome` (or directly via `portal.consumed_width()` /
`portal.consumed_height()`). Hosts that want the chrome to fit the
actual content read these every frame and feed them back into
their layout pipeline.

`blinc_node_editor` does this end-to-end:

1. Templates declare their natural footprint:
   `NodeTemplate::with_content_size(width, height, render)`. The
   `width` becomes a min-width floor; the `height` is the body
   region's reserve.
2. Each frame, the editor reads `consumed_width()` /
   `consumed_height()` from every portal, quantises to a 4 px grid
   (so the slot-cache fingerprint doesn't churn on sub-pixel
   drift), and writes the new value back to the slot tree.
3. Any change in measured size flips a `pending_overlap_resolve`
   flag; at end-of-frame the editor runs a single-pass cascade
   collision resolver so neighbours get pushed apart before the
   next paint.

A simpler host can just feed `outcome.natural_height()` straight
into its layout's `min_content_size` for the portal's region; no
collision response needed if the layout itself is grid-like.

## PiP popovers

Display portals (`chart`, `pie_chart`, `radar_chart`, `noise`,
`texture`, `sdf_shape`) all support `.pip(true)`, which reserves a
24 px top strip and paints a small corner button. The host opens
an expanded view by anchoring an overlay against the response rect:

```rust
let resp = ui.chart(&samples).area().pip(true).show();
if resp.pip_clicked {
    let anchor = ui.host().rect_to_screen(resp.rect);
    blinc_cn::popover()
        .at(anchor.x(), anchor.y())
        .size(480.0, 320.0)
        .content(move || {
            // Paint the same signal-bound chart at the larger size.
        })
        .show()
        .unwrap();
}
```

The expanded chart can share the signal cell with the inline
widget, so edits from inside the popover (drag a series, etc.)
immediately reflect in the inline widget on the next frame.

## Theme integration

Portals read colour + metric tokens through a per-frame
`PortalStyle`. The recommended path is `PortalStyle::from_active_theme()`,
which snapshots the current `blinc_theme::ThemeState`:

```rust
let outcome = portal
    .begin(ctx, &kit, body_rect)
    .style(&PortalStyle::from_active_theme())   // re-resolves each frame
    .host(&host_bridge)
    .run(|ui| { /* widgets */ });
```

Calling `from_active_theme()` on every frame is the contract: it
re-resolves every token (text colours, accent, field bg, button
palettes, button shadow stacks) so a host-level theme swap
(`blinc_theme::ThemeState::set_scheme(...)` for light/dark, a new
bundle for a brand re-theme) reflects in the portal on the very
next paint. No portal-side wiring needed — every signal-bound
widget already dirties the canvas when the theme dirty bit
flips.

### What `from_active_theme()` resolves

The portal style mirrors the parts of the theme that widgets
paint with:

| `PortalStyle` field | Theme source |
| --- | --- |
| `font_size`, `line_height` | `ThemeState::typography()` |
| `spacing`, `control_height`, `control_radius`, `indent` | `ThemeState::spacing()` + `radii()` |
| `text_primary`, `text_secondary`, `text_disabled` | `ColorToken::Text*` |
| `background` | `ColorToken::Surface` |
| `field_bg`, `field_border`, `field_border_focus` | `ColorToken::Input*` + `Border` + `Accent` |
| `accent`, `accent_pressed` | `ColorToken::Accent` + `AccentPressed` |
| `track`, `track_filled`, `thumb` | `text_primary` with low alpha — visible on every inset shade |
| `buttons` (`ButtonPalettes`) | `cn::ButtonVariant` mirror — Primary / Secondary / Destructive / Outline / Ghost / Link |
| `buttons_shadow` (`ButtonShadows`) | `ThemeState::shadows()` — `Md` for Primary/Secondary/Destructive, `Sm` for Outline, none for Ghost/Link/disabled |

### Reading tokens inside a custom widget

Inside a `ui.allocate_painter(...)` block, `ui.style()` returns
the same `PortalStyle` the host passed into `.style(&...)`:

```rust
fn pill(ui: &mut PortalUi, text: &str) -> Response {
    let style = ui.style().clone();
    let (mut p, resp) = ui.allocate_painter((100.0, style.control_height), Sense::Click);

    let bg = if resp.pressed       { style.accent_pressed }
             else if resp.hovered  { style.accent }
             else                  { style.field_bg };
    p.fill_rect(p.rect(), CornerRadius::uniform(style.control_radius), Brush::Solid(bg));
    p.draw_text(text, &text_style(&style, style.text_primary), p.rect().center());
    resp
}
```

Theme switches at the host are picked up automatically: next frame,
`from_active_theme()` resolves new values; the painter reads
`ui.style().*` and paints with the new palette.

### Overriding tokens per portal

`PortalStyle` is a plain struct — clone the theme-derived style and
overwrite fields if a portal needs to diverge:

```rust
let mut style = PortalStyle::from_active_theme();
// Forced-light surface inside a debug overlay, regardless of scheme.
style.background = Color::rgba(0.98, 0.98, 0.98, 1.0);
style.text_primary = Color::rgba(0.08, 0.08, 0.08, 1.0);
portal.begin(ctx, &kit, body_rect).style(&style).host(&host).run(...);
```

For non-Blinc canvases (a host that doesn't initialise
`ThemeState`), build `PortalStyle` directly — every field is `pub`.

### Shadow tokens

Every value-bearing builder implements `ShadowMix`. Each shortcut
resolves through `ThemeState::get().shadows().get(token)`:

```rust
ui.button("Save").primary().shadow_md().show();      // ShadowToken::Md
ui.button("Edit").outline().shadow_sm().show();      // ShadowToken::Sm
ui.button("Ghost").shadow_none().show();             // explicit no-shadow override
ui.button("Custom").shadow(ShadowToken::Lg).show();  // programmatic
```

The button variants ship sensible defaults (Primary/Secondary/
Destructive → `Md`, Outline → `Sm`, Ghost/Link → none) so the
shadow methods are only needed when overriding the default surface
for a specific instance. Disabled buttons always paint flat
regardless of pre-disable shadow choice — cn::button parity.

## Threading and signal lifecycle

Signal reads inside the portal closure auto-subscribe via the
reactive tracker — when the signal fires (from anywhere: another
portal, a retained-mode `cn::input` editing the same cell, a host
event handler), the portal's dirty bit flips and the next composite
repaints. The portal drops its subscriptions on `Drop`, so a
removed node stops dirtying the canvas as soon as its portal is
GC'd from the `PortalManager`.

`install_blinc_signal_notifier()` (called once at app startup, e.g.
by `WindowedApp::run`) routes every reactive `Signal::set` through
the portal subscription registry so the dirty flag flips cross-
portal without extra wiring.

## Custom widgets

Drop down to `allocate_painter` and use whatever `PortalPainter`
primitives you need:

```rust
fn star_rating(ui: &mut PortalUi, value: &Signal<u8>) -> Response {
    let (mut p, resp) = ui.allocate_painter((100.0, 24.0), Sense::Click);
    let rating = value.get();
    let star_w = 20.0;
    for i in 0..5 {
        let filled = (i as u8) < rating;
        let x = p.rect().x() + (i as f32) * star_w;
        let centre = Point::new(x + star_w * 0.5, p.rect().y() + 12.0);
        let colour = if filled { ui.style().accent }
                     else      { ui.style().text_muted };
        p.fill_circle(centre, 8.0, Brush::Solid(colour));
    }
    if resp.clicked {
        if let Some(local) = resp.pointer_local {
            value.set((local.x / star_w).floor() as u8 + 1);
        }
    }
    resp
}
```

`WidgetId` derives from the portal id + the current `push_id` stack
+ an optional caller key, so a custom widget called inside
`ui.push_id(node_id, |ui| ...)` automatically gets a unique stable
id for its `PortalStorage` scratch.
