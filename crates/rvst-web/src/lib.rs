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
#[cfg(target_arch = "wasm32")]
mod browser {
    use super::*;
    use rvst_ui::{EditorFn, egui};
    use wasm_bindgen::{JsCast, JsValue};
    struct WebApp<P: Plugin> {
        host: FakeHost<P>,
        editor: EditorFn,
    }
    impl<P: Plugin> eframe::App for WebApp<P> {
        fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
            let ctx = ui.ctx();
            rvst_ui::set_runtime_label(
                ctx,
                "Browser · deterministic control preview · audio DSP disabled",
            );
            (self.editor)(ctx, &mut self.host);
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
                        host: FakeHost::default(),
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
