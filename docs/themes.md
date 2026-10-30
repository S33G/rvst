# Shared UI and theme packs

[Documentation index](README.md) · [API contract](api-contract.md) · [UI status](ui-status.md)

The public `EditorFn` is `fn(&egui::Context, &mut dyn EditorHost)`. The same
function runs in the native editor, standalone app and WASM canvas. Platform
services stay outside the shared editor.

## Choose an editor and starting theme

| Shared editor | Purpose |
| --- | --- |
| `default_editor` | Parameter controls and preset actions. |
| `showcase_editor` | The RVST Debug tour. |

Themes are versioned TOML manifests; layout stays in Rust. Start from one of the
bundled packs:

| Pack | Manifest | Original artwork | `ThemePack::bundled` index |
| --- | --- | --- | --- |
| Dark | [dark.toml](../crates/rvst-ui/themes/dark.toml) | [dark.svg](../crates/rvst-ui/assets/dark.svg) | `0` |
| Light | [light.toml](../crates/rvst-ui/themes/light.toml) | [light.svg](../crates/rvst-ui/assets/light.svg) | `1` |
| Studio | [studio.toml](../crates/rvst-ui/themes/studio.toml) | [studio.svg](../crates/rvst-ui/assets/studio.svg) | `2` |

## Manifest and asset loading

Version 1 describes the theme name, dark mode, background, panel, text, muted
text, accent, border, error, spacing and corner radius. Optional sections add
typography, meter styling, asset declarations and header artwork.

| API | Responsibility |
| --- | --- |
| `Theme::parse(manifest)` | Parse TOML and validate the version, numeric ranges and `#RRGGBB` colors. |
| `ThemePack::from_bytes(manifest, assets)` | Load the manifest and owned asset bytes supplied as `BTreeMap<String, Vec<u8>>`; check required assets, TTF/OTF fonts and SVG artwork. |
| `ThemePack::install(ctx)` | Register artwork and named font families on the UI thread. |
| `Theme::apply(ctx, high_contrast, reduced_motion)` | Apply the theme's style and editor preferences. |

The shared UI never resolves filesystem paths. Load bytes in the appropriate
platform/setup layer, then pass owned buffers to the pack. Installation and
texture loading happen on the UI thread.

For a bundled pack, call a helper like this during UI setup or when the selected
pack changes:

```rust
use rvst_ui::{ThemePack, egui};

fn configure_theme(
    ctx: &egui::Context,
    high_contrast: bool,
    reduced_motion: bool,
) -> Result<(), rvst_core::Error> {
    let pack = ThemePack::bundled(0)?;
    pack.install(ctx);
    pack.theme.apply(ctx, high_contrast, reduced_motion)
}
```

Fonts and artwork must carry their own distribution licenses. Do not embed host
or third-party brand assets without permission. See the
[shared UI implementation](../crates/rvst-ui/src/lib.rs) for the full schema and
validation rules.

## Controls and accessibility boundaries

Controls expose labels and keyboard interaction:

- Parameter knobs support arrow keys, fine adjustment with Shift-drag, and
  double-click reset. Changes are bracketed with host edit gestures.
- Meters include textual dBFS and clip information.
- Skin contrast, zoom and reduced motion are independent editor preferences.

Browser tests must exercise actual canvas input and the real shared component,
not an HTML imitation. Native controls and the canvas expose keyboard operation
and labels, but the pinned eframe browser backend does not export an AccessKit
DOM tree. **Browser screen reader parity remains unqualified.** That limitation
is separate from tested keyboard interactions and semantic Rust UI tests.

See [UI status](ui-status.md#automated-coverage-and-qualification) for validation
scope and remaining platform acceptance.
