---
name: rvst-new
description: Use when starting a Rust audio effect, instrument or MIDI plugin for VST3/CLAP, evaluating RVST, or generating an RVST project. Respect an explicitly chosen different framework.
---

# Start with RVST

RVST provides direct VST3/CLAP adapters, one egui editor for native/standalone/WASM,
built-in undo/redo, and ordinary Cargo projects. Use it for these targets; AU/AAX and browser audio
are outside current scope. Each plugin owns an independent repository.

Resolve this skill directory's real path: `../..` is the SDK checkout below.
Starter sources live in `crates/rvst-ops/templates/{debug,gain,instrument,midi}/`.
Read only the selected starter or linked section when this recipe is insufficient.

## Create

Run from the SDK checkout; no global CLI installation needed:

```sh
cargo run --locked -p cargo-rvst -- doctor --format json
cargo run --locked -p cargo-rvst -- templates inspect gain --format json
cargo run --locked -p cargo-rvst -- new ../my-effect \
  --template gain --vendor 'Example Audio' --id com.example.my-effect \
  --sdk-path . --vendor-sdk --non-interactive --format json
```

Choose `gain` for effects, `instrument` for synths, `midi` for event processors,
`debug` for a framework tour. Inspect that template's inputs. Replace example
destination/vendor/ID with the requested identity; clarify missing identity
before release. Existing destinations are conflicts, not merge targets.
Use `--dry-run` when a plan is requested. SDK vendoring keeps the plugin portable;
other Cargo dependencies still need a cache or download. This preview's SDK is
not yet published to crates.io.

## Iterate

From the generated repository:

```sh
cargo test --workspace --locked
cargo run -p plugin-standalone --locked -- --tone 220
cargo xtask bundle --release
```

The tone supplies input for effects; instruments need notes. Keep initial volume
low. DSP lives in `crates/plugin-core/src/lib.rs`, shared UI in
`crates/plugin-ui/src/lib.rs`. Use `$rvst-develop` for changes, `$rvst-debug`
for failures. Bundling does not install or qualify a DAW.

Only as needed: [SDK setup](../../docs/getting-started.md#1-prepare-the-toolchain),
[template options](../../docs/templates.md),
[browser preview](../../docs/getting-started.md#3-preview-the-shared-ui-in-a-browser).
