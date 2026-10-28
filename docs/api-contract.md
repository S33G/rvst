# Shared API contract

The workspace uses Rust 2024, SDK version 0.1.0 and Rust 1.98. The public core
contract is defined in `crates/rvst-core/src/lib.rs`; this document describes its
current shape. Plugin code must obey the real-time rules even when the adapters
provide panic containment and allocation instrumentation.

`rvst_core::Plugin: Send + 'static` has:

- `fn descriptor() -> &'static PluginDescriptor;`
- `fn new() -> Self;`
- `fn prepare(&mut self, config: ProcessConfig) -> Result<(), Error>;`
- `fn process(&mut self, block: &mut AudioBlock<'_, f32>, events: &[Event], transport: &Transport, output: &mut dyn EventSink) -> ProcessStatus;`
- `fn process_f64(...) -> ProcessStatus` same arguments with f64, default Error/unsupported status.
- `fn reset(&mut self);`
- `fn parameter(&self, id: u32) -> Option<f64>;`
- `fn set_parameter(&mut self, id: u32, value: f64) -> Result<(), Error>;` values are plain domain.
- `fn save_state(&self) -> Result<Vec<u8>, Error>;`
- `fn load_state(&mut self, data: &[u8]) -> Result<(), Error>;`
- `fn latency(&self) -> u32` default 0; `fn tail(&self) -> Tail` default None.
- `fn diagnostic_events() -> &'static [(u32, &'static str)]` defaults to no labels.
- `fn connect_diagnostics(&mut self, sink: Box<dyn DiagnosticSink>)` defaults to no connection.
- `fn editor_context(&self) -> Option<EditorContext>` defaults to None.
- `fn editor_size() -> (u32, u32)` defaults to `(720, 480)`; `editor_min_size()` defaults to `(240, 160)`.
- `fn adapter_compatibility() -> AdapterCompatibility` defaults to the current wire conventions.

`PluginDescriptor` fields: `id,name,vendor,version,url,email: &'static str`, `vst3_id: [u8;16]`, `kind: PluginKind` (Effect,Instrument,Midi), `parameters: &'static [ParameterSpec]`, `inputs,outputs: &'static [AudioBus]`, `midi_input,midi_output,supports_f64: bool`.
`AudioBus { name: &'static str, channels: u32, auxiliary: bool }`.
`ParameterSpec { id:u32, name:&'static str, unit:&'static str, min:f64,max:f64,default:f64,step:Option<f64>,bypass:bool }`; methods normalize,denormalize,clamp.
`ProcessConfig { sample_rate:f64, max_frames:usize, input_channels:usize, output_channels:usize }`.
`AudioBlock` constructed with `new(inputs:&[T],outputs:&mut[T],frames:usize,input_channels:usize,output_channels:usize)->Result<Self,Error>` uses packed planar channels and exposes public fields. Inputs and outputs never alias. Adapters preallocate scratch and copy host buffers; never construct aliased Rust references.
`Transport { playing:bool, recording:bool, tempo:Option<f64>, sample_position:Option<i64>, beat_position:Option<f64>, time_signature:Option<(u16,u16)> }` Default.
`Event { frame:u32, kind:EventKind }`, Copy. `EventKind`: `Parameter{id:u32,value:f64}`, `Modulation{id:u32,amount:f64}`, `ParameterRamp(ParameterRamp)`, `NoteOn{channel:u8,key:u8,velocity:f64,note_id:i32}`, `NoteOff{channel:u8,key:u8,velocity:f64,note_id:i32}`, `NoteRelease{channel:Option<u8>,key:Option<u8>,note_id:Option<i32>,velocity:f64,choke:bool}`, `ChannelPressure{channel:u8,pressure:f64}`, `PolyPressure{channel:u8,key:u8,pressure:f64,note_id:i32}`, `PitchBend{channel:u8,value:f64}`, `ControlChange{channel:u8,controller:u8,value:f64}`, `AllNotesOff`.
`ParameterRamp { id:u32, start_frame:u32, end_frame:u32, start_normalized:f64, end_normalized:f64 }` interpolates in normalized parameter space. `NoteRelease` matches every specified field; None is a wildcard, and `choke` requests an immediate stop.
`EventSink { fn push(&mut self,event:Event)->bool; }` returns false when the receiver cannot accept an event. `NullEventSink` always returns false; it is not an accepting output collector.
`ProcessStatus`: Continue, Tail, Sleep, Error. `Tail`: None, Samples(u64), Infinite.
`Error(String)`, Display/Error and From<&str>/String.

`EditorHost` is the non-audio UI interface: `fn editor_context(&self)->Option<EditorContext>` defaults to None; `fn descriptor(&self)->&'static PluginDescriptor; fn parameter(&self,id:u32)->Option<f64>; fn begin_edit(&mut self,id:u32); fn set_parameter(&mut self,id:u32,value:f64); fn end_edit(&mut self,id:u32); fn save_state(&mut self)->Result<Vec<u8>,Error>; fn load_state(&mut self,data:&[u8])->Result<(),Error>;`.
`rvst-ui` defines `pub type EditorFn = fn(&egui::Context, &mut dyn rvst_core::EditorHost);` and the default inspector. Native export macros accept a Plugin type and EditorFn. `rvst-platform::NativeEditor` provides open/show/hide/resize/close and standalone run methods; see [UI status](ui-status.md) for ownership and platform limits.

`EditorContext` is `Arc<dyn Any + Send + Sync>`, acquired off audio for per-instance
shared UI state or bounded command mailboxes. It must not expose mutable DSP.
`DiagnosticSink: Send` accepts `emit(code:u32, sample_clock:u64, value:f64)->bool`;
implementations must avoid allocation, locks and formatting. Diagnostic loss is
counted separately from musical event delivery.

`AdapterCompatibility` explicitly preserves legacy normalized CLAP parameters,
length-prefixed CLAP state, output-port IDs and a VST3 MIDI parameter base. New
plugins retain its defaults. A policy's existence does not qualify migration of
an older plugin or preservation of every historical host session.

`rvst-runtime` provides `LinearSmoother`, atomic timing metrics, bounded telemetry
and recording channels, and offline replay. Recordings distinguish reset starts
from complete DSP checkpoints and retain block boundaries, precision, audio,
transport, input/output events and observed processing status. Incomplete
recordings cannot establish deterministic verification.

Core has no dependency on UI/native adapters. Both formats require f32; f64 is
optional. Native adapters preallocate event scratch and recover from lost musical
events with bounded reset/release behavior. Event-free blocks use whole-block
processing; eventful blocks preserve offsets through smaller calls. See
[real-time rules](realtime.md) for obligations that remain with plugin code.
