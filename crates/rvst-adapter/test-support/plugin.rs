use rvst_core::*;
static PARAMS: [ParameterSpec; 1] = [ParameterSpec {
    id: 42,
    name: "Gain",
    unit: "x",
    min: 0.,
    max: 2.,
    default: 1.,
    step: None,
    bypass: false,
}];
static BUSES: [AudioBus; 1] = [AudioBus {
    name: "Stereo",
    channels: 2,
    auxiliary: false,
}];
static DESC: PluginDescriptor = PluginDescriptor {
    id: "org.rvst.test",
    name: "ABI gain",
    vendor: "RVST",
    version: "0.1.0",
    url: "",
    email: "",
    vst3_id: *b"rvst-test-gain12",
    kind: PluginKind::Effect,
    parameters: &PARAMS,
    inputs: &BUSES,
    outputs: &BUSES,
    midi_input: true,
    midi_output: true,
    supports_f64: false,
};
pub struct Gain {
    pub gain: f64,
    pub context: std::sync::Arc<std::sync::atomic::AtomicU64>,
}
impl Plugin for Gain {
    fn descriptor() -> &'static PluginDescriptor {
        &DESC
    }
    fn new() -> Self {
        Self {
            gain: 1.,
            context: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }
    fn editor_context(&self) -> Option<EditorContext> {
        Some(self.context.clone())
    }
    fn prepare(&mut self, _: ProcessConfig) -> Result<(), Error> {
        Ok(())
    }
    fn process(
        &mut self,
        b: &mut AudioBlock<'_, f32>,
        events: &[Event],
        _: &Transport,
        out: &mut dyn EventSink,
    ) -> ProcessStatus {
        for e in events {
            if let EventKind::Parameter { id: 42, value } = e.kind {
                self.gain = value
            } else {
                out.push(*e);
            }
        }
        for (i, o) in b.inputs.iter().zip(b.outputs.iter_mut()) {
            *o = *i * self.gain as f32;
        }
        ProcessStatus::Continue
    }
    fn reset(&mut self) {}
    fn parameter(&self, id: u32) -> Option<f64> {
        if id == 42 { Some(self.gain) } else { None }
    }
    fn set_parameter(&mut self, id: u32, v: f64) -> Result<(), Error> {
        if id != 42 {
            return Err("unknown ID".into());
        }
        self.gain = v;
        Ok(())
    }
    fn save_state(&self) -> Result<Vec<u8>, Error> {
        Ok(self.gain.to_le_bytes().to_vec())
    }
    fn load_state(&mut self, b: &[u8]) -> Result<(), Error> {
        let a: [u8; 8] = b.try_into().map_err(|_| Error("bad state".into()))?;
        let v = f64::from_le_bytes(a);
        if !v.is_finite() {
            return Err("bad value".into());
        }
        self.gain = v;
        Ok(())
    }
}
