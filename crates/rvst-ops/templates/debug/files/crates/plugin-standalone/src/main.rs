fn main() -> Result<(), Box<dyn std::error::Error>> {
    rvst_standalone::run::<plugin_core::DemoPlugin>(plugin_ui::editor)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn midi_demo_note_sounds_in_standalone_without_an_external_midi_port() {
        let mut engine = rvst_standalone::AudioEngine::<plugin_core::DemoPlugin>::new(
            48_000.,
            rvst_standalone::InputSource::Silence,
        )
        .unwrap();
        let controls = engine
            .mirror
            .editor_context
            .as_ref()
            .unwrap()
            .clone()
            .downcast::<plugin_core::DebugControls>()
            .unwrap();
        controls.play_note(60);
        let mut output = [0.0f32; 1024];
        engine.render(&mut output, 2);
        assert!(output.iter().any(|value| value.abs() > 0.01));
        for _ in 0..32 {
            engine.render(&mut output, 2);
        }
        assert!(output.iter().all(|value| value.abs() < 1e-6));
    }
}
