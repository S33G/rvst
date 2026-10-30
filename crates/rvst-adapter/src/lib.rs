//! Internal, format-independent native adapter machinery.
//! Audio processing uses owned scratch, including when a host aliases its input/output channels.
use rvst_core::*;
use std::sync::{
    Arc,
    atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering},
};
mod state;
pub use state::{
    NativeState, decode_native_state, encode_native_state, load_project_state, save_project_state,
};

/// Catch a plugin panic before it crosses a native ABI boundary. A panic payload
/// is deliberately leaked: dropping an arbitrary plugin payload on audio is unsafe.
pub fn boundary<T>(fallback: T, f: impl FnOnce() -> T) -> T {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(value) => value,
        Err(payload) => {
            std::mem::forget(payload);
            fallback
        }
    }
}

const EDIT_KIND_MASK: u64 = 7;
const EDIT_QUEUED: u64 = 1;
const EDIT_APPLYING: u64 = 2;
const EDIT_HOST_QUEUED: u64 = 3;
const EDIT_HOST_SENT: u64 = 4;

pub struct ParameterMirror {
    pub latency: AtomicU32,
    pub ui_metrics: Option<Arc<rvst_runtime::Metrics>>,
    pub editor_context: Option<EditorContext>,
    pub inspector_url: Option<String>,
    pub meters: Arc<rvst_ui::Meters>,
    pub descriptor: &'static PluginDescriptor,
    pub edit_history: EditHistory,
    values: Vec<AtomicU64>,
    edits: Vec<AtomicU64>,
    dirty: Vec<AtomicU64>,
    edit_generations: Vec<AtomicU64>,
    edit_sequence: AtomicU64,
    generation: AtomicU64,
    gestures: Vec<AtomicU8>,
}
impl ParameterMirror {
    pub fn new(descriptor: &'static PluginDescriptor) -> Arc<Self> {
        Self::with_services(descriptor, None, None)
    }
    pub fn with_services(
        descriptor: &'static PluginDescriptor,
        editor_context: Option<EditorContext>,
        inspector_url: Option<String>,
    ) -> Arc<Self> {
        Arc::new(Self {
            latency: AtomicU32::new(0),
            ui_metrics: None,
            editor_context,
            inspector_url,
            meters: Arc::new(rvst_ui::Meters::default()),
            descriptor,
            edit_history: EditHistory::default(),
            generation: AtomicU64::new(1),
            edit_sequence: AtomicU64::new(1),
            values: descriptor
                .parameters
                .iter()
                .map(|p| AtomicU64::new(p.default.to_bits()))
                .collect(),
            edits: descriptor
                .parameters
                .iter()
                .map(|p| AtomicU64::new(p.default.to_bits()))
                .collect(),
            dirty: descriptor
                .parameters
                .iter()
                .map(|_| AtomicU64::new(0))
                .collect(),
            edit_generations: descriptor
                .parameters
                .iter()
                .map(|_| AtomicU64::new(0))
                .collect(),
            gestures: descriptor
                .parameters
                .iter()
                .map(|_| AtomicU8::new(0))
                .collect(),
        })
    }
    /// Snapshot applied/pending values for a diagnostic worker. Allocates; never call on audio.
    pub fn parameters_snapshot(&self) -> serde_json::Value {
        serde_json::json!(self.descriptor.parameters.iter().enumerate().map(|(i, p)| {
            let pending = self.pending(i);
            serde_json::json!({"id":p.id,"name":p.name,"unit":p.unit,"min":p.min,"max":p.max,
                "value":f64::from_bits(self.values[i].load(Ordering::Acquire)),"pending":pending})
        }).collect::<Vec<_>>())
    }
    /// Snapshot live meters/latency for a diagnostic worker. Allocates; never call on audio.
    pub fn meters_snapshot(&self) -> serde_json::Value {
        let current = self.meters.snapshot();
        serde_json::json!({"input_peak":current.input_peak,"output_peak":current.output_peak,
            "sample_clock":current.sample_clock,"latency_samples":self.latency.load(Ordering::Acquire)})
    }
    pub fn get(&self, id: u32) -> Option<f64> {
        let i = self.descriptor.parameters.iter().position(|p| p.id == id)?;
        Some(
            self.pending(i)
                .unwrap_or_else(|| f64::from_bits(self.values[i].load(Ordering::Acquire))),
        )
    }
    fn pending(&self, index: usize) -> Option<f64> {
        let marker = self.dirty[index].load(Ordering::Acquire);
        (marker != 0
            && self.edit_generations[index].load(Ordering::Acquire) == self.state_generation())
        .then(|| f64::from_bits(self.edits[index].load(Ordering::Acquire)))
    }
    /// Generation of the native state, shared with deferred host notifications.
    pub fn state_generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }
    /// Call only after a successful native restore, with the audio consumer excluded.
    /// The generation rejects even a UI producer that resumes after the restore.
    /// Gesture markers are preserved so an already forwarded begin can still end.
    /// This path performs only atomic operations and is safe on host state callbacks.
    pub fn state_restored(&self) {
        self.discard_pending_edits();
        self.edit_history.invalidate();
    }
    fn discard_pending_edits(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }
    pub fn publish(&self, id: u32, value: f64) {
        if let Some(i) = self.descriptor.parameters.iter().position(|p| p.id == id) {
            self.values[i].store(value.to_bits(), Ordering::Release);
            let marker = self.dirty[i].load(Ordering::Acquire);
            if marker & EDIT_KIND_MASK == EDIT_APPLYING {
                let _ =
                    self.dirty[i].compare_exchange(marker, 0, Ordering::AcqRel, Ordering::Acquire);
            }
        }
    }
    pub fn edit(&self, id: u32, value: f64) {
        if let Some(i) = self.descriptor.parameters.iter().position(|p| p.id == id) {
            self.queue_edit(i, value, self.state_generation());
        }
    }
    fn queue_edit(&self, index: usize, value: f64, generation: u64) {
        self.queue_edit_kind(index, value, generation, EDIT_QUEUED);
    }
    fn queue_edit_kind(&self, index: usize, value: f64, generation: u64, kind: u64) -> u64 {
        let marker = (self.edit_sequence.fetch_add(1, Ordering::Relaxed) << 3) | kind;
        self.edits[index].store(
            self.descriptor.parameters[index].clamp(value).to_bits(),
            Ordering::Release,
        );
        self.edit_generations[index].store(generation, Ordering::Release);
        self.dirty[index].store(marker, Ordering::Release);
        marker
    }
    /// Retain VST editor intent while the host carries the value to its processor.
    /// Unlike ordinary edits, this is never consumed by `Runtime::edits`.
    pub fn defer_host_edit(&self, id: u32, value: f64, generation: u64) -> u64 {
        self.descriptor
            .parameters
            .iter()
            .position(|p| p.id == id)
            .map_or(0, |i| {
                self.queue_edit_kind(i, value, generation, EDIT_HOST_QUEUED)
            })
    }
    pub fn host_edit_dispatched(&self, id: u32, marker: u64) {
        if marker & EDIT_KIND_MASK == EDIT_HOST_QUEUED
            && let Some(i) = self.descriptor.parameters.iter().position(|p| p.id == id)
        {
            let _ = self.dirty[i].compare_exchange(
                marker,
                (marker & !EDIT_KIND_MASK) | EDIT_HOST_SENT,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
        }
    }
    /// Capture before applying an incoming host event, so an older event cannot
    /// acknowledge a newer UI dispatch that occurs while processing it.
    pub fn host_edit_token(&self, id: u32) -> u64 {
        self.descriptor
            .parameters
            .iter()
            .position(|p| p.id == id)
            .map_or(0, |i| {
                let marker = self.dirty[i].load(Ordering::Acquire);
                if marker & EDIT_KIND_MASK == EDIT_HOST_SENT
                    && self.edit_generations[i].load(Ordering::Acquire) == self.state_generation()
                {
                    marker
                } else {
                    0
                }
            })
    }
    /// An explicit host event may acknowledge or supersede its captured target.
    /// Ordinary processor snapshots cannot, nor can events older than a target.
    pub fn acknowledge_host_edit(&self, id: u32, value: f64, token: u64) {
        if let Some(i) = self.descriptor.parameters.iter().position(|p| p.id == id) {
            self.values[i].store(value.to_bits(), Ordering::Release);
            if token & EDIT_KIND_MASK == EDIT_HOST_SENT {
                let _ =
                    self.dirty[i].compare_exchange(token, 0, Ordering::AcqRel, Ordering::Acquire);
            }
        }
    }
    pub fn gesture(&self, id: u32, begin: bool) {
        if let Some(i) = self.descriptor.parameters.iter().position(|p| p.id == id) {
            self.gestures[i].fetch_or(if begin { 1 } else { 2 }, Ordering::AcqRel);
        }
    }
    pub fn take_gesture(&self, index: usize) -> u8 {
        self.gestures[index].swap(0, Ordering::AcqRel)
    }
    pub fn retry_edit(&self, index: usize) {
        // Called by the audio consumer while the same state-restore gate is held.
        // Do not replace an edit queued by the UI since take_edit.
        let _ = self.dirty[index].compare_exchange(
            0,
            (self.edit_sequence.fetch_add(1, Ordering::Relaxed) << 3) | EDIT_QUEUED,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }
    pub fn take_edit(&self, index: usize) -> Option<(u32, f64)> {
        let marker = self.dirty[index].load(Ordering::Acquire);
        if marker & EDIT_KIND_MASK == EDIT_QUEUED
            && self.edit_generations[index].load(Ordering::Acquire) == self.state_generation()
            && self.dirty[index]
                .compare_exchange(
                    marker,
                    (marker & !EDIT_KIND_MASK) | EDIT_APPLYING,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
        {
            Some((
                self.descriptor.parameters[index].id,
                f64::from_bits(self.edits[index].load(Ordering::Acquire)),
            ))
        } else {
            None
        }
    }
}

/// UI-only view. Host adapters own gesture forwarding; atomics carry plain values.
pub struct MirrorEditor(pub Arc<ParameterMirror>);
impl EditorHost for MirrorEditor {
    fn edit_history(&self) -> Option<EditHistory> {
        Some(self.0.edit_history.clone())
    }
    fn editor_context(&self) -> Option<EditorContext> {
        self.0.editor_context.clone()
    }
    fn descriptor(&self) -> &'static PluginDescriptor {
        self.0.descriptor
    }
    fn parameter(&self, id: u32) -> Option<f64> {
        self.0.get(id)
    }
    fn begin_edit(&mut self, id: u32) {
        self.0.gesture(id, true)
    }
    fn set_parameter(&mut self, id: u32, value: f64) {
        self.0.edit(id, value)
    }
    fn end_edit(&mut self, id: u32) {
        self.0.gesture(id, false)
    }
    fn save_state(&mut self) -> Result<Vec<u8>, Error> {
        save_parameter_preset(&self.0)
    }
    fn load_state(&mut self, data: &[u8]) -> Result<(), Error> {
        let values = parse_parameter_preset(self.0.descriptor, data)?;
        for (id, value) in values {
            self.0.edit(id, value);
        }
        Ok(())
    }
}

pub const EVENT_CAPACITY: usize = 1024;
#[cfg(feature = "devtools")]
struct Debug {
    session: rvst_devtools::DebugSession,
    logger: rvst_devtools::RtLog,
    metrics: Arc<rvst_runtime::Metrics>,
    clock: u64,
}
#[cfg(feature = "devtools")]
impl Debug {
    fn new(d: &PluginDescriptor) -> Option<Self> {
        let (session, logger) = rvst_devtools::DebugSession::start(d.id, d.name).ok()?;
        session.log(
            rvst_devtools::Level::Info,
            "adapter",
            "Native plugin instance created",
        );
        session.set_snapshot(serde_json::json!({"descriptor":d,"lifecycle":"created"}));
        let metrics = session.metrics();
        Some(Self {
            session,
            logger,
            metrics,
            clock: 0,
        })
    }
}
/// Own rejected parameter errors until a control-thread collection point.
/// Capacity is fixed before processing; overflow leaks only the rejected payload,
/// since arbitrary heap destruction is never permitted on the audio thread.
pub struct RetiredErrors {
    errors: Vec<Error>,
    overflow: u64,
}
impl Default for RetiredErrors {
    fn default() -> Self {
        Self {
            errors: Vec::with_capacity(64),
            overflow: 0,
        }
    }
}
impl RetiredErrors {
    pub fn retain(&mut self, error: Error) {
        if self.errors.len() < self.errors.capacity() {
            self.errors.push(error);
        } else {
            self.overflow = self.overflow.saturating_add(1);
            std::mem::forget(error);
        }
    }
    /// Call only outside processing, with exclusive ownership of the processor.
    pub fn collect_on_control_thread(&mut self) {
        self.errors.clear();
    }
    pub fn overflow_count(&self) -> u64 {
        self.overflow
    }
}

pub struct Runtime<P: Plugin> {
    #[cfg(feature = "devtools")]
    debug: Option<Debug>,
    pub plugin: DropShield<P>,
    pub retired_errors: RetiredErrors,
    pub mirror: Arc<ParameterMirror>,
    pub input32: Vec<f32>,
    pub output32: Vec<f32>,
    pub block_input32: Vec<f32>,
    pub block_output32: Vec<f32>,
    pub block_input64: Vec<f64>,
    pub block_output64: Vec<f64>,
    pub input64: Vec<f64>,
    pub output64: Vec<f64>,
    pub events: Vec<Event>,
    pub max_frames: usize,
    sample_clock: u64,
    pub sample_rate: f64,
    pub lost_events: u64,
    pub output_reset_pending: bool,
}
impl<P: Plugin> Runtime<P> {
    #[allow(unused_mut)]
    pub fn new() -> Self {
        let mut plugin = P::new();
        #[cfg(feature = "devtools")]
        let debug = Debug::new(P::descriptor());
        #[cfg(feature = "devtools")]
        if let Some(debug) = &debug {
            for &(code, name) in P::diagnostic_events() {
                debug.session.register_event(code, name);
            }
            if let Ok(log) = debug.session.plugin_log() {
                plugin.connect_diagnostics(Box::new(log));
            }
        }
        #[allow(unused_mut)]
        let mut mirror =
            ParameterMirror::with_services(P::descriptor(), plugin.editor_context(), {
                #[cfg(feature = "devtools")]
                {
                    debug.as_ref().map(|d| d.session.info().url.clone())
                }
                #[cfg(not(feature = "devtools"))]
                {
                    None
                }
            });
        #[cfg(feature = "devtools")]
        if let Some(debug) = &debug {
            Arc::get_mut(&mut mirror)
                .expect("unshared mirror during setup")
                .ui_metrics = Some(debug.metrics.clone());
            let parameters = mirror.clone();
            let _ = debug
                .session
                .register_state_provider("parameters", move || parameters.parameters_snapshot());
            let meters = mirror.clone();
            let _ = debug
                .session
                .register_state_provider("meters", move || meters.meters_snapshot());
        }
        Self {
            mirror,
            input32: vec![],
            output32: vec![],
            block_input32: vec![],
            block_output32: vec![],
            block_input64: vec![],
            block_output64: vec![],
            input64: vec![],
            output64: vec![],
            events: Vec::with_capacity(EVENT_CAPACITY),
            #[cfg(feature = "devtools")]
            debug,
            plugin: DropShield::new(plugin),
            retired_errors: RetiredErrors::default(),
            max_frames: 0,
            sample_clock: 0,
            sample_rate: 44100.,
            lost_events: 0,
            output_reset_pending: false,
        }
    }
    pub fn prepare(&mut self, sample_rate: f64, max_frames: usize) -> bool {
        self.retired_errors.collect_on_control_thread();
        let d = P::descriptor();
        let ins: usize = d.inputs.iter().map(|b| b.channels as usize).sum();
        let outs: usize = d.outputs.iter().map(|b| b.channels as usize).sum();
        if d.validate().is_err()
            || !sample_rate.is_finite()
            || sample_rate <= 0.
            || max_frames == 0
            || max_frames > 1_048_576
        {
            return false;
        }
        if ins
            .max(outs)
            .checked_mul(max_frames)
            .is_none_or(|n| n > 16_777_216)
        {
            return false;
        }
        if self
            .plugin
            .prepare(ProcessConfig {
                sample_rate,
                max_frames,
                input_channels: ins,
                output_channels: outs,
            })
            .is_err()
        {
            return false;
        }
        self.input32.resize(ins, 0.);
        self.output32.resize(outs, 0.);
        self.block_input32.resize(ins * max_frames, 0.);
        self.block_output32.resize(outs * max_frames, 0.);
        if d.supports_f64 {
            self.input64.resize(ins, 0.);
            self.output64.resize(outs, 0.);
            self.block_input64.resize(ins * max_frames, 0.);
            self.block_output64.resize(outs * max_frames, 0.);
        }
        self.sample_rate = sample_rate;
        self.max_frames = max_frames;
        self.publish();
        self.debug_lifecycle("Native plugin prepared");
        true
    }
    pub fn debug_lifecycle(&self, message: &str) {
        #[cfg(feature = "devtools")]
        if let Some(d) = &self.debug {
            d.session
                .log(rvst_devtools::Level::Info, "adapter", message);
            d.session.set_snapshot(serde_json::json!({"descriptor":P::descriptor(),"sample_rate":self.sample_rate,"max_frames":self.max_frames}));
        }
        #[cfg(not(feature = "devtools"))]
        let _ = message;
    }
    pub fn debug_event(&mut self, code: u32, value: f64) {
        #[cfg(feature = "devtools")]
        if let Some(d) = &mut self.debug {
            d.logger.emit(code, d.clock, value);
        }
        #[cfg(not(feature = "devtools"))]
        let _ = (code, value);
    }
    #[cfg(feature = "devtools")]
    pub fn debug_callback(&mut self, frames: usize, start: std::time::Instant) {
        if let Some(d) = &mut self.debug {
            let elapsed = start.elapsed().as_nanos().min(u64::MAX as u128) as u64;
            d.metrics
                .record_callback(elapsed, (frames as f64 / self.sample_rate * 1e9) as u64);
            d.logger.emit(1, d.clock, elapsed as f64);
            d.clock = d.clock.saturating_add(frames as u64);
        }
    }
    pub fn publish(&self) {
        for p in P::descriptor().parameters {
            if let Some(v) = self.plugin.parameter(p.id) {
                self.mirror.publish(p.id, v)
            }
        }
    }
    pub fn edits(&mut self) {
        for i in 0..P::descriptor().parameters.len() {
            if let Some((id, value)) = self.mirror.take_edit(i) {
                self.set_parameter_rt(id, value);
            }
        }
    }
    /// Apply a plain parameter value without dropping a rejected heap-owned error.
    pub fn set_parameter_rt(&mut self, id: u32, value: f64) -> bool {
        let Some(spec) = P::descriptor().parameter(id) else {
            return false;
        };
        let ok = match self.plugin.set_parameter(id, spec.clamp(value)) {
            Ok(()) => true,
            Err(error) => {
                self.retired_errors.retain(error);
                false
            }
        };
        if let Some(current) = self.plugin.parameter(id) {
            self.mirror.publish(id, current);
        }
        ok
    }
    pub fn push(&mut self, mut e: Event) {
        e.frame = 0;
        if let EventKind::Parameter { id, value } = e.kind {
            let Some(spec) = P::descriptor().parameter(id) else {
                return;
            };
            let value = spec.clamp(value);
            e.kind = EventKind::Parameter { id, value };
            if !self.set_parameter_rt(id, value) {
                return;
            }
        }
        if self.events.len() < EVENT_CAPACITY {
            self.events.push(e)
        } else {
            self.lost_events = self.lost_events.saturating_add(1);
            // Collapse overloaded sample to an emergency release. No note-off can leave a stuck voice.
            self.events.clear();
            self.events.push(Event {
                frame: 0,
                kind: EventKind::AllNotesOff,
            });
            self.plugin.reset();
        }
    }
    pub fn bypassed(&self) -> bool {
        P::descriptor().parameters.iter().any(|p| {
            p.bypass
                && self
                    .plugin
                    .parameter(p.id)
                    .is_some_and(|v| v >= (p.min + p.max) / 2.)
        })
    }
    pub fn debug_url(&self) -> Option<String> {
        self.mirror.inspector_url.clone()
    }
    fn meter32(&mut self, frames: usize, block: bool) {
        let (input, output) = if block {
            (
                &self.block_input32[..self.input32.len() * frames],
                &self.block_output32[..self.output32.len() * frames],
            )
        } else {
            (&self.input32[..], &self.output32[..])
        };
        let peak = |x: &[f32]| x.iter().fold(0f32, |a, b| a.max(b.abs()));
        self.sample_clock = self.sample_clock.saturating_add(frames as u64);
        self.mirror
            .meters
            .publish(peak(input), peak(output), self.sample_clock);
    }
    fn meter64(&mut self, frames: usize, block: bool) {
        let (input, output) = if block {
            (
                &self.block_input64[..self.input64.len() * frames],
                &self.block_output64[..self.output64.len() * frames],
            )
        } else {
            (&self.input64[..], &self.output64[..])
        };
        let peak = |x: &[f64]| x.iter().fold(0f64, |a, b| a.max(b.abs())) as f32;
        self.sample_clock = self.sample_clock.saturating_add(frames as u64);
        self.mirror
            .meters
            .publish(peak(input), peak(output), self.sample_clock);
    }
    pub fn block32(
        &mut self,
        frames: usize,
        t: &Transport,
        sink: &mut dyn EventSink,
    ) -> ProcessStatus {
        let ins = self.input32.len();
        let outs = self.output32.len();
        let input = &self.block_input32[..ins * frames];
        let output = &mut self.block_output32[..outs * frames];
        output.fill(0.);
        let mut b = AudioBlock::new(input, output, frames, ins, outs).expect("prepared dimensions");
        let status = self.plugin.process(&mut b, &[], t, sink);
        if self.bypassed() {
            for channel in 0..outs {
                let output = &mut self.block_output32[channel * frames..(channel + 1) * frames];
                if channel < ins {
                    output.copy_from_slice(
                        &self.block_input32[channel * frames..(channel + 1) * frames],
                    );
                } else {
                    output.fill(0.);
                }
            }
        }
        self.meter32(frames, true);
        status
    }
    pub fn block64(
        &mut self,
        frames: usize,
        t: &Transport,
        sink: &mut dyn EventSink,
    ) -> ProcessStatus {
        let ins = self.input64.len();
        let outs = self.output64.len();
        let input = &self.block_input64[..ins * frames];
        let output = &mut self.block_output64[..outs * frames];
        output.fill(0.);
        let mut b = AudioBlock::new(input, output, frames, ins, outs).expect("prepared dimensions");
        let status = self.plugin.process_f64(&mut b, &[], t, sink);
        if self.bypassed() {
            for channel in 0..outs {
                let output = &mut self.block_output64[channel * frames..(channel + 1) * frames];
                if channel < ins {
                    output.copy_from_slice(
                        &self.block_input64[channel * frames..(channel + 1) * frames],
                    );
                } else {
                    output.fill(0.);
                }
            }
        }
        self.meter64(frames, true);
        status
    }
    pub fn tick32(&mut self, t: &Transport, sink: &mut dyn EventSink) -> ProcessStatus {
        self.output32.fill(0.);
        let mut b = AudioBlock::new(
            &self.input32,
            &mut self.output32,
            1,
            self.input32.len(),
            P::descriptor()
                .outputs
                .iter()
                .map(|b| b.channels as usize)
                .sum(),
        )
        .expect("prepared dimensions");
        let result = self.plugin.process(&mut b, &self.events, t, sink);
        self.events.clear();
        if self.bypassed() {
            for (i, o) in self.output32.iter_mut().enumerate() {
                *o = self.input32.get(i).copied().unwrap_or(0.);
            }
        }
        self.meter32(1, false);
        result
    }
    pub fn tick64(&mut self, t: &Transport, sink: &mut dyn EventSink) -> ProcessStatus {
        self.output64.fill(0.);
        let mut b = AudioBlock::new(
            &self.input64,
            &mut self.output64,
            1,
            self.input64.len(),
            P::descriptor()
                .outputs
                .iter()
                .map(|b| b.channels as usize)
                .sum(),
        )
        .expect("prepared dimensions");
        let result = self.plugin.process_f64(&mut b, &self.events, t, sink);
        self.events.clear();
        if self.bypassed() {
            for (i, o) in self.output64.iter_mut().enumerate() {
                *o = self.input64.get(i).copied().unwrap_or(0.);
            }
        }
        self.meter64(1, false);
        result
    }
}
impl<P: Plugin> Default for Runtime<P> {
    fn default() -> Self {
        Self::new()
    }
}
pub fn tail(t: Tail) -> u32 {
    match t {
        Tail::None => 0,
        Tail::Samples(n) => n.min((u32::MAX - 1) as u64) as u32,
        Tail::Infinite => u32::MAX,
    }
}
pub fn midi(data: [u8; 3]) -> Option<EventKind> {
    let c = data[0] & 15;
    let k = data[1] & 127;
    let v = data[2] & 127;
    Some(match data[0] & 240 {
        0x90 if v > 0 => EventKind::NoteOn {
            channel: c,
            key: k,
            velocity: v as f64 / 127.,
            note_id: -1,
        },
        0x80 | 0x90 => EventKind::NoteOff {
            channel: c,
            key: k,
            velocity: v as f64 / 127.,
            note_id: -1,
        },
        0xa0 => EventKind::PolyPressure {
            channel: c,
            key: k,
            pressure: v as f64 / 127.,
            note_id: -1,
        },
        0xb0 if k == 123 || k == 120 => EventKind::NoteRelease {
            channel: Some(c),
            key: None,
            note_id: None,
            velocity: 0.,
            choke: k == 120,
        },
        0xb0 => EventKind::ControlChange {
            channel: c,
            controller: k,
            value: v as f64 / 127.,
        },
        0xd0 => EventKind::ChannelPressure {
            channel: c,
            pressure: k as f64 / 127.,
        },
        0xe0 => EventKind::PitchBend {
            channel: c,
            value: (((v as u16) << 7) | k as u16) as f64 / 8192. - 1.,
        },
        _ => return None,
    })
}

/// A nonblocking audio/control ownership gate. Audio never waits: during a
/// control operation the native adapter silences that callback. State IO uses
/// bounded waiting only on the calling control thread.
#[derive(Default)]
pub struct AccessGate(std::sync::atomic::AtomicU8);
pub struct AccessGuard<'a>(&'a AccessGate);
impl AccessGate {
    pub fn audio(&self) -> Option<AccessGuard<'_>> {
        self.0
            .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| AccessGuard(self))
    }
    pub fn control(&self) -> Option<AccessGuard<'_>> {
        let end = std::time::Instant::now() + std::time::Duration::from_millis(100);
        loop {
            if self
                .0
                .compare_exchange(0, 2, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                return Some(AccessGuard(self));
            }
            if std::time::Instant::now() >= end {
                return None;
            }
            std::thread::yield_now();
        }
    }
}
impl Drop for AccessGuard<'_> {
    fn drop(&mut self) {
        self.0.0.store(0, Ordering::Release);
    }
}

/// Versioned parameter preset; intentionally distinct from opaque DAW state.
pub fn save_parameter_preset(mirror: &ParameterMirror) -> Result<Vec<u8>, Error> {
    serde_json::to_vec(&serde_json::json!({"rvst_parameter_preset":1,"plugin_id":mirror.descriptor.id,"values":mirror.descriptor.parameters.iter().map(|p|(p.id,mirror.get(p.id).unwrap_or(p.default))).collect::<Vec<_>>()})).map_err(|e|Error(e.to_string()))
}
pub fn parse_parameter_preset(
    descriptor: &PluginDescriptor,
    data: &[u8],
) -> Result<Vec<(u32, f64)>, Error> {
    if data.len() > 1024 * 1024 {
        return Err("parameter preset too large".into());
    }
    let v: serde_json::Value = serde_json::from_slice(data).map_err(|e| Error(e.to_string()))?;
    if v["rvst_parameter_preset"] != 1 || v["plugin_id"] != descriptor.id {
        return Err("parameter preset belongs to another plugin or version".into());
    }
    let values: Vec<(u32, f64)> =
        serde_json::from_value(v["values"].clone()).map_err(|e| Error(e.to_string()))?;
    if values.len() != descriptor.parameters.len() {
        return Err("incomplete parameter preset".into());
    }
    for (index, (id, value)) in values.iter().enumerate() {
        let Some(p) = descriptor.parameter(*id) else {
            return Err("unknown parameter in preset".into());
        };
        if !value.is_finite()
            || *value < p.min
            || *value > p.max
            || values[..index].iter().any(|(prior, _)| prior == id)
        {
            return Err("invalid parameter preset".into());
        }
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;
    mod fixture {
        include!("../test-support/plugin.rs");
    }
    #[global_allocator]
    static ALLOC: rvst_test::GuardedAllocator<std::alloc::System> =
        rvst_test::GuardedAllocator(std::alloc::System);
    #[test]
    fn history_survives_editor_reopen_and_automation_but_is_per_instance() {
        let mirror = ParameterMirror::new(fixture::Gain::descriptor());
        let mut editor = HistoryHost::new(MirrorEditor(mirror.clone()));
        editor.begin_edit(42);
        editor.set_parameter(42, 0.5);
        editor.end_edit(42);
        assert!(editor.undo_label().is_some());
        drop(editor);

        assert_eq!(mirror.take_edit(0), Some((42, 0.5)));
        mirror.publish(42, 0.75);
        let mut reopened = HistoryHost::new(MirrorEditor(mirror.clone()));
        assert!(reopened.undo_label().is_some());
        assert!(reopened.undo().unwrap());
        assert_eq!(mirror.get(42), Some(1.));
        assert!(!reopened.undo().unwrap());
        assert!(reopened.redo_label().is_some());
        mirror.publish(42, 1.5);
        assert!(reopened.redo_label().is_some());
        let other = HistoryHost::new(MirrorEditor(ParameterMirror::new(
            fixture::Gain::descriptor(),
        )));
        assert!(other.undo_label().is_none());
    }
    #[test]
    fn restored_state_discards_pending_and_in_flight_old_edits_without_allocation() {
        let mirror = ParameterMirror::new(fixture::Gain::descriptor());
        let before_restore = mirror.generation.load(Ordering::Acquire);
        mirror.edit(42, 0.25);
        let guard = rvst_test::AllocationGuard::start();
        mirror.state_restored();
        mirror.publish(42, 0.75);
        let restored = mirror.get(42);
        // The UI may have read the old generation immediately before the restore.
        mirror.queue_edit(0, 0.5, before_restore);
        let pending = mirror.take_edit(0);
        let current = mirror.get(42);
        let counts = guard.finish();
        assert_eq!(counts, rvst_test::AllocationCounts::default());
        assert_eq!(restored, Some(0.75));
        assert_eq!(pending, None);
        assert_eq!(current, Some(0.75));
        mirror.edit(42, 1.25);
        assert_eq!(mirror.get(42), Some(1.25));
        assert_eq!(mirror.take_edit(0), Some((42, 1.25)));
    }
    #[test]
    fn retry_preserves_a_newer_pending_edit() {
        let mirror = ParameterMirror::new(fixture::Gain::descriptor());
        mirror.edit(42, 0.25);
        assert_eq!(mirror.take_edit(0), Some((42, 0.25)));
        mirror.edit(42, 0.75);
        mirror.retry_edit(0);
        assert_eq!(mirror.take_edit(0), Some((42, 0.75)));
    }
    #[test]
    fn history_readback_keeps_target_during_audio_handoff() {
        struct HandoffEditor(MirrorEditor);
        impl EditorHost for HandoffEditor {
            fn edit_history(&self) -> Option<EditHistory> {
                self.0.edit_history()
            }
            fn descriptor(&self) -> &'static PluginDescriptor {
                self.0.descriptor()
            }
            fn parameter(&self, id: u32) -> Option<f64> {
                // Force the audio consumer to take a value immediately before
                // HistoryHost reads back the accepted edit, before DSP applies it.
                let _ = self.0.0.take_edit(0);
                self.0.parameter(id)
            }
            fn begin_edit(&mut self, id: u32) {
                self.0.begin_edit(id);
            }
            fn set_parameter(&mut self, id: u32, value: f64) {
                self.0.set_parameter(id, value);
            }
            fn end_edit(&mut self, id: u32) {
                self.0.end_edit(id);
            }
            fn save_state(&mut self) -> Result<Vec<u8>, Error> {
                self.0.save_state()
            }
            fn load_state(&mut self, data: &[u8]) -> Result<(), Error> {
                self.0.load_state(data)
            }
        }
        let mirror = ParameterMirror::new(fixture::Gain::descriptor());
        let mut editor = HistoryHost::new(HandoffEditor(MirrorEditor(mirror.clone())));
        editor.begin_edit(42);
        editor.set_parameter(42, 0.25);
        editor.end_edit(42);
        assert_eq!(mirror.get(42), Some(0.25));
        assert!(editor.undo_label().is_some());
        mirror.publish(42, 0.25);
        assert!(editor.undo().unwrap());
        assert_eq!(mirror.get(42), Some(1.));
    }
    #[test]
    fn deferred_host_target_waits_for_explicit_ack_and_can_be_superseded() {
        let mirror = ParameterMirror::new(fixture::Gain::descriptor());
        let token = mirror.defer_host_edit(42, 0.25, mirror.state_generation());
        assert_eq!(mirror.take_edit(0), None);
        mirror.publish(42, 1.);
        assert_eq!(mirror.get(42), Some(0.25));
        mirror.acknowledge_host_edit(42, 0.75, mirror.host_edit_token(42));
        assert_eq!(mirror.get(42), Some(0.25));
        mirror.host_edit_dispatched(42, token);
        mirror.acknowledge_host_edit(42, 0.75, mirror.host_edit_token(42));
        assert_eq!(mirror.get(42), Some(0.75));
        mirror.publish(42, 0.5);
        assert_eq!(mirror.get(42), Some(0.5));
        let old = mirror.defer_host_edit(42, 0.25, mirror.state_generation());
        let new = mirror.defer_host_edit(42, 1.5, mirror.state_generation());
        mirror.host_edit_dispatched(42, old);
        mirror.acknowledge_host_edit(42, 0.25, mirror.host_edit_token(42));
        assert_eq!(mirror.get(42), Some(1.5));
        mirror.host_edit_dispatched(42, new);
        mirror.acknowledge_host_edit(42, 1.5, mirror.host_edit_token(42));
        assert_eq!(mirror.get(42), Some(1.5));
        let old = mirror.defer_host_edit(42, 0.25, mirror.state_generation());
        mirror.host_edit_dispatched(42, old);
        let received = mirror.host_edit_token(42);
        let newer = mirror.defer_host_edit(42, 1.75, mirror.state_generation());
        mirror.host_edit_dispatched(42, newer);
        mirror.acknowledge_host_edit(42, 0.25, received);
        assert_eq!(mirror.get(42), Some(1.75));
        mirror.acknowledge_host_edit(42, 0.5, mirror.host_edit_token(42));
        assert_eq!(mirror.get(42), Some(0.5));
    }
    #[test]
    fn overflow_releases_notes_without_heap_activity() {
        let mut rt = Runtime::<fixture::Gain>::new();
        assert!(rt.prepare(48000., 16));
        let guard = rvst_test::AllocationGuard::start();
        for _ in 0..EVENT_CAPACITY + 1 {
            rt.push(Event {
                frame: 0,
                kind: EventKind::NoteOff {
                    channel: 0,
                    key: 60,
                    velocity: 0.,
                    note_id: 1,
                },
            });
        }
        let counts = guard.finish();
        assert_eq!(counts, rvst_test::AllocationCounts::default());
        assert_eq!(rt.lost_events, 1);
        assert_eq!(rt.events.len(), 1);
        assert_eq!(rt.events[0].kind, EventKind::AllNotesOff);
    }
    #[test]
    fn preset_validation_is_all_or_nothing() {
        let mirror = ParameterMirror::new(fixture::Gain::descriptor());
        let saved = save_parameter_preset(&mirror).unwrap();
        assert_eq!(
            parse_parameter_preset(mirror.descriptor, &saved).unwrap(),
            vec![(42, 1.)]
        );
        assert!(
            parse_parameter_preset(
                mirror.descriptor,
                br#"{"rvst_parameter_preset":1,"plugin_id":"org.rvst.test","values":[[42,99.0]]}"#
            )
            .is_err()
        );
        assert_eq!(mirror.get(42), Some(1.));
    }
    #[test]
    fn control_and_audio_ownership_never_overlap() {
        let gate = Arc::new(AccessGate::default());
        let mut tasks = Vec::new();
        let value = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        for _ in 0..2 {
            let gate = gate.clone();
            let value = value.clone();
            tasks.push(std::thread::spawn(move || {
                for _ in 0..1000 {
                    if let Some(_guard) = gate.audio() {
                        assert_eq!(value.fetch_add(1, Ordering::SeqCst), 0);
                        assert_eq!(value.fetch_sub(1, Ordering::SeqCst), 1);
                    }
                }
            }));
        }
        for _ in 0..100 {
            let _guard = gate.control().unwrap();
            assert_eq!(value.fetch_add(1, Ordering::SeqCst), 0);
            assert_eq!(value.fetch_sub(1, Ordering::SeqCst), 1);
        }
        for t in tasks {
            t.join().unwrap();
        }
    }
}

/// Own a plugin whose destructor must never unwind through a C/COM release.
pub struct DropShield<T>(std::mem::ManuallyDrop<T>);
impl<T> DropShield<T> {
    pub fn new(value: T) -> Self {
        Self(std::mem::ManuallyDrop::new(value))
    }
}
impl<T> std::ops::Deref for DropShield<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}
impl<T> std::ops::DerefMut for DropShield<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}
impl<T> Drop for DropShield<T> {
    fn drop(&mut self) {
        boundary((), || unsafe { std::mem::ManuallyDrop::drop(&mut self.0) });
    }
}

#[cfg(test)]
mod regression_tests {
    use super::*;
    mod fixture {
        include!("../test-support/plugin.rs");
    }
    struct Reject {
        inner: fixture::Gain,
        error: Option<Error>,
    }
    impl Plugin for Reject {
        fn descriptor() -> &'static PluginDescriptor {
            fixture::Gain::descriptor()
        }
        fn new() -> Self {
            Self {
                inner: fixture::Gain::new(),
                error: Some(Error("preallocated rejection".into())),
            }
        }
        fn prepare(&mut self, c: ProcessConfig) -> Result<(), Error> {
            self.inner.prepare(c)
        }
        fn process(
            &mut self,
            b: &mut AudioBlock<'_, f32>,
            e: &[Event],
            t: &Transport,
            s: &mut dyn EventSink,
        ) -> ProcessStatus {
            self.inner.process(b, e, t, s)
        }
        fn reset(&mut self) {}
        fn parameter(&self, id: u32) -> Option<f64> {
            self.inner.parameter(id)
        }
        fn set_parameter(&mut self, _: u32, _: f64) -> Result<(), Error> {
            Err(self.error.take().unwrap())
        }
        fn save_state(&self) -> Result<Vec<u8>, Error> {
            self.inner.save_state()
        }
        fn load_state(&mut self, b: &[u8]) -> Result<(), Error> {
            self.inner.load_state(b)
        }
    }
    #[test]
    fn pending_editor_value_survives_audio_publication() {
        let mut rt = Runtime::<fixture::Gain>::new();
        rt.mirror.edit(42, 1.5);
        rt.mirror.publish(42, 0.25);
        rt.edits();
        assert_eq!(rt.plugin.parameter(42), Some(1.5));
    }
    #[test]
    fn rejected_editor_value_is_retired_without_audio_heap_activity() {
        let mut rt = Runtime::<Reject>::new();
        assert!(rt.prepare(48000., 16));
        rt.mirror.edit(42, 1.5);
        let guard = rvst_test::AllocationGuard::start();
        rt.edits();
        let counts = guard.finish();
        assert_eq!(counts, rvst_test::AllocationCounts::default());
        assert_eq!(rt.mirror.get(42), Some(1.0));
    }
}

#[cfg(test)]
mod retirement_tests {
    use super::*;
    #[test]
    fn bounded_error_retirement_never_drops_on_audio_and_reclaims_on_control() {
        let mut retired = RetiredErrors::default();
        let mut errors: Vec<_> = (0..65)
            .map(|_| Some(Error("preallocated error".into())))
            .collect();
        let guard = rvst_test::AllocationGuard::start();
        for error in &mut errors {
            retired.retain(error.take().unwrap());
        }
        let counts = guard.finish();
        assert_eq!(counts, rvst_test::AllocationCounts::default());
        assert_eq!(retired.overflow_count(), 1);
        let guard = rvst_test::AllocationGuard::start();
        retired.collect_on_control_thread();
        let counts = guard.finish();
        assert_eq!(counts.deallocations, 64);
    }
}

#[cfg(all(test, feature = "devtools"))]
mod inspector_tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpStream,
    };
    mod fixture {
        include!("../test-support/plugin.rs");
    }
    fn inspect(rt: &Runtime<fixture::Gain>) -> serde_json::Value {
        let session = &rt.debug.as_ref().expect("inspector started").session;
        let info = session.info();
        let token = info.url.split_once('#').unwrap().1;
        let mut stream = TcpStream::connect(info.address).unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        write!(stream, "GET /api/events HTTP/1.1\r\nHost: {}\r\nX-RVST-Token: {token}\r\nConnection: close\r\n\r\n", info.address).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap()
    }
    #[test]
    fn inspector_follows_parameter_and_meter_mirrors_without_audio_allocation() {
        let mut rt = Runtime::<fixture::Gain>::new();
        assert!(rt.prepare(48000., 16));
        let before = inspect(&rt);
        assert_eq!(before["snapshot"]["live"]["parameters"][0]["value"], 1.0);
        let guard = rvst_test::AllocationGuard::start();
        assert!(rt.set_parameter_rt(42, 1.5));
        rt.mirror.meters.publish(0.25, 0.5, 128);
        rt.mirror.latency.store(64, Ordering::Release);
        let counts = guard.finish();
        assert_eq!(counts, rvst_test::AllocationCounts::default());
        let after = inspect(&rt);
        assert_eq!(after["snapshot"]["live"]["parameters"][0]["value"], 1.5);
        assert_eq!(after["snapshot"]["live"]["meters"]["sample_clock"], 128);
        assert_eq!(after["snapshot"]["live"]["meters"]["output_peak"], 0.5);
        assert_eq!(after["snapshot"]["live"]["meters"]["latency_samples"], 64);
        rt.mirror.edit(42, 1.75);
        let pending = inspect(&rt);
        assert_eq!(pending["snapshot"]["live"]["parameters"][0]["value"], 1.5);
        assert_eq!(
            pending["snapshot"]["live"]["parameters"][0]["pending"],
            1.75
        );
        rt.edits();
        let applied = inspect(&rt);
        assert_eq!(applied["snapshot"]["live"]["parameters"][0]["value"], 1.75);
        assert!(applied["snapshot"]["live"]["parameters"][0]["pending"].is_null());
    }
}
