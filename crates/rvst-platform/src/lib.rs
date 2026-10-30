//! Native embedding. All calls belong on the host main thread.
#[cfg(not(target_arch = "wasm32"))]
mod native {
    pub use baseview::ParentWindowHandle;
    use egui_baseview::{EguiWindow, EguiWindowSettings};
    use rvst_core::{EditorHost, Error};
    use rvst_ui::{EditorFn, egui};
    /// Construct a parent from the native handle provided by a plugin host.
    /// # Safety
    /// The handle must identify a live view/window on the current UI thread and remain
    /// valid until the child editor has closed. Linux supports X11 handles only.
    pub unsafe fn parent_from_raw(ptr: *mut std::ffi::c_void) -> Result<ParentWindowHandle, Error> {
        use raw_window_handle::{HandleError, HasWindowHandle, RawWindowHandle, WindowHandle};
        if ptr.is_null() {
            return Err("null native parent".into());
        }
        #[cfg(target_os = "macos")]
        let raw = RawWindowHandle::AppKit(raw_window_handle::AppKitWindowHandle::new(
            std::ptr::NonNull::new(ptr).unwrap(),
        ));
        #[cfg(target_os = "windows")]
        let raw = RawWindowHandle::Win32(raw_window_handle::Win32WindowHandle::new(
            std::num::NonZeroIsize::new(ptr as isize).unwrap(),
        ));
        #[cfg(all(unix, not(target_os = "macos")))]
        let raw = RawWindowHandle::Xlib(raw_window_handle::XlibWindowHandle::new(ptr as u64));
        struct Parent(RawWindowHandle);
        impl HasWindowHandle for Parent {
            fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
                // SAFETY: caller guarantees the validity and thread ownership of the raw host handle.
                Ok(unsafe { WindowHandle::borrow_raw(self.0) })
            }
        }
        Ok(ParentWindowHandle::from_window(&Parent(raw)))
    }
    #[derive(Debug, Clone)]
    pub struct EditorOptions {
        pub title: String,
        pub width: u32,
        pub height: u32,
        pub scale: f32,
        pub inspector_url: Option<String>,
        pub meters: Option<std::sync::Arc<rvst_ui::Meters>>,
        /// Optional inspector timing. Measures editor CPU work, excluding rendering/presentation.
        pub ui_metrics: Option<std::sync::Arc<rvst_runtime::Metrics>>,
        pub close_after: Option<std::time::Duration>,
    }
    impl Default for EditorOptions {
        fn default() -> Self {
            Self {
                title: "RVST".into(),
                width: 720,
                height: 480,
                scale: 1.0,
                inspector_url: None,
                meters: None,
                ui_metrics: None,
                close_after: None,
            }
        }
    }
    struct App {
        host: rvst_core::HistoryHost<Box<dyn EditorHost + Send>>,
        editor: EditorFn,
        inspector_url: Option<String>,
        meters: Option<std::sync::Arc<rvst_ui::Meters>>,
        ui_metrics: Option<std::sync::Arc<rvst_runtime::Metrics>>,
        deadline: Option<std::time::Instant>,
    }
    impl App {
        fn new(host: Box<dyn EditorHost + Send>, editor: EditorFn, options: EditorOptions) -> Self {
            Self {
                host: rvst_core::HistoryHost::new(host),
                editor,
                inspector_url: options.inspector_url,
                meters: options.meters,
                ui_metrics: options.ui_metrics,
                deadline: options
                    .close_after
                    .map(|duration| std::time::Instant::now() + duration),
            }
        }
        fn draw_editor(&mut self, ctx: &egui::Context) {
            if let Some(metrics) = &self.ui_metrics {
                let start = std::time::Instant::now();
                rvst_ui::editor_frame(ctx, &mut self.host, self.editor);
                metrics.record_ui_frame(start.elapsed().as_nanos().min(u64::MAX as u128) as u64);
            } else {
                rvst_ui::editor_frame(ctx, &mut self.host, self.editor);
            }
        }
    }
    impl egui_baseview::App for App {
        fn build(
            &mut self,
            ctx: egui::Context,
            _: &mut egui_baseview::Frame,
        ) -> Result<(), baseview::HandlerError> {
            rvst_ui::set_runtime_label(&ctx, "Native plugin editor");
            if let Some(url) = self.inspector_url.take() {
                rvst_ui::set_inspector_url(&ctx, url);
            }
            Ok(())
        }
        fn ui(&mut self, ui: &mut egui::Ui, _: &mut egui_baseview::Frame) {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(33));
            if let Some(deadline) = self.deadline {
                if std::time::Instant::now() >= deadline {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                } else {
                    ui.ctx().request_repaint_after(
                        deadline.saturating_duration_since(std::time::Instant::now()),
                    );
                }
            }
            if let Some(meters) = &self.meters {
                rvst_ui::set_meter_snapshot(ui.ctx(), meters.snapshot());
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(33));
            }
            self.draw_editor(ui.ctx());
        }
    }
    pub struct NativeEditor {
        window: Option<baseview::Window>,
    }
    impl NativeEditor {
        pub fn open(
            host: Box<dyn EditorHost + Send>,
            editor: EditorFn,
            options: EditorOptions,
            parent: Option<ParentWindowHandle>,
        ) -> Result<Self, Error> {
            if options.width == 0
                || options.height == 0
                || !options.scale.is_finite()
                || options.scale <= 0.0
            {
                return Err("invalid editor size or scale".into());
            }
            let mut settings = EguiWindowSettings::new()
                .with_title(options.title.clone())
                .with_size(baseview::dpi::LogicalSize::new(
                    options.width as f64,
                    options.height as f64,
                ))
                .with_zoom_factor(options.scale);
            settings.parent = parent;
            let window = EguiWindow::create(settings, App::new(host, editor, options))
                .map_err(|e| Error(e.to_string()))?;
            Ok(Self {
                window: Some(window),
            })
        }
        pub fn is_open(&self) -> bool {
            self.window.as_ref().is_some_and(|w| w.is_open())
        }
        pub fn resize(&self, width: u32, height: u32) -> Result<(), Error> {
            if width == 0 || height == 0 {
                return Err("invalid editor size".into());
            }
            self.window
                .as_ref()
                .ok_or_else(|| Error("editor closed".into()))?
                .resize(baseview::dpi::LogicalSize::new(width as f64, height as f64))
                .map_err(|e| Error(e.to_string()))
        }
        pub fn show(&self) -> Result<(), Error> {
            self.window
                .as_ref()
                .ok_or_else(|| Error("editor closed".into()))?
                .show()
                .map_err(|e| Error(e.to_string()))
        }
        pub fn hide(&self) -> Result<(), Error> {
            self.window
                .as_ref()
                .ok_or_else(|| Error("editor closed".into()))?
                .hide()
                .map_err(|e| Error(e.to_string()))
        }
        pub fn close(&mut self) {
            if let Some(window) = self.window.take() {
                window.close();
            }
        }
        pub fn run(mut self) -> Result<(), Error> {
            self.window
                .take()
                .ok_or_else(|| Error("editor closed".into()))?
                .run_until_closed()
                .map_err(|e| Error(e.to_string()))
        }
    }
    impl Drop for NativeEditor {
        fn drop(&mut self) {
            self.close();
        }
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        static DESCRIPTOR: rvst_core::PluginDescriptor = rvst_core::PluginDescriptor {
            id: "test",
            name: "Test",
            vendor: "RVST",
            version: "1",
            url: "",
            email: "",
            vst3_id: [1; 16],
            kind: rvst_core::PluginKind::Effect,
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
        #[derive(Default)]
        struct Host(f64);
        impl EditorHost for Host {
            fn descriptor(&self) -> &'static rvst_core::PluginDescriptor {
                &DESCRIPTOR
            }
            fn parameter(&self, id: u32) -> Option<f64> {
                (id == 1).then_some(self.0)
            }
            fn begin_edit(&mut self, _: u32) {}
            fn set_parameter(&mut self, _: u32, value: f64) {
                self.0 = value;
            }
            fn end_edit(&mut self, _: u32) {}
            fn save_state(&mut self) -> Result<Vec<u8>, Error> {
                Ok(Vec::new())
            }
            fn load_state(&mut self, _: &[u8]) -> Result<(), Error> {
                Ok(())
            }
        }
        #[test]
        fn native_editor_reports_only_enabled_ui_timing() {
            let metrics = std::sync::Arc::new(rvst_runtime::Metrics::default());
            let mut app = App {
                host: rvst_core::HistoryHost::new(
                    Box::new(Host::default()) as Box<dyn EditorHost + Send>
                ),
                editor: |ctx, _| {
                    ctx.layer_painter(egui::LayerId::background()).rect_filled(
                        egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(100., 100.)),
                        0.,
                        egui::Color32::WHITE,
                    );
                },
                inspector_url: None,
                meters: None,
                ui_metrics: Some(metrics.clone()),
                deadline: None,
            };
            let ctx = egui::Context::default();
            let mut output = ctx.run_ui(Default::default(), |ui| app.draw_editor(ui.ctx()));
            output.textures_delta.clear();
            assert!(!output.shapes.is_empty());
            assert_eq!(metrics.snapshot().ui_frames, 1);
            assert!(metrics.snapshot().mean_ui_ns.is_finite());
            app.ui_metrics = None;
            let mut output = ctx.run_ui(Default::default(), |ui| app.draw_editor(ui.ctx()));
            output.textures_delta.clear();
            assert!(!output.shapes.is_empty());
            assert_eq!(metrics.snapshot().ui_frames, 1);
        }
        #[test]
        fn native_runner_installs_history_and_keyboard_for_custom_editors() {
            let mut app = App::new(
                Box::new(Host::default()),
                |_, _| {},
                EditorOptions::default(),
            );
            app.host.begin_edit(1);
            app.host.set_parameter(1, 0.8);
            app.host.end_edit(1);
            assert!(app.host.undo_label().is_some());
            let ctx = egui::Context::default();
            let input = egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::Z,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers {
                        ctrl: true,
                        command: true,
                        ..Default::default()
                    },
                }],
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| app.draw_editor(ui.ctx()));
            output.textures_delta.clear();
            assert_eq!(app.host.parameter(1), Some(0.0));
            assert!(app.host.redo_label().is_some());
        }
    }
}
#[cfg(not(target_arch = "wasm32"))]
pub use native::*;
