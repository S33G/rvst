//! Shared egui controls. This crate has no native window, device, or filesystem dependency.
pub use egui;
use rvst_core::{EditorHost, Error, ParameterSpec};
use serde::Deserialize;
pub type EditorFn = fn(&egui::Context, &mut dyn EditorHost);

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct Typography {
    pub body_size: f32,
    pub heading_size: f32,
    pub monospace_size: f32,
    pub font_family: String,
}
impl Default for Typography {
    fn default() -> Self {
        Self {
            body_size: 15.0,
            heading_size: 26.0,
            monospace_size: 13.0,
            font_family: "proportional".into(),
        }
    }
}
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct MeterStyle {
    pub height: f32,
    pub show_decibels: bool,
}
impl Default for MeterStyle {
    fn default() -> Self {
        Self {
            height: 20.0,
            show_decibels: true,
        }
    }
}
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ThemeAssetKind {
    Font,
    Image,
}
#[derive(Clone, Debug, Deserialize)]
pub struct ThemeAsset {
    pub id: String,
    pub kind: ThemeAssetKind,
}
/// Portable, byte-owned assets. The shared crate never resolves filesystem paths.
pub struct ThemePack {
    pub theme: Theme,
    assets: std::collections::BTreeMap<String, Vec<u8>>,
}
impl ThemePack {
    pub fn from_bytes(
        manifest: &str,
        assets: std::collections::BTreeMap<String, Vec<u8>>,
    ) -> Result<Self, Error> {
        let theme = Theme::parse(manifest)?;
        if theme.assets.len() > 32 {
            return Err("theme has too many assets".into());
        }
        let mut seen = std::collections::BTreeSet::new();
        for asset in &theme.assets {
            if asset.id.is_empty()
                || !asset
                    .id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                || !seen.insert(&asset.id)
            {
                return Err("invalid or duplicate theme asset ID".into());
            }
            let bytes = assets
                .get(&asset.id)
                .ok_or_else(|| Error(format!("missing theme asset {}", asset.id)))?;
            if bytes.is_empty() || bytes.len() > 16 * 1024 * 1024 {
                return Err("theme asset must be 1 byte through 16 MiB".into());
            }
            match asset.kind {
                ThemeAssetKind::Font => {
                    ttf_parser::Face::parse(bytes, 0)
                        .map_err(|_| Error(format!("invalid font asset {}", asset.id)))?;
                }
                ThemeAssetKind::Image => {
                    let text = std::str::from_utf8(bytes)
                        .map_err(|_| Error("theme artwork must be SVG".into()))?;
                    if !text.contains("<svg") {
                        return Err("theme artwork must be SVG".into());
                    }
                }
            }
        }
        if let Some(id) = &theme.header_artwork
            && !theme
                .assets
                .iter()
                .any(|a| &a.id == id && a.kind == ThemeAssetKind::Image)
        {
            return Err("header artwork must reference an image asset".into());
        }
        if !matches!(
            theme.typography.font_family.as_str(),
            "proportional" | "monospace"
        ) && !theme
            .assets
            .iter()
            .any(|a| a.id == theme.typography.font_family && a.kind == ThemeAssetKind::Font)
        {
            return Err("font family must reference a font asset".into());
        }
        Ok(Self { theme, assets })
    }
    pub fn bundled(index: usize) -> Result<Self, Error> {
        let (manifest, logo) = match index {
            1 => (
                include_str!("../themes/light.toml"),
                include_bytes!("../assets/light.svg").as_slice(),
            ),
            2 => (
                include_str!("../themes/studio.toml"),
                include_bytes!("../assets/studio.svg").as_slice(),
            ),
            _ => (
                include_str!("../themes/dark.toml"),
                include_bytes!("../assets/dark.svg").as_slice(),
            ),
        };
        Self::from_bytes(
            manifest,
            [("logo".into(), logo.to_vec())].into_iter().collect(),
        )
    }
    pub fn asset_uri(&self, id: &str) -> String {
        format!("bytes://rvst-theme/{}/{id}.svg", self.theme.name)
    }
    /// Register images and named font families; call on setup or when a pack changes.
    pub fn install(&self, ctx: &egui::Context) {
        egui_extras::install_image_loaders(ctx);
        let mut fonts = egui::FontDefinitions::default();
        let mut has_fonts = false;
        for asset in &self.theme.assets {
            let bytes = self.assets.get(&asset.id).expect("validated theme asset");
            match asset.kind {
                ThemeAssetKind::Image => {
                    ctx.include_bytes(self.asset_uri(&asset.id), bytes.clone())
                }
                ThemeAssetKind::Font => {
                    fonts.font_data.insert(
                        asset.id.clone(),
                        std::sync::Arc::new(egui::FontData::from_owned(bytes.clone())),
                    );
                    fonts.families.insert(
                        egui::FontFamily::Name(asset.id.clone().into()),
                        vec![asset.id.clone()],
                    );
                    has_fonts = true;
                }
            }
        }
        if has_fonts {
            ctx.set_fonts(fonts);
        }
    }
}
fn apply_bundled_theme(ctx: &egui::Context, index: usize, contrast: bool, reduced: bool) {
    let id = egui::Id::new("rvst-installed-theme");
    if ctx.data(|d| d.get_temp::<usize>(id)) != Some(index) {
        let pack = ThemePack::bundled(index).expect("valid bundled pack");
        pack.install(ctx);
        ctx.data_mut(|d| d.insert_temp(id, index));
    }
    Theme::bundled(index)
        .apply(ctx, contrast, reduced)
        .expect("validated bundled theme");
}
fn header_logo(ui: &mut egui::Ui, index: usize) {
    let name = ["Dark", "Light", "Studio"].get(index).unwrap_or(&"Dark");
    ui.add(
        egui::Image::new(format!("bytes://rvst-theme/{name}/logo.svg"))
            .fit_to_exact_size(egui::vec2(32.0, 32.0))
            .alt_text("RVST waveform logo"),
    );
}

#[derive(Clone, Debug, Deserialize)]
pub struct Theme {
    pub version: u32,
    pub name: String,
    pub dark: bool,
    pub background: String,
    pub panel: String,
    pub text: String,
    pub muted: String,
    pub accent: String,
    pub border: String,
    pub error: String,
    pub spacing: f32,
    pub corner_radius: u8,
    #[serde(default)]
    pub typography: Typography,
    #[serde(default)]
    pub meter: MeterStyle,
    #[serde(default)]
    pub assets: Vec<ThemeAsset>,
    #[serde(default)]
    pub header_artwork: Option<String>,
}
fn color(value: &str) -> Result<egui::Color32, Error> {
    if value.len() != 7 || !value.starts_with('#') {
        return Err("theme color must be #RRGGBB".into());
    }
    let value =
        u32::from_str_radix(&value[1..], 16).map_err(|_| Error("invalid theme color".into()))?;
    Ok(egui::Color32::from_rgb(
        (value >> 16) as u8,
        (value >> 8) as u8,
        value as u8,
    ))
}
impl Theme {
    pub fn parse(source: &str) -> Result<Self, Error> {
        let theme: Self = toml::from_str(source).map_err(|e| Error(format!("theme: {e}")))?;
        if theme.version != 1
            || !theme.spacing.is_finite()
            || !(0.0..=48.0).contains(&theme.spacing)
        {
            return Err("unsupported or invalid theme".into());
        }
        if !theme.typography.body_size.is_finite()
            || !(8.0..=48.0).contains(&theme.typography.body_size)
            || !theme.typography.heading_size.is_finite()
            || !(8.0..=72.0).contains(&theme.typography.heading_size)
            || !theme.typography.monospace_size.is_finite()
            || !(8.0..=48.0).contains(&theme.typography.monospace_size)
            || !theme.meter.height.is_finite()
            || !(4.0..=64.0).contains(&theme.meter.height)
        {
            return Err("invalid theme typography or meter sizing".into());
        }
        for value in [
            &theme.background,
            &theme.panel,
            &theme.text,
            &theme.muted,
            &theme.accent,
            &theme.border,
            &theme.error,
        ] {
            color(value)?;
        }
        Ok(theme)
    }
    pub fn bundled(index: usize) -> Self {
        Self::parse(match index {
            1 => include_str!("../themes/light.toml"),
            2 => include_str!("../themes/studio.toml"),
            _ => include_str!("../themes/dark.toml"),
        })
        .expect("bundled theme is validated by tests")
    }
    pub fn apply(
        &self,
        ctx: &egui::Context,
        high_contrast: bool,
        reduced_motion: bool,
    ) -> Result<(), Error> {
        let mut style = egui::Style::default();
        let mut v = if self.dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };
        let background = color(&self.background)?;
        let panel = color(&self.panel)?;
        let text = color(&self.text)?;
        let accent = color(&self.accent)?;
        v.panel_fill = background;
        v.window_fill = panel;
        v.faint_bg_color = panel;
        v.extreme_bg_color = background;
        v.override_text_color = Some(text);
        v.selection.bg_fill = accent;
        v.selection.stroke = egui::Stroke::new(1.0, background);
        v.error_fg_color = color(&self.error)?;
        for widget in [
            &mut v.widgets.noninteractive,
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
        ] {
            widget.bg_fill = panel;
            widget.weak_bg_fill = panel;
            widget.fg_stroke = egui::Stroke::new(if high_contrast { 2.0 } else { 1.0 }, text);
            widget.bg_stroke = egui::Stroke::new(
                if high_contrast { 2.0 } else { 1.0 },
                if high_contrast {
                    text
                } else {
                    color(&self.border)?
                },
            );
            widget.corner_radius = egui::CornerRadius::same(self.corner_radius);
        }
        v.widgets.active.bg_stroke.color = accent;
        v.widgets.hovered.bg_stroke.color = accent;
        let family = match self.typography.font_family.as_str() {
            "proportional" => egui::FontFamily::Proportional,
            "monospace" => egui::FontFamily::Monospace,
            name => egui::FontFamily::Name(name.into()),
        };
        style.text_styles.insert(
            egui::TextStyle::Body,
            egui::FontId::new(self.typography.body_size, family.clone()),
        );
        style.text_styles.insert(
            egui::TextStyle::Button,
            egui::FontId::new(self.typography.body_size, family.clone()),
        );
        style.text_styles.insert(
            egui::TextStyle::Heading,
            egui::FontId::new(self.typography.heading_size, family),
        );
        style.text_styles.insert(
            egui::TextStyle::Monospace,
            egui::FontId::monospace(self.typography.monospace_size),
        );
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("rvst-meter-style"), self.meter.clone()));
        style.visuals = v;
        style.spacing.item_spacing = egui::vec2(self.spacing, self.spacing);
        if reduced_motion {
            style.animation_time = 0.0;
        }
        ctx.set_global_style(style);
        Ok(())
    }
}

/// Async byte services keep filesystem paths and platform handles outside shared UI.
/// A platform starts work and later returns bytes through poll; UI never blocks.
pub type RequestId = u64;
pub enum ByteResponse {
    Opened(Vec<u8>),
    Saved,
    Cancelled,
    Failed(String),
}
pub trait HostServices {
    fn request_open(&mut self, extension: &str) -> RequestId;
    fn request_save(&mut self, suggested_name: &str, bytes: Vec<u8>) -> RequestId;
    fn poll(&mut self, request: RequestId) -> Option<ByteResponse>;
}

#[derive(Clone)]
struct InspectorState {
    theme: usize,
    contrast: bool,
    reduced_motion: bool,
    zoom: f32,
    preset: Option<Vec<u8>>,
    status: String,
}
impl Default for InspectorState {
    fn default() -> Self {
        Self {
            theme: 0,
            contrast: false,
            reduced_motion: false,
            zoom: 1.0,
            preset: None,
            status: String::new(),
        }
    }
}

/// Rotary interaction with slider keyboard semantics, accessible text and fine adjustment.
pub fn parameter_knob(
    ui: &mut egui::Ui,
    host: &mut dyn EditorHost,
    spec: &ParameterSpec,
) -> egui::Response {
    let mut value = host.parameter(spec.id).unwrap_or(spec.default);
    ui.vertical(|ui| {
        ui.label(spec.name);
        let mut normalized = spec.normalize(value);
        let (rect, mut response) =
            ui.allocate_exact_size(egui::vec2(64.0, 64.0), egui::Sense::click_and_drag());
        response.widget_info(|| egui::WidgetInfo::slider(true, value, spec.name));
        if response.clicked() {
            response.request_focus();
        }
        let old = value;
        if response.drag_started() {
            host.begin_edit(spec.id);
        }
        if response.dragged() {
            let delta = ui.input(|i| i.pointer.delta());
            let fine = ui.input(|i| if i.modifiers.shift { 0.001 } else { 0.005 });
            normalized = (normalized + (delta.x - delta.y) as f64 * fine).clamp(0.0, 1.0);
            value = spec.denormalize(normalized);
        }
        if response.has_focus() {
            let direction = ui.input(|i| {
                i.num_presses(egui::Key::ArrowUp) as i32
                    + i.num_presses(egui::Key::ArrowRight) as i32
                    - i.num_presses(egui::Key::ArrowDown) as i32
                    - i.num_presses(egui::Key::ArrowLeft) as i32
            });
            if direction != 0 {
                value = spec.clamp(
                    value + direction as f64 * spec.step.unwrap_or((spec.max - spec.min) / 100.0),
                );
            }
        }
        if response.double_clicked() {
            value = spec.default;
        }
        if value != old {
            if !response.dragged() {
                host.begin_edit(spec.id);
            }
            host.set_parameter(spec.id, value);
            response.mark_changed();
        }
        if response.drag_stopped() || (value != old && !response.dragged()) {
            host.end_edit(spec.id);
        }
        let visuals = ui.style().interact(&response);
        let center = rect.center();
        ui.painter()
            .circle(center, 25.0, visuals.bg_fill, visuals.bg_stroke);
        let angle = (spec.normalize(value) as f32 * 1.5 + 0.75) * std::f32::consts::PI;
        ui.painter().line_segment(
            [center, center + egui::vec2(angle.cos(), angle.sin()) * 19.0],
            egui::Stroke::new(3.0, ui.visuals().selection.bg_fill),
        );
        ui.label(format!("{value:.2} {}", spec.unit));
        response.on_hover_text(format!(
            "{}: arrows adjust; Shift-drag fine; double-click resets",
            spec.name
        ))
    })
    .inner
}
/// Input is linear amplitude; text remains available without relying on color alone.
pub fn level_meter(ui: &mut egui::Ui, label: &str, peak: f32) {
    let peak = if peak.is_finite() { peak.max(0.0) } else { 0.0 };
    let db = 20.0 * peak.max(0.00001).log10();
    let style = ui
        .ctx()
        .data(|d| d.get_temp::<MeterStyle>(egui::Id::new("rvst-meter-style")))
        .unwrap_or_default();
    let text = if style.show_decibels {
        format!(
            "{label}: {db:.1} dBFS{}",
            if peak >= 1.0 { " · CLIP" } else { "" }
        )
    } else {
        label.into()
    };
    ui.add(
        egui::ProgressBar::new(((db + 60.0) / 60.0).clamp(0.0, 1.0))
            .desired_height(style.height)
            .text(text),
    );
}

pub fn set_inspector_url(ctx: &egui::Context, url: String) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("rvst-inspector-url"), url));
}

/// Default editor; the exact same function is called by native and web runners.
pub fn default_editor(ctx: &egui::Context, host: &mut dyn EditorHost) {
    let id = egui::Id::new("rvst-inspector");
    let mut state = ctx.data_mut(|d| d.get_temp::<InspectorState>(id).unwrap_or_default());
    apply_bundled_theme(ctx, state.theme, state.contrast, state.reduced_motion);
    ctx.set_zoom_factor(state.zoom);
    let mut root = egui::Ui::new(
        ctx.clone(),
        egui::Id::new("rvst-editor-root"),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.content_rect()),
    );
    egui::CentralPanel::default().show(&mut root, |ui| {
        ui.horizontal(|ui| {
            header_logo(ui, state.theme);
            ui.heading(host.descriptor().name);
        });
        ui.label(format!(
            "{} · {}",
            host.descriptor().vendor,
            host.descriptor().version
        ));
        if let Some(url) = ctx.data(|d| d.get_temp::<String>(egui::Id::new("rvst-inspector-url"))) {
            ui.horizontal_wrapped(|ui| {
                ui.hyperlink_to("Open native inspector in browser", &url);
                if ui.button("Copy inspector URL").clicked() {
                    ctx.copy_text(url.clone());
                }
            });
        }
        ui.separator();
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for spec in host.descriptor().parameters {
                    ui.push_id(spec.id, |ui| {
                        parameter_knob(ui, host, spec);
                    });
                }
            });
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                if ui.button("Reset parameters").clicked() {
                    for spec in host.descriptor().parameters {
                        host.begin_edit(spec.id);
                        host.set_parameter(spec.id, spec.default);
                        host.end_edit(spec.id);
                    }
                }
                if ui.button("Store parameter preset").clicked() {
                    match host.save_state() {
                        Ok(bytes) => {
                            state.preset = Some(bytes);
                            state.status = "Preset stored in this editor session".into()
                        }
                        Err(e) => state.status = e.to_string(),
                    }
                }
                if ui
                    .add_enabled(
                        state.preset.is_some(),
                        egui::Button::new("Recall parameter preset"),
                    )
                    .clicked()
                {
                    state.status = match host.load_state(state.preset.as_ref().unwrap()) {
                        Ok(()) => "Preset recalled".into(),
                        Err(e) => e.to_string(),
                    };
                }
            });
            if !state.status.is_empty() {
                ui.label(&state.status);
            }
            ui.collapsing("Appearance and accessibility", |ui| {
                egui::ComboBox::from_label("Theme")
                    .selected_text(["Dark", "Light", "Studio"][state.theme])
                    .show_ui(ui, |ui| {
                        for (i, name) in ["Dark", "Light", "Studio"].iter().enumerate() {
                            ui.selectable_value(&mut state.theme, i, *name);
                        }
                    });
                ui.checkbox(&mut state.contrast, "High contrast");
                ui.checkbox(&mut state.reduced_motion, "Reduced motion");
                ui.add(egui::Slider::new(&mut state.zoom, 0.75..=2.0).text("UI scale"));
            });
        });
    });
    ctx.data_mut(|d| d.insert_temp(id, state));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_theme_tokens_validate_and_differ() {
        let themes = (0..3).map(Theme::bundled).collect::<Vec<_>>();
        assert_ne!(themes[0].background, themes[1].background);
        assert_ne!(themes[0].accent, themes[2].accent);
        for t in themes {
            t.apply(&egui::Context::default(), true, true).unwrap();
        }
    }
    #[test]
    fn invalid_theme_schema_rejected() {
        let source = include_str!("../themes/dark.toml");
        assert!(Theme::parse(&source.replace("version = 1", "version = 99")).is_err());
        assert!(Theme::parse(&source.replace("#121820", "not-a-color")).is_err());
    }
}

#[cfg(test)]
mod interaction_tests {
    use super::*;
    use rvst_core::{PluginDescriptor, PluginKind};
    static PARAMS: [ParameterSpec; 1] = [ParameterSpec {
        id: 7,
        name: "Gain",
        unit: "dB",
        min: 0.0,
        max: 2.0,
        default: 1.0,
        step: Some(0.1),
        bypass: false,
    }];
    static DESC: PluginDescriptor = PluginDescriptor {
        id: "test",
        name: "Test",
        vendor: "RVST",
        version: "1",
        url: "",
        email: "",
        vst3_id: [1; 16],
        kind: PluginKind::Effect,
        parameters: &PARAMS,
        inputs: &[],
        outputs: &[],
        midi_input: false,
        midi_output: false,
        supports_f64: false,
    };
    #[derive(Default)]
    struct Host {
        value: f64,
        gestures: Vec<bool>,
    }
    impl EditorHost for Host {
        fn descriptor(&self) -> &'static PluginDescriptor {
            &DESC
        }
        fn parameter(&self, _: u32) -> Option<f64> {
            Some(self.value)
        }
        fn begin_edit(&mut self, _: u32) {
            self.gestures.push(true)
        }
        fn set_parameter(&mut self, _: u32, v: f64) {
            self.value = v
        }
        fn end_edit(&mut self, _: u32) {
            self.gestures.push(false)
        }
        fn save_state(&mut self) -> Result<Vec<u8>, Error> {
            Ok(vec![])
        }
        fn load_state(&mut self, _: &[u8]) -> Result<(), Error> {
            Ok(())
        }
    }
    #[test]
    fn keyboard_knob_sends_balanced_gesture() {
        let ctx = egui::Context::default();
        let mut host = Host {
            value: 1.0,
            ..Default::default()
        };
        let mut id = None;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            id = Some(parameter_knob(ui, &mut host, &PARAMS[0]).id);
        });
        output.textures_delta.clear();
        ctx.memory_mut(|m| m.request_focus(id.unwrap()));
        let input = egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::ArrowUp,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::default(),
            }],
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            parameter_knob(ui, &mut host, &PARAMS[0]);
        });
        output.textures_delta.clear();
        assert!((host.value - 1.1).abs() < 1e-9);
        assert_eq!(host.gestures, vec![true, false]);
    }

    #[test]
    fn unchanged_keyboard_edit_has_no_unbalanced_gesture() {
        let ctx = egui::Context::default();
        let mut host = Host {
            value: PARAMS[0].max,
            ..Default::default()
        };
        let mut id = None;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            id = Some(parameter_knob(ui, &mut host, &PARAMS[0]).id);
        });
        output.textures_delta.clear();
        ctx.memory_mut(|m| m.request_focus(id.unwrap()));
        let input = egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::ArrowUp,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            parameter_knob(ui, &mut host, &PARAMS[0]);
        });
        output.textures_delta.clear();
        assert!(host.gestures.is_empty());
    }
    #[test]
    fn inspector_generates_paint_for_every_theme() {
        for theme in 0..3 {
            let ctx = egui::Context::default();
            ctx.data_mut(|d| {
                d.insert_temp(
                    egui::Id::new("rvst-inspector"),
                    InspectorState {
                        theme,
                        ..Default::default()
                    },
                )
            });
            let mut host = Host::default();
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                default_editor(ui.ctx(), &mut host)
            });
            assert!(!output.shapes.is_empty());
            output.textures_delta.clear();
        }
    }
}

/// Source labels must describe the actual runner; no fabricated callback metrics.
pub fn set_runtime_label(ctx: &egui::Context, label: impl Into<String>) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("rvst-runtime-label"), label.into()));
}
#[derive(Clone, Default)]
struct TourState {
    page: usize,
    theme: usize,
    contrast: bool,
    reduced_motion: bool,
    zoom: f32,
    preset: Option<Vec<u8>>,
    events: Vec<String>,
    notice: String,
}
/// Guided debug plugin UI. Parameters and events come from the supplied host.
pub fn showcase_editor(ctx: &egui::Context, host: &mut dyn EditorHost) {
    showcase_editor_with(ctx, host, |_, _| {});
}
/// Add plugin-specific controls to the Events page without duplicating the shared tour.
pub fn showcase_editor_with(
    ctx: &egui::Context,
    host: &mut dyn EditorHost,
    extra_events: impl FnOnce(&mut egui::Ui, &mut dyn EditorHost),
) {
    let id = egui::Id::new("rvst-showcase");
    let mut state = ctx.data_mut(|d| {
        d.get_temp::<TourState>(id).unwrap_or_else(|| TourState {
            zoom: 1.0,
            ..Default::default()
        })
    });
    apply_bundled_theme(ctx, state.theme, state.contrast, state.reduced_motion);
    ctx.set_zoom_factor(state.zoom);
    let mut root = egui::Ui::new(
        ctx.clone(),
        egui::Id::new("rvst-showcase-root"),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.content_rect()),
    );
    egui::CentralPanel::default().show(&mut root,|ui|{
        ui.horizontal(|ui|{header_logo(ui,state.theme);ui.heading("RVST / playground");ui.label(host.descriptor().version);});
        ui.label("Turn a control. Hear the result. Inspect the engine.");
        ui.small(ctx.data(|d|d.get_temp::<String>(egui::Id::new("rvst-runtime-label"))).unwrap_or_else(||"Plugin host".into()));
        ui.add_space(12.0);
        ui.horizontal_wrapped(|ui|{for(i,title)in ["1 · Signal","2 · Events","3 · Themes","4 · Inspect"].iter().enumerate(){ui.selectable_value(&mut state.page,i,*title);}});
        ui.separator();
        egui::ScrollArea::vertical().show(ui,|ui|match state.page{
            0=>{
                ui.heading("Make it move");
                ui.label("These are live plugin parameters. Drag a knob, use arrow keys, or Shift-drag for a fine adjustment. Double-click restores its default.");
                ui.add_space(12.0);
                if host.descriptor().parameters.iter().any(|p|p.name=="Tone"){ui.label("Start here: raise Tone above 0 Hz. Waveform: 0 sine, 1 saw, 2 square. Try the MIDI keyboard on Events.");}
                ui.horizontal_wrapped(|ui|{for spec in host.descriptor().parameters{ui.push_id(spec.id,|ui|{let before=host.parameter(spec.id);let response=parameter_knob(ui,host,spec);if response.changed(){state.events.push(format!("{} → {:.3} {}",spec.name,host.parameter(spec.id).unwrap_or(spec.default),spec.unit));}else{let _=before;}});}});
                if host.descriptor().parameters.is_empty(){ui.label("This plugin exposes no parameters.");}
                ui.add_space(12.0);
                if let Some(meters)=ctx.data(|d|d.get_temp::<MeterSnapshot>(egui::Id::new("rvst-meters"))){level_meter(ui,"Input",meters.input_peak);level_meter(ui,"Output",meters.output_peak);ui.small(format!("Native audio clock: {} frames",meters.sample_clock));}else{ui.label("Audio meters appear when connected to the native runner.");}
                ui.separator();
                ui.horizontal_wrapped(|ui|{
                    if ui.button("Store parameter preset").clicked(){match host.save_state(){Ok(v)=>{state.preset=Some(v);state.notice="Preset stored for this session. Change a knob, then recall it.".into()},Err(e)=>state.notice=e.to_string()}}
                    if ui.add_enabled(state.preset.is_some(),egui::Button::new("Recall parameter preset")).clicked(){state.notice=match host.load_state(state.preset.as_ref().unwrap()){Ok(())=>{state.events.push("Preset recalled".into());"Preset recalled".into()},Err(e)=>e.to_string()};}
                    if ui.button("Reset all knobs").clicked(){for p in host.descriptor().parameters{host.begin_edit(p.id);host.set_parameter(p.id,p.default);host.end_edit(p.id);}state.events.push("Parameters reset".into());}
                });
                ui.label(&state.notice);
            },
            1=>{
                ui.heading("See your changes");
                extra_events(ui,host);ui.label("This event trail records control changes made in this editor. The browser inspector carries native callback logs and transport metrics.");
                if state.events.is_empty(){ui.label("No edits yet. Return to Signal and turn a knob.");}
                for event in state.events.iter().rev().take(32){ui.monospace(event);}
                if ui.button("Clear event trail").clicked(){state.events.clear();}
                ui.separator();ui.label(format!("MIDI input: {} · MIDI output: {}",host.descriptor().midi_input,host.descriptor().midi_output));
                ui.small("MIDI capabilities describe the plugin contract. Note playback depends on the connected native host.");
            },
            2=>{
                ui.heading("Make it yours");ui.label("Three versioned token themes. The same Rust layout runs inside your DAW, the standalone app, and the browser.");
                ui.horizontal(|ui|{for(i,name)in ["Dark","Light","Studio"].iter().enumerate(){ui.selectable_value(&mut state.theme,i,*name);}});
                ui.checkbox(&mut state.contrast,"High contrast controls");ui.checkbox(&mut state.reduced_motion,"Reduced motion");ui.add(egui::Slider::new(&mut state.zoom,0.75..=2.0).text("UI scale"));
                ui.separator();let theme=Theme::bundled(state.theme);ui.label(format!("{} · theme schema v{}",theme.name,theme.version));ui.small("Keyboard: Tab to a knob, arrows to adjust. All values also have text labels.");
            },
            _=>{
                ui.heading("Look inside the native engine");
                if let Some(url)=ctx.data(|d|d.get_temp::<String>(egui::Id::new("rvst-inspector-url"))){ui.label("Connected native runtime. Open the live logger and callback metrics in your browser.");ui.hyperlink_to("Open native inspector",&url);if ui.button("Copy inspector URL").clicked(){ctx.copy_text(url);}}
                else{ui.label("Native inspector is available when the native runner is built with the devtools feature. This browser view previews controls only.");}
                ui.separator();ui.monospace(format!("{}\n{}\n{} input channels → {} output channels\nf64 processing: {}",host.descriptor().name,host.descriptor().id,host.descriptor().input_channels(),host.descriptor().output_channels(),host.descriptor().supports_f64));
                egui::Grid::new("rvst-feature-matrix").striped(true).show(ui,|ui|{
                    for (name,available) in [
                        ("Double precision DSP",host.descriptor().supports_f64),
                        ("MIDI input",host.descriptor().midi_input),
                        ("MIDI output",host.descriptor().midi_output),
                        ("Auxiliary sidechain",host.descriptor().inputs.iter().any(|bus|bus.auxiliary)),
                        ("Smoothing controls",host.descriptor().parameters.iter().any(|p|p.name=="Smoothing")),
                    ]{ui.label(name);ui.label(if available{"Plugin supports"}else{"Not exposed by this plugin"});ui.end_row();}
                });
                ui.small("Capabilities describe plugin DSP. Device/DAW routing and browser audio availability are separate.");
                ui.label("Try the tour: adjust gain → store a preset → change it → recall → switch theme → inspect native logs.");
            }
        });
        ui.add_space(12.0);ui.separator();ui.small("Built in Rust · one shared editor · no browser audio processing");
    });
    if state.events.len() > 96 {
        state.events.drain(..state.events.len() - 96);
    }
    ctx.data_mut(|d| d.insert_temp(id, state));
}
#[derive(Clone, Copy, Debug, Default)]
pub struct MeterSnapshot {
    pub input_peak: f32,
    pub output_peak: f32,
    pub sample_clock: u64,
}
/// Lock-free scalar meter transport; the UI only reads snapshots and does not own DSP.
#[derive(Debug, Default)]
pub struct Meters {
    input: std::sync::atomic::AtomicU32,
    output: std::sync::atomic::AtomicU32,
    clock: std::sync::atomic::AtomicU64,
}
impl Meters {
    pub fn publish(&self, input: f32, output: f32, clock: u64) {
        use std::sync::atomic::Ordering;
        self.input.store(input.to_bits(), Ordering::Relaxed);
        self.output.store(output.to_bits(), Ordering::Relaxed);
        self.clock.store(clock, Ordering::Release);
    }
    pub fn snapshot(&self) -> MeterSnapshot {
        use std::sync::atomic::Ordering;
        let sample_clock = self.clock.load(Ordering::Acquire);
        MeterSnapshot {
            input_peak: f32::from_bits(self.input.load(Ordering::Relaxed)),
            output_peak: f32::from_bits(self.output.load(Ordering::Relaxed)),
            sample_clock,
        }
    }
}
pub fn set_meter_snapshot(ctx: &egui::Context, snapshot: MeterSnapshot) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("rvst-meters"), snapshot));
}

#[cfg(test)]
mod theme_pack_tests {
    use super::*;
    #[test]
    fn portable_pack_installs_named_font_and_artwork() {
        let source = include_str!("../themes/studio.toml")
            .replace("font_family = \"proportional\"", "font_family = \"custom\"")
            + "\n[[assets]]\nid = \"custom\"\nkind = \"font\"\n";
        let fonts = egui::FontDefinitions::default();
        let font = fonts.font_data.values().next().unwrap().font.to_vec();
        let assets = [
            (
                "logo".into(),
                include_bytes!("../assets/studio.svg").to_vec(),
            ),
            ("custom".into(), font),
        ]
        .into_iter()
        .collect();
        let pack = ThemePack::from_bytes(&source, assets).unwrap();
        let ctx = egui::Context::default();
        pack.install(&ctx);
        pack.theme.apply(&ctx, false, false).unwrap();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.label("Portable font");
            ui.add(
                egui::Image::new(pack.asset_uri("logo")).fit_to_exact_size(egui::vec2(32., 32.)),
            );
        });
        assert!(!output.shapes.is_empty());
        output.textures_delta.clear();
    }
    #[test]
    fn missing_asset_invalid_font_and_duplicate_id_rejected() {
        let source = include_str!("../themes/dark.toml");
        assert!(ThemePack::from_bytes(source, Default::default()).is_err());
        let source = source.to_owned() + "\n[[assets]]\nid = \"broken\"\nkind = \"font\"\n";
        assert!(
            ThemePack::from_bytes(
                &source,
                [
                    ("logo".into(), include_bytes!("../assets/dark.svg").to_vec()),
                    ("broken".into(), vec![0, 1, 2])
                ]
                .into_iter()
                .collect()
            )
            .is_err()
        );
    }
}

#[cfg(test)]
mod artwork_loading_regression {
    use super::*;
    #[test]
    fn all_bundled_artwork_decodes_to_pixels() {
        for index in 0..3 {
            let context = egui::Context::default();
            let pack = ThemePack::bundled(index).unwrap();
            pack.install(&context);
            let image =
                context.try_load_image(&pack.asset_uri("logo"), egui::load::SizeHint::default());
            assert!(
                matches!(image, Ok(egui::load::ImagePoll::Ready { .. })),
                "theme image did not decode"
            );
        }
    }
}
