//! CPAL device runner. No input device is opened; input is explicit silence, tone, or WAV.
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rvst_adapter::{MirrorEditor, ParameterMirror};
use rvst_core::*;
use rvst_ui::EditorFn;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

#[derive(Clone, Debug, Default)]
pub enum InputSource {
    #[default]
    Silence,
    TestTone {
        frequency: f64,
        amplitude: f32,
    },
    Wav(std::path::PathBuf),
}
#[derive(Clone, Debug, Default)]
pub struct StandaloneOptions {
    pub input: InputSource,
    pub duration: Option<std::time::Duration>,
    pub no_ui: bool,
}
impl StandaloneOptions {
    pub fn from_args(args: impl IntoIterator<Item = String>) -> Result<Self, Error> {
        let mut options = Self::default();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--tone" => {
                    let frequency = args
                        .next()
                        .ok_or_else(|| Error("--tone requires Hz".into()))?
                        .parse::<f64>()
                        .map_err(|_| Error("invalid tone frequency".into()))?;
                    if !frequency.is_finite() || frequency <= 0.0 {
                        return Err("tone must be positive Hz".into());
                    }
                    options.input = InputSource::TestTone {
                        frequency,
                        amplitude: 0.1,
                    };
                }
                "--file" => {
                    options.input = InputSource::Wav(
                        args.next()
                            .ok_or_else(|| Error("--file requires a WAV path".into()))?
                            .into(),
                    );
                }
                "--seconds" => {
                    let value = args
                        .next()
                        .ok_or_else(|| Error("--seconds requires a duration".into()))?
                        .parse::<f64>()
                        .map_err(|_| Error("invalid duration".into()))?;
                    if !value.is_finite() || value <= 0.0 {
                        return Err("duration must be positive".into());
                    }
                    options.duration = Some(std::time::Duration::from_secs_f64(value));
                }
                "--no-ui" => options.no_ui = true,
                _ => {
                    return Err(Error(format!(
                        "unknown argument {arg}; supported: --tone Hz, --file input.wav, --seconds N, --no-ui"
                    )));
                }
            }
        }
        if options.no_ui && options.duration.is_none() {
            return Err("--no-ui requires --seconds".into());
        }
        Ok(options)
    }
}
struct WavData {
    samples: Vec<f32>,
    channels: usize,
    rate: f64,
}
impl WavData {
    fn load(path: &std::path::Path) -> Result<Self, Error> {
        let mut reader = hound::WavReader::open(path).map_err(|e| Error(e.to_string()))?;
        let spec = reader.spec();
        if spec.channels == 0 || spec.sample_rate == 0 {
            return Err("invalid WAV shape".into());
        }
        let samples = match spec.sample_format {
            hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<Vec<_>, _>>(),
            hound::SampleFormat::Int => {
                let scale = 2f32.powi(spec.bits_per_sample as i32 - 1);
                reader
                    .samples::<i32>()
                    .map(|s| s.map(|s| s as f32 / scale))
                    .collect::<Result<Vec<_>, _>>()
            }
        }
        .map_err(|e| Error(e.to_string()))?;
        Ok(Self {
            samples,
            channels: spec.channels as usize,
            rate: spec.sample_rate as f64,
        })
    }
    fn sample(&self, time: f64, channel: usize) -> f32 {
        let position = time * self.rate;
        let frame = position.floor() as usize;
        let frac = (position - frame as f64) as f32;
        let c = channel % self.channels;
        let a = self
            .samples
            .get(frame * self.channels + c)
            .copied()
            .unwrap_or(0.0);
        let b = self
            .samples
            .get((frame + 1) * self.channels + c)
            .copied()
            .unwrap_or(0.0);
        a + (b - a) * frac
    }
}
// The standalone has no external MIDI output route. Consume events successfully
// so musical plugins do not mistake intentional discard for host queue overflow.
struct DiscardEvents;
impl EventSink for DiscardEvents {
    fn push(&mut self, _: Event) -> bool {
        true
    }
}
/// Reusable block processor; allocates on construction, never in render.
pub struct AudioEngine<P: Plugin> {
    pub meters: Arc<rvst_ui::Meters>,
    #[cfg(feature = "devtools")]
    metrics: Option<Arc<rvst_runtime::Metrics>>,
    #[cfg(feature = "devtools")]
    rt_log: Option<rvst_devtools::RtLog>,
    plugin: P,
    retired_errors: rvst_adapter::RetiredErrors,
    pub mirror: Arc<ParameterMirror>,
    input: Vec<f32>,
    output: Vec<f32>,
    source: InputSource,
    wav: Option<WavData>,
    rate: f64,
    pub sample_clock: u64,
    pub failures: Arc<AtomicU64>,
}
impl<P: Plugin> AudioEngine<P> {
    pub fn new(rate: f64, source: InputSource) -> Result<Self, Error> {
        P::descriptor().validate()?;
        if !rate.is_finite() || rate <= 0.0 {
            return Err("invalid sample rate".into());
        }
        if let InputSource::TestTone {
            frequency,
            amplitude,
        } = &source
            && (!frequency.is_finite()
                || *frequency <= 0.0
                || *frequency >= rate / 2.0
                || !amplitude.is_finite()
                || amplitude.abs() > 1.0)
        {
            return Err("invalid test tone".into());
        }
        let wav = match &source {
            InputSource::Wav(path) => Some(WavData::load(path)?),
            _ => None,
        };
        let mut plugin = P::new();
        let d = P::descriptor();
        plugin.prepare(ProcessConfig {
            sample_rate: rate,
            max_frames: 512,
            input_channels: d.input_channels(),
            output_channels: d.output_channels(),
        })?;
        let mirror = ParameterMirror::with_services(d, plugin.editor_context(), None);
        for spec in d.parameters {
            if let Some(value) = plugin.parameter(spec.id) {
                mirror.publish(spec.id, value);
            }
        }
        mirror.latency.store(plugin.latency(), Ordering::Release);
        Ok(Self {
            #[cfg(feature = "devtools")]
            metrics: None,
            #[cfg(feature = "devtools")]
            rt_log: None,
            meters: mirror.meters.clone(),
            plugin,
            retired_errors: rvst_adapter::RetiredErrors::default(),
            mirror,
            input: vec![0.0; 512 * d.input_channels()],
            output: vec![0.0; 512 * d.output_channels()],
            source,
            wav,
            rate,
            sample_clock: 0,
            failures: Arc::new(AtomicU64::new(0)),
        })
    }
    /// Connect a live inspector during setup; the caller keeps the session alive.
    #[cfg(feature = "devtools")]
    pub fn connect_inspector(
        &mut self,
        session: &rvst_devtools::DebugSession,
        rt_log: rvst_devtools::RtLog,
    ) -> Result<(), Error> {
        let parameters = self.mirror.clone();
        session
            .register_state_provider("parameters", move || parameters.parameters_snapshot())
            .map_err(|e| Error(e.to_string()))?;
        let meters = self.mirror.clone();
        session
            .register_state_provider("meters", move || meters.meters_snapshot())
            .map_err(|e| Error(e.to_string()))?;
        for &(code, name) in P::diagnostic_events() {
            session.register_event(code, name);
        }
        self.plugin.connect_diagnostics(Box::new(
            session.plugin_log().map_err(|e| Error(e.to_string()))?,
        ));
        self.metrics = Some(session.metrics());
        self.rt_log = Some(rt_log);
        Ok(())
    }
    pub fn render<T: cpal::SizedSample + cpal::FromSample<f32>>(
        &mut self,
        device_output: &mut [T],
        device_channels: usize,
    ) {
        if device_channels == 0 {
            return;
        }
        #[cfg(feature = "devtools")]
        let started = std::time::Instant::now();
        #[cfg(feature = "devtools")]
        let frame_count = device_output.len() / device_channels;
        let d = P::descriptor();
        let ni = d.input_channels();
        let no = d.output_channels();
        for i in 0..d.parameters.len() {
            if let Some((id, value)) = self.mirror.take_edit(i) {
                if let Err(error) = self.plugin.set_parameter(id, value) {
                    self.retired_errors.retain(error);
                }
                if let Some(current) = self.plugin.parameter(id) {
                    self.mirror.publish(id, current);
                }
            }
        }
        let complete_len = device_output.len() / device_channels * device_channels;
        device_output[complete_len..].fill(T::from_sample(0.0));
        for chunk in device_output[..complete_len].chunks_mut(512 * device_channels) {
            let frames = chunk.len() / device_channels;
            for c in 0..ni {
                for f in 0..frames {
                    let time = (self.sample_clock + f as u64) as f64 / self.rate;
                    self.input[c * frames + f] = match &self.source {
                        InputSource::Silence => 0.0,
                        InputSource::TestTone {
                            frequency,
                            amplitude,
                        } => (time * frequency * std::f64::consts::TAU).sin() as f32 * amplitude,
                        InputSource::Wav(_) => {
                            self.wav.as_ref().map(|w| w.sample(time, c)).unwrap_or(0.0)
                        }
                    };
                }
            }
            self.output[..frames * no].fill(0.0);
            let mut block = AudioBlock::new(
                &self.input[..frames * ni],
                &mut self.output[..frames * no],
                frames,
                ni,
                no,
            )
            .expect("preallocated shape");
            let transport = Transport {
                playing: true,
                sample_position: Some(self.sample_clock as i64),
                ..Transport::default()
            };
            let status = rvst_adapter::boundary(ProcessStatus::Error, || {
                self.plugin
                    .process(&mut block, &[], &transport, &mut DiscardEvents)
            });
            if status == ProcessStatus::Error {
                self.output[..frames * no].fill(0.0);
                self.failures.fetch_add(1, Ordering::Relaxed);
            }
            for (f, frame) in chunk.chunks_mut(device_channels).enumerate() {
                for (c, sample) in frame.iter_mut().enumerate() {
                    let value = if c < no {
                        self.output[c * frames + f]
                    } else {
                        0.0
                    };
                    *sample = T::from_sample(if value.is_finite() {
                        value.clamp(-1.0, 1.0)
                    } else {
                        0.0
                    });
                }
            }
            self.sample_clock = self.sample_clock.saturating_add(frames as u64);
            let input_peak = self.input[..frames * ni]
                .iter()
                .fold(0.0f32, |peak, value| peak.max(value.abs()));
            let output_peak = self.output[..frames * no]
                .iter()
                .fold(0.0f32, |peak, value| peak.max(value.abs()));
            self.meters
                .publish(input_peak, output_peak, self.sample_clock);
            self.mirror
                .latency
                .store(self.plugin.latency(), Ordering::Release);
        }
        #[cfg(feature = "devtools")]
        {
            if let Some(metrics) = &self.metrics {
                metrics.record_callback(
                    started.elapsed().as_nanos().min(u64::MAX as u128) as u64,
                    (frame_count as f64 / self.rate * 1e9) as u64,
                );
            }
            if self.sample_clock % (self.rate as u64) < frame_count as u64
                && let Some(log) = &mut self.rt_log
            {
                let _ = log.emit(
                    1,
                    self.sample_clock,
                    self.failures.load(Ordering::Relaxed) as f64,
                );
            }
        }
    }
}

struct StandaloneEditor {
    inner: MirrorEditor,
    context: Option<EditorContext>,
}
impl EditorHost for StandaloneEditor {
    fn edit_history(&self) -> Option<EditHistory> {
        self.inner.edit_history()
    }
    fn descriptor(&self) -> &'static PluginDescriptor {
        self.inner.descriptor()
    }
    fn editor_context(&self) -> Option<EditorContext> {
        self.context.clone()
    }
    fn parameter(&self, id: u32) -> Option<f64> {
        self.inner.parameter(id)
    }
    fn begin_edit(&mut self, id: u32) {
        self.inner.begin_edit(id)
    }
    fn set_parameter(&mut self, id: u32, value: f64) {
        self.inner.set_parameter(id, value)
    }
    fn end_edit(&mut self, id: u32) {
        self.inner.end_edit(id)
    }
    fn save_state(&mut self) -> Result<Vec<u8>, Error> {
        self.inner.save_state()
    }
    fn load_state(&mut self, data: &[u8]) -> Result<(), Error> {
        self.inner.load_state(data)
    }
}

pub fn run<P: Plugin>(editor: EditorFn) -> Result<(), Error> {
    run_with_options::<P>(
        editor,
        StandaloneOptions::from_args(std::env::args().skip(1))?,
    )
}

pub fn run_with_options<P: Plugin>(
    editor: EditorFn,
    options: StandaloneOptions,
) -> Result<(), Error> {
    let device = cpal::default_host()
        .default_output_device()
        .ok_or_else(|| Error("No default output device".into()))?;
    let supported = device
        .default_output_config()
        .map_err(|e| Error(e.to_string()))?;
    let config: cpal::StreamConfig = supported.into();
    let channels = config.channels as usize;
    let mut engine = AudioEngine::<P>::new(config.sample_rate as f64, options.input)?;
    #[cfg(feature = "devtools")]
    let session = {
        let (session, rt_log) =
            rvst_devtools::DebugSession::start(P::descriptor().id, P::descriptor().name)
                .map_err(|e| Error(e.to_string()))?;
        session.log(
            rvst_devtools::Level::Info,
            "standalone",
            "CPAL output configured; no microphone device opened",
        );
        session.set_snapshot(serde_json::json!({"runtime":"native-standalone","sample_rate":config.sample_rate,"output_channels":config.channels,"plugin":P::descriptor()}));
        engine.connect_inspector(&session, rt_log)?;
        eprintln!("RVST native inspector: {}", session.info().url);
        session
    };
    let editor_context = engine.plugin.editor_context();
    let meters = engine.meters.clone();
    let mirror = engine.mirror.clone();
    let failures = engine.failures.clone();
    let errors = Arc::new(AtomicU64::new(0));
    let stream_errors = errors.clone();
    let error_callback = move |_error: cpal::Error| {
        stream_errors.fetch_add(1, Ordering::Relaxed);
    };
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => device.build_output_stream(
            config,
            move |out: &mut [f32], _| engine.render(out, channels),
            error_callback,
            None,
        ),
        cpal::SampleFormat::F64 => device.build_output_stream(
            config,
            move |out: &mut [f64], _| engine.render(out, channels),
            error_callback,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_output_stream(
            config,
            move |out: &mut [i16], _| engine.render(out, channels),
            error_callback,
            None,
        ),
        cpal::SampleFormat::U16 => device.build_output_stream(
            config,
            move |out: &mut [u16], _| engine.render(out, channels),
            error_callback,
            None,
        ),
        other => return Err(Error(format!("Unsupported output sample format {other:?}"))),
    }
    .map_err(|e| Error(e.to_string()))?;
    stream.play().map_err(|e| Error(e.to_string()))?;
    #[cfg(feature = "devtools")]
    session.log(
        rvst_devtools::Level::Info,
        "standalone",
        "Audio stream started",
    );
    if options.no_ui {
        std::thread::sleep(options.duration.expect("validated duration"));
    } else {
        let options = rvst_platform::EditorOptions {
            title: P::descriptor().name.into(),
            meters: Some(meters),
            close_after: options.duration,
            #[cfg(feature = "devtools")]
            ui_metrics: Some(session.metrics()),
            #[cfg(feature = "devtools")]
            inspector_url: Some(session.info().url.clone()),
            ..Default::default()
        };
        let window = rvst_platform::NativeEditor::open(
            Box::new(StandaloneEditor {
                inner: MirrorEditor(mirror),
                context: editor_context,
            }),
            editor,
            options,
            None,
        )?;
        window.show()?;
        window.run()?;
    }
    drop(stream);
    #[cfg(feature = "devtools")]
    session.log(
        rvst_devtools::Level::Info,
        "standalone",
        "Audio stream stopped",
    );
    if errors.load(Ordering::Relaxed) > 0 || failures.load(Ordering::Relaxed) > 0 {
        return Err("audio stream or processor reported errors".into());
    }
    Ok(())
}

#[cfg(test)]
mod history_tests {
    use super::*;
    mod fixture {
        include!("../../rvst-adapter/test-support/plugin.rs");
    }

    #[test]
    fn reopened_standalone_editor_uses_instance_history() {
        let mirror = ParameterMirror::new(fixture::Gain::descriptor());
        let mut editor = HistoryHost::new(StandaloneEditor {
            inner: MirrorEditor(mirror.clone()),
            context: None,
        });
        editor.begin_edit(42);
        editor.set_parameter(42, 0.5);
        editor.end_edit(42);
        drop(editor);
        let reopened = HistoryHost::new(StandaloneEditor {
            inner: MirrorEditor(mirror),
            context: None,
        });
        assert!(reopened.undo_label().is_some());
    }
}
