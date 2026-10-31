//! Host-independent contracts. Audio processing never requires an editor.
//! Values crossing [`EditorHost`] and [`EventKind::Parameter`] use plain units.
use serde::{Deserialize, Serialize};
use std::fmt;

mod undo;
pub use undo::{EditHistory, HistoryHost, HistoryRestoreGuard, HistorySnapshot, RestoredHistory};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
impl std::error::Error for Error {}
impl From<&str> for Error {
    fn from(v: &str) -> Self {
        Self(v.into())
    }
}
impl From<String> for Error {
    fn from(v: String) -> Self {
        Self(v)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PluginKind {
    Effect,
    Instrument,
    Midi,
}
#[derive(Debug, Clone, Copy, Serialize)]
pub struct AudioBus {
    pub name: &'static str,
    pub channels: u32,
    pub auxiliary: bool,
}
#[derive(Debug, Clone, Copy, Serialize)]
pub struct ParameterSpec {
    pub id: u32,
    pub name: &'static str,
    pub unit: &'static str,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    pub step: Option<f64>,
    pub bypass: bool,
}
impl ParameterSpec {
    pub fn clamp(&self, value: f64) -> f64 {
        let value = if value.is_finite() {
            value
        } else {
            self.default
        };
        let value = value.clamp(self.min, self.max);
        match self.step.filter(|s| s.is_finite() && *s > 0.0) {
            Some(step) => {
                (self.min + ((value - self.min) / step).round() * step).clamp(self.min, self.max)
            }
            None => value,
        }
    }
    pub fn normalize(&self, value: f64) -> f64 {
        (self.clamp(value) - self.min) / (self.max - self.min)
    }
    pub fn denormalize(&self, value: f64) -> f64 {
        if !value.is_finite() {
            return self.default;
        }
        self.clamp(self.min + value.clamp(0.0, 1.0) * (self.max - self.min))
    }
    pub fn validate(&self) -> Result<(), Error> {
        if !self.min.is_finite()
            || !self.max.is_finite()
            || self.min >= self.max
            || !self.default.is_finite()
            || !(self.min..=self.max).contains(&self.default)
            || self.step.is_some_and(|s| !s.is_finite() || s <= 0.0)
        {
            return Err(Error(format!("invalid parameter {}", self.id)));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct PluginDescriptor {
    pub id: &'static str,
    pub name: &'static str,
    pub vendor: &'static str,
    pub version: &'static str,
    pub url: &'static str,
    pub email: &'static str,
    pub vst3_id: [u8; 16],
    pub kind: PluginKind,
    pub parameters: &'static [ParameterSpec],
    pub inputs: &'static [AudioBus],
    pub outputs: &'static [AudioBus],
    pub midi_input: bool,
    pub midi_output: bool,
    pub supports_f64: bool,
}
impl PluginDescriptor {
    pub fn input_channels(&self) -> usize {
        self.inputs.iter().map(|b| b.channels as usize).sum()
    }
    pub fn output_channels(&self) -> usize {
        self.outputs.iter().map(|b| b.channels as usize).sum()
    }
    pub fn parameter(&self, id: u32) -> Option<&ParameterSpec> {
        self.parameters.iter().find(|p| p.id == id)
    }
    /// Control-thread validation before exposing an instance to a host.
    pub fn validate(&self) -> Result<(), Error> {
        if self.id.is_empty() || self.name.is_empty() || self.vst3_id == [0; 16] {
            return Err("plugin identity is incomplete".into());
        }
        for (i, p) in self.parameters.iter().enumerate() {
            p.validate()?;
            if self.parameters[..i].iter().any(|q| q.id == p.id) {
                return Err("duplicate parameter ID".into());
            }
        }
        if self
            .inputs
            .iter()
            .chain(self.outputs)
            .any(|b| b.channels == 0 || b.channels > 64)
        {
            return Err("audio bus must have 1..64 channels".into());
        }
        if self.input_channels() > 256 || self.output_channels() > 256 {
            return Err("too many audio channels".into());
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct ProcessConfig {
    pub sample_rate: f64,
    pub max_frames: usize,
    pub input_channels: usize,
    pub output_channels: usize,
}
impl ProcessConfig {
    pub fn validate(&self) -> Result<(), Error> {
        if !self.sample_rate.is_finite()
            || self.sample_rate <= 0.0
            || self.max_frames == 0
            || self.input_channels > 256
            || self.output_channels > 256
        {
            return Err("invalid processing configuration".into());
        }
        if self
            .max_frames
            .checked_mul(self.input_channels.max(self.output_channels))
            .is_none()
        {
            return Err("processing capacity overflow".into());
        }
        Ok(())
    }
}
/// Packed planar storage. Adapters must copy aliased host buffers into distinct,
/// preallocated storage before creating this safe view.
pub struct AudioBlock<'a, T> {
    pub inputs: &'a [T],
    pub outputs: &'a mut [T],
    pub frames: usize,
    pub input_channels: usize,
    pub output_channels: usize,
}
impl<'a, T> AudioBlock<'a, T> {
    pub fn new(
        inputs: &'a [T],
        outputs: &'a mut [T],
        frames: usize,
        input_channels: usize,
        output_channels: usize,
    ) -> Result<Self, Error> {
        if frames.checked_mul(input_channels) != Some(inputs.len())
            || frames.checked_mul(output_channels) != Some(outputs.len())
        {
            return Err("invalid planar audio shape".into());
        }
        Ok(Self {
            inputs,
            outputs,
            frames,
            input_channels,
            output_channels,
        })
    }
    pub fn input(&self, channel: usize) -> Option<&[T]> {
        if channel >= self.input_channels {
            return None;
        }
        Some(&self.inputs[channel * self.frames..(channel + 1) * self.frames])
    }
    pub fn output_mut(&mut self, channel: usize) -> Option<&mut [T]> {
        if channel >= self.output_channels {
            return None;
        }
        Some(&mut self.outputs[channel * self.frames..(channel + 1) * self.frames])
    }
}
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Transport {
    pub playing: bool,
    pub recording: bool,
    pub tempo: Option<f64>,
    pub sample_position: Option<i64>,
    pub beat_position: Option<f64>,
    pub time_signature: Option<(u16, u16)>,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct ParameterRamp {
    pub id: u32,
    pub start_frame: u32,
    pub end_frame: u32,
    pub start_normalized: f64,
    pub end_normalized: f64,
}
impl ParameterRamp {
    pub fn normalized_at(&self, frame: u32) -> f64 {
        if frame >= self.end_frame {
            return self.end_normalized;
        }
        if frame <= self.start_frame {
            return self.start_normalized;
        }
        let t = (frame - self.start_frame) as f64 / (self.end_frame - self.start_frame) as f64;
        self.start_normalized + (self.end_normalized - self.start_normalized) * t
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Event {
    pub frame: u32,
    pub kind: EventKind,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum EventKind {
    Parameter {
        id: u32,
        value: f64,
    },
    /// Offset in plain parameter units, applied to its base value.
    /// Format adapters convert host-domain offsets before delivery.
    Modulation {
        id: u32,
        amount: f64,
    },
    ParameterRamp(ParameterRamp),
    NoteOn {
        channel: u8,
        key: u8,
        velocity: f64,
        note_id: i32,
    },
    NoteOff {
        channel: u8,
        key: u8,
        velocity: f64,
        note_id: i32,
    },
    /// Scoped release: every specified field must match; `None` is a wildcard.
    /// `choke` stops matching voices immediately rather than running their release tail.
    NoteRelease {
        channel: Option<u8>,
        key: Option<u8>,
        note_id: Option<i32>,
        velocity: f64,
        choke: bool,
    },
    ChannelPressure {
        channel: u8,
        pressure: f64,
    },
    PolyPressure {
        channel: u8,
        key: u8,
        pressure: f64,
        note_id: i32,
    },
    PitchBend {
        channel: u8,
        value: f64,
    },
    ControlChange {
        channel: u8,
        controller: u8,
        value: f64,
    },
    AllNotesOff,
}
pub trait EventSink {
    /// Returns false if the receiver cannot accept the event.
    fn push(&mut self, event: Event) -> bool;
}
pub struct NullEventSink;
impl EventSink for NullEventSink {
    fn push(&mut self, _: Event) -> bool {
        false
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ProcessStatus {
    Continue,
    Tail,
    Sleep,
    Error,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Tail {
    None,
    Samples(u64),
    Infinite,
}

/// Core plugin lifecycle. State operations run off the audio thread. A failed
/// restore must leave the previous state intact. `process` must not allocate,
/// lock, perform I/O or destroy heap-owned values.
/// Numeric diagnostics travel separately from musical events and may drop.
/// Implementations must not allocate, lock or format text in `emit`.
pub trait DiagnosticSink: Send {
    fn emit(&mut self, code: u32, sample_clock: u64, value: f64) -> bool;
}

/// Thread-safe, plugin-owned editor state or command mailbox. Never expose a
/// mutable processor through this handle. Acquisition happens off the audio thread.
pub type EditorContext = std::sync::Arc<dyn std::any::Any + Send + Sync>;

/// Explicit legacy host-wire policies. New plugins should retain defaults.
/// These switches preserve already-shipped identities; they are not UI options.
#[derive(Debug, Default, Clone, Copy)]
pub struct AdapterCompatibility {
    pub normalized_clap_parameters: bool,
    pub clap_state_length_prefix: bool,
    pub clap_output_ids_follow_inputs: bool,
    pub vst3_midi_parameter_base: Option<u32>,
}

pub trait Plugin: Send + 'static {
    fn editor_size() -> (u32, u32) {
        (720, 480)
    }
    fn editor_min_size() -> (u32, u32) {
        (240, 160)
    }
    fn adapter_compatibility() -> AdapterCompatibility {
        AdapterCompatibility::default()
    }
    fn descriptor() -> &'static PluginDescriptor;
    fn new() -> Self;
    /// Called on the control thread. Retain the sink to emit bounded numeric
    /// events from processing; never destroy or replace it inside a callback.
    /// Numeric event names registered off the audio thread for readable inspectors.
    fn diagnostic_events() -> &'static [(u32, &'static str)] {
        &[]
    }
    fn connect_diagnostics(&mut self, _sink: Box<dyn DiagnosticSink>) {}
    fn editor_context(&self) -> Option<EditorContext> {
        None
    }
    fn prepare(&mut self, config: ProcessConfig) -> Result<(), Error>;
    fn process(
        &mut self,
        block: &mut AudioBlock<'_, f32>,
        events: &[Event],
        transport: &Transport,
        output: &mut dyn EventSink,
    ) -> ProcessStatus;
    fn process_f64(
        &mut self,
        block: &mut AudioBlock<'_, f64>,
        _events: &[Event],
        _transport: &Transport,
        _output: &mut dyn EventSink,
    ) -> ProcessStatus {
        block.outputs.fill(0.0);
        ProcessStatus::Error
    }
    fn reset(&mut self);
    fn parameter(&self, id: u32) -> Option<f64>;
    fn set_parameter(&mut self, id: u32, value: f64) -> Result<(), Error>;
    fn save_state(&self) -> Result<Vec<u8>, Error>;
    fn load_state(&mut self, data: &[u8]) -> Result<(), Error>;
    fn latency(&self) -> u32 {
        0
    }
    fn tail(&self) -> Tail {
        Tail::None
    }
}
/// Editor/control-thread interface. The implementation routes gestures through
/// the format-specific host, rather than calling audio-thread DSP directly.
pub trait EditorHost {
    /// Optional instance-owned history, retained when an editor window closes.
    fn edit_history(&self) -> Option<EditHistory> {
        None
    }
    /// Next undo action, or `None` when no undo action is available.
    fn undo_label(&self) -> Option<String> {
        None
    }
    /// Next redo action, or `None` when unavailable or an edit is in progress.
    fn redo_label(&self) -> Option<String> {
        None
    }
    /// Replay the previous UI action. Returns whether an action was available.
    /// This is an editor/control-thread operation.
    fn undo(&mut self) -> Result<bool, Error> {
        Ok(false)
    }
    /// Replay the last undone UI action on the editor/control thread.
    fn redo(&mut self) -> Result<bool, Error> {
        Ok(false)
    }
    /// Combine several UI edits into one undo step. Groups may be nested.
    fn begin_edit_group(&mut self, _label: &str) {}
    fn end_edit_group(&mut self) {}
    /// Optional per-instance plugin-owned, thread-safe editor context.
    fn editor_context(&self) -> Option<EditorContext> {
        None
    }
    fn descriptor(&self) -> &'static PluginDescriptor;
    fn parameter(&self, id: u32) -> Option<f64>;
    fn begin_edit(&mut self, id: u32);
    fn set_parameter(&mut self, id: u32, value: f64);
    fn end_edit(&mut self, id: u32);
    fn save_state(&mut self) -> Result<Vec<u8>, Error>;
    fn load_state(&mut self, data: &[u8]) -> Result<(), Error>;
}

impl<T: EditorHost + ?Sized> EditorHost for Box<T> {
    fn edit_history(&self) -> Option<EditHistory> {
        (**self).edit_history()
    }
    fn undo_label(&self) -> Option<String> {
        (**self).undo_label()
    }
    fn redo_label(&self) -> Option<String> {
        (**self).redo_label()
    }
    fn undo(&mut self) -> Result<bool, Error> {
        (**self).undo()
    }
    fn redo(&mut self) -> Result<bool, Error> {
        (**self).redo()
    }
    fn begin_edit_group(&mut self, label: &str) {
        (**self).begin_edit_group(label)
    }
    fn end_edit_group(&mut self) {
        (**self).end_edit_group()
    }
    fn editor_context(&self) -> Option<EditorContext> {
        (**self).editor_context()
    }
    fn descriptor(&self) -> &'static PluginDescriptor {
        (**self).descriptor()
    }
    fn parameter(&self, id: u32) -> Option<f64> {
        (**self).parameter(id)
    }
    fn begin_edit(&mut self, id: u32) {
        (**self).begin_edit(id)
    }
    fn set_parameter(&mut self, id: u32, value: f64) {
        (**self).set_parameter(id, value)
    }
    fn end_edit(&mut self, id: u32) {
        (**self).end_edit(id)
    }
    fn save_state(&mut self) -> Result<Vec<u8>, Error> {
        (**self).save_state()
    }
    fn load_state(&mut self, data: &[u8]) -> Result<(), Error> {
        (**self).load_state(data)
    }
}
