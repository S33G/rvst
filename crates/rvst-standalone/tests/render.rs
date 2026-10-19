use rvst_core::*;
use rvst_standalone::*;
static PARAMS: [ParameterSpec; 1] = [ParameterSpec {
    id: 7,
    name: "Gain",
    unit: "",
    min: 0.0,
    max: 2.0,
    default: 1.0,
    step: None,
    bypass: false,
}];
static BUSES: [AudioBus; 1] = [AudioBus {
    name: "Main",
    channels: 2,
    auxiliary: false,
}];
static DESC: PluginDescriptor = PluginDescriptor {
    id: "test.gain",
    name: "Test gain",
    vendor: "RVST",
    version: "0.1",
    url: "",
    email: "",
    vst3_id: [1; 16],
    kind: PluginKind::Effect,
    parameters: &PARAMS,
    inputs: &BUSES,
    outputs: &BUSES,
    midi_input: false,
    midi_output: false,
    supports_f64: false,
};
struct Gain(f64);
impl Plugin for Gain {
    fn latency(&self) -> u32 {
        (self.0 * 64.) as u32
    }
    fn diagnostic_events() -> &'static [(u32, &'static str)] {
        &[(500, "Fixture parameter applied")]
    }
    fn descriptor() -> &'static PluginDescriptor {
        &DESC
    }
    fn new() -> Self {
        Self(1.0)
    }
    fn prepare(&mut self, c: ProcessConfig) -> Result<(), Error> {
        c.validate()
    }
    fn process(
        &mut self,
        b: &mut AudioBlock<'_, f32>,
        _: &[Event],
        _: &Transport,
        _: &mut dyn EventSink,
    ) -> ProcessStatus {
        for (i, o) in b.inputs.iter().zip(b.outputs.iter_mut()) {
            *o = *i * self.0 as f32;
        }
        ProcessStatus::Continue
    }
    fn reset(&mut self) {}
    fn parameter(&self, id: u32) -> Option<f64> {
        (id == 7).then_some(self.0)
    }
    fn set_parameter(&mut self, id: u32, v: f64) -> Result<(), Error> {
        if id != 7 {
            return Err("unknown parameter".into());
        }
        self.0 = PARAMS[0].clamp(v);
        Ok(())
    }
    fn save_state(&self) -> Result<Vec<u8>, Error> {
        Ok(self.0.to_le_bytes().to_vec())
    }
    fn load_state(&mut self, v: &[u8]) -> Result<(), Error> {
        self.0 = f64::from_le_bytes(v.try_into().map_err(|_| Error("bad state".into()))?);
        Ok(())
    }
}
#[test]
fn silence_default_opens_no_input_path() {
    let mut e = AudioEngine::<Gain>::new(48000.0, InputSource::default()).unwrap();
    let mut out = vec![f32::NAN; 2054];
    e.render(&mut out, 2);
    assert!(out.iter().all(|v| *v == 0.0));
    assert_eq!(e.sample_clock, 1027);
}
#[test]
fn tone_and_control_edits_apply_at_block_boundary() {
    let mut e = AudioEngine::<Gain>::new(
        48000.0,
        InputSource::TestTone {
            frequency: 1000.0,
            amplitude: 0.1,
        },
    )
    .unwrap();
    e.mirror.edit(7, 0.5);
    let mut out = vec![0.0f32; 2054];
    e.render(&mut out, 2);
    for (f, frame) in out.as_chunks::<2>().0.iter().enumerate() {
        let expected = (f as f64 * 1000.0 * std::f64::consts::TAU / 48000.0).sin() as f32 * 0.05;
        assert!((frame[0] - expected).abs() < 1e-6);
        assert_eq!(frame[0], frame[1]);
    }
}
#[test]
fn unused_device_channels_are_silent() {
    let mut e = AudioEngine::<Gain>::new(
        48000.0,
        InputSource::TestTone {
            frequency: 1000.0,
            amplitude: 0.1,
        },
    )
    .unwrap();
    let mut out = vec![0.0f32; 33];
    e.render(&mut out, 3);
    assert!(out.as_chunks::<3>().0.iter().all(|f| f[2] == 0.0));
}
#[test]
fn cli_requires_finite_bounded_options() {
    assert!(StandaloneOptions::from_args(["--no-ui".into()]).is_err());
    assert!(StandaloneOptions::from_args(["--seconds".into(), "NaN".into()]).is_err());
    assert!(
        AudioEngine::<Gain>::new(
            48000.0,
            InputSource::TestTone {
                frequency: 30000.0,
                amplitude: 0.1
            }
        )
        .is_err()
    );
    assert!(
        StandaloneOptions::from_args(["--no-ui".into(), "--seconds".into(), "0.01".into()]).is_ok()
    );
}

#[test]
fn partial_device_frame_is_silenced() {
    let mut e = AudioEngine::<Gain>::new(48000.0, InputSource::default()).unwrap();
    let mut output = [f32::NAN; 5];
    e.render(&mut output, 2);
    assert_eq!(output, [0.0; 5]);
    assert_eq!(e.sample_clock, 2);
}

#[global_allocator]
static ALLOC: rvst_test::GuardedAllocator<std::alloc::System> =
    rvst_test::GuardedAllocator(std::alloc::System);
struct Reject {
    inner: Gain,
    error: Option<Error>,
}
impl Plugin for Reject {
    fn descriptor() -> &'static PluginDescriptor {
        Gain::descriptor()
    }
    fn new() -> Self {
        Self {
            inner: Gain::new(),
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
fn rejected_parameter_does_not_destroy_heap_memory_on_audio() {
    let mut engine = AudioEngine::<Reject>::new(48000., InputSource::Silence).unwrap();
    engine.mirror.edit(7, 0.5);
    let mut out = [0f32; 64];
    let guard = rvst_test::AllocationGuard::start();
    engine.render(&mut out, 2);
    let counts = guard.finish();
    assert_eq!(counts, rvst_test::AllocationCounts::default());
    assert_eq!(engine.mirror.get(7), Some(1.0));
}

#[cfg(feature = "devtools")]
#[test]
fn standalone_inspector_reports_live_parameters_meters_latency_and_named_logs() {
    let directory =
        std::env::temp_dir().join(format!("rvst-standalone-inspector-{}", std::process::id()));
    let (session, rt_log) =
        rvst_devtools::DebugSession::start_in(DESC.id, DESC.name, &directory).unwrap();
    let mut engine = AudioEngine::<Gain>::new(
        48000.,
        InputSource::TestTone {
            frequency: 1000.,
            amplitude: 0.1,
        },
    )
    .unwrap();
    engine.connect_inspector(&session, rt_log).unwrap();
    assert!(std::sync::Arc::ptr_eq(
        &engine.meters,
        &engine.mirror.meters
    ));
    let before = rvst_devtools::snapshot(session.info(), 0).unwrap();
    assert_eq!(before.snapshot["live"]["parameters"][0]["value"], 1.);
    assert_eq!(before.snapshot["live"]["meters"]["latency_samples"], 64);
    engine.mirror.edit(7, 1.5);
    let mut output = [0f32; 64];
    let guard = rvst_test::AllocationGuard::start();
    engine.render(&mut output, 2);
    assert_eq!(guard.finish(), rvst_test::AllocationCounts::default());
    let mut log = session.plugin_log().unwrap();
    assert!(log.emit(500, engine.sample_clock, 1.5));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let after = loop {
        let after = rvst_devtools::snapshot(session.info(), 0).unwrap();
        if after
            .events
            .iter()
            .any(|event| event.message == "Fixture parameter applied")
        {
            break after;
        }
        assert!(std::time::Instant::now() < deadline);
    };
    assert_eq!(after.snapshot["live"]["parameters"][0]["value"], 1.5);
    assert_eq!(after.snapshot["live"]["meters"]["sample_clock"], 32);
    assert_eq!(after.snapshot["live"]["meters"]["latency_samples"], 96);
    assert!(
        after.snapshot["live"]["meters"]["output_peak"]
            .as_f64()
            .unwrap()
            > 0.1
    );
    assert_eq!(after.metrics.callbacks, 1);
    drop(session);
    std::fs::remove_dir(directory).unwrap();
}
