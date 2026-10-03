use rvst_core::{
    AudioBlock, Error, Event, EventSink, Plugin, ProcessConfig, ProcessStatus, Transport,
};
use serde::{Deserialize, Serialize};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Precision {
    F32,
    F64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordedBlock {
    pub input: Vec<f64>,
    pub output: Vec<f64>,
    pub frames: usize,
    pub events: Vec<Event>,
    pub transport: Transport,
    pub precision: Precision,
    #[serde(default)]
    pub output_events: Option<Vec<Event>>,
    #[serde(default)]
    pub status: Option<ProcessStatus>,
}
struct Slot {
    input: Vec<f64>,
    output: Vec<f64>,
    frames: usize,
    events: Vec<Event>,
    transport: Transport,
    precision: Precision,
    output_events: Vec<Event>,
    status: Option<ProcessStatus>,
}
impl Slot {
    fn new(samples: usize, events: usize) -> Self {
        Self {
            input: Vec::with_capacity(samples),
            output: Vec::with_capacity(samples),
            frames: 0,
            events: Vec::with_capacity(events),
            transport: Transport::default(),
            precision: Precision::F32,
            output_events: Vec::with_capacity(events),
            status: None,
        }
    }
}
pub struct RtRecorder {
    ready: rtrb::Producer<Slot>,
    free: rtrb::Consumer<Slot>,
    pending: Option<Slot>,
    lost: Arc<AtomicU64>,
}
pub struct RecordingDrain {
    ready: rtrb::Consumer<Slot>,
    free: rtrb::Producer<Slot>,
    lost: Arc<AtomicU64>,
}
/// Preallocate every slot and event buffer. `max_samples` covers each input/output
/// planar array. Captures beyond either bound are rejected and counted.
pub fn recording_channel(
    slots: usize,
    max_samples: usize,
    max_events: usize,
) -> (RtRecorder, RecordingDrain) {
    let slots = slots.max(1);
    let (ready_tx, ready_rx) = rtrb::RingBuffer::new(slots);
    let (mut free_tx, free_rx) = rtrb::RingBuffer::new(slots);
    for _ in 0..slots {
        assert!(free_tx.push(Slot::new(max_samples, max_events)).is_ok());
    }
    let lost = Arc::new(AtomicU64::new(0));
    (
        RtRecorder {
            ready: ready_tx,
            free: free_rx,
            pending: None,
            lost: lost.clone(),
        },
        RecordingDrain {
            ready: ready_rx,
            free: free_tx,
            lost,
        },
    )
}
impl RtRecorder {
    pub fn capture_f32(
        &mut self,
        input: &[f32],
        output: &[f32],
        frames: usize,
        events: &[Event],
        transport: Transport,
    ) -> bool {
        self.capture(
            input,
            output,
            frames,
            events,
            transport,
            Precision::F32,
            None,
        )
    }
    pub fn capture_f64(
        &mut self,
        input: &[f64],
        output: &[f64],
        frames: usize,
        events: &[Event],
        transport: Transport,
    ) -> bool {
        self.capture(
            input,
            output,
            frames,
            events,
            transport,
            Precision::F64,
            None,
        )
    }
    /// Record the complete observed callback, including output events and status.
    /// The output event slice must contain every event; if its sink overflowed,
    /// call mark_incomplete so replay will refuse deterministic verification.
    pub fn capture_block_f32(
        &mut self,
        block: &AudioBlock<'_, f32>,
        events: &[Event],
        transport: Transport,
        output_events: &[Event],
        status: ProcessStatus,
    ) -> bool {
        self.capture(
            block.inputs,
            block.outputs,
            block.frames,
            events,
            transport,
            Precision::F32,
            Some((output_events, status)),
        )
    }
    pub fn capture_block_f64(
        &mut self,
        block: &AudioBlock<'_, f64>,
        events: &[Event],
        transport: Transport,
        output_events: &[Event],
        status: ProcessStatus,
    ) -> bool {
        self.capture(
            block.inputs,
            block.outputs,
            block.frames,
            events,
            transport,
            Precision::F64,
            Some((output_events, status)),
        )
    }
    pub fn mark_incomplete(&self) {
        self.lost.fetch_add(1, Ordering::Relaxed);
    }
    #[allow(clippy::too_many_arguments)]
    fn capture<T: Copy + Into<f64>>(
        &mut self,
        input: &[T],
        output: &[T],
        frames: usize,
        events: &[Event],
        transport: Transport,
        precision: Precision,
        observation: Option<(&[Event], ProcessStatus)>,
    ) -> bool {
        let Some(mut slot) = self.pending.take().or_else(|| self.free.pop().ok()) else {
            self.lost.fetch_add(1, Ordering::Relaxed);
            return false;
        };
        if input.len() > slot.input.capacity()
            || output.len() > slot.output.capacity()
            || events.len() > slot.events.capacity()
            || observation.is_some_and(|(events, _)| events.len() > slot.output_events.capacity())
        {
            self.pending = Some(slot);
            self.lost.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        slot.input.clear();
        slot.output.clear();
        slot.events.clear();
        slot.output_events.clear();
        slot.status = None;
        slot.input.extend(input.iter().copied().map(Into::into));
        slot.output.extend(output.iter().copied().map(Into::into));
        slot.events.extend_from_slice(events);
        slot.frames = frames;
        slot.transport = transport;
        slot.precision = precision;
        if let Some((events, status)) = observation {
            slot.output_events.extend_from_slice(events);
            slot.status = Some(status);
        }
        match self.ready.push(slot) {
            Ok(()) => true,
            Err(rtrb::PushError::Full(slot)) => {
                self.pending = Some(slot);
                self.lost.fetch_add(1, Ordering::Relaxed);
                false
            }
        }
    }
}
impl RecordingDrain {
    /// Allocates the persisted block off-thread, then returns the slot to its pool.
    pub fn pop(&mut self) -> Option<RecordedBlock> {
        let slot = self.ready.pop().ok()?;
        let record = RecordedBlock {
            input: slot.input.clone(),
            output: slot.output.clone(),
            frames: slot.frames,
            events: slot.events.clone(),
            transport: slot.transport,
            precision: slot.precision,
            output_events: slot.status.map(|_| slot.output_events.clone()),
            status: slot.status,
        };
        // With equal pool/ready capacities, removing a ready slot guarantees room.
        assert!(self.free.push(slot).is_ok());
        Some(record)
    }
    pub fn lost_blocks(&self) -> u64 {
        self.lost.load(Ordering::Relaxed)
    }
}
/// How the processor reaches the captured initial condition.
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CaptureStart {
    #[default]
    Reset,
    Checkpoint,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recording {
    #[serde(default)]
    pub start: CaptureStart,
    pub schema_version: u32,
    pub plugin_id: String,
    pub config: ProcessConfig,
    pub initial_state: Vec<u8>,
    /// True only when recording started directly after prepare/load/reset, or a
    /// plugin-specific complete DSP checkpoint. Persistent parameter state alone
    /// cannot reproduce delay lines from an arbitrary live session.
    pub reproducible_start: bool,
    pub lost_blocks: u64,
    pub blocks: Vec<RecordedBlock>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayReport {
    pub blocks: usize,
    pub compared_samples: usize,
    pub compared_output_events: usize,
    pub max_absolute_error: f64,
    pub matches: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockReplay {
    pub index: usize,
    pub compared_samples: usize,
    pub compared_output_events: usize,
    pub max_absolute_error: f64,
    pub audio_matches: bool,
    pub events_match: bool,
    pub status_matches: bool,
}
/// An offline, stateful block stepper. Each call processes exactly one captured
/// boundary. No processing occurs on the browser or audio thread.
pub struct ReplayCursor<'a, P: Plugin> {
    recording: &'a Recording,
    plugin: P,
    next: usize,
    tolerance: f64,
}
impl Recording {
    pub fn cursor<P: Plugin>(&self, tolerance: f64) -> Result<ReplayCursor<'_, P>, Error> {
        if self.schema_version != 1 || self.plugin_id != P::descriptor().id {
            return Err("recording schema or plugin identity mismatch".into());
        }
        if !self.reproducible_start || self.lost_blocks != 0 {
            return Err("recording is incomplete or has no reproducible start".into());
        }
        if !tolerance.is_finite() || tolerance < 0.0 {
            return Err("invalid replay tolerance".into());
        }
        self.config.validate()?;
        P::descriptor().validate()?;
        let mut plugin = P::new();
        plugin.prepare(self.config)?;
        plugin.load_state(&self.initial_state)?;
        if self.start == CaptureStart::Reset {
            plugin.reset();
        }
        Ok(ReplayCursor {
            recording: self,
            plugin,
            next: 0,
            tolerance,
        })
    }
    pub fn replay<P: Plugin>(&self, tolerance: f64) -> Result<ReplayReport, Error> {
        let mut cursor = self.cursor::<P>(tolerance)?;
        let mut report = ReplayReport {
            blocks: 0,
            compared_samples: 0,
            compared_output_events: 0,
            max_absolute_error: 0.,
            matches: true,
        };
        while let Some(block) = cursor.step()? {
            report.blocks += 1;
            report.compared_samples += block.compared_samples;
            report.compared_output_events += block.compared_output_events;
            report.max_absolute_error = report.max_absolute_error.max(block.max_absolute_error);
            report.matches &= block.audio_matches && block.events_match && block.status_matches;
        }
        Ok(report)
    }
}
struct ReplayEvents {
    events: Vec<Event>,
    overflow: bool,
}
impl EventSink for ReplayEvents {
    fn push(&mut self, e: Event) -> bool {
        if self.events.len() == self.events.capacity() {
            self.overflow = true;
            false
        } else {
            self.events.push(e);
            true
        }
    }
}
impl<P: Plugin> ReplayCursor<'_, P> {
    pub fn position(&self) -> usize {
        self.next
    }
    pub fn step(&mut self) -> Result<Option<BlockReplay>, Error> {
        let Some(block) = self.recording.blocks.get(self.next) else {
            return Ok(None);
        };
        let config = self.recording.config;
        if block.status.is_none() {
            return Err("recording does not include processing status".into());
        }
        if block.frames > config.max_frames
            || block.input.len()
                != block
                    .frames
                    .checked_mul(config.input_channels)
                    .ok_or_else(|| Error::from("recording shape overflow"))?
            || block.output.len()
                != block
                    .frames
                    .checked_mul(config.output_channels)
                    .ok_or_else(|| Error::from("recording shape overflow"))?
        {
            return Err("invalid recorded block shape".into());
        }
        if block
            .events
            .iter()
            .any(|e| e.frame as usize >= block.frames)
            || block.events.windows(2).any(|e| e[0].frame > e[1].frame)
        {
            return Err("invalid recorded event timing".into());
        }
        if P::descriptor().midi_output && block.output_events.is_none() {
            return Err("recording does not include musical output events".into());
        }
        let expected = block.output_events.as_deref().unwrap_or(&[]);
        let mut sink = ReplayEvents {
            events: Vec::with_capacity(expected.len().saturating_add(1)),
            overflow: false,
        };
        let (actual, status): (Vec<f64>, ProcessStatus) = match block.precision {
            Precision::F32 => {
                let input: Vec<f32> = block.input.iter().map(|v| *v as f32).collect();
                let mut output = vec![0f32; block.output.len()];
                let mut audio = AudioBlock::new(
                    &input,
                    &mut output,
                    block.frames,
                    config.input_channels,
                    config.output_channels,
                )?;
                let status =
                    self.plugin
                        .process(&mut audio, &block.events, &block.transport, &mut sink);
                (output.into_iter().map(f64::from).collect(), status)
            }
            Precision::F64 => {
                if !P::descriptor().supports_f64 {
                    return Err("plugin does not support recorded precision".into());
                }
                let mut output = vec![0.; block.output.len()];
                let mut audio = AudioBlock::new(
                    &block.input,
                    &mut output,
                    block.frames,
                    config.input_channels,
                    config.output_channels,
                )?;
                let status =
                    self.plugin
                        .process_f64(&mut audio, &block.events, &block.transport, &mut sink);
                (output, status)
            }
        };
        if status == ProcessStatus::Error {
            return Err("plugin rejected replay block".into());
        }
        let mut max_error = 0f64;
        for (a, b) in actual.iter().zip(&block.output) {
            let error = (a - b).abs();
            if !error.is_finite() {
                return Err("non-finite replay output".into());
            }
            max_error = max_error.max(error);
        }
        let result = BlockReplay {
            index: self.next,
            compared_samples: actual.len(),
            compared_output_events: expected.len(),
            max_absolute_error: max_error,
            audio_matches: max_error <= self.tolerance,
            events_match: !sink.overflow && sink.events == expected,
            status_matches: block.status.is_none_or(|s| s == status),
        };
        self.next += 1;
        Ok(Some(result))
    }
}
