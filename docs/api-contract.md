# Shared API contract

[Documentation index](README.md) · [Architecture](architecture.md) · [Real-time rules](realtime.md)

The workspace uses Rust 2024, SDK version 0.1.0 and Rust 1.98. The source of truth
is [`rvst-core/src/lib.rs`](../crates/rvst-core/src/lib.rs). This page groups the
current contract by lifecycle, processing data and editor integration.

Plugin code must obey the real-time rules even when adapters provide panic
containment and allocation instrumentation. Core has no dependency on UI or
native adapters. Both formats require f32; f64 is optional.

## Plugin lifecycle

`Plugin: Send + 'static` requires these methods:

| Method | Contract |
| --- | --- |
| `descriptor() -> &'static PluginDescriptor` | Stable identity, parameter and bus declarations. |
| `new() -> Self` | Construct the processor. |
| `prepare(&mut self, config: ProcessConfig) -> Result<(), Error>` | Prepare storage before audio processing. |
| `reset(&mut self)` | Clear DSP history. |
| `parameter(&self, id: u32) -> Option<f64>` | Read a value in plain units. |
| `set_parameter(&mut self, id: u32, value: f64) -> Result<(), Error>` | Set a value in plain units. |
| `save_state(&self) -> Result<Vec<u8>, Error>` | Serialize plugin-owned state off the audio thread. |
| `load_state(&mut self, data: &[u8]) -> Result<(), Error>` | Restore off the audio thread; failure must preserve previous state. |

The required processing method has this signature:

```rust
fn process(
    &mut self,
    block: &mut AudioBlock<'_, f32>,
    events: &[Event],
    transport: &Transport,
    output: &mut dyn EventSink,
) -> ProcessStatus;
```

`process_f64` takes the same arguments with `AudioBlock<'_, f64>`. Its default
implementation silences the output and returns `ProcessStatus::Error`.

### Optional methods and defaults

| Method | Default |
| --- | --- |
| `latency(&self) -> u32` | `0` |
| `tail(&self) -> Tail` | `Tail::None` |
| `diagnostic_events() -> &'static [(u32, &'static str)]` | No labels (`&[]`). |
| `connect_diagnostics(&mut self, sink: Box<dyn DiagnosticSink>)` | No connection. |
| `editor_context(&self) -> Option<EditorContext>` | `None` |
| `editor_size() -> (u32, u32)` | `(720, 480)` |
| `editor_min_size() -> (u32, u32)` | `(240, 160)` |
| `adapter_compatibility() -> AdapterCompatibility` | Current wire conventions. |

## Descriptors and parameters

`PluginDescriptor` fixes the following declarations:

| Fields | Type |
| --- | --- |
| `id`, `name`, `vendor`, `version`, `url`, `email` | `&'static str` |
| `vst3_id` | `[u8; 16]` |
| `kind` | `PluginKind`: `Effect`, `Instrument` or `Midi` |
| `parameters` | `&'static [ParameterSpec]` |
| `inputs`, `outputs` | `&'static [AudioBus]` |
| `midi_input`, `midi_output`, `supports_f64` | `bool` |

`AudioBus` contains `name: &'static str`, `channels: u32` and `auxiliary: bool`.

| `ParameterSpec` fields | Type |
| --- | --- |
| `id` | `u32` |
| `name`, `unit` | `&'static str` |
| `min`, `max`, `default` | `f64` |
| `step` | `Option<f64>` |
| `bypass` | `bool` |

`ParameterSpec` supplies `normalize`, `denormalize` and `clamp`. Values crossing
`EditorHost` and `EventKind::Parameter` use **plain units**. A `ParameterRamp`
interpolates in **normalized parameter space**.

## Audio, transport and processing results

`ProcessConfig` contains `sample_rate: f64`, `max_frames: usize`,
`input_channels: usize` and `output_channels: usize`.

`AudioBlock<'a, T>` uses packed planar channels and exposes these public fields:

```rust
inputs: &'a [T],
outputs: &'a mut [T],
frames: usize,
input_channels: usize,
output_channels: usize,
```

`AudioBlock::new(inputs, outputs, frames, input_channels, output_channels)`
returns `Result<Self, Error>` and validates the planar shape. Inputs and outputs
never alias. Adapters preallocate scratch and copy host buffers before creating
Rust references. `input(channel)` and `output_mut(channel)` provide channel views.

| `Transport` fields | Type |
| --- | --- |
| `playing`, `recording` | `bool` |
| `tempo` | `Option<f64>` |
| `sample_position` | `Option<i64>` |
| `beat_position` | `Option<f64>` |
| `time_signature` | `Option<(u16, u16)>` |

`Transport` implements `Default`. Processing results use:

- `ProcessStatus`: `Continue`, `Tail`, `Sleep`, `Error`.
- `Tail`: `None`, `Samples(u64)`, `Infinite`.
- `Error(String)`: implements `Display`, `std::error::Error`, `From<&str>` and
  `From<String>`.

## Events and musical output

`Event { frame: u32, kind: EventKind }` is `Copy`. Input and output events retain
sample offsets. The available `EventKind` variants are grouped below.

| Parameter event | Fields |
| --- | --- |
| `Parameter` | `id: u32`, `value: f64` |
| `Modulation` | `id: u32`, `amount: f64` (offset in plain units) |
| `ParameterRamp` | A `ParameterRamp` value (see below). |

| Musical event | Fields |
| --- | --- |
| `NoteOn`, `NoteOff` | `channel: u8`, `key: u8`, `velocity: f64`, `note_id: i32` |
| `NoteRelease` | `channel: Option<u8>`, `key: Option<u8>`, `note_id: Option<i32>`, `velocity: f64`, `choke: bool` |
| `ChannelPressure` | `channel: u8`, `pressure: f64` |
| `PolyPressure` | `channel: u8`, `key: u8`, `pressure: f64`, `note_id: i32` |
| `PitchBend` | `channel: u8`, `value: f64` |
| `ControlChange` | `channel: u8`, `controller: u8`, `value: f64` |
| `AllNotesOff` | No fields. |

`ParameterRamp` contains `id: u32`, `start_frame: u32`, `end_frame: u32`,
`start_normalized: f64` and `end_normalized: f64`. It represents linear
interpolation between normalized endpoints.

`NoteRelease` matches every specified field; `None` is a wildcard. `choke`
requests an immediate stop instead of a release tail.

`EventSink::push(&mut self, event: Event) -> bool` returns `false` when the
receiver cannot accept an event. **`NullEventSink` always returns `false`; it is
not an accepting output collector.** Native adapters preallocate event scratch
and recover from lost musical events with bounded reset/release behavior.
Event-free blocks use whole-block processing; eventful blocks preserve offsets
through smaller calls.

## Editor integration

`EditorHost` is the non-audio UI interface. It routes gestures through the format
host rather than calling audio-thread DSP directly:

```rust
pub trait EditorHost {
    fn editor_context(&self) -> Option<EditorContext> { None }
    fn descriptor(&self) -> &'static PluginDescriptor;
    fn parameter(&self, id: u32) -> Option<f64>;
    fn begin_edit(&mut self, id: u32);
    fn set_parameter(&mut self, id: u32, value: f64);
    fn end_edit(&mut self, id: u32);
    fn save_state(&mut self) -> Result<Vec<u8>, Error>;
    fn load_state(&mut self, data: &[u8]) -> Result<(), Error>;
}
```

`rvst-ui` defines `EditorFn = fn(&egui::Context, &mut dyn rvst_core::EditorHost)`
and the default inspector. Native export macros accept a `Plugin` type and an
`EditorFn`. `rvst-platform::NativeEditor` provides open, show, hide, resize, close
and standalone run methods. See [UI status](ui-status.md) for ownership and
platform limits, and [themes](themes.md) for shared controls.

`EditorContext` is `Arc<dyn Any + Send + Sync>`, acquired off audio for
per-instance shared UI state or bounded command mailboxes. It must not expose
mutable DSP.

### Editor history

Native and browser runners decorate `EditorHost` with `HistoryHost`, which
records user edits and replays them through the same host notification methods.
The trait also provides default-compatible `edit_history`, `undo_label`,
`redo_label`, `undo`, `redo`, `begin_edit_group` and `end_edit_group` methods.
Custom controls using ordinary edit gestures inherit history automatically.

`EditHistory` is a shared, bounded, per-instance handle retained by native
parameter mirrors. Native project state includes versioned undo and redo history;
legacy states load with empty history. History storage runs on editor/control
threads; audio callbacks do not acquire its lock. Host automation does not
populate the history. See [undo and redo](undo-redo.md) for
grouping, keyboard behavior, opaque presets and lifetime limits.

## Diagnostics, compatibility and replay

`DiagnosticSink: Send` accepts
`emit(&mut self, code: u32, sample_clock: u64, value: f64) -> bool`.
Implementations must avoid allocation, locks and formatting. Diagnostic loss is
counted separately from musical event delivery. See [diagnostics](diagnostics.md)
for registering labels, inspecting live state and replaying recordings.

`AdapterCompatibility` explicitly preserves legacy normalized CLAP parameters,
length-prefixed CLAP state, output-port IDs and a VST3 MIDI parameter base. New
plugins retain its defaults. A policy's existence does not qualify migration of
an older plugin or preservation of every historical host session.

`rvst-runtime` provides `LinearSmoother`, atomic timing metrics, bounded telemetry
and recording channels, and offline replay. Recordings distinguish reset starts
from complete DSP checkpoints and retain block boundaries, precision, audio,
transport, input/output events and observed processing status. **Incomplete
recordings cannot establish deterministic verification.** See the
[real-time rules](realtime.md) for obligations that remain with plugin code.
