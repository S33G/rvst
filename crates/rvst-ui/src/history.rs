use crate::EditorFn;
use rvst_core::EditorHost;

const ERROR_ID: &str = "rvst-history-error";
const PENDING_ID: &str = "rvst-history-pending";
const MAX_PENDING_ACTIONS: usize = 16;

#[derive(Clone, Copy)]
enum Action {
    Undo,
    Redo,
}

#[derive(Clone, Default)]
struct PendingActions {
    actions: [Option<Action>; MAX_PENDING_ACTIONS],
    len: usize,
}

impl PendingActions {
    fn push(&mut self, action: Action) {
        if let Some(slot) = self.actions.get_mut(self.len) {
            *slot = Some(action);
            self.len += 1;
        }
    }
}

impl Action {
    fn from_event(event: &egui::Event) -> Option<Self> {
        let egui::Event::Key {
            key,
            pressed: true,
            modifiers,
            ..
        } = event
        else {
            return None;
        };
        // Exact matching keeps Alt combinations and unrelated Shift shortcuts
        // available to the editor. Redo takes precedence over plain undo.
        match key {
            egui::Key::Z
                if modifiers.matches_exact(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT) =>
            {
                Some(Self::Redo)
            }
            egui::Key::Z if modifiers.matches_exact(egui::Modifiers::COMMAND) => Some(Self::Undo),
            egui::Key::Y if modifiers.matches_exact(egui::Modifiers::CTRL) => Some(Self::Redo),
            _ => None,
        }
    }

    fn apply(self, ctx: &egui::Context, host: &mut dyn EditorHost) {
        let (name, result) = match self {
            Self::Undo => ("Undo", host.undo()),
            Self::Redo => ("Redo", host.redo()),
        };
        ctx.data_mut(|data| match result {
            Ok(_) => data.remove::<String>(egui::Id::new(ERROR_ID)),
            Err(error) => {
                data.insert_temp(egui::Id::new(ERROR_ID), format!("{name} failed: {error}"));
            }
        });
        ctx.request_repaint();
    }
}

fn history_error(ctx: &egui::Context) -> Option<String> {
    ctx.data(|data| data.get_temp(egui::Id::new(ERROR_ID)))
}

/// Draw any plugin editor and handle its undo/redo shortcuts.
///
/// Native and browser runners call this for both custom and shared editors.
/// Focused text fields retain their own undo history, including a field that
/// gains or loses focus during this frame.
/// Up to 16 shortcuts received during a drag wait for release, so a replay
/// cannot split an active parameter gesture. Taking text focus cancels them.
pub fn editor_frame(ctx: &egui::Context, host: &mut dyn EditorHost, editor: EditorFn) {
    let text_was_focused = ctx.text_edit_focused();
    editor(ctx, host);
    let mut pending = ctx.data_mut(|data| {
        data.remove_temp::<PendingActions>(egui::Id::new(PENDING_ID))
            .unwrap_or_default()
    });
    if !text_was_focused && !ctx.text_edit_focused() {
        ctx.input_mut(|input| {
            input.events.retain(|event| {
                if let Some(action) = Action::from_event(event) {
                    pending.push(action);
                    false
                } else {
                    true
                }
            });
        });
        if ctx.dragged_id().is_some() {
            if pending.len > 0 {
                ctx.data_mut(|data| data.insert_temp(egui::Id::new(PENDING_ID), pending));
            }
        } else {
            for action in pending.actions.into_iter().flatten() {
                action.apply(ctx, host);
            }
        }
    }
    if let Some(error) = history_error(ctx) {
        let mut open = true;
        egui::Window::new("Undo / redo")
            .id(egui::Id::new("rvst-history-error-window"))
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(error);
            });
        if !open {
            ctx.data_mut(|data| data.remove::<String>(egui::Id::new(ERROR_ID)));
        }
    }
}

fn history_buttons(ui: &mut egui::Ui, host: &mut dyn EditorHost) -> [egui::Response; 2] {
    let can_replay = ui.ctx().dragged_id().is_none();
    let undo_label = host.undo_label();
    let undo_shortcut = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::Z);
    let undo = ui
        .add_enabled(
            can_replay && undo_label.is_some(),
            egui::Button::new("Undo").shortcut_text(ui.ctx().format_shortcut(&undo_shortcut)),
        )
        .on_hover_text(
            undo_label.map_or_else(|| "Nothing to undo".into(), |label| format!("Undo {label}")),
        );
    if undo.clicked() {
        Action::Undo.apply(ui.ctx(), host);
    }
    let redo_label = host.redo_label();
    let redo_shortcut = egui::KeyboardShortcut::new(
        egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
        egui::Key::Z,
    );
    let redo = ui
        .add_enabled(
            can_replay && redo_label.is_some(),
            egui::Button::new("Redo").shortcut_text(ui.ctx().format_shortcut(&redo_shortcut)),
        )
        .on_hover_text(
            redo_label.map_or_else(|| "Nothing to redo".into(), |label| format!("Redo {label}")),
        );
    if redo.clicked() {
        Action::Redo.apply(ui.ctx(), host);
    }
    [undo, redo]
}

/// Undo/redo controls for shared or custom plugin editors.
/// Buttons expose the next action in their tooltip and disable when unavailable.
pub fn history_controls(ui: &mut egui::Ui, host: &mut dyn EditorHost) {
    ui.horizontal_wrapped(|ui| {
        history_buttons(ui, host);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parameter_knob;
    use rvst_core::{Error, HistoryHost, ParameterSpec, PluginDescriptor, PluginKind};
    use std::sync::{Arc, Mutex};

    const PARAMETER: ParameterSpec = ParameterSpec {
        id: 7,
        name: "Gain",
        unit: "dB",
        min: 0.0,
        max: 2.0,
        default: 1.0,
        step: Some(0.1),
        bypass: false,
    };
    static DESCRIPTOR: PluginDescriptor = PluginDescriptor {
        id: "history-ui-test",
        name: "History test",
        vendor: "RVST",
        version: "1",
        url: "",
        email: "",
        vst3_id: [1; 16],
        kind: PluginKind::Effect,
        parameters: &[PARAMETER],
        inputs: &[],
        outputs: &[],
        midi_input: false,
        midi_output: false,
        supports_f64: false,
    };

    #[derive(Default)]
    struct Host {
        value: f64,
        undo_calls: usize,
        redo_calls: usize,
        fail: bool,
    }
    impl EditorHost for Host {
        fn descriptor(&self) -> &'static PluginDescriptor {
            &DESCRIPTOR
        }
        fn parameter(&self, _: u32) -> Option<f64> {
            Some(self.value)
        }
        fn begin_edit(&mut self, _: u32) {}
        fn set_parameter(&mut self, _: u32, value: f64) {
            self.value = value;
        }
        fn end_edit(&mut self, _: u32) {}
        fn save_state(&mut self) -> Result<Vec<u8>, Error> {
            Ok(Vec::new())
        }
        fn load_state(&mut self, _: &[u8]) -> Result<(), Error> {
            Ok(())
        }
        fn undo_label(&self) -> Option<String> {
            (self.value == 1.0).then(|| "Gain".into())
        }
        fn redo_label(&self) -> Option<String> {
            (self.undo_calls > 0 && self.value == 0.0).then(|| "Gain".into())
        }
        fn undo(&mut self) -> Result<bool, Error> {
            self.undo_calls += 1;
            if self.fail {
                return Err("host rejected change".into());
            }
            let changed = self.value == 1.0;
            self.value = 0.0;
            Ok(changed)
        }
        fn redo(&mut self) -> Result<bool, Error> {
            self.redo_calls += 1;
            if self.fail {
                return Err("host rejected change".into());
            }
            self.value = 1.0;
            Ok(true)
        }
    }

    fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::RawInput {
        egui::RawInput {
            events: vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }],
            ..Default::default()
        }
    }

    fn frame(
        ctx: &egui::Context,
        input: egui::RawInput,
        host: &mut dyn EditorHost,
        editor: EditorFn,
    ) {
        let mut output = ctx.run_ui(input, |ui| editor_frame(ui.ctx(), host, editor));
        output.textures_delta.clear();
    }

    fn custom_editor(ctx: &egui::Context, _: &mut dyn EditorHost) {
        ctx.data_mut(|data| data.insert_temp(egui::Id::new("custom-editor-ran"), true));
    }

    fn root(ctx: &egui::Context) -> egui::Ui {
        egui::Ui::new(
            ctx.clone(),
            egui::Id::new("history-test-root"),
            egui::UiBuilder::new().max_rect(ctx.content_rect()),
        )
    }

    fn knob_editor(ctx: &egui::Context, host: &mut dyn EditorHost) {
        let response = parameter_knob(&mut root(ctx), host, &PARAMETER);
        ctx.data_mut(|data| data.insert_temp(egui::Id::new("knob-id"), response.id));
        ctx.data_mut(|data| data.insert_temp(egui::Id::new("knob-rect"), response.rect));
    }

    #[derive(Default)]
    struct GestureTrace {
        value: f64,
        editing: bool,
        gestures: Vec<bool>,
        values: Vec<f64>,
    }

    struct TracedHost(Arc<Mutex<GestureTrace>>);

    impl EditorHost for TracedHost {
        fn descriptor(&self) -> &'static PluginDescriptor {
            &DESCRIPTOR
        }
        fn parameter(&self, _: u32) -> Option<f64> {
            Some(self.0.lock().unwrap().value)
        }
        fn begin_edit(&mut self, _: u32) {
            let mut trace = self.0.lock().unwrap();
            assert!(!trace.editing, "nested host gesture");
            trace.editing = true;
            trace.gestures.push(true);
        }
        fn set_parameter(&mut self, _: u32, value: f64) {
            let mut trace = self.0.lock().unwrap();
            assert!(trace.editing, "parameter update outside host gesture");
            trace.value = value;
            trace.values.push(value);
        }
        fn end_edit(&mut self, _: u32) {
            let mut trace = self.0.lock().unwrap();
            assert!(trace.editing, "host gesture ended twice");
            trace.editing = false;
            trace.gestures.push(false);
        }
        fn save_state(&mut self) -> Result<Vec<u8>, Error> {
            Ok(Vec::new())
        }
        fn load_state(&mut self, _: &[u8]) -> Result<(), Error> {
            Ok(())
        }
    }

    fn pointer(position: egui::Pos2, pressed: Option<bool>) -> egui::RawInput {
        let mut events = vec![egui::Event::PointerMoved(position)];
        if let Some(pressed) = pressed {
            events.push(egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
        egui::RawInput {
            events,
            ..Default::default()
        }
    }

    fn begin_knob_drag(ctx: &egui::Context, host: &mut dyn EditorHost) -> egui::Pos2 {
        frame(ctx, Default::default(), host, knob_editor);
        let center = ctx
            .data(|data| data.get_temp::<egui::Rect>(egui::Id::new("knob-rect")))
            .unwrap()
            .center();
        frame(ctx, pointer(center, Some(true)), host, knob_editor);
        let dragged = center + egui::vec2(20.0, 0.0);
        frame(ctx, pointer(dragged, None), host, knob_editor);
        assert!(ctx.dragged_id().is_some());
        dragged
    }

    #[test]
    fn shortcut_during_pointer_drag_waits_for_one_complete_host_gesture() {
        let ctx = egui::Context::default();
        let trace = Arc::new(Mutex::new(GestureTrace {
            value: 1.0,
            ..Default::default()
        }));
        let mut host = HistoryHost::new(TracedHost(trace.clone()));
        let dragged = begin_knob_drag(&ctx, &mut host);
        let first_value = host.parameter(PARAMETER.id).unwrap();
        assert!(first_value > 1.0);
        frame(
            &ctx,
            key(egui::Key::Z, egui::Modifiers::COMMAND),
            &mut host,
            knob_editor,
        );
        assert_eq!(host.parameter(PARAMETER.id), Some(first_value));
        assert_eq!(trace.lock().unwrap().gestures, [true]);
        let dragged = dragged + egui::vec2(20.0, 0.0);
        frame(&ctx, pointer(dragged, None), &mut host, knob_editor);
        let final_value = host.parameter(PARAMETER.id).unwrap();
        assert!(final_value > first_value);
        assert_eq!(trace.lock().unwrap().gestures, [true]);
        frame(&ctx, pointer(dragged, Some(false)), &mut host, knob_editor);
        assert_eq!(host.parameter(PARAMETER.id), Some(1.0));
        {
            let trace = trace.lock().unwrap();
            assert!(!trace.editing);
            assert_eq!(trace.gestures, [true, false, true, false]);
            assert_eq!(trace.values, [first_value, final_value, 1.0]);
        }
        assert!(
            host.undo_label().is_none(),
            "drag should create one undo step"
        );
        assert_eq!(host.redo_label().as_deref(), Some("Gain"));
        frame(
            &ctx,
            key(
                egui::Key::Z,
                egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
            ),
            &mut host,
            knob_editor,
        );
        assert_eq!(host.parameter(PARAMETER.id), Some(final_value));
        assert!(host.undo().unwrap());
        assert!(
            !host.undo().unwrap(),
            "no intermediate drag steps should remain"
        );
    }

    #[test]
    fn deferred_shortcuts_are_bounded_and_replay_once_on_release() {
        let ctx = egui::Context::default();
        let mut host = Host {
            value: 1.0,
            ..Default::default()
        };
        let dragged = begin_knob_drag(&ctx, &mut host);
        let mut input = key(egui::Key::Z, egui::Modifiers::COMMAND);
        input.events = vec![input.events[0].clone(); MAX_PENDING_ACTIONS * 4];
        frame(&ctx, input, &mut host, knob_editor);
        assert_eq!(host.undo_calls, 0);
        assert_eq!(
            ctx.data(|data| data.get_temp::<PendingActions>(egui::Id::new(PENDING_ID)))
                .unwrap()
                .len,
            MAX_PENDING_ACTIONS
        );
        frame(&ctx, pointer(dragged, Some(false)), &mut host, knob_editor);
        assert_eq!(host.undo_calls, MAX_PENDING_ACTIONS);
        frame(&ctx, Default::default(), &mut host, knob_editor);
        assert_eq!(host.undo_calls, MAX_PENDING_ACTIONS);
    }

    #[test]
    fn taking_text_focus_cancels_deferred_parameter_shortcuts() {
        let ctx = egui::Context::default();
        let mut host = Host {
            value: 1.0,
            ..Default::default()
        };
        let dragged = begin_knob_drag(&ctx, &mut host);
        frame(
            &ctx,
            key(egui::Key::Z, egui::Modifiers::COMMAND),
            &mut host,
            knob_editor,
        );
        frame(
            &ctx,
            key(egui::Key::Z, egui::Modifiers::COMMAND),
            &mut host,
            |ctx, host| {
                ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("text-field")));
                text_editor(ctx, host);
            },
        );
        assert!(ctx.text_edit_focused());
        assert!(
            ctx.data(|data| data.get_temp::<PendingActions>(egui::Id::new(PENDING_ID)))
                .is_none()
        );
        frame(&ctx, pointer(dragged, Some(false)), &mut host, text_editor);
        ctx.memory_mut(|memory| memory.surrender_focus(egui::Id::new("text-field")));
        frame(&ctx, Default::default(), &mut host, knob_editor);
        assert_eq!(host.undo_calls, 0);
    }

    fn text_editor(ctx: &egui::Context, _: &mut dyn EditorHost) {
        let mut text = ctx.data_mut(|data| {
            data.get_temp::<String>(egui::Id::new("text-value"))
                .unwrap_or_default()
        });
        root(ctx).add(egui::TextEdit::singleline(&mut text).id(egui::Id::new("text-field")));
        ctx.data_mut(|data| data.insert_temp(egui::Id::new("text-value"), text));
    }

    #[test]
    fn custom_editors_get_undo_and_redo_for_mac_and_control_shortcuts() {
        for command in [
            egui::Modifiers::COMMAND | egui::Modifiers::MAC_CMD,
            egui::Modifiers::COMMAND | egui::Modifiers::CTRL,
        ] {
            let ctx = egui::Context::default();
            let mut host = Host {
                value: 1.0,
                ..Default::default()
            };
            frame(&ctx, key(egui::Key::Z, command), &mut host, custom_editor);
            assert_eq!(host.value, 0.0);
            assert_eq!((host.undo_calls, host.redo_calls), (1, 0));
            assert!(
                ctx.data(|data| data.get_temp::<bool>(egui::Id::new("custom-editor-ran")))
                    .unwrap()
            );
            assert!(!ctx.input(|input| input.key_pressed(egui::Key::Z)));
            frame(
                &ctx,
                key(egui::Key::Z, command | egui::Modifiers::SHIFT),
                &mut host,
                custom_editor,
            );
            assert_eq!(host.value, 1.0);
            assert_eq!((host.undo_calls, host.redo_calls), (1, 1));
            frame(&ctx, key(egui::Key::Z, command), &mut host, custom_editor);
            frame(
                &ctx,
                key(
                    egui::Key::Y,
                    egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
                ),
                &mut host,
                custom_editor,
            );
            assert_eq!((host.undo_calls, host.redo_calls), (2, 2));
        }
    }

    #[test]
    fn focused_knob_allows_history_shortcuts_and_keeps_focus() {
        let ctx = egui::Context::default();
        let mut host = Host {
            value: 1.0,
            ..Default::default()
        };
        frame(&ctx, Default::default(), &mut host, knob_editor);
        let id = ctx
            .data(|data| data.get_temp::<egui::Id>(egui::Id::new("knob-id")))
            .unwrap();
        ctx.memory_mut(|memory| memory.request_focus(id));
        frame(
            &ctx,
            key(egui::Key::Z, egui::Modifiers::COMMAND),
            &mut host,
            knob_editor,
        );
        assert_eq!(host.value, 0.0);
        assert_eq!(host.undo_calls, 1);
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(id));
    }

    #[test]
    fn text_field_undo_and_redo_leave_parameter_history_alone() {
        let ctx = egui::Context::default();
        let mut host = Host {
            value: 1.0,
            ..Default::default()
        };
        frame(&ctx, Default::default(), &mut host, text_editor);
        ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("text-field")));
        frame(
            &ctx,
            egui::RawInput {
                events: vec![egui::Event::Text("hello".into())],
                ..Default::default()
            },
            &mut host,
            text_editor,
        );
        assert_eq!(
            ctx.data(|data| data.get_temp::<String>(egui::Id::new("text-value")))
                .unwrap(),
            "hello"
        );
        frame(
            &ctx,
            key(egui::Key::Z, egui::Modifiers::COMMAND),
            &mut host,
            text_editor,
        );
        assert_eq!(
            ctx.data(|data| data.get_temp::<String>(egui::Id::new("text-value")))
                .unwrap(),
            ""
        );
        frame(
            &ctx,
            key(
                egui::Key::Z,
                egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
            ),
            &mut host,
            text_editor,
        );
        assert_eq!(
            ctx.data(|data| data.get_temp::<String>(egui::Id::new("text-value")))
                .unwrap(),
            "hello"
        );
        assert_eq!(host.value, 1.0);
        assert_eq!((host.undo_calls, host.redo_calls), (0, 0));
    }

    #[test]
    fn text_focus_lost_during_frame_still_protects_parameter_history() {
        let ctx = egui::Context::default();
        let mut host = Host {
            value: 1.0,
            ..Default::default()
        };
        frame(&ctx, Default::default(), &mut host, text_editor);
        ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("text-field")));
        frame(
            &ctx,
            key(egui::Key::Z, egui::Modifiers::COMMAND),
            &mut host,
            |ctx, host| {
                text_editor(ctx, host);
                ctx.memory_mut(|memory| memory.surrender_focus(egui::Id::new("text-field")));
            },
        );
        assert_eq!(host.undo_calls, 0);
    }

    #[test]
    fn text_focus_gained_during_frame_protects_parameter_history() {
        let ctx = egui::Context::default();
        let mut host = Host {
            value: 1.0,
            ..Default::default()
        };
        frame(
            &ctx,
            key(egui::Key::Z, egui::Modifiers::COMMAND),
            &mut host,
            |ctx, host| {
                ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("text-field")));
                text_editor(ctx, host);
            },
        );
        assert!(ctx.text_edit_focused());
        assert_eq!(host.undo_calls, 0);
    }

    #[test]
    fn custom_editor_can_consume_a_shortcut_before_shared_handling() {
        let ctx = egui::Context::default();
        let mut host = Host {
            value: 1.0,
            ..Default::default()
        };
        frame(
            &ctx,
            key(egui::Key::Z, egui::Modifiers::COMMAND),
            &mut host,
            |ctx, _| {
                assert!(ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::Z)));
            },
        );
        assert_eq!(host.undo_calls, 0);
    }

    #[test]
    fn plain_alt_and_shift_control_y_do_not_trigger_history() {
        let ctx = egui::Context::default();
        let mut host = Host {
            value: 1.0,
            ..Default::default()
        };
        for (pressed, modifiers) in [
            (egui::Key::Z, egui::Modifiers::NONE),
            (
                egui::Key::Z,
                egui::Modifiers::COMMAND | egui::Modifiers::ALT,
            ),
            (
                egui::Key::Y,
                egui::Modifiers::CTRL | egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
            ),
            (
                egui::Key::Y,
                egui::Modifiers::MAC_CMD | egui::Modifiers::COMMAND,
            ),
        ] {
            frame(&ctx, key(pressed, modifiers), &mut host, custom_editor);
            assert!(ctx.input(|input| input.key_pressed(pressed)));
        }
        assert_eq!((host.undo_calls, host.redo_calls), (0, 0));
    }

    fn buttons(ctx: &egui::Context, input: egui::RawInput, host: &mut Host) -> [egui::Response; 2] {
        let mut responses = None;
        let mut output = ctx.run_ui(input, |ui| {
            responses = Some(ui.horizontal(|ui| history_buttons(ui, host)).inner);
        });
        output.textures_delta.clear();
        responses.unwrap()
    }

    fn click(ctx: &egui::Context, host: &mut Host, position: egui::Pos2) {
        for pressed in [true, false] {
            buttons(
                ctx,
                egui::RawInput {
                    events: vec![
                        egui::Event::PointerMoved(position),
                        egui::Event::PointerButton {
                            pos: position,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                    ..Default::default()
                },
                host,
            );
        }
    }

    #[test]
    fn buttons_disable_without_history_and_clicks_replay_actions() {
        let ctx = egui::Context::default();
        let mut host = Host::default();
        let responses = buttons(&ctx, Default::default(), &mut host);
        assert!(responses.iter().all(|response| !response.enabled()));
        click(&ctx, &mut host, responses[0].rect.center());
        assert_eq!(host.undo_calls, 0);
        host.value = 1.0;
        let responses = buttons(&ctx, Default::default(), &mut host);
        assert!(responses[0].enabled());
        assert!(!responses[1].enabled());
        click(&ctx, &mut host, responses[0].rect.center());
        assert_eq!(host.undo_calls, 1);
        let responses = buttons(&ctx, Default::default(), &mut host);
        assert!(!responses[0].enabled());
        assert!(responses[1].enabled());
        click(&ctx, &mut host, responses[1].rect.center());
        assert_eq!(host.redo_calls, 1);
        assert_eq!(host.value, 1.0);
    }

    #[test]
    fn failed_shortcut_keeps_state_and_exposes_error_until_success() {
        let ctx = egui::Context::default();
        let mut host = Host {
            value: 1.0,
            fail: true,
            ..Default::default()
        };
        frame(
            &ctx,
            key(egui::Key::Z, egui::Modifiers::COMMAND),
            &mut host,
            custom_editor,
        );
        assert_eq!(host.value, 1.0);
        assert_eq!(
            history_error(&ctx).as_deref(),
            Some("Undo failed: host rejected change")
        );
        host.fail = false;
        frame(
            &ctx,
            key(egui::Key::Z, egui::Modifiers::COMMAND),
            &mut host,
            custom_editor,
        );
        assert_eq!(host.value, 0.0);
        assert_eq!(history_error(&ctx), None);
    }
}
