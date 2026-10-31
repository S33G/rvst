//! Actual shared Rust UI in a browser, backed by a deterministic control host.
//! This intentionally does not execute audio DSP; native runners process audio.
use rvst_core::{EditorHost, Error, Plugin, PluginDescriptor};

pub struct FakeHost<P: Plugin> {
    pub plugin: P,
    pub gestures: Vec<(u32, bool)>,
}
impl<P: Plugin> Default for FakeHost<P> {
    fn default() -> Self {
        Self {
            plugin: P::new(),
            gestures: Vec::new(),
        }
    }
}
impl<P: Plugin> EditorHost for FakeHost<P> {
    fn editor_context(&self) -> Option<rvst_core::EditorContext> {
        self.plugin.editor_context()
    }
    fn descriptor(&self) -> &'static PluginDescriptor {
        P::descriptor()
    }
    fn parameter(&self, id: u32) -> Option<f64> {
        self.plugin.parameter(id)
    }
    fn begin_edit(&mut self, id: u32) {
        if self.gestures.len() >= 256 {
            self.gestures.drain(..128);
        }
        self.gestures.push((id, true));
    }
    fn set_parameter(&mut self, id: u32, value: f64) {
        let _ = self.plugin.set_parameter(id, value);
    }
    fn end_edit(&mut self, id: u32) {
        if self.gestures.len() >= 256 {
            self.gestures.drain(..128);
        }
        self.gestures.push((id, false));
    }
    fn save_state(&mut self) -> Result<Vec<u8>, Error> {
        self.plugin.save_state()
    }
    fn load_state(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.plugin.load_state(bytes)
    }
}
#[cfg(any(test, target_arch = "wasm32"))]
fn editor_host<P: Plugin>() -> rvst_core::HistoryHost<FakeHost<P>> {
    rvst_core::HistoryHost::new(FakeHost::default())
}
#[cfg(target_arch = "wasm32")]
mod browser {
    use super::*;
    use rvst_ui::{EditorFn, egui};
    use wasm_bindgen::{JsCast, JsValue};
    struct WebApp<P: Plugin> {
        host: rvst_core::HistoryHost<FakeHost<P>>,
        editor: EditorFn,
    }
    impl<P: Plugin> eframe::App for WebApp<P> {
        fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
            let ctx = ui.ctx();
            rvst_ui::set_runtime_label(
                ctx,
                "Browser · deterministic control preview · audio DSP disabled",
            );
            rvst_ui::editor_frame(ctx, &mut self.host, self.editor);
        }
    }
    /// Mount into an existing canvas. No native dependencies or microphone APIs.
    pub async fn start<P: Plugin>(canvas_id: &str, editor: EditorFn) -> Result<(), JsValue> {
        let document = web_sys::window()
            .and_then(|w| w.document())
            .ok_or_else(|| JsValue::from_str("document unavailable"))?;
        let canvas = document
            .get_element_by_id(canvas_id)
            .ok_or_else(|| JsValue::from_str("RVST canvas missing"))?
            .dyn_into::<web_sys::HtmlCanvasElement>()?;
        let runner = eframe::WebRunner::new();
        runner
            .start(
                canvas,
                eframe::WebOptions::default(),
                Box::new(move |_| {
                    Ok(Box::new(WebApp::<P> {
                        host: editor_host(),
                        editor,
                    }))
                }),
            )
            .await?;
        // The JS callbacks retain the running canvas application.
        std::mem::forget(runner);
        Ok(())
    }
}
#[cfg(target_arch = "wasm32")]
pub use browser::start;

#[cfg(test)]
mod tests {
    use super::*;
    use rvst_core::{
        AudioBlock, Event, EventSink, PluginKind, ProcessConfig, ProcessStatus, Transport,
    };

    static DESCRIPTOR: PluginDescriptor = PluginDescriptor {
        id: "test",
        name: "Test",
        vendor: "RVST",
        version: "1",
        url: "",
        email: "",
        vst3_id: [1; 16],
        kind: PluginKind::Effect,
        parameters: &[rvst_core::ParameterSpec {
            id: 1,
            name: "Gain",
            unit: "",
            min: 0.0,
            max: 1.0,
            default: 0.0,
            step: None,
            bypass: false,
        }],
        inputs: &[],
        outputs: &[],
        midi_input: false,
        midi_output: false,
        supports_f64: false,
    };
    struct TestPlugin(f64);
    impl Plugin for TestPlugin {
        fn descriptor() -> &'static PluginDescriptor {
            &DESCRIPTOR
        }
        fn new() -> Self {
            Self(0.0)
        }
        fn prepare(&mut self, _: ProcessConfig) -> Result<(), Error> {
            Ok(())
        }
        fn process(
            &mut self,
            _: &mut AudioBlock<'_, f32>,
            _: &[Event],
            _: &Transport,
            _: &mut dyn EventSink,
        ) -> ProcessStatus {
            ProcessStatus::Continue
        }
        fn reset(&mut self) {}
        fn parameter(&self, id: u32) -> Option<f64> {
            (id == 1).then_some(self.0)
        }
        fn set_parameter(&mut self, id: u32, value: f64) -> Result<(), Error> {
            if id != 1 {
                return Err("unknown parameter".into());
            }
            self.0 = value;
            Ok(())
        }
        fn save_state(&self) -> Result<Vec<u8>, Error> {
            Ok(self.0.to_le_bytes().to_vec())
        }
        fn load_state(&mut self, data: &[u8]) -> Result<(), Error> {
            self.0 = f64::from_le_bytes(data.try_into().map_err(|_| Error::from("invalid state"))?);
            Ok(())
        }
    }

    #[test]
    fn browser_host_construction_enables_parameter_and_preset_history() {
        let mut host = editor_host::<TestPlugin>();
        host.begin_edit(1);
        host.set_parameter(1, 0.4);
        host.set_parameter(1, 0.7);
        host.end_edit(1);
        assert!(host.undo().unwrap());
        assert_eq!(host.parameter(1), Some(0.0));
        assert!(host.redo().unwrap());
        assert_eq!(host.parameter(1), Some(0.7));
        host.load_state(&0.2_f64.to_le_bytes()).unwrap();
        assert!(host.undo().unwrap());
        assert_eq!(host.parameter(1), Some(0.7));
        assert!(host.redo().unwrap());
        assert_eq!(host.parameter(1), Some(0.2));
    }
}
