# Getting started

## The RVST Debug tour

Build the separate showcase using the first-run command in the root README.
The workbench's Start with RVST Debug action invokes the same operation.

1. **Signal:** start at low gain. Enable the tone to hear an actual native
   oscillator; move gain and bypass. Observe the output meters.
2. **Events:** in standalone, use the on-screen note buttons and All notes off.
   For an external MIDI controller, load the native plugin in a DAW and route
   the controller to that track. Standalone does not open physical MIDI devices.
   Observe the note/parameter event stream; turn Log marker to leave a numeric
   breadcrumb.
3. **Themes:** switch dark, light and studio skins. Try keyboard navigation,
   scaling, high contrast and reduced motion.
4. **State:** save a parameter preset, change a control, restore it. Processor
   state is independent of editor skin/zoom state.
5. **Inspect:** open the browser inspector. Match the instance ID and process ID
   to the native plugin. Filter plugin events, pause the display and inspect
   callback timing/deadline counters. No account or cloud endpoint is involved.
6. **Reproduce:** in the generated repository, run `cargo xtask record capture.json`,
   then `cargo xtask replay capture.json --step`. Each line identifies the
   original callback block and checks its observed outputs.

The deterministic recording example is an **offline** host fixture. The runtime
recorder API can capture live callbacks, but starting a live recording after DSP
has accumulated history requires a complete DSP checkpoint or a controlled reset.
A parameter snapshot alone is not a deterministic audio checkpoint.

## Browser preview

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.130 --locked
cargo xtask web --serve --open
```

The WASM runner calls the same editor function as VST3, CLAP and standalone. Its
host simulates parameters and state; it deliberately does not process browser
audio. Open Chrome DevTools to inspect the browser frontend. Native DSP stepping
uses LLDB or GDB. See the diagnostics guide for the boundary.

## Normal Cargo workflows

```sh
cargo build --locked
cargo test --workspace --locked
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo xtask bundle --release
```

`cargo xtask install` performs explicit user-scope installation; a plain bundle
never installs or publishes. Keep a stable identity after shipping a plugin.
