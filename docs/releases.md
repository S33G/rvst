# Release and compatibility qualification

1. Build from a clean commit and locked dependencies. Record rustc, Cargo,
   operating system, architecture, format, plugin category and exact host version.
2. Run `cargo fmt --all --check`, workspace tests, Clippy and generated-project
   clone tests. Build the WASM target. Exercise controls in Chrome, Firefox and
   Safari; save screenshots plus semantic/input assertions.
3. Run the Steinberg VST3 validator and CLAP validator on the packaged binary.
   A validator pass and a DAW pass are distinct records.
4. Test REAPER and Bitwig on each supported OS; Ableton Live and Cubase on
   macOS/Windows. Include load/unload, multiple instances, parameter automation,
   sidechain routing, MIDI/MPE, state roundtrip, bypass, latency and offline render.
5. Generate dependency notices with `python3 scripts/dependency-notices.py`.
   Review all licence expressions and asset licences. Source licences are included
   with RVST and generated projects; third-party obligations still apply.
6. Bundle with `cargo xtask bundle --release`. Signing/notarisation is a separate
   deployment step. Use your own signing identity and credentials, then verify
   signatures before labelling artifacts signed. Never bake credentials into a
   template or example. Hook external signing after the bundle helper returns its
   artifact path; no shell template hooks execute during scaffolding.

Qualification targets: macOS 14.2+ arm64 and x64; Windows 11 x64; Ubuntu 24.04 x64
X11/XWayland. A table cell is **untested** until evidence exists. Unsupported is a
separate status. Do not convert compilation, CI definitions or historical Hollow
Point evidence into current RVST DAW passes.

Publishing order follows Cargo dependency order: core, runtime/test, devtools,
UI, platform/adapter, adapters/runners/build, operations, CLI. Check each package
with `cargo package --list`; publish only after dependencies exist on the target
registry. Current development uses explicit path overrides or a vendored SDK.
No crate publication or Git push is implied by generating/bundling a project.
