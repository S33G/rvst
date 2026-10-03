use rvst_core::*;
use rvst_runtime::*;
static PARAMS: [ParameterSpec; 1] = [ParameterSpec {
    id: 1,
    name: "Gain",
    unit: "",
    min: 0.,
    max: 2.,
    default: 1.,
    step: None,
    bypass: false,
}];
static BUS: [AudioBus; 1] = [AudioBus {
    name: "Main",
    channels: 1,
    auxiliary: false,
}];
static DESC: PluginDescriptor = PluginDescriptor {
    id: "org.rvst.test",
    name: "Test",
    vendor: "Test",
    version: "1",
    url: "",
    email: "",
    vst3_id: [1; 16],
    kind: PluginKind::Effect,
    parameters: &PARAMS,
    inputs: &BUS,
    outputs: &BUS,
    midi_input: true,
    midi_output: true,
    supports_f64: false,
};
struct TestPlugin {
    gain: f32,
}
impl Plugin for TestPlugin {
    fn descriptor() -> &'static PluginDescriptor {
        &DESC
    }
    fn new() -> Self {
        Self { gain: 1. }
    }
    fn prepare(&mut self, c: ProcessConfig) -> Result<(), Error> {
        c.validate()
    }
    fn process(
        &mut self,
        b: &mut AudioBlock<'_, f32>,
        events: &[Event],
        _: &Transport,
        sink: &mut dyn EventSink,
    ) -> ProcessStatus {
        for i in 0..b.frames {
            for e in events.iter().filter(|e| e.frame as usize == i) {
                match e.kind {
                    EventKind::Parameter { id: 1, value } => self.gain = value as f32,
                    _ => {
                        sink.push(*e);
                    }
                }
            }
            b.outputs[i] = b.inputs[i] * self.gain;
        }
        ProcessStatus::Continue
    }
    fn reset(&mut self) {}
    fn parameter(&self, id: u32) -> Option<f64> {
        (id == 1).then_some(self.gain as f64)
    }
    fn set_parameter(&mut self, _: u32, v: f64) -> Result<(), Error> {
        self.gain = v as f32;
        Ok(())
    }
    fn save_state(&self) -> Result<Vec<u8>, Error> {
        Ok(self.gain.to_le_bytes().to_vec())
    }
    fn load_state(&mut self, v: &[u8]) -> Result<(), Error> {
        self.gain = f32::from_le_bytes(v.try_into().map_err(|_| Error::from("bad state"))?);
        Ok(())
    }
}
fn capture() -> Recording {
    Recording {
        start: CaptureStart::Reset,
        schema_version: 1,
        plugin_id: DESC.id.into(),
        config: ProcessConfig {
            sample_rate: 48000.,
            max_frames: 4,
            input_channels: 1,
            output_channels: 1,
        },
        initial_state: 1f32.to_le_bytes().to_vec(),
        reproducible_start: true,
        lost_blocks: 0,
        blocks: vec![
            RecordedBlock {
                input: vec![1., 2.],
                output: vec![1., 4.],
                frames: 2,
                events: vec![Event {
                    frame: 1,
                    kind: EventKind::Parameter { id: 1, value: 2. },
                }],
                transport: Transport::default(),
                precision: Precision::F32,
                output_events: Some(vec![]),
                status: Some(ProcessStatus::Continue),
            },
            RecordedBlock {
                input: vec![3.],
                output: vec![6.],
                frames: 1,
                events: vec![Event {
                    frame: 0,
                    kind: EventKind::NoteOff {
                        channel: 0,
                        key: 60,
                        velocity: 0.,
                        note_id: 3,
                    },
                }],
                transport: Transport::default(),
                precision: Precision::F32,
                output_events: Some(vec![Event {
                    frame: 0,
                    kind: EventKind::NoteOff {
                        channel: 0,
                        key: 60,
                        velocity: 0.,
                        note_id: 3,
                    },
                }]),
                status: Some(ProcessStatus::Continue),
            },
        ],
    }
}
#[test]
fn step_retains_state_and_verifies_audio_events_and_status() {
    let r = capture();
    let mut cursor = r.cursor::<TestPlugin>(0.).unwrap();
    assert_eq!(cursor.step().unwrap().unwrap().index, 0);
    assert_eq!(cursor.step().unwrap().unwrap().index, 1);
    assert!(cursor.step().unwrap().is_none());
    let report = r.replay::<TestPlugin>(0.).unwrap();
    assert!(report.matches);
    assert_eq!(report.compared_samples, 3);
    assert_eq!(report.compared_output_events, 1);
}
#[test]
fn incomplete_and_unrecorded_midi_output_cannot_claim_verification() {
    let mut r = capture();
    r.lost_blocks = 1;
    assert!(r.replay::<TestPlugin>(0.).is_err());
    r.lost_blocks = 0;
    r.blocks[0].output_events = None;
    assert!(r.replay::<TestPlugin>(0.).is_err());
}
#[test]
fn replay_detects_wrong_output_events_and_audio() {
    let mut r = capture();
    r.blocks[1].output_events = Some(vec![]);
    assert!(!r.replay::<TestPlugin>(0.).unwrap().matches);
    let mut r = capture();
    r.blocks[1].output[0] = 0.;
    assert!(!r.replay::<TestPlugin>(0.).unwrap().matches);
}

#[test]
fn checkpoint_replay_keeps_restored_dsp_history_and_requires_observed_status() {
    struct History(TestPlugin);
    impl Plugin for History {
        fn descriptor() -> &'static PluginDescriptor {
            &DESC
        }
        fn new() -> Self {
            Self(TestPlugin::new())
        }
        fn prepare(&mut self, c: ProcessConfig) -> Result<(), Error> {
            self.0.prepare(c)
        }
        fn reset(&mut self) {
            self.0.gain = 0.;
        }
        fn process(
            &mut self,
            b: &mut AudioBlock<'_, f32>,
            e: &[Event],
            t: &Transport,
            o: &mut dyn EventSink,
        ) -> ProcessStatus {
            self.0.process(b, e, t, o)
        }
        fn parameter(&self, id: u32) -> Option<f64> {
            self.0.parameter(id)
        }
        fn set_parameter(&mut self, id: u32, v: f64) -> Result<(), Error> {
            self.0.set_parameter(id, v)
        }
        fn save_state(&self) -> Result<Vec<u8>, Error> {
            self.0.save_state()
        }
        fn load_state(&mut self, b: &[u8]) -> Result<(), Error> {
            self.0.load_state(b)
        }
    }
    let mut r = capture();
    r.start = CaptureStart::Checkpoint;
    assert!(r.replay::<History>(0.).unwrap().matches);
    r.start = CaptureStart::Reset;
    assert!(!r.replay::<History>(0.).unwrap().matches);
    r.blocks[0].status = None;
    assert!(r.replay::<History>(0.).is_err());
}
