pub fn editor(ctx: &rvst_ui::egui::Context, host: &mut dyn rvst_core::EditorHost) {
    rvst_ui::showcase_editor_with(ctx, host, |ui, host| {
        ui.heading("Play the MIDI voice");
        ui.label("Tap a note for a 300 ms voice. Notes travel through the real plugin and its MIDI output.");
        let controls = host
            .editor_context()
            .and_then(|value| value.downcast::<plugin_core::DebugControls>().ok());
        let native = !cfg!(target_arch = "wasm32");
        ui.add_enabled_ui(native && controls.is_some(), |ui| {
            ui.horizontal_wrapped(|ui| {
                for (offset, name) in [
                    "C4", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
                ]
                .iter()
                .enumerate()
                {
                    if ui.button(*name).clicked()
                        && let Some(controls) = &controls
                    {
                        controls.play_note(60 + offset as u8);
                    }
                }
                if ui.button("All notes off").clicked()
                    && let Some(controls) = &controls
                {
                    controls.all_notes_off();
                }
            });
        });
        if !native {
            ui.small("MIDI playback is available in the native runner. Browser preview does not process audio.");
        }
        ui.separator();
    });
}
