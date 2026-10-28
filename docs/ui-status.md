# Shared UI, native runner, and web status

## Implemented

`rvst-ui` owns `EditorFn = fn(&egui::Context, &mut dyn rvst_core::EditorHost)`. The same `default_editor`, `showcase_editor`, and plugin-specific function run on native egui-baseview and browser eframe. `showcase_editor_with` lets plugin authors add controls to the Events tab without duplicating the tour layout. No HTML substitute renders the plugin controls.

The default inspector includes keyboard-accessible parameter knobs, parameter gesture boundaries, reset and session parameter presets. The showcase adds Signal, Events, Themes, and Inspect pages. Knobs support arrow keys, fine Shift-drag, and double-click reset. Themes support scaling, high contrast and reduced-motion settings; meters include readable dBFS and clip text.

Dark, Light and Studio ship as versioned TOML plus SVG artwork. `ThemePack::from_bytes(manifest, BTreeMap<String, Vec<u8>>)` validates the portable manifest and its required font/image IDs. Fonts accept valid TTF/OTF face 0; artwork currently accepts SVG. `install(ctx)` registers byte-owned artwork and named fonts; `theme.apply(ctx, contrast, reduced_motion)` applies typography, colors, radius, spacing and meter styling. Layout stays in Rust. Shared UI never reads local paths.

`HostServices` defines asynchronous request/poll byte operations for platform open/save services. This is the extension contract; a built-in file-dialog implementation is not supplied. Session Store/Recall presets are functional via the editor host and do not claim to export files.

`rvst-platform::NativeEditor::open` creates a native editor from an owned `EditorHost + Send`, `EditorFn`, `EditorOptions` and optional `ParentWindowHandle`. The caller follows the host's window ownership rules. macOS/Windows use the owning UI thread; X11 Baseview runs the editor on its window thread. The native adapters marshal host-facing work appropriately: VST3 uses the host's `IRunLoop` for X11 gestures, and CLAP consumes editor parameter mailboxes through its host-facing parameter path. An editor callback must not assume it runs on the DAW's main thread on every platform.

Call `show()` for an embedded window; `run()` runs a standalone window. Close, hide, resize and optional finite duration are supported. `parent_from_raw` is unsafe: the caller must supply a live AppKit NSView, Win32 HWND or X11 window with the documented lifetime/thread ownership. Windows and Linux native GUI/runtime acceptance remains outstanding.

`rvst-standalone::run::<P>(editor)` runs CPAL output plus the shared native UI. The default input is silence; no microphone is opened. Explicit `--tone Hz` and `--file input.wav` provide routed input. WAV input is decoded before processing and linearly resampled. `--seconds N` bounds the run, including with UI; `--no-ui` requires a finite duration. DSP uses preallocated blocks of at most 512 frames and the common atomic parameter mirror. It copies finite bounded output to the device format. This runner executes the plugin's f32 path; native format adapters additionally expose f64.

The `rvst-standalone` library's `devtools` feature defaults off. Generated `plugin-standalone` wrappers enable it through their default Cargo features, including release builds; use `--no-default-features` on that wrapper to omit it. It connects plugin-authored numeric diagnostics and event names, bounded runtime logs, callback metrics, live parameter/meter/latency snapshots and a tokenized loopback inspector. The native UI offers open/copy actions for that URL. Optional editor CPU timing measures the editor function, excluding backend rendering and presentation. Meter values come from the native audio buffers. EditorContext is acquired per instance before the audio thread takes ownership and forwarded to the editor.

Standalone has no physical MIDI input/output device integration. The Debug editor's note buttons send commands through EditorContext. Standalone accepts and discards outgoing MIDI events locally; it does not forward them to a device. Route an external MIDI controller through a DAW hosting the native plugin.

`rvst-web::start::<P>(canvas_id, editor)` starts a genuine Rust/WASM eframe app on an existing canvas. Its deterministic FakeHost owns P's control/state model and forwards its EditorContext. It does not execute DSP or ask for audio input. The UI labels this limitation explicitly. Browser dependencies exclude CPAL, Baseview and rvst-platform.

## Debug showcase

The debug template has eight parameters: gain, tone, bypass, waveform, log marker, sidechain ducking, smoothing time and bounded workload. It implements stereo main/auxiliary input, f32/f64 processing, the runtime LinearSmoother, normalized ramps, MIDI voice/pressure/bend, MIDI event forwarding with emergency release recovery, 20 ms release tail, zero latency, versioned atomic preset restore and real numeric diagnostics.

Its plugin-specific UI adds twelve note buttons and All notes off to the Events page. Commands travel through per-instance atomic EditorContext state. Each demo note emits MIDI on/off and automatically releases after 300 ms. Browser playback controls are visibly disabled because browser DSP is deferred. Separate instances do not share note commands.

## Automated coverage and qualification

The repository contains headless tests for keyboard gesture balance, theme
painting, font/artwork validation, default silence, tone samples across block
boundaries, parameter edits, unused channels, partial device frames and callback
allocation/deallocation counts. Inspector tests exercise live standalone values,
meters, latency and named logs. The platform test measures a real egui editor
function with optional timing enabled and disabled. These tests do not open a
DAW or prove window-system accessibility.

Generated Debug tests cover its DSP, state, MIDI and per-instance editor command
contract. Final regenerated Debug artifact, browser interaction and native host
validation remain pending until results for those exact artifacts are recorded.
See [native adapter status](native-status.md) for the separately scoped gain
fixture validator evidence; that evidence does not qualify Debug or another
plugin.

Run the relevant SDK and generated-project tests with locked dependencies. A
WASM build requires the target installed for the actual rustc selected by Cargo;
setting `RUSTUP_TOOLCHAIN` does not change a directly invoked non-rustup compiler.

Windows/Linux native windows and audio, physical MIDI routing through DAWs,
macOS accessibility tooling and DAW-specific resize/scale/keyboard behavior
remain acceptance work. Built-in asynchronous OS/browser file-dialog adapters
and browser audio processing are not implemented. Browser editor and native DSP
are separate execution environments even though they share the editor function.
