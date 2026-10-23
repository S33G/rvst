fn main() -> Result<(), Box<dyn std::error::Error>> {
    rvst_standalone::run::<plugin_core::DemoPlugin>(plugin_ui::editor)?;
    Ok(())
}
