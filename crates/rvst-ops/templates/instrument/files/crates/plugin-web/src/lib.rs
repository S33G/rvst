#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub async fn start() -> Result<(), wasm_bindgen::JsValue> {
    rvst_web::start::<plugin_core::DemoPlugin>("rvst-canvas", plugin_ui::editor).await
}
