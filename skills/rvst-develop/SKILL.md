---
name: rvst-develop
description: Use when changing an RVST plugin's DSP, parameters, state, shared egui editor or themes, or modifying the RVST SDK, templates or Cargo CLI.
---

# Develop RVST

Resolve this skill directory's real path: `../..` is the SDK checkout.
Establish the working repository first: `rvst-project.json` identifies generated
plugins; their `Cargo.toml` identifies the actual SDK dependency. A vendored
SDK may differ from this checkout. Keep plugin algorithms in the plugin repo.

## Open the owner

Paths below are relative to the working repository. Read the changed symbol and
its tests first; load a linked guide only for the relevant contract.

| Change | Start here |
| --- | --- |
| Plugin DSP, descriptors, parameters, state | `crates/plugin-core/src/lib.rs` |
| Plugin layout/themes | `crates/plugin-ui/src/lib.rs`; [theme controls](../../docs/themes.md) |
| Plugin exports/build helpers | `crates/plugin-native/src/lib.rs`, `crates/xtask/src/main.rs` |
| SDK host behavior | `crates/rvst-adapter`, `crates/rvst-vst3`, `crates/rvst-clap` |
| SDK CLI / execution / starters | `crates/cargo-rvst`, `crates/rvst-ops/src`, `crates/rvst-ops/templates` |

## Preserve contracts

- Keep released plugin/class/parameter IDs stable. State-load failure preserves
  previous state.
- `EditorHost` and parameter events use **plain units**; `ParameterRamp` endpoints
  use **normalized units**. Preserve sample offsets and advertised f32/f64 behavior.
- Default editor derives controls from descriptors. Custom controls use
  `rvst_ui::parameter_knob` or balanced `begin_edit → set_parameter → end_edit`;
  runners provide undo/redo automatically. Group multi-control actions with
  `begin_edit_group`/`end_edit_group`; [history](../../docs/undo-redo.md).
  The editor never directly mutates DSP. [API details](../../docs/api-contract.md).
- Allocate in prepare. Callbacks, including audio-side setters/error paths,
  must avoid allocation, deallocation, locks, I/O and formatting. Guard meaningful
  DSP regressions with `rvst_test::GuardedAllocator` and `AllocationGuard`.
  [Test recipes](../../docs/realtime.md).

## Verify the change

In a generated plugin, start with `cargo test -p plugin-core --locked` for DSP,
or `cargo check -p plugin-ui --locked` plus actual editor input for UI.
Finish relevant workspace tests, formatting and Clippy. Browser preview uses
`cargo xtask web --serve --open`; match wasm-bindgen CLI to `Cargo.lock`.
It validates the shared UI, not audio or native hosting.

For SDK changes use [contributor checks](../../CONTRIBUTING.md#check-a-change).
Template edits affect future projects: run `python3 scripts/check-generated.py
--template gain --vendor-sdk` from the SDK, replacing `gain` as appropriate.
Keep different generated projects' Cargo target directories separate.
