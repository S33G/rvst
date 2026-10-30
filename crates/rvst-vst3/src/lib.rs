//! Direct VST3 component, controller and COM factory.
#![allow(non_snake_case, unsafe_op_in_unsafe_fn)]
use rvst_adapter::{Runtime, boundary};
use rvst_core::{Event as CoreEvent, EventKind, Plugin, ProcessStatus, Transport};
pub use rvst_ui::EditorFn;
use std::{
    cell::{Cell, UnsafeCell},
    ffi::{c_char, c_void},
    marker::PhantomData,
    ptr,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
};
pub use vst3;
mod compat;
mod connection;
#[cfg(any(target_os = "linux", test))]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod dispatch;
mod timer;
use vst3::{
    Class, ComRef, ComWrapper,
    Steinberg::{Vst::*, *},
};
unsafe fn retain(p: *mut FUnknown) {
    boundary((), || {
        ((*(*p).vtbl).addRef)(p);
    })
}
unsafe fn release(p: *mut FUnknown) {
    boundary((), || {
        ((*(*p).vtbl).release)(p);
    })
}
fn narrow(s: &str, d: &mut [c_char]) {
    d.fill(0);
    let n = d.len().saturating_sub(1);
    for (a, b) in d.iter_mut().take(n).zip(s.bytes()) {
        *a = b as c_char;
    }
}
fn wide(s: &str, d: &mut [u16]) {
    d.fill(0);
    let n = d.len().saturating_sub(1);
    for (a, b) in d.iter_mut().take(n).zip(s.encode_utf16()) {
        *a = b;
    }
}
fn cid<P: Plugin>() -> TUID {
    compat::class_id(P::descriptor().vst3_id, cfg!(target_os = "windows"))
}
fn controller_cid<P: Plugin>() -> TUID {
    let mut c = cid::<P>();
    c[15] ^= 0x55;
    c
}
fn parameter<P: Plugin>(id: u32) -> Option<&'static rvst_core::ParameterSpec> {
    P::descriptor().parameters.iter().find(|p| p.id == id)
}
fn buses<P: Plugin>(dir: BusDirection) -> &'static [rvst_core::AudioBus] {
    if dir == BusDirections_::kInput as i32 {
        P::descriptor().inputs
    } else {
        P::descriptor().outputs
    }
}
fn arrangement(ch: u32) -> SpeakerArrangement {
    match ch {
        1 => SpeakerArr::kMono,
        2 => SpeakerArr::kStereo,
        0 => 0,
        n if n < 64 => (1u64 << n) - 1,
        _ => u64::MAX,
    }
}

trait NativeSample: Copy + Default {
    unsafe fn channels(bus: &AudioBusBuffers) -> *mut *mut Self;
}
impl NativeSample for f32 {
    unsafe fn channels(bus: &AudioBusBuffers) -> *mut *mut Self {
        unsafe { bus.__field0.channelBuffers32 }
    }
}
impl NativeSample for f64 {
    unsafe fn channels(bus: &AudioBusBuffers) -> *mut *mut Self {
        unsafe { bus.__field0.channelBuffers64 }
    }
}
unsafe fn valid_buses<T: NativeSample>(
    provided: &[AudioBusBuffers],
    declared: &[rvst_core::AudioBus],
    active: &[AtomicBool],
    input: bool,
) -> bool {
    for (index, spec) in declared.iter().enumerate() {
        if !active[index].load(Ordering::Acquire) {
            continue;
        }
        let Some(bus) = provided.get(index) else {
            return false;
        };
        if bus.numChannels != spec.channels as i32 {
            return false;
        }
        for channel in 0..spec.channels as usize {
            if input && bus.silenceFlags & (1u64 << channel) != 0 {
                continue;
            }
            let channels = T::channels(bus);
            if channels.is_null() || (*channels.add(channel)).is_null() {
                return false;
            }
        }
    }
    true
}
unsafe fn copy_inputs<T: NativeSample>(
    provided: &[AudioBusBuffers],
    declared: &[rvst_core::AudioBus],
    active: &[AtomicBool],
    target: &mut [T],
    frames: usize,
) {
    target.fill(T::default());
    let mut base = 0;
    for (index, spec) in declared.iter().enumerate() {
        if active[index].load(Ordering::Acquire)
            && let Some(bus) = provided.get(index)
        {
            for channel in 0..spec.channels as usize {
                if bus.silenceFlags & (1u64 << channel) == 0 {
                    ptr::copy_nonoverlapping(
                        *T::channels(bus).add(channel),
                        target.as_mut_ptr().add((base + channel) * frames),
                        frames,
                    );
                }
            }
        }
        base += spec.channels as usize;
    }
}
unsafe fn copy_outputs<T: NativeSample>(
    provided: &[AudioBusBuffers],
    declared: &[rvst_core::AudioBus],
    active: &[AtomicBool],
    source: &[T],
    frames: usize,
) {
    let mut base = 0;
    for (index, spec) in declared.iter().enumerate() {
        if active[index].load(Ordering::Acquire)
            && let Some(bus) = provided.get(index)
        {
            for channel in 0..spec.channels as usize {
                ptr::copy_nonoverlapping(
                    source.as_ptr().add((base + channel) * frames),
                    *T::channels(bus).add(channel),
                    frames,
                );
            }
        }
        base += spec.channels as usize;
    }
}

struct Processor<P: Plugin> {
    registration: connection::Registration,
    connection: connection::Endpoint,
    gate: rvst_adapter::AccessGate,
    runtime: UnsafeCell<Runtime<P>>,
    initial: UnsafeCell<Vec<f64>>,
    host_edit_tokens: UnsafeCell<Vec<u64>>,
    input_active: Vec<AtomicBool>,
    output_active: Vec<AtomicBool>,
    active: AtomicBool,
    processing: AtomicBool,
    missed_audio: AtomicBool,
    latency: AtomicU32,
    tail: AtomicU32,
}
impl<P: Plugin> Processor<P> {
    fn new() -> Self {
        let runtime = Runtime::new();
        let registration = connection::Registration::new(&runtime.mirror);
        Self {
            registration,
            connection: connection::Endpoint::default(),
            gate: Default::default(),
            runtime: UnsafeCell::new(runtime),
            initial: UnsafeCell::new(vec![0.; P::descriptor().parameters.len()]),
            host_edit_tokens: UnsafeCell::new(vec![0; P::descriptor().parameters.len()]),
            input_active: P::descriptor()
                .inputs
                .iter()
                .map(|_| AtomicBool::new(true))
                .collect(),
            output_active: P::descriptor()
                .outputs
                .iter()
                .map(|_| AtomicBool::new(true))
                .collect(),
            active: AtomicBool::new(false),
            processing: AtomicBool::new(false),
            missed_audio: AtomicBool::new(false),
            latency: AtomicU32::new(0),
            tail: AtomicU32::new(0),
        }
    }
}
impl<P: Plugin> Class for Processor<P> {
    type Interfaces = (
        IComponent,
        IAudioProcessor,
        IProcessContextRequirements,
        IConnectionPoint,
    );
}
impl<P: Plugin> IPluginBaseTrait for Processor<P> {
    unsafe fn initialize(&self, _: *mut FUnknown) -> tresult {
        boundary(kInternalError, || kResultOk)
    }
    unsafe fn terminate(&self) -> tresult {
        boundary(kInternalError, || kResultOk)
    }
}
impl<P: Plugin> IComponentTrait for Processor<P> {
    unsafe fn getControllerClassId(&self, out: *mut TUID) -> tresult {
        boundary(kInternalError, || {
            if out.is_null() {
                return kInvalidArgument;
            }
            *out = controller_cid::<P>();
            kResultOk
        })
    }
    unsafe fn setIoMode(&self, _: IoMode) -> tresult {
        boundary(kInternalError, || kResultOk)
    }
    unsafe fn getBusCount(&self, media: MediaType, dir: BusDirection) -> i32 {
        boundary(0, || {
            if media == MediaTypes_::kAudio as i32 {
                buses::<P>(dir).len() as i32
            } else if media == MediaTypes_::kEvent as i32 {
                i32::from(if dir == BusDirections_::kInput as i32 {
                    P::descriptor().midi_input
                } else {
                    P::descriptor().midi_output
                })
            } else {
                0
            }
        })
    }
    unsafe fn getBusInfo(
        &self,
        media: MediaType,
        dir: BusDirection,
        index: i32,
        out: *mut BusInfo,
    ) -> tresult {
        boundary(kInternalError, || {
            if out.is_null() || index < 0 || index >= self.getBusCount(media, dir) {
                return kInvalidArgument;
            }
            *out = std::mem::zeroed();
            let o = &mut *out;
            o.mediaType = media;
            o.direction = dir;
            #[allow(clippy::unnecessary_cast)] // VST enum signedness differs on Windows.
            let default_active = BusInfo_::BusFlags_::kDefaultActive as u32;
            o.flags = default_active;
            if media == MediaTypes_::kAudio as i32 {
                let b = &buses::<P>(dir)[index as usize];
                o.channelCount = b.channels as i32;
                o.busType = if b.auxiliary {
                    BusTypes_::kAux
                } else {
                    BusTypes_::kMain
                } as i32;
                wide(b.name, &mut o.name);
            } else {
                o.channelCount = 16;
                o.busType = BusTypes_::kMain as i32;
                wide("Notes", &mut o.name);
            }
            kResultOk
        })
    }
    unsafe fn getRoutingInfo(&self, _: *mut RoutingInfo, _: *mut RoutingInfo) -> tresult {
        boundary(kInternalError, || kNotImplemented)
    }
    unsafe fn activateBus(
        &self,
        media: MediaType,
        dir: BusDirection,
        index: i32,
        state: TBool,
    ) -> tresult {
        boundary(kInternalError, || {
            if index >= 0 && index < self.getBusCount(media, dir) {
                if media == MediaTypes_::kAudio as i32 {
                    let active = if dir == BusDirections_::kInput as i32 {
                        &self.input_active
                    } else {
                        &self.output_active
                    };
                    active[index as usize].store(state != 0, Ordering::Release);
                }
                kResultOk
            } else {
                kInvalidArgument
            }
        })
    }
    unsafe fn setActive(&self, state: TBool) -> tresult {
        boundary(kInternalError, || {
            self.active.store(state != 0, Ordering::Release);
            kResultOk
        })
    }
    unsafe fn setState(&self, s: *mut IBStream) -> tresult {
        boundary(kInternalError, || {
            let Some(bytes) = read_state(s) else {
                return kResultFalse;
            };
            let Some(_access) = self.gate.control() else {
                return kResultFalse;
            };
            let rt = &mut *self.runtime.get();
            rt.retired_errors.collect_on_control_thread();
            if rvst_adapter::load_project_state(&mut *rt.plugin, &rt.mirror, None, &bytes).is_err()
            {
                return kResultFalse;
            }
            rt.debug_lifecycle("Native state loaded");
            kResultOk
        })
    }
    unsafe fn getState(&self, s: *mut IBStream) -> tresult {
        boundary(kInternalError, || {
            let Some(_access) = self.gate.control() else {
                return kResultFalse;
            };
            let rt = &mut *self.runtime.get();
            rt.retired_errors.collect_on_control_thread();
            let Ok(bytes) = rvst_adapter::save_project_state(&mut *rt.plugin, &rt.mirror) else {
                return kResultFalse;
            };
            drop(_access);
            write_state(s, &bytes)
        })
    }
}
impl<P: Plugin> IProcessContextRequirementsTrait for Processor<P> {
    unsafe fn getProcessContextRequirements(&self) -> u32 {
        boundary(0, || {
            // project time music, tempo, signature, transport
            4 | 64 | 128 | 1024
        })
    }
}
impl<P: Plugin> IAudioProcessorTrait for Processor<P> {
    unsafe fn setBusArrangements(
        &self,
        ins: *mut SpeakerArrangement,
        ni: i32,
        outs: *mut SpeakerArrangement,
        no: i32,
    ) -> tresult {
        boundary(kInternalError, || {
            let d = P::descriptor();
            if ni < 0
                || no < 0
                || ni as usize > d.inputs.len()
                || no as usize > d.outputs.len()
                || d.inputs[ni.max(0) as usize..].iter().any(|b| !b.auxiliary)
                || d.outputs[no.max(0) as usize..].iter().any(|b| !b.auxiliary)
                || (ni > 0 && ins.is_null())
                || (no > 0 && outs.is_null())
            {
                return kResultFalse;
            }
            for (i, b) in d.inputs.iter().take(ni as usize).enumerate() {
                if *ins.add(i) != arrangement(b.channels) {
                    return kResultFalse;
                }
            }
            for (i, b) in d.outputs.iter().take(no as usize).enumerate() {
                if *outs.add(i) != arrangement(b.channels) {
                    return kResultFalse;
                }
            }
            for (i, active) in self.input_active.iter().enumerate() {
                active.store(i < ni as usize, Ordering::Release);
            }
            for (i, active) in self.output_active.iter().enumerate() {
                active.store(i < no as usize, Ordering::Release);
            }
            kResultOk
        })
    }
    unsafe fn getBusArrangement(
        &self,
        dir: BusDirection,
        index: i32,
        out: *mut SpeakerArrangement,
    ) -> tresult {
        boundary(kInternalError, || {
            if out.is_null() || index < 0 {
                return kInvalidArgument;
            }
            if let Some(b) = buses::<P>(dir).get(index as usize) {
                *out = arrangement(b.channels);
                kResultOk
            } else {
                kInvalidArgument
            }
        })
    }
    unsafe fn canProcessSampleSize(&self, size: i32) -> tresult {
        boundary(kInternalError, || {
            if size == SymbolicSampleSizes_::kSample32 as i32
                || (size == SymbolicSampleSizes_::kSample64 as i32 && P::descriptor().supports_f64)
            {
                kResultOk
            } else {
                kResultFalse
            }
        })
    }
    unsafe fn getLatencySamples(&self) -> u32 {
        boundary(0, || self.latency.load(Ordering::Acquire))
    }
    unsafe fn setupProcessing(&self, setup: *mut ProcessSetup) -> tresult {
        boundary(kInternalError, || {
            if setup.is_null() || self.processing.load(Ordering::Acquire) {
                return kInvalidArgument;
            }
            let s = &*setup;
            if s.maxSamplesPerBlock <= 0
                || self.canProcessSampleSize(s.symbolicSampleSize) != kResultOk
            {
                return kResultFalse;
            }
            let rt = &mut *self.runtime.get();
            if !rt.prepare(s.sampleRate, s.maxSamplesPerBlock as usize) {
                return kResultFalse;
            }
            self.latency.store(rt.plugin.latency(), Ordering::Release);
            rt.mirror
                .latency
                .store(rt.plugin.latency(), Ordering::Release);
            self.tail
                .store(rvst_adapter::tail(rt.plugin.tail()), Ordering::Release);
            kResultOk
        })
    }
    unsafe fn setProcessing(&self, state: TBool) -> tresult {
        boundary(kInternalError, || {
            if state != 0
                && (!self.active.load(Ordering::Acquire) || (&*self.runtime.get()).max_frames == 0)
            {
                return kResultFalse;
            }
            self.processing.store(state != 0, Ordering::Release);
            if let Some(_access) = self.gate.audio() {
                (&mut *self.runtime.get()).debug_event(if state != 0 { 2 } else { 3 }, 0.);
            }
            if state == 0
                && let Some(_access) = self.gate.audio()
            {
                (&mut *self.runtime.get()).plugin.reset();
            }
            kResultOk
        })
    }
    unsafe fn process(&self, data: *mut ProcessData) -> tresult {
        boundary(kInternalError, || {
            #[cfg(feature = "devtools")]
            let debug_start = std::time::Instant::now();
            if data.is_null() {
                return kInvalidArgument;
            }
            let d = &*data;
            let Some(_access) = self.gate.audio() else {
                self.missed_audio.store(true, Ordering::Release);
                silence(d);
                return kResultOk;
            };
            let rt = &mut *self.runtime.get();
            if self.missed_audio.swap(false, Ordering::AcqRel) {
                rt.plugin.reset();
                rt.output_reset_pending = true;
            }
            let desc = P::descriptor();
            if d.numSamples < 0
                || d.numSamples as usize > rt.max_frames
                || self.canProcessSampleSize(d.symbolicSampleSize) != kResultOk
            {
                return kResultFalse;
            }
            let n = d.numSamples as usize;
            let initial = &mut *self.initial.get();
            let host_edit_tokens = &mut *self.host_edit_tokens.get();
            for (i, p) in desc.parameters.iter().enumerate() {
                initial[i] = p.normalize(rt.plugin.parameter(p.id).unwrap_or(p.default));
                host_edit_tokens[i] = rt.mirror.host_edit_token(p.id);
            }
            if n == 0 {
                if let Some(ch) = ComRef::from_raw(d.inputParameterChanges) {
                    for j in 0..ch.getParameterCount() {
                        if let Some(q) = ComRef::from_raw(ch.getParameterData(j)) {
                            let count = q.getPointCount();
                            if count > 0 {
                                let mut frame = 0;
                                let mut value = 0.;
                                if q.getPoint(count - 1, &mut frame, &mut value) == kResultOk
                                    && let Some((index, p)) = desc
                                        .parameters
                                        .iter()
                                        .enumerate()
                                        .find(|(_, p)| p.id == q.getParameterId())
                                {
                                    rt.set_parameter_rt(p.id, p.denormalize(value));
                                    if let Some(applied) = rt.plugin.parameter(p.id) {
                                        rt.mirror.acknowledge_host_edit(
                                            p.id,
                                            applied,
                                            host_edit_tokens[index],
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
                rt.edits();
                rt.publish();
                return kResultOk;
            }
            if !self.processing.load(Ordering::Acquire)
                || d.numInputs < 0
                || d.numOutputs < 0
                || d.numInputs as usize > desc.inputs.len()
                || d.numOutputs as usize > desc.outputs.len()
            {
                return kResultFalse;
            }
            if (d.numInputs > 0 && d.inputs.is_null()) || (d.numOutputs > 0 && d.outputs.is_null())
            {
                return kInvalidArgument;
            }
            let ins = if d.numInputs == 0 {
                &[][..]
            } else {
                std::slice::from_raw_parts(d.inputs, d.numInputs as usize)
            };
            let outs = if d.numOutputs == 0 {
                &[][..]
            } else {
                std::slice::from_raw_parts(d.outputs, d.numOutputs as usize)
            };
            let is64 = d.symbolicSampleSize == SymbolicSampleSizes_::kSample64 as i32;
            let valid = if is64 {
                valid_buses::<f64>(ins, desc.inputs, &self.input_active, true)
                    && valid_buses::<f64>(outs, desc.outputs, &self.output_active, false)
            } else {
                valid_buses::<f32>(ins, desc.inputs, &self.input_active, true)
                    && valid_buses::<f32>(outs, desc.outputs, &self.output_active, false)
            };
            if !valid {
                return kInvalidArgument;
            }
            // Snapshot the entire input before writing any output, including eventful
            // blocks and channel aliases. Inactive buses retain their planar offsets.
            if is64 {
                copy_inputs(
                    ins,
                    desc.inputs,
                    &self.input_active,
                    &mut rt.block_input64[..desc.input_channels() * n],
                    n,
                );
            } else {
                copy_inputs(
                    ins,
                    desc.inputs,
                    &self.input_active,
                    &mut rt.block_input32[..desc.input_channels() * n],
                    n,
                );
            }
            let mut output_lost = false;
            if rt.output_reset_pending {
                let mut reset = Output {
                    list: d.outputEvents,
                    frame: 0,
                    lost: &mut output_lost,
                };
                if rvst_core::EventSink::push(
                    &mut reset,
                    CoreEvent {
                        frame: 0,
                        kind: EventKind::AllNotesOff,
                    },
                ) {
                    rt.output_reset_pending = false;
                }
            }
            let mut cursor = 0;
            let event_list = ComRef::from_raw(d.inputEvents);
            rt.edits();
            let has_automation = ComRef::from_raw(d.inputParameterChanges)
                .is_some_and(|c| c.getParameterCount() > 0);
            if !has_automation && event_list.is_none_or(|e| e.getEventCount() == 0) {
                let t = transport(d.processContext, 0, rt.sample_rate);
                let mut sink = Output {
                    list: d.outputEvents,
                    frame: 0,
                    lost: &mut output_lost,
                };
                let status = if is64 {
                    rt.block64(n, &t, &mut sink)
                } else {
                    rt.block32(n, &t, &mut sink)
                };
                if status == ProcessStatus::Error {
                    return kResultFalse;
                }
            } else {
                for frame in 0..n {
                    if let Some(ch) = ComRef::from_raw(d.inputParameterChanges) {
                        for j in 0..ch.getParameterCount() {
                            if let Some(q) = ComRef::from_raw(ch.getParameterData(j)) {
                                let id = q.getParameterId();
                                if let Some((ix, p)) =
                                    desc.parameters.iter().enumerate().find(|(_, p)| p.id == id)
                                {
                                    if let Some(value) =
                                        queue_value(q, frame as i32, initial[ix], p.step.is_some())
                                    {
                                        rt.push(CoreEvent {
                                            frame: 0,
                                            kind: EventKind::Parameter {
                                                id,
                                                value: p.denormalize(value),
                                            },
                                        });
                                    }
                                } else if compat::midi_address::<P>(id).is_some() {
                                    // MIDI values are timestamped points, never ramps.
                                    for point in 0..q.getPointCount() {
                                        let mut offset = 0;
                                        let mut value = 0.;
                                        if q.getPoint(point, &mut offset, &mut value) == kResultOk
                                            && offset == frame as i32
                                            && let Some(kind) = compat::midi_event::<P>(id, value)
                                        {
                                            rt.push(CoreEvent { frame: 0, kind });
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if let Some(list) = event_list {
                        while cursor < list.getEventCount() {
                            let mut e: Event = std::mem::zeroed();
                            if list.getEvent(cursor, &mut e) != kResultOk {
                                cursor += 1;
                                continue;
                            }
                            if e.sampleOffset > frame as i32 {
                                break;
                            }
                            if let Some(kind) = note(&e) {
                                rt.push(CoreEvent { frame: 0, kind });
                            }
                            cursor += 1;
                        }
                    }
                    if is64 {
                        for c in 0..rt.input64.len() {
                            rt.input64[c] = rt.block_input64[c * n + frame];
                        }
                    } else {
                        for c in 0..rt.input32.len() {
                            rt.input32[c] = rt.block_input32[c * n + frame];
                        }
                    }
                    let t = transport(d.processContext, frame, rt.sample_rate);
                    let mut sink = Output {
                        list: d.outputEvents,
                        frame: frame as i32,
                        lost: &mut output_lost,
                    };
                    let status = if is64 {
                        rt.tick64(&t, &mut sink)
                    } else {
                        rt.tick32(&t, &mut sink)
                    };
                    if is64 {
                        for c in 0..rt.output64.len() {
                            rt.block_output64[c * n + frame] = rt.output64[c];
                        }
                    } else {
                        for c in 0..rt.output32.len() {
                            rt.block_output32[c * n + frame] = rt.output32[c];
                        }
                    }
                    if status == ProcessStatus::Error {
                        return kResultFalse;
                    }
                }
            }
            if is64 {
                copy_outputs(
                    outs,
                    desc.outputs,
                    &self.output_active,
                    &rt.block_output64,
                    n,
                );
            } else {
                copy_outputs(
                    outs,
                    desc.outputs,
                    &self.output_active,
                    &rt.block_output32,
                    n,
                );
            }
            for k in 0..d.numOutputs as usize {
                (*d.outputs.add(k)).silenceFlags = 0;
            }
            rt.output_reset_pending |= output_lost;
            if let Some(changes) = ComRef::from_raw(d.inputParameterChanges) {
                for index in 0..changes.getParameterCount() {
                    if let Some(queue) = ComRef::from_raw(changes.getParameterData(index))
                        && queue.getPointCount() > 0
                        && let Some(parameter_index) = desc
                            .parameters
                            .iter()
                            .position(|p| p.id == queue.getParameterId())
                        && let Some(applied) = rt.plugin.parameter(queue.getParameterId())
                    {
                        rt.mirror.acknowledge_host_edit(
                            queue.getParameterId(),
                            applied,
                            host_edit_tokens[parameter_index],
                        );
                    }
                }
            }
            if output_lost {
                rt.debug_event(6, 1.);
            }
            #[cfg(feature = "devtools")]
            rt.debug_callback(n, debug_start);
            let latency = rt.plugin.latency();
            self.latency.store(latency, Ordering::Release);
            rt.mirror.latency.store(latency, Ordering::Release);
            self.tail
                .store(rvst_adapter::tail(rt.plugin.tail()), Ordering::Release);
            kResultOk
        })
    }
    unsafe fn getTailSamples(&self) -> u32 {
        boundary(0, || self.tail.load(Ordering::Acquire))
    }
}
/// VST3's implicit first point is the previous block value at sample -1.
unsafe fn queue_value(
    q: ComRef<IParamValueQueue>,
    frame: i32,
    initial: f64,
    stepped: bool,
) -> Option<f64> {
    let count = q.getPointCount();
    if count <= 0 {
        return None;
    }
    let mut previous = (-1, initial);
    for index in 0..count {
        let mut offset = 0;
        let mut value = 0.;
        if q.getPoint(index, &mut offset, &mut value) != kResultOk || !value.is_finite() {
            continue;
        }
        if frame <= offset {
            if stepped {
                return Some(if frame == offset { value } else { previous.1 });
            }
            let width = offset - previous.0;
            return Some(if width <= 0 {
                value
            } else {
                previous.1 + (value - previous.1) * (frame - previous.0) as f64 / width as f64
            });
        }
        previous = (offset, value);
    }
    Some(previous.1)
}
unsafe fn note(e: &Event) -> Option<EventKind> {
    Some(match e.r#type as Event_::EventTypes {
        Event_::EventTypes_::kNoteOnEvent => {
            let n = e.__field0.noteOn;
            EventKind::NoteOn {
                channel: n.channel as u8,
                key: n.pitch as u8,
                velocity: n.velocity as f64,
                note_id: n.noteId,
            }
        }
        Event_::EventTypes_::kNoteOffEvent => {
            let n = e.__field0.noteOff;
            EventKind::NoteOff {
                channel: n.channel as u8,
                key: n.pitch as u8,
                velocity: n.velocity as f64,
                note_id: n.noteId,
            }
        }
        Event_::EventTypes_::kPolyPressureEvent => {
            let n = e.__field0.polyPressure;
            EventKind::PolyPressure {
                channel: n.channel as u8,
                key: n.pitch as u8,
                pressure: n.pressure as f64,
                note_id: n.noteId,
            }
        }
        _ => return None,
    })
}
#[allow(clippy::unnecessary_cast)] // VST enum signedness differs on Windows.
unsafe fn transport(p: *const ProcessContext, frame: usize, sr: f64) -> Transport {
    let mut t = Transport::default();
    if p.is_null() {
        return t;
    }
    let p = &*p;
    use ProcessContext_::StatesAndFlags_::*;
    let state = p.state as ProcessContext_::StatesAndFlags;
    t.playing = state & kPlaying != 0;
    t.recording = state & kRecording != 0;
    t.sample_position = Some(p.projectTimeSamples + if t.playing { frame as i64 } else { 0 });
    if state & kTempoValid != 0 {
        t.tempo = Some(p.tempo)
    }
    if state & kProjectTimeMusicValid != 0 {
        t.beat_position = Some(
            p.projectTimeMusic
                + if t.playing && state & kTempoValid != 0 && sr > 0. {
                    frame as f64 / sr * p.tempo / 60.
                } else {
                    0.
                },
        )
    }
    if state & kTimeSigValid != 0 {
        t.time_signature = Some((p.timeSigNumerator as u16, p.timeSigDenominator as u16))
    }
    t
}
struct Output<'a> {
    list: *mut IEventList,
    frame: i32,
    lost: &'a mut bool,
}
impl rvst_core::EventSink for Output<'_> {
    fn push(&mut self, e: CoreEvent) -> bool {
        let result = unsafe { self.send(e) };
        if !result
            && matches!(
                e.kind,
                EventKind::NoteOff { .. } | EventKind::NoteRelease { .. } | EventKind::AllNotesOff
            )
        {
            *self.lost = true;
        }
        result
    }
}
impl Output<'_> {
    unsafe fn send(&mut self, e: CoreEvent) -> bool {
        boundary(false, || {
            let Some(list) = ComRef::from_raw(self.list) else {
                return false;
            };
            let mut out: Event = std::mem::zeroed();
            out.sampleOffset = self.frame + e.frame as i32;
            match e.kind {
                EventKind::NoteOn {
                    channel,
                    key,
                    velocity,
                    note_id,
                } => {
                    out.r#type = Event_::EventTypes_::kNoteOnEvent as u16;
                    out.__field0.noteOn = NoteOnEvent {
                        channel: channel as i16,
                        pitch: key as i16,
                        tuning: 0.,
                        velocity: velocity as f32,
                        length: 0,
                        noteId: note_id,
                    };
                }
                EventKind::NoteOff {
                    channel,
                    key,
                    velocity,
                    note_id,
                } => {
                    out.r#type = Event_::EventTypes_::kNoteOffEvent as u16;
                    out.__field0.noteOff = NoteOffEvent {
                        channel: channel as i16,
                        pitch: key as i16,
                        tuning: 0.,
                        velocity: velocity as f32,
                        noteId: note_id,
                    };
                }
                EventKind::PolyPressure {
                    channel,
                    key,
                    pressure,
                    note_id,
                } => {
                    out.r#type = Event_::EventTypes_::kPolyPressureEvent as u16;
                    out.__field0.polyPressure = PolyPressureEvent {
                        channel: channel as i16,
                        pitch: key as i16,
                        pressure: pressure as f32,
                        noteId: note_id,
                    };
                }
                EventKind::ControlChange {
                    channel,
                    controller,
                    value,
                } => {
                    out.r#type = Event_::EventTypes_::kLegacyMIDICCOutEvent as u16;
                    out.__field0.midiCCOut = LegacyMIDICCOutEvent {
                        controlNumber: controller,
                        channel: channel as i8,
                        value: (value.clamp(0., 1.) * 127.).round() as i8,
                        value2: 0,
                    };
                }
                EventKind::ChannelPressure { channel, pressure } => {
                    out.r#type = Event_::EventTypes_::kLegacyMIDICCOutEvent as u16;
                    out.__field0.midiCCOut = LegacyMIDICCOutEvent {
                        controlNumber: 128,
                        channel: channel as i8,
                        value: (pressure.clamp(0., 1.) * 127.).round() as i8,
                        value2: 0,
                    };
                }
                EventKind::NoteRelease {
                    channel,
                    key,
                    note_id,
                    velocity,
                    choke,
                } => {
                    // VST3 has note IDs but no per-note choke; scoped channel-mode
                    // releases use the corresponding MIDI controller message.
                    let first = channel.unwrap_or(0);
                    let last = channel.unwrap_or(15);
                    let mut ok = true;
                    for ch in first..=last {
                        if key.is_some() || note_id.is_some() {
                            out.r#type = Event_::EventTypes_::kNoteOffEvent as u16;
                            out.__field0.noteOff = NoteOffEvent {
                                channel: ch as i16,
                                pitch: key.map_or(-1, i16::from),
                                tuning: 0.,
                                velocity: velocity as f32,
                                noteId: note_id.unwrap_or(-1),
                            };
                        } else {
                            out.r#type = Event_::EventTypes_::kLegacyMIDICCOutEvent as u16;
                            out.__field0.midiCCOut = LegacyMIDICCOutEvent {
                                controlNumber: if choke { 120 } else { 123 },
                                channel: ch as i8,
                                value: 0,
                                value2: 0,
                            };
                        }
                        ok &= list.addEvent(&mut out) == kResultOk;
                    }
                    return ok;
                }
                EventKind::PitchBend { channel, value } => {
                    let v = ((value.clamp(-1., 1.) + 1.) * 8192.).round().min(16383.) as u16;
                    out.r#type = Event_::EventTypes_::kLegacyMIDICCOutEvent as u16;
                    out.__field0.midiCCOut = LegacyMIDICCOutEvent {
                        controlNumber: 129,
                        channel: channel as i8,
                        value: (v & 127) as i8,
                        value2: (v >> 7) as i8,
                    };
                }
                EventKind::AllNotesOff => {
                    out.r#type = Event_::EventTypes_::kLegacyMIDICCOutEvent as u16;
                    let mut ok = true;
                    for c in 0..16 {
                        out.__field0.midiCCOut = LegacyMIDICCOutEvent {
                            controlNumber: 123,
                            channel: c,
                            value: 0,
                            value2: 0,
                        };
                        ok &= list.addEvent(&mut out) == kResultOk;
                    }
                    return ok;
                }
                _ => return false,
            }
            list.addEvent(&mut out) == kResultOk
        })
    }
}

struct Controller<P: Plugin> {
    connection: connection::Endpoint,
    linked: UnsafeCell<Option<std::sync::Arc<rvst_adapter::ParameterMirror>>>,
    mirror: std::sync::Arc<rvst_adapter::ParameterMirror>,
    plugin: UnsafeCell<rvst_adapter::DropShield<P>>,
    handler: Cell<*mut IComponentHandler>,
    host_context: Cell<*mut FUnknown>,
    timer: UnsafeCell<Option<timer::MainTimer>>,
    editor: EditorFn,
}
impl<P: Plugin> Class for Controller<P> {
    type Interfaces = (IEditController, IConnectionPoint, IMidiMapping);
}
impl<P: Plugin> Drop for Controller<P> {
    fn drop(&mut self) {
        unsafe {
            (&mut *self.timer.get()).take();
            let context = self.host_context.replace(ptr::null_mut());
            if !context.is_null() {
                release(context);
            }
            if let Some(h) = ComRef::from_raw(self.handler.get()) {
                release(h.as_ptr() as _);
            }
        }
    }
}
impl<P: Plugin> IPluginBaseTrait for Controller<P> {
    unsafe fn initialize(&self, context: *mut FUnknown) -> tresult {
        boundary(kInternalError, || {
            if !self.host_context.get().is_null() {
                return kResultFalse;
            }
            if !context.is_null() {
                retain(context);
                self.host_context.set(context);
            }
            self.restart_timer();
            kResultOk
        })
    }
    unsafe fn terminate(&self) -> tresult {
        boundary(kInternalError, || {
            (&mut *self.timer.get()).take();
            let context = self.host_context.replace(ptr::null_mut());
            if !context.is_null() {
                release(context);
            }
            kResultOk
        })
    }
}
impl<P: Plugin> IMidiMappingTrait for Controller<P> {
    unsafe fn getMidiControllerAssignment(
        &self,
        bus: i32,
        channel: i16,
        controller: i16,
        out: *mut u32,
    ) -> tresult {
        boundary(kInternalError, || {
            if out.is_null()
                || bus != 0
                || !(0..16).contains(&channel)
                || !(0..130).contains(&controller)
            {
                return kInvalidArgument;
            }
            let Some(base) = compat::midi_base::<P>() else {
                return kResultFalse;
            };
            *out = base + channel as u32 * 130 + controller as u32;
            kResultOk
        })
    }
}
impl<P: Plugin> IEditControllerTrait for Controller<P> {
    unsafe fn setComponentState(&self, s: *mut IBStream) -> tresult {
        boundary(kInternalError, || self.setState(s))
    }
    unsafe fn setState(&self, s: *mut IBStream) -> tresult {
        boundary(kInternalError, || {
            let Some(data) = read_state(s) else {
                return kResultFalse;
            };
            let mirror = self.editor_mirror();
            let fallback =
                (!std::sync::Arc::ptr_eq(&mirror, &self.mirror)).then_some(self.mirror.as_ref());
            if rvst_adapter::load_project_state(&mut **self.plugin.get(), &mirror, fallback, &data)
                .is_ok()
            {
                kResultOk
            } else {
                kResultFalse
            }
        })
    }
    unsafe fn getState(&self, s: *mut IBStream) -> tresult {
        boundary(kInternalError, || {
            let Ok(data) =
                rvst_adapter::save_project_state(&mut **self.plugin.get(), &self.editor_mirror())
            else {
                return kResultFalse;
            };
            write_state(s, &data)
        })
    }
    unsafe fn getParameterCount(&self) -> i32 {
        boundary(0, || {
            P::descriptor().parameters.len() as i32
                + if compat::midi_base::<P>().is_some() {
                    compat::MIDI_COUNT as i32
                } else {
                    0
                }
        })
    }
    unsafe fn getParameterInfo(&self, index: i32, out: *mut ParameterInfo) -> tresult {
        boundary(kInternalError, || {
            if out.is_null() || index < 0 {
                return kInvalidArgument;
            }
            let Some(p) = P::descriptor().parameters.get(index as usize) else {
                return if compat::midi_info::<P>(index as usize, &mut *out) {
                    kResultOk
                } else {
                    kInvalidArgument
                };
            };
            *out = std::mem::zeroed();
            let o = &mut *out;
            o.id = p.id;
            wide(p.name, &mut o.title);
            wide(p.name, &mut o.shortTitle);
            wide(p.unit, &mut o.units);
            o.defaultNormalizedValue = p.normalize(p.default);
            o.stepCount = p
                .step
                .map_or(0, |step| ((p.max - p.min) / step).round() as i32);
            o.flags = ParameterInfo_::ParameterFlags_::kCanAutomate
                | if p.bypass {
                    ParameterInfo_::ParameterFlags_::kIsBypass
                } else {
                    0
                };
            kResultOk
        })
    }
    unsafe fn getParamStringByValue(&self, id: u32, value: f64, out: *mut String128) -> tresult {
        boundary(kInternalError, || {
            if out.is_null() {
                return kInvalidArgument;
            }
            if compat::midi_address::<P>(id).is_some() {
                wide(&format!("{value:.3}"), &mut *out);
                return kResultOk;
            }
            let Some(p) = parameter::<P>(id) else {
                return kInvalidArgument;
            };
            wide(&format!("{:.3}", p.denormalize(value)), &mut *out);
            kResultOk
        })
    }
    unsafe fn getParamValueByString(&self, id: u32, text: *mut TChar, out: *mut f64) -> tresult {
        boundary(kInternalError, || {
            if text.is_null() || out.is_null() {
                return kInvalidArgument;
            }
            let Some(p) = parameter::<P>(id) else {
                return kInvalidArgument;
            };
            let mut len = 0;
            while len < 128 && *text.add(len) != 0 {
                len += 1
            }
            let data = std::slice::from_raw_parts(text as *const u16, len);
            if let Ok(s) = String::from_utf16(data)
                && let Ok(value) = s.parse::<f64>()
                && value.is_finite()
            {
                *out = p.normalize(value);
                return kResultOk;
            }
            kInvalidArgument
        })
    }
    unsafe fn normalizedParamToPlain(&self, id: u32, v: f64) -> f64 {
        boundary(0., || {
            if compat::midi_address::<P>(id).is_some() {
                v.clamp(0., 1.)
            } else {
                parameter::<P>(id).map_or(0., |p| p.denormalize(v))
            }
        })
    }
    unsafe fn plainParamToNormalized(&self, id: u32, v: f64) -> f64 {
        boundary(0., || {
            if compat::midi_address::<P>(id).is_some() {
                v.clamp(0., 1.)
            } else {
                parameter::<P>(id).map_or(0., |p| p.normalize(v))
            }
        })
    }
    unsafe fn getParamNormalized(&self, id: u32) -> f64 {
        boundary(0., || {
            parameter::<P>(id).map_or(0., |p| {
                p.normalize(self.editor_mirror().get(id).unwrap_or(p.default))
            })
        })
    }
    unsafe fn setParamNormalized(&self, id: u32, value: f64) -> tresult {
        boundary(kInternalError, || {
            if !value.is_finite() {
                return kInvalidArgument;
            }
            if compat::midi_address::<P>(id).is_some() {
                return kResultOk;
            }
            let Some(p) = parameter::<P>(id) else {
                return kInvalidArgument;
            };
            let mirror = self.editor_mirror();
            let host_edit_token = mirror.host_edit_token(id);
            if (&mut *self.plugin.get())
                .set_parameter(id, p.denormalize(value))
                .is_ok()
            {
                if std::sync::Arc::ptr_eq(&mirror, &self.mirror) {
                    mirror.acknowledge_host_edit(id, p.denormalize(value), host_edit_token);
                } else {
                    mirror.publish(id, p.denormalize(value));
                }
                kResultOk
            } else {
                kResultFalse
            }
        })
    }
    unsafe fn setComponentHandler(&self, h: *mut IComponentHandler) -> tresult {
        boundary(kInternalError, || {
            if let Some(new) = ComRef::from_raw(h) {
                retain(new.as_ptr() as _);
            }
            let old = self.handler.replace(h);
            self.restart_timer();
            if let Some(old) = ComRef::from_raw(old) {
                release(old.as_ptr() as _);
            }
            kResultOk
        })
    }
    unsafe fn createView(&self, name: *const c_char) -> *mut IPlugView {
        boundary(ptr::null_mut(), || {
            if name.is_null() || std::ffi::CStr::from_ptr(name) != c"editor" {
                return ptr::null_mut();
            }
            if let Some(h) = ComRef::from_raw(self.handler.get()) {
                retain(h.as_ptr() as _);
            }
            let handler = Handler(self.handler.get());
            ComWrapper::new(View {
                mirror: self.editor_mirror().clone(),
                handler,
                editor: self.editor,
                window: UnsafeCell::new(None),
                width: Cell::new(P::editor_size().0 as i32),
                height: Cell::new(P::editor_size().1 as i32),
                minimum: P::editor_min_size(),
                #[cfg(target_os = "linux")]
                dispatcher: UnsafeCell::new(None),
            })
            .to_com_ptr::<IPlugView>()
            .map_or(ptr::null_mut(), |p| p.into_raw())
        })
    }
}
unsafe fn read_state(s: *mut IBStream) -> Option<Vec<u8>> {
    let s = ComRef::from_raw(s)?;
    let mut out = Vec::new();
    let mut block = [0u8; 4096];
    loop {
        let mut count = 0;
        let result = s.read(block.as_mut_ptr() as _, 4096, &mut count);
        if !(0..=4096).contains(&count) {
            return None;
        }
        if count == 0 {
            break;
        }
        if out.len() + count as usize > 64 * 1024 * 1024 {
            return None;
        }
        out.extend_from_slice(&block[..count as usize]);
        if result != kResultOk {
            break;
        }
    }
    Some(out)
}
unsafe fn write_state(s: *mut IBStream, data: &[u8]) -> tresult {
    boundary(kInternalError, || {
        let Some(s) = ComRef::from_raw(s) else {
            return kInvalidArgument;
        };
        let mut pos = 0;
        while pos < data.len() {
            let len = (data.len() - pos).min(i32::MAX as usize) as i32;
            let mut n = 0;
            if s.write(data.as_ptr().add(pos) as _, len, &mut n) != kResultOk || n <= 0 || n > len {
                return kResultFalse;
            }
            pos += n as usize;
        }
        kResultOk
    })
}

struct Factory<P: Plugin> {
    editor: EditorFn,
    _type: PhantomData<P>,
}
impl<P: Plugin> Class for Factory<P> {
    type Interfaces = (IPluginFactory2,);
}
impl<P: Plugin> IPluginFactoryTrait for Factory<P> {
    unsafe fn getFactoryInfo(&self, out: *mut PFactoryInfo) -> tresult {
        boundary(kInternalError, || {
            if out.is_null() {
                return kInvalidArgument;
            }
            *out = std::mem::zeroed();
            let d = P::descriptor();
            narrow(d.vendor, &mut (*out).vendor);
            narrow(d.url, &mut (*out).url);
            narrow(d.email, &mut (*out).email);
            (*out).flags = PFactoryInfo_::FactoryFlags_::kUnicode as i32;
            kResultOk
        })
    }
    unsafe fn countClasses(&self) -> i32 {
        boundary(0, || 2)
    }
    unsafe fn getClassInfo(&self, index: i32, out: *mut PClassInfo) -> tresult {
        boundary(kInternalError, || {
            if out.is_null() || !(0..2).contains(&index) {
                return kInvalidArgument;
            }
            *out = std::mem::zeroed();
            let o = &mut *out;
            o.cid = if index == 0 {
                cid::<P>()
            } else {
                controller_cid::<P>()
            };
            o.cardinality = PClassInfo_::ClassCardinality_::kManyInstances as i32;
            narrow(
                if index == 0 {
                    "Audio Module Class"
                } else {
                    "Component Controller Class"
                },
                &mut o.category,
            );
            narrow(P::descriptor().name, &mut o.name);
            kResultOk
        })
    }
    unsafe fn createInstance(
        &self,
        class_id: FIDString,
        iid: FIDString,
        out: *mut *mut c_void,
    ) -> tresult {
        boundary(kInternalError, || {
            if class_id.is_null() || iid.is_null() || out.is_null() {
                return kInvalidArgument;
            }
            *out = ptr::null_mut();
            let id = ptr::read_unaligned(class_id as *const TUID);
            let instance = if id == cid::<P>() {
                ComWrapper::new(Processor::<P>::new()).to_com_ptr::<FUnknown>()
            } else if id == controller_cid::<P>() {
                ComWrapper::new(Controller::<P> {
                    connection: connection::Endpoint::default(),
                    linked: UnsafeCell::new(None),
                    mirror: rvst_adapter::ParameterMirror::new(P::descriptor()),
                    plugin: UnsafeCell::new(rvst_adapter::DropShield::new(P::new())),
                    handler: Cell::new(ptr::null_mut()),
                    host_context: Cell::new(ptr::null_mut()),
                    timer: UnsafeCell::new(None),
                    editor: self.editor,
                })
                .to_com_ptr::<FUnknown>()
            } else {
                None
            };
            if let Some(obj) = instance {
                {
                    let p = obj.as_ptr();
                    ((*(*p).vtbl).queryInterface)(p, iid as *mut TUID, out)
                }
            } else {
                kInvalidArgument
            }
        })
    }
}
pub fn factory<P: Plugin>(editor: EditorFn) -> *mut IPluginFactory {
    boundary(ptr::null_mut(), || {
        ComWrapper::new(Factory::<P> {
            editor,
            _type: PhantomData,
        })
        .to_com_ptr::<IPluginFactory>()
        .map_or(ptr::null_mut(), |p| p.into_raw())
    })
}
#[macro_export]
macro_rules! export_plugin {
    ($plugin:ty,$editor:path) => {
        #[unsafe(no_mangle)]
        pub extern "system" fn GetPluginFactory() -> *mut $crate::vst3::Steinberg::IPluginFactory {
            $crate::factory::<$plugin>($editor)
        }
        #[cfg(target_os = "macos")]
        #[unsafe(no_mangle)]
        pub extern "system" fn bundleEntry(_: *mut std::ffi::c_void) -> bool {
            true
        }
        #[cfg(target_os = "macos")]
        #[unsafe(no_mangle)]
        pub extern "system" fn bundleExit() -> bool {
            true
        }
        #[cfg(target_os = "windows")]
        #[unsafe(no_mangle)]
        pub extern "system" fn InitDll() -> bool {
            true
        }
        #[cfg(target_os = "windows")]
        #[unsafe(no_mangle)]
        pub extern "system" fn ExitDll() -> bool {
            true
        }
        #[cfg(target_os = "linux")]
        #[unsafe(no_mangle)]
        pub extern "system" fn ModuleEntry(_: *mut std::ffi::c_void) -> bool {
            true
        }
        #[cfg(target_os = "linux")]
        #[unsafe(no_mangle)]
        pub extern "system" fn ModuleExit() -> bool {
            true
        }
    };
}

struct Handler(*mut IComponentHandler);
// macOS/Windows invoke the host on their owning UI thread. X11 uses the
// host IRunLoop dispatcher and never transfers this pointer to its window thread.
unsafe impl Send for Handler {}
impl Clone for Handler {
    fn clone(&self) -> Self {
        unsafe {
            if let Some(h) = ComRef::from_raw(self.0) {
                retain(h.as_ptr() as _);
            }
        }
        Self(self.0)
    }
}
impl Drop for Handler {
    fn drop(&mut self) {
        unsafe {
            if let Some(h) = ComRef::from_raw(self.0) {
                release(h.as_ptr() as _);
            }
        }
    }
}
struct VstEditor {
    mirror: std::sync::Arc<rvst_adapter::ParameterMirror>,
    #[cfg(not(target_os = "linux"))]
    handler: Handler,
    #[cfg(target_os = "linux")]
    mailbox: std::sync::Arc<dispatch::Mailbox>,
}
impl rvst_core::EditorHost for VstEditor {
    fn edit_history(&self) -> Option<rvst_core::EditHistory> {
        Some(self.mirror.edit_history.clone())
    }
    fn editor_context(&self) -> Option<rvst_core::EditorContext> {
        self.mirror.editor_context.clone()
    }
    fn descriptor(&self) -> &'static rvst_core::PluginDescriptor {
        self.mirror.descriptor
    }
    fn parameter(&self, id: u32) -> Option<f64> {
        self.mirror.get(id)
    }
    fn begin_edit(&mut self, id: u32) {
        #[cfg(target_os = "linux")]
        self.mailbox.begin(id);
        #[cfg(not(target_os = "linux"))]
        unsafe {
            if let Some(h) = ComRef::from_raw(self.handler.0) {
                h.beginEdit(id);
            }
        }
    }
    fn set_parameter(&mut self, id: u32, v: f64) {
        if let Some(p) = self.mirror.descriptor.parameter(id) {
            let generation = self.mirror.state_generation();
            let v = p.clamp(v);
            let token = self.mirror.defer_host_edit(id, v, generation);
            #[cfg(target_os = "linux")]
            self.mailbox
                .value_at_generation(id, p.normalize(v), generation, token);
            #[cfg(not(target_os = "linux"))]
            unsafe {
                if let Some(h) = ComRef::from_raw(self.handler.0) {
                    self.mirror.host_edit_dispatched(id, token);
                    h.performEdit(id, p.normalize(v));
                }
            }
        }
    }
    fn end_edit(&mut self, id: u32) {
        #[cfg(target_os = "linux")]
        self.mailbox.end(id);
        #[cfg(not(target_os = "linux"))]
        unsafe {
            if let Some(h) = ComRef::from_raw(self.handler.0) {
                h.endEdit(id);
            }
        }
    }
    fn save_state(&mut self) -> Result<Vec<u8>, rvst_core::Error> {
        rvst_adapter::save_parameter_preset(&self.mirror)
    }
    fn load_state(&mut self, data: &[u8]) -> Result<(), rvst_core::Error> {
        let values = rvst_adapter::parse_parameter_preset(self.mirror.descriptor, data)?;
        for (id, value) in values {
            self.begin_edit(id);
            self.set_parameter(id, value);
            self.end_edit(id);
        }
        Ok(())
    }
}
struct View {
    mirror: std::sync::Arc<rvst_adapter::ParameterMirror>,
    handler: Handler,
    editor: EditorFn,
    window: UnsafeCell<Option<rvst_platform::NativeEditor>>,
    width: Cell<i32>,
    height: Cell<i32>,
    minimum: (u32, u32),
    #[cfg(target_os = "linux")]
    dispatcher: UnsafeCell<Option<dispatch::Dispatch>>,
}
impl Class for View {
    type Interfaces = (IPlugView,);
}
impl IPlugViewTrait for View {
    unsafe fn isPlatformTypeSupported(&self, kind: FIDString) -> tresult {
        boundary(kInternalError, || {
            if kind.is_null() {
                return kResultFalse;
            }
            let kind = std::ffi::CStr::from_ptr(kind);
            let expected = if cfg!(target_os = "macos") {
                c"NSView"
            } else if cfg!(target_os = "windows") {
                c"HWND"
            } else {
                c"X11EmbedWindowID"
            };
            if kind == expected {
                kResultTrue
            } else {
                kResultFalse
            }
        })
    }
    unsafe fn attached(&self, parent: *mut c_void, kind: FIDString) -> tresult {
        boundary(kInternalError, || {
            if self.isPlatformTypeSupported(kind) != kResultTrue || (&*self.window.get()).is_some()
            {
                return kResultFalse;
            }
            let Ok(parent) = rvst_platform::parent_from_raw(parent) else {
                return kInvalidArgument;
            };
            #[cfg(target_os = "linux")]
            let Some(dispatcher) = (&*self.dispatcher.get()).as_ref() else {
                return kResultFalse;
            };
            let host = Box::new(VstEditor {
                mirror: self.mirror.clone(),
                #[cfg(not(target_os = "linux"))]
                handler: self.handler.clone(),
                #[cfg(target_os = "linux")]
                mailbox: dispatcher.mailbox.clone(),
            });
            let options = rvst_platform::EditorOptions {
                title: self.mirror.descriptor.name.into(),
                inspector_url: self.mirror.inspector_url.clone(),
                meters: Some(self.mirror.meters.clone()),
                ui_metrics: self.mirror.ui_metrics.clone(),
                width: self.width.get() as u32,
                height: self.height.get() as u32,
                ..Default::default()
            };
            match rvst_platform::NativeEditor::open(host, self.editor, options, Some(parent)) {
                Ok(w) => {
                    if w.show().is_err() {
                        return kResultFalse;
                    }
                    *self.window.get() = Some(w);
                    kResultOk
                }
                Err(_) => kResultFalse,
            }
        })
    }
    unsafe fn removed(&self) -> tresult {
        boundary(kInternalError, || {
            (&mut *self.window.get()).take();
            #[cfg(target_os = "linux")]
            if let Some(dispatcher) = (&*self.dispatcher.get()).as_ref() {
                dispatcher.close_gestures();
            }
            kResultOk
        })
    }
    unsafe fn onWheel(&self, _: f32) -> tresult {
        boundary(kInternalError, || kResultFalse)
    }
    unsafe fn onKeyDown(&self, _: char16, _: int16, _: int16) -> tresult {
        boundary(kInternalError, || kResultFalse)
    }
    unsafe fn onKeyUp(&self, _: char16, _: int16, _: int16) -> tresult {
        boundary(kInternalError, || kResultFalse)
    }
    unsafe fn getSize(&self, r: *mut ViewRect) -> tresult {
        boundary(kInternalError, || {
            if r.is_null() {
                return kInvalidArgument;
            }
            *r = ViewRect {
                left: 0,
                top: 0,
                right: self.width.get(),
                bottom: self.height.get(),
            };
            kResultOk
        })
    }
    unsafe fn onSize(&self, r: *mut ViewRect) -> tresult {
        boundary(kInternalError, || {
            if r.is_null() {
                return kInvalidArgument;
            }
            let w = (*r).right - (*r).left;
            let h = (*r).bottom - (*r).top;
            if w <= 0 || h <= 0 {
                return kInvalidArgument;
            }
            if let Some(window) = &*self.window.get()
                && window.resize(w as u32, h as u32).is_err()
            {
                return kResultFalse;
            }
            self.width.set(w);
            self.height.set(h);
            kResultOk
        })
    }
    unsafe fn onFocus(&self, _: TBool) -> tresult {
        boundary(kInternalError, || kResultOk)
    }
    unsafe fn setFrame(&self, _frame: *mut IPlugFrame) -> tresult {
        boundary(kInternalError, || {
            #[cfg(target_os = "linux")]
            {
                (&mut *self.window.get()).take();
                (&mut *self.dispatcher.get()).take();
                if !_frame.is_null() {
                    let Some(dispatcher) =
                        dispatch::Dispatch::new(_frame, self.handler.clone(), self.mirror.clone())
                    else {
                        return kResultFalse;
                    };
                    *self.dispatcher.get() = Some(dispatcher);
                }
            }
            kResultOk
        })
    }
    unsafe fn canResize(&self) -> tresult {
        boundary(kInternalError, || kResultTrue)
    }
    unsafe fn checkSizeConstraint(&self, r: *mut ViewRect) -> tresult {
        boundary(kInternalError, || {
            if r.is_null() {
                return kInvalidArgument;
            }
            (*r).right = (*r).left + ((*r).right - (*r).left).clamp(self.minimum.0 as i32, 4096);
            (*r).bottom = (*r).top + ((*r).bottom - (*r).top).clamp(self.minimum.1 as i32, 4096);
            kResultOk
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[global_allocator]
    static ALLOC: rvst_test::GuardedAllocator<std::alloc::System> =
        rvst_test::GuardedAllocator(std::alloc::System);
    pub(super) mod fixture {
        include!("../../rvst-adapter/test-support/plugin.rs");
    }
    type StateBytes = std::rc::Rc<std::cell::RefCell<Vec<u8>>>;
    struct StateStream {
        data: StateBytes,
        offset: Cell<usize>,
    }
    impl Class for StateStream {
        type Interfaces = (IBStream,);
    }
    impl IBStreamTrait for StateStream {
        unsafe fn read(&self, out: *mut c_void, size: i32, read: *mut i32) -> tresult {
            let data = self.data.borrow();
            let count = (size as usize).min(data.len() - self.offset.get());
            ptr::copy_nonoverlapping(data.as_ptr().add(self.offset.get()), out as *mut u8, count);
            self.offset.set(self.offset.get() + count);
            *read = count as i32;
            kResultOk
        }
        unsafe fn write(&self, bytes: *mut c_void, size: i32, written: *mut i32) -> tresult {
            self.data
                .borrow_mut()
                .extend_from_slice(std::slice::from_raw_parts(
                    bytes as *const u8,
                    size as usize,
                ));
            *written = size;
            kResultOk
        }
        unsafe fn seek(&self, _: i64, _: i32, _: *mut i64) -> tresult {
            kNotImplemented
        }
        unsafe fn tell(&self, _: *mut i64) -> tresult {
            kNotImplemented
        }
    }
    fn state_stream(data: Vec<u8>) -> vst3::ComPtr<IBStream> {
        state_buffer(data).0
    }
    fn state_buffer(data: Vec<u8>) -> (vst3::ComPtr<IBStream>, StateBytes) {
        let data = std::rc::Rc::new(std::cell::RefCell::new(data));
        let stream = ComWrapper::new(StateStream {
            data: data.clone(),
            offset: Cell::new(0),
        })
        .to_com_ptr::<IBStream>()
        .unwrap();
        (stream, data)
    }
    fn history_editor(
        mirror: std::sync::Arc<rvst_adapter::ParameterMirror>,
    ) -> rvst_core::HistoryHost<VstEditor> {
        rvst_core::HistoryHost::new(VstEditor {
            #[cfg(target_os = "linux")]
            mailbox: dispatch::Mailbox::new(mirror.clone()),
            mirror,
            #[cfg(not(target_os = "linux"))]
            handler: Handler(ptr::null_mut()),
        })
    }
    fn record_edit(editor: &mut impl rvst_core::EditorHost) {
        editor.begin_edit(42);
        editor.set_parameter(42, 0.25);
        editor.end_edit(42);
        assert!(editor.undo_label().is_some());
    }
    fn test_controller(connected: bool) -> Controller<fixture::Gain> {
        let fallback = rvst_adapter::ParameterMirror::new(fixture::Gain::descriptor());
        let linked =
            connected.then(|| rvst_adapter::ParameterMirror::new(fixture::Gain::descriptor()));
        Controller::<fixture::Gain> {
            connection: connection::Endpoint::default(),
            linked: UnsafeCell::new(linked),
            mirror: fallback,
            plugin: UnsafeCell::new(rvst_adapter::DropShield::new(fixture::Gain::new())),
            handler: Cell::new(ptr::null_mut()),
            host_context: Cell::new(ptr::null_mut()),
            timer: UnsafeCell::new(None),
            editor: rvst_ui::default_editor,
        }
    }
    fn assert_project_roundtrip(
        mirror: std::sync::Arc<rvst_adapter::ParameterMirror>,
        save: impl Fn(*mut IBStream) -> tresult,
        load: impl Fn(*mut IBStream) -> tresult,
    ) {
        use rvst_core::EditorHost;
        let mut editor = history_editor(mirror.clone());
        record_edit(&mut editor);
        editor.set_parameter(42, 0.5);
        assert!(editor.undo().unwrap());
        let (output, bytes) = state_buffer(Vec::new());
        assert_eq!(save(output.as_ptr()), kResultOk);
        let saved = bytes.borrow().clone();
        let decoded =
            rvst_adapter::decode_native_state(fixture::Gain::descriptor(), &saved).unwrap();
        assert_eq!(decoded.plugin_state, 0.25f64.to_le_bytes());
        assert!(decoded.history.is_some());
        editor.set_parameter(42, 1.5);
        let invalid_history = rvst_adapter::encode_native_state(
            fixture::Gain::descriptor(),
            &1.75f64.to_le_bytes(),
            b"corrupt history",
        )
        .unwrap();
        let invalid = state_stream(invalid_history);
        assert_eq!(load(invalid.as_ptr()), kResultFalse);
        assert_eq!(mirror.get(42), Some(1.5));
        assert!(editor.undo_label().is_some());
        let saved = state_stream(saved);
        assert_eq!(load(saved.as_ptr()), kResultOk);
        assert_eq!(mirror.get(42), Some(0.25));
        assert!(editor.undo_label().is_some());
        assert!(editor.redo_label().is_some());
        assert!(editor.redo().unwrap());
        assert_eq!(mirror.get(42), Some(0.5));
        assert!(editor.undo().unwrap());
        assert!(editor.undo().unwrap());
        assert_eq!(mirror.get(42), Some(1.));
        assert!(!editor.undo().unwrap());
    }
    #[test]
    fn processor_and_controller_project_state_roundtrip_preserves_history() {
        unsafe {
            let processor = Processor::<fixture::Gain>::new();
            assert_project_roundtrip(
                (&*processor.runtime.get()).mirror.clone(),
                |stream| processor.getState(stream),
                |stream| processor.setState(stream),
            );
            for connected in [false, true] {
                let controller = test_controller(connected);
                assert_project_roundtrip(
                    controller.editor_mirror(),
                    |stream| controller.getState(stream),
                    |stream| controller.setState(stream),
                );
            }
        }
    }
    #[test]
    fn zero_frame_flush_preserves_undispatched_editor_intent_for_project_save() {
        use rvst_core::EditorHost;
        unsafe {
            let processor = Processor::<fixture::Gain>::new();
            let mirror = (&*processor.runtime.get()).mirror.clone();
            let mut editor = history_editor(mirror.clone());
            record_edit(&mut editor);
            let mut data: ProcessData = std::mem::zeroed();
            assert_eq!(processor.process(&mut data), kResultOk);
            assert_eq!((&*processor.runtime.get()).plugin.parameter(42), Some(1.));
            assert_eq!(editor.parameter(42), Some(0.25));
            let (output, bytes) = state_buffer(Vec::new());
            assert_eq!(processor.getState(output.as_ptr()), kResultOk);
            let saved = bytes.borrow();
            let decoded =
                rvst_adapter::decode_native_state(fixture::Gain::descriptor(), &saved).unwrap();
            assert_eq!(decoded.plugin_state, 0.25f64.to_le_bytes());
            assert!(decoded.history.is_some());
        }
    }
    #[test]
    fn processor_restore_invalidates_reopened_history_only_on_success() {
        use rvst_core::EditorHost;
        unsafe {
            let processor = Processor::<fixture::Gain>::new();
            let mirror = (&*processor.runtime.get()).mirror.clone();
            let mut editor = history_editor(mirror.clone());
            record_edit(&mut editor);
            drop(editor);
            let reopened = history_editor(mirror.clone());
            assert!(reopened.undo_label().is_some());
            let bad = state_stream(vec![0]);
            assert_eq!(processor.setState(bad.as_ptr()), kResultFalse);
            assert!(reopened.undo_label().is_some());
            let saved = state_stream(0.75f64.to_le_bytes().to_vec());
            assert_eq!(processor.setState(saved.as_ptr()), kResultOk);
            assert!(reopened.undo_label().is_none());
            assert_eq!(mirror.get(42), Some(0.75));
        }
    }
    #[test]
    fn controller_restore_invalidates_connected_and_fallback_history_only_on_success() {
        use rvst_core::EditorHost;
        unsafe {
            for connected in [false, true] {
                let controller = test_controller(connected);
                let mut editor = history_editor(controller.editor_mirror());
                record_edit(&mut editor);
                let mut fallback_editor = history_editor(controller.mirror.clone());
                if connected {
                    record_edit(&mut fallback_editor);
                }
                assert_eq!(controller.setParamNormalized(42, 0.4), kResultOk);
                assert!(editor.undo_label().is_some());
                let bad = state_stream(vec![0]);
                assert_eq!(controller.setState(bad.as_ptr()), kResultFalse);
                assert!(editor.undo_label().is_some());
                assert!(fallback_editor.undo_label().is_some());
                let saved = state_stream(0.75f64.to_le_bytes().to_vec());
                assert_eq!(controller.setState(saved.as_ptr()), kResultOk);
                assert!(editor.undo_label().is_none());
                assert!(fallback_editor.undo_label().is_none());
                assert_eq!(controller.editor_mirror().get(42), Some(0.75));
                assert_eq!(controller.mirror.get(42), Some(0.75));
            }
        }
    }
    struct Queue(Vec<(i32, f64)>);
    impl Class for Queue {
        type Interfaces = (IParamValueQueue,);
    }
    impl IParamValueQueueTrait for Queue {
        unsafe fn getParameterId(&self) -> u32 {
            42
        }
        unsafe fn getPointCount(&self) -> i32 {
            self.0.len() as i32
        }
        unsafe fn getPoint(&self, index: i32, offset: *mut i32, value: *mut f64) -> tresult {
            if let Some((t, v)) = self.0.get(index as usize) {
                *offset = *t;
                *value = *v;
                kResultOk
            } else {
                kInvalidArgument
            }
        }
        unsafe fn addPoint(&self, _: i32, _: f64, _: *mut i32) -> tresult {
            kNotImplemented
        }
    }
    #[test]
    fn implicit_sample_minus_one_linear_ramp_and_stepped_boundaries() {
        unsafe {
            let q = ComWrapper::new(Queue(vec![(3, 1.), (7, 0.)]))
                .to_com_ptr::<IParamValueQueue>()
                .unwrap();
            let q = ComRef::from_raw(q.as_ptr()).unwrap();
            assert_eq!(queue_value(q, 0, 0., false), Some(0.25));
            assert_eq!(queue_value(q, 3, 0., false), Some(1.));
            assert_eq!(queue_value(q, 5, 0., false), Some(0.5));
            assert_eq!(queue_value(q, 2, 0., true), Some(0.));
            assert_eq!(queue_value(q, 3, 0., true), Some(1.));
        }
    }
    #[test]
    fn native_factory_separate_component_controller_and_refcounts() {
        unsafe {
            let f = factory::<fixture::Gain>(rvst_ui::default_editor);
            assert!(!f.is_null());
            let f = vst3::ComPtr::from_raw(f).unwrap();
            assert_eq!(f.countClasses(), 2);
            let mut c: PClassInfo = std::mem::zeroed();
            assert_eq!(f.getClassInfo(0, &mut c), kResultOk);
            let mut out = ptr::null_mut();
            use vst3::Interface;
            assert_eq!(
                f.createInstance(c.cid.as_ptr(), IComponent::IID.as_ptr() as _, &mut out),
                kResultOk
            );
            let component = vst3::ComPtr::<IComponent>::from_raw(out as _).unwrap();
            let mut controller_id = [0; 16];
            assert_eq!(
                component.getControllerClassId(&mut controller_id),
                kResultOk
            );
            assert_ne!(controller_id, c.cid);
            let proc = component.cast::<IAudioProcessor>().unwrap();
            assert_eq!(
                proc.canProcessSampleSize(SymbolicSampleSizes_::kSample32 as i32),
                kResultOk
            );
            assert_ne!(
                proc.canProcessSampleSize(SymbolicSampleSizes_::kSample64 as i32),
                kResultOk
            );
            let mut setup = ProcessSetup {
                processMode: 0,
                symbolicSampleSize: 0,
                maxSamplesPerBlock: 16,
                sampleRate: 48000.,
            };
            assert_eq!(proc.setupProcessing(&mut setup), kResultOk);
            assert_eq!(component.setActive(1), kResultOk);
            assert_eq!(proc.setProcessing(1), kResultOk);
            let mut l = [1.; 4];
            let mut r = [2.; 4];
            let mut ptrs = [l.as_mut_ptr(), r.as_mut_ptr()];
            let mut bus = AudioBusBuffers {
                numChannels: 2,
                silenceFlags: 0,
                __field0: AudioBusBuffers__type0 {
                    channelBuffers32: ptrs.as_mut_ptr(),
                },
            };
            let mut data: ProcessData = std::mem::zeroed();
            data.numSamples = 4;
            data.numInputs = 1;
            data.numOutputs = 1;
            data.inputs = &mut bus;
            data.outputs = &mut bus;
            let guard = rvst_test::AllocationGuard::start();
            let status = proc.process(&mut data);
            let counts = guard.finish();
            assert_eq!(counts, rvst_test::AllocationCounts::default());
            assert_eq!(status, kResultOk);
            assert_eq!(l, [1.; 4]);
            assert_eq!(r, [2.; 4]);
            assert_eq!(proc.setProcessing(0), kResultOk);
            assert_eq!(component.setActive(0), kResultOk);
            let mut out = ptr::null_mut();
            assert_eq!(
                f.createInstance(
                    controller_id.as_ptr(),
                    IEditController::IID.as_ptr() as _,
                    &mut out
                ),
                kResultOk
            );
            let controller = vst3::ComPtr::<IEditController>::from_raw(out as _).unwrap();
            assert_eq!(controller.getParameterCount(), 2081);
            let mapping = controller.cast::<IMidiMapping>().unwrap();
            let mut midi_id = 0;
            assert_eq!(
                mapping.getMidiControllerAssignment(0, 15, 129, &mut midi_id),
                kResultOk
            );
            assert_eq!(midi_id, (1u32 << 31) - 1);
            assert_eq!(
                mapping.getMidiControllerAssignment(1, 0, 1, &mut midi_id),
                kInvalidArgument
            );
            let mut info: ParameterInfo = std::mem::zeroed();
            assert_eq!(controller.getParameterInfo(1, &mut info), kResultOk);
            assert_eq!(info.id, (1u32 << 31) - 2080);
            assert_ne!(info.flags & ParameterInfo_::ParameterFlags_::kIsHidden, 0);
            assert_eq!(
                controller.getParameterInfo(2081, &mut info),
                kInvalidArgument
            );
            assert_eq!(controller.setParamNormalized(42, 0.25), kResultOk);
            assert_eq!(controller.getParamNormalized(42), 0.25);
        }
    }
}

unsafe fn silence(data: &ProcessData) {
    if data.outputs.is_null() || data.numSamples < 0 {
        return;
    }
    for b in 0..data.numOutputs.max(0) as usize {
        let b = &mut *data.outputs.add(b);
        for c in 0..b.numChannels.max(0) as usize {
            if data.symbolicSampleSize == SymbolicSampleSizes_::kSample32 as i32 {
                if !b.__field0.channelBuffers32.is_null() {
                    let p = *b.__field0.channelBuffers32.add(c);
                    if !p.is_null() {
                        ptr::write_bytes(p, 0, data.numSamples as usize);
                    }
                }
            } else if !b.__field0.channelBuffers64.is_null() {
                let p = *b.__field0.channelBuffers64.add(c);
                if !p.is_null() {
                    ptr::write_bytes(p, 0, data.numSamples as usize);
                }
            }
        }
        b.silenceFlags = u64::MAX;
    }
}

impl<P: Plugin> IPluginFactory2Trait for Factory<P> {
    unsafe fn getClassInfo2(&self, index: i32, out: *mut PClassInfo2) -> tresult {
        boundary(kInternalError, || {
            if out.is_null() {
                return kInvalidArgument;
            }
            let mut base: PClassInfo = std::mem::zeroed();
            let result = self.getClassInfo(index, &mut base);
            if result != kResultOk {
                return result;
            }
            *out = std::mem::zeroed();
            let info = &mut *out;
            info.cid = base.cid;
            info.cardinality = base.cardinality;
            info.category = base.category;
            info.name = base.name;
            let d = P::descriptor();
            narrow(d.vendor, &mut info.vendor);
            narrow(d.version, &mut info.version);
            narrow("VST 3.7.12", &mut info.sdkVersion);
            if index == 0 {
                narrow(
                    match d.kind {
                        rvst_core::PluginKind::Effect => "Fx",
                        rvst_core::PluginKind::Instrument => "Instrument|Synth",
                        rvst_core::PluginKind::Midi => "Fx|Tools",
                    },
                    &mut info.subCategories,
                );
            }
            kResultOk
        })
    }
}

impl<P: Plugin> Controller<P> {
    unsafe fn restart_timer(&self) {
        (&mut *self.timer.get()).take();
        if self.handler.get().is_null() {
            return;
        }
        retain(self.handler.get() as _);
        let handler = Handler(self.handler.get());
        let mirror = self.editor_mirror();
        let mut previous = mirror.latency.load(Ordering::Acquire);
        *self.timer.get() = timer::MainTimer::new(self.host_context.get(), move || {
            let latency = mirror.latency.load(Ordering::Acquire);
            if latency != previous {
                previous = latency;
                if let Some(h) = ComRef::from_raw(handler.0) {
                    h.restartComponent(RestartFlags_::kLatencyChanged);
                }
            }
        });
    }
    fn editor_mirror(&self) -> std::sync::Arc<rvst_adapter::ParameterMirror> {
        unsafe {
            (&*self.linked.get())
                .as_ref()
                .unwrap_or(&self.mirror)
                .clone()
        }
    }
}

#[cfg(test)]
mod bus_regression_tests {
    use super::*;
    use rvst_core::{AudioBlock, AudioBus, Error, EventSink, PluginDescriptor, ProcessConfig};
    struct MultiBus;
    impl Plugin for MultiBus {
        fn descriptor() -> &'static PluginDescriptor {
            static D: std::sync::OnceLock<PluginDescriptor> = std::sync::OnceLock::new();
            D.get_or_init(|| {
                let mut d = super::tests::fixture::Gain::descriptor().clone();
                d.inputs = &[
                    AudioBus {
                        name: "Main",
                        channels: 2,
                        auxiliary: false,
                    },
                    AudioBus {
                        name: "Aux one",
                        channels: 2,
                        auxiliary: true,
                    },
                    AudioBus {
                        name: "Aux two",
                        channels: 2,
                        auxiliary: true,
                    },
                ];
                d.outputs = &[
                    AudioBus {
                        name: "Main",
                        channels: 2,
                        auxiliary: false,
                    },
                    AudioBus {
                        name: "Aux",
                        channels: 2,
                        auxiliary: true,
                    },
                ];
                d.supports_f64 = true;
                d
            })
        }
        fn new() -> Self {
            Self
        }
        fn prepare(&mut self, _: ProcessConfig) -> Result<(), Error> {
            Ok(())
        }
        fn process(
            &mut self,
            b: &mut AudioBlock<'_, f32>,
            _: &[CoreEvent],
            _: &Transport,
            _: &mut dyn EventSink,
        ) -> ProcessStatus {
            for c in 0..2 {
                for f in 0..b.frames {
                    b.outputs[c * b.frames + f] = b.inputs[c * b.frames + f]
                        + b.inputs[(c + 2) * b.frames + f]
                        + b.inputs[(c + 4) * b.frames + f];
                }
            }
            ProcessStatus::Continue
        }
        fn process_f64(
            &mut self,
            b: &mut AudioBlock<'_, f64>,
            _: &[CoreEvent],
            _: &Transport,
            _: &mut dyn EventSink,
        ) -> ProcessStatus {
            for c in 0..2 {
                for f in 0..b.frames {
                    b.outputs[c * b.frames + f] = b.inputs[c * b.frames + f]
                        + b.inputs[(c + 2) * b.frames + f]
                        + b.inputs[(c + 4) * b.frames + f];
                }
            }
            ProcessStatus::Continue
        }
        fn reset(&mut self) {}
        fn parameter(&self, _: u32) -> Option<f64> {
            Some(1.)
        }
        fn set_parameter(&mut self, _: u32, _: f64) -> Result<(), Error> {
            Ok(())
        }
        fn save_state(&self) -> Result<Vec<u8>, Error> {
            Ok(vec![])
        }
        fn load_state(&mut self, _: &[u8]) -> Result<(), Error> {
            Ok(())
        }
    }
    #[test]
    fn inactive_aux_buses_allow_omission_and_sparse_placeholders() {
        unsafe {
            let proc = Processor::<MultiBus>::new();
            let mut setup = ProcessSetup {
                processMode: 0,
                symbolicSampleSize: 0,
                maxSamplesPerBlock: 16,
                sampleRate: 48000.,
            };
            assert_eq!(proc.setupProcessing(&mut setup), kResultOk);
            proc.setActive(1);
            proc.setProcessing(1);
            proc.activateBus(0, 0, 1, 0);
            proc.activateBus(0, 0, 2, 0);
            proc.activateBus(0, 1, 1, 0);
            let mut l = [1f32; 4];
            let mut r = [2f32; 4];
            let mut pointers = [l.as_mut_ptr(), r.as_mut_ptr()];
            let main = AudioBusBuffers {
                numChannels: 2,
                silenceFlags: 0,
                __field0: AudioBusBuffers__type0 {
                    channelBuffers32: pointers.as_mut_ptr(),
                },
            };
            let mut inputs = [
                main,
                AudioBusBuffers {
                    numChannels: 0,
                    silenceFlags: 0,
                    __field0: AudioBusBuffers__type0 {
                        channelBuffers32: ptr::null_mut(),
                    },
                },
                main,
            ];
            let mut output = main;
            let mut d: ProcessData = std::mem::zeroed();
            d.numSamples = 4;
            d.numInputs = 1;
            d.numOutputs = 1;
            d.inputs = inputs.as_mut_ptr();
            d.outputs = &mut output;
            let guard = rvst_test::AllocationGuard::start();
            let status = proc.process(&mut d);
            let counts = guard.finish();
            assert_eq!(status, kResultOk);
            assert_eq!(counts, rvst_test::AllocationCounts::default());
            assert_eq!(l, [1.; 4]);
            assert_eq!(r, [2.; 4]);
            // Aux two keeps its declared planar offset despite the inactive hole before it.
            proc.activateBus(0, 0, 2, 1);
            d.numInputs = 3;
            assert_eq!(proc.process(&mut d), kResultOk);
            assert_eq!(l, [2.; 4]);
            assert_eq!(r, [4.; 4]);
            proc.setProcessing(0);
        }
    }
}

#[cfg(test)]
mod alias_regression_tests {
    use super::*;
    struct Notes;
    impl Class for Notes {
        type Interfaces = (IEventList,);
    }
    impl IEventListTrait for Notes {
        unsafe fn getEventCount(&self) -> i32 {
            1
        }
        unsafe fn getEvent(&self, index: i32, out: *mut Event) -> tresult {
            if index != 0 {
                return kInvalidArgument;
            }
            unsafe {
                *out = std::mem::zeroed();
                (*out).r#type = Event_::EventTypes_::kNoteOnEvent as u16;
                (*out).__field0.noteOn = NoteOnEvent {
                    channel: 0,
                    pitch: 60,
                    tuning: 0.,
                    velocity: 1.,
                    length: 0,
                    noteId: 1,
                };
            }
            kResultOk
        }
        unsafe fn addEvent(&self, _: *mut Event) -> tresult {
            kResultOk
        }
    }
    #[test]
    fn eventful_processing_snapshots_overlapping_host_channels() {
        unsafe {
            let proc = Processor::<tests::fixture::Gain>::new();
            let mut setup = ProcessSetup {
                processMode: 0,
                symbolicSampleSize: 0,
                maxSamplesPerBlock: 16,
                sampleRate: 48000.,
            };
            assert_eq!(proc.setupProcessing(&mut setup), kResultOk);
            proc.setActive(1);
            proc.setProcessing(1);
            let mut l = [1f32, 2., 3., 4., 0.];
            let mut r = [5f32, 6., 7., 8., 0.];
            let mut ip = [l.as_mut_ptr(), r.as_mut_ptr()];
            let mut op = [l.as_mut_ptr().add(1), r.as_mut_ptr().add(1)];
            let mut input = AudioBusBuffers {
                numChannels: 2,
                silenceFlags: 0,
                __field0: AudioBusBuffers__type0 {
                    channelBuffers32: ip.as_mut_ptr(),
                },
            };
            let mut output = AudioBusBuffers {
                numChannels: 2,
                silenceFlags: 0,
                __field0: AudioBusBuffers__type0 {
                    channelBuffers32: op.as_mut_ptr(),
                },
            };
            let notes = ComWrapper::new(Notes).to_com_ptr::<IEventList>().unwrap();
            let mut d: ProcessData = std::mem::zeroed();
            d.numSamples = 4;
            d.numInputs = 1;
            d.numOutputs = 1;
            d.inputs = &mut input;
            d.outputs = &mut output;
            d.inputEvents = notes.as_ptr();
            let guard = rvst_test::AllocationGuard::start();
            let result = proc.process(&mut d);
            let counts = guard.finish();
            assert_eq!(result, kResultOk);
            assert_eq!(counts, rvst_test::AllocationCounts::default());
            assert_eq!(l, [1., 1., 2., 3., 4.]);
            assert_eq!(r, [5., 5., 6., 7., 8.]);
        }
    }
}
