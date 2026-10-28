# Shared UI and theme packs

The public `EditorFn` is `fn(&egui::Context, &mut dyn EditorHost)`. That same
function runs in the native editor, standalone app and WASM canvas. Platform
services never belong in the shared editor. `default_editor` supplies parameter
controls and preset actions; `showcase_editor` provides the RVST Debug tour.

A theme is a versioned TOML manifest. Start with `crates/rvst-ui/themes/dark.toml`,
`light.toml`, or `studio.toml`. Version 1 includes name, dark mode, background,
panel, text, muted text, accent, border, error, spacing and corner radius.
`Theme::parse` validates version, numeric ranges and #RRGGBB colors. Layout stays
Rust. Fonts and artwork must carry their own distribution licences; do not embed
host or third-party brand assets without permission.

Controls expose labels and keyboard interaction. Parameter knobs support arrows,
Shift-drag and double-click reset, and bracket changes with host edit gestures.
Meters include textual dBFS/clip information. Skin contrast, zoom and reduced
motion are independent editor preferences. Browser tests must exercise actual
canvas input and the real shared component, not an HTML imitation.

`ThemePack::from_bytes` loads the versioned manifest, validated TTF/OTF fonts
and SVG artwork. Assets are supplied as owned byte buffers; installation and
texture loading happen on the UI thread. See the three bundled packs in
`crates/rvst-ui/themes` and the original artwork in `crates/rvst-ui/assets`.

Native controls and the canvas expose keyboard operation and labels. The pinned
eframe browser backend does not export an AccessKit DOM tree; browser screen
reader parity remains unqualified. This limitation is separate from the tested
keyboard interactions and semantic Rust UI tests.
