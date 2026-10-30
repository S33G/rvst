//! Direct CLAP ABI adapter. No wrapper framework or backend is involved.
#![allow(unsafe_op_in_unsafe_fn)]
pub use clap_sys;
mod compat;
use clap_sys::{
    entry::*,
    events::*,
    ext::{
        audio_ports::*, audio_ports_config::*, gui::*, latency::*, note_ports::*, params::*,
        state::*, tail::*,
    },
    factory::plugin_factory::*,
    host::*,
    plugin::*,
    process::*,
    stream::*,
    version::*,
};
use rvst_adapter::{Runtime, boundary};
pub use rvst_core;
use rvst_core::*;
pub use rvst_ui::EditorFn;
use std::{
    cell::UnsafeCell,
    ffi::{CStr, CString, c_char, c_void},
    marker::PhantomData,
    ptr,
    sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering},
};

pub struct Descriptor {
    pub raw: clap_plugin_descriptor,
    _strings: Vec<CString>,
    _features: Vec<*const c_char>,
}
unsafe impl Send for Descriptor {}
unsafe impl Sync for Descriptor {}
impl Descriptor {
    pub fn new(d: &PluginDescriptor) -> Self {
        let strings: Vec<_> = [
            d.id,
            d.name,
            d.vendor,
            d.url,
            d.version,
            match d.kind {
                PluginKind::Effect => "audio-effect",
                PluginKind::Instrument => "instrument",
                PluginKind::Midi => "note-effect",
            },
        ]
        .into_iter()
        .map(|s| CString::new(s).expect("descriptor contains NUL"))
        .collect();
        let features = vec![strings[5].as_ptr(), ptr::null()];
        let raw = clap_plugin_descriptor {
            clap_version: CLAP_VERSION,
            id: strings[0].as_ptr(),
            name: strings[1].as_ptr(),
            vendor: strings[2].as_ptr(),
            url: strings[3].as_ptr(),
            manual_url: c"".as_ptr(),
            support_url: c"".as_ptr(),
            version: strings[4].as_ptr(),
            description: c"".as_ptr(),
            features: features.as_ptr(),
        };
        Self {
            raw,
            _strings: strings,
            _features: features,
        }
    }
}
pub trait Definition: 'static {
    type Plugin: Plugin;
    fn descriptor() -> &'static Descriptor;
    fn editor() -> EditorFn;
}
pub struct Export<D>(PhantomData<D>);
struct Instance<D: Definition> {
    gate: rvst_adapter::AccessGate,
    raw: clap_plugin,
    mirror: std::sync::Arc<rvst_adapter::ParameterMirror>,
    gui: UnsafeCell<Gui>,
    runtime: UnsafeCell<Runtime<D::Plugin>>,
    processing: AtomicBool,
    missed_audio: AtomicBool,
    active: AtomicBool,
    latency: AtomicU32,
    latency_restart_pending: AtomicBool,
    host_latency: AtomicPtr<clap_host_latency>,
    tail: AtomicU32,
    _host: *const clap_host,
    host_params: AtomicPtr<clap_host_params>,
    configuration: AtomicU32,
    _type: PhantomData<D>,
}
impl<D: Definition> Export<D> {
    const GUI: clap_plugin_gui = clap_plugin_gui {
        is_api_supported: Some(Self::gui_supported),
        get_preferred_api: Some(Self::gui_preferred),
        create: Some(Self::gui_create),
        destroy: Some(Self::gui_destroy),
        set_scale: Some(Self::gui_scale),
        get_size: Some(Self::gui_size),
        can_resize: Some(Self::gui_can_resize),
        get_resize_hints: Some(Self::gui_hints),
        adjust_size: Some(Self::gui_adjust),
        set_size: Some(Self::gui_set_size),
        set_parent: Some(Self::gui_parent),
        set_transient: None,
        suggest_title: None,
        show: Some(Self::gui_show),
        hide: Some(Self::gui_hide),
    };
    unsafe extern "C" fn gui_supported(
        _: *const clap_plugin,
        api: *const c_char,
        floating: bool,
    ) -> bool {
        boundary(false, || {
            !floating && !api.is_null() && CStr::from_ptr(api) == gui_api()
        })
    }
    unsafe extern "C" fn gui_preferred(
        _: *const clap_plugin,
        api: *mut *const c_char,
        floating: *mut bool,
    ) -> bool {
        boundary(false, || {
            if api.is_null() || floating.is_null() {
                return false;
            }
            *api = gui_api().as_ptr();
            *floating = false;
            true
        })
    }
    unsafe extern "C" fn gui_create(
        p: *const clap_plugin,
        api: *const c_char,
        floating: bool,
    ) -> bool {
        boundary(false, || {
            if !Self::gui_supported(p, api, floating) {
                return false;
            }
            let g = &mut *Self::instance(p).gui.get();
            if g.created {
                return false;
            }
            g.created = true;
            true
        })
    }
    unsafe extern "C" fn gui_destroy(p: *const clap_plugin) {
        boundary((), || {
            let g = &mut *Self::instance(p).gui.get();
            g.editor.take();
            g.created = false;
        })
    }
    unsafe extern "C" fn gui_scale(p: *const clap_plugin, scale: f64) -> bool {
        boundary(false, || {
            if !scale.is_finite() || scale <= 0. {
                return false;
            }
            let g = &mut *Self::instance(p).gui.get();
            if g.editor.is_some() {
                return false;
            }
            g.options.scale = scale as f32;
            true
        })
    }
    unsafe extern "C" fn gui_size(p: *const clap_plugin, w: *mut u32, h: *mut u32) -> bool {
        boundary(false, || {
            if w.is_null() || h.is_null() {
                return false;
            }
            let g = &*Self::instance(p).gui.get();
            *w = g.options.width;
            *h = g.options.height;
            true
        })
    }
    unsafe extern "C" fn gui_can_resize(_: *const clap_plugin) -> bool {
        boundary(false, || true)
    }
    unsafe extern "C" fn gui_hints(_: *const clap_plugin, h: *mut clap_gui_resize_hints) -> bool {
        boundary(false, || {
            if h.is_null() {
                return false;
            }
            *h = clap_gui_resize_hints {
                can_resize_horizontally: true,
                can_resize_vertically: true,
                preserve_aspect_ratio: false,
                aspect_ratio_width: 0,
                aspect_ratio_height: 0,
            };
            true
        })
    }
    unsafe extern "C" fn gui_adjust(_: *const clap_plugin, w: *mut u32, h: *mut u32) -> bool {
        boundary(false, || {
            if w.is_null() || h.is_null() {
                return false;
            }
            let (min_w, min_h) = D::Plugin::editor_min_size();
            *w = (*w).clamp(min_w, 4096.max(min_w));
            *h = (*h).clamp(min_h, 4096.max(min_h));
            true
        })
    }
    unsafe extern "C" fn gui_set_size(p: *const clap_plugin, w: u32, h: u32) -> bool {
        boundary(false, || {
            let (min_w, min_h) = D::Plugin::editor_min_size();
            if w < min_w || h < min_h || w == 0 || h == 0 {
                return false;
            }
            let g = &mut *Self::instance(p).gui.get();
            if let Some(e) = &g.editor
                && e.resize(w, h).is_err()
            {
                return false;
            }
            g.options.width = w;
            g.options.height = h;
            true
        })
    }
    unsafe extern "C" fn gui_parent(p: *const clap_plugin, window: *const clap_window) -> bool {
        boundary(false, || {
            if window.is_null() || !Self::gui_supported(p, (*window).api, false) {
                return false;
            }
            let i = Self::instance(p);
            let g = &mut *i.gui.get();
            if !g.created || g.editor.is_some() {
                return false;
            }
            let raw = (*window).specific.ptr;
            let Ok(parent) = rvst_platform::parent_from_raw(raw) else {
                return false;
            };
            let host = Box::new(ClapEditor {
                mirror: i.mirror.clone(),
                host: i._host as usize,
                params: i.host_params.load(Ordering::Acquire) as usize,
            });
            g.options.inspector_url = i.mirror.inspector_url.clone();
            g.options.meters = Some(i.mirror.meters.clone());
            g.options.ui_metrics = i.mirror.ui_metrics.clone();
            match rvst_platform::NativeEditor::open(
                host,
                D::editor(),
                g.options.clone(),
                Some(parent),
            ) {
                Ok(e) => {
                    let _ = e.hide();
                    g.editor = Some(e);
                    true
                }
                Err(_) => false,
            }
        })
    }
    unsafe extern "C" fn gui_show(p: *const clap_plugin) -> bool {
        boundary(false, || {
            (&*Self::instance(p).gui.get())
                .editor
                .as_ref()
                .is_some_and(|e| e.show().is_ok())
        })
    }
    unsafe extern "C" fn gui_hide(p: *const clap_plugin) -> bool {
        boundary(false, || {
            (&*Self::instance(p).gui.get())
                .editor
                .as_ref()
                .is_some_and(|e| e.hide().is_ok())
        })
    }
    pub const ENTRY: clap_plugin_entry = clap_plugin_entry {
        clap_version: CLAP_VERSION,
        init: Some(Self::init_entry),
        deinit: Some(Self::deinit_entry),
        get_factory: Some(Self::factory),
    };
    const FACTORY: clap_plugin_factory = clap_plugin_factory {
        get_plugin_count: Some(Self::count),
        get_plugin_descriptor: Some(Self::descriptor),
        create_plugin: Some(Self::create),
    };
    const PARAMS: clap_plugin_params = clap_plugin_params {
        count: Some(Self::param_count),
        get_info: Some(Self::param_info),
        get_value: Some(Self::param_value),
        value_to_text: Some(Self::param_text),
        text_to_value: Some(Self::text_param),
        flush: Some(Self::flush),
    };
    const PORTS: clap_plugin_audio_ports = clap_plugin_audio_ports {
        count: Some(Self::port_count),
        get: Some(Self::port_info),
    };
    const CONFIGS: clap_plugin_audio_ports_config = clap_plugin_audio_ports_config {
        count: Some(Self::config_count),
        get: Some(Self::config_get),
        select: Some(Self::config_select),
    };
    const CONFIG_INFO: clap_plugin_audio_ports_config_info = clap_plugin_audio_ports_config_info {
        current_config: Some(Self::current_config),
        get: Some(Self::config_port_info),
    };
    unsafe extern "C" fn config_count(_: *const clap_plugin) -> u32 {
        if D::Plugin::descriptor().inputs.iter().any(|b| b.auxiliary) {
            2
        } else {
            1
        }
    }
    unsafe extern "C" fn current_config(p: *const clap_plugin) -> u32 {
        Self::instance(p).configuration.load(Ordering::Acquire)
    }
    unsafe extern "C" fn config_select(p: *const clap_plugin, config: u32) -> bool {
        boundary(false, || {
            let i = Self::instance(p);
            if i.active.load(Ordering::Acquire) || config >= Self::config_count(p) {
                return false;
            }
            i.configuration.store(config, Ordering::Release);
            true
        })
    }
    unsafe extern "C" fn config_get(
        p: *const clap_plugin,
        index: u32,
        out: *mut clap_audio_ports_config,
    ) -> bool {
        boundary(false, || {
            if out.is_null() || index >= Self::config_count(p) {
                return false;
            }
            let d = D::Plugin::descriptor();
            *out = std::mem::zeroed();
            let o = &mut *out;
            o.id = index;
            copy_text(
                if index == 0 {
                    "All inputs"
                } else {
                    "Main inputs only"
                },
                &mut o.name,
            );
            o.input_port_count = compat::input_count::<D::Plugin>(index) as u32;
            o.output_port_count = d.outputs.len() as u32;
            if let Some(b) = d.inputs.iter().find(|b| !b.auxiliary) {
                o.has_main_input = true;
                o.main_input_channel_count = b.channels;
                o.main_input_port_type = port_type(b.channels);
            }
            if let Some(b) = d.outputs.iter().find(|b| !b.auxiliary) {
                o.has_main_output = true;
                o.main_output_channel_count = b.channels;
                o.main_output_port_type = port_type(b.channels);
            }
            true
        })
    }
    unsafe extern "C" fn config_port_info(
        p: *const clap_plugin,
        config: u32,
        index: u32,
        input: bool,
        out: *mut clap_audio_port_info,
    ) -> bool {
        boundary(false, || {
            if config >= Self::config_count(p) {
                return false;
            }
            Self::port_info_for_config(config, index, input, out)
        })
    }
    const NOTES: clap_plugin_note_ports = clap_plugin_note_ports {
        count: Some(Self::note_count),
        get: Some(Self::note_info),
    };
    const STATE: clap_plugin_state = clap_plugin_state {
        save: Some(Self::save),
        load: Some(Self::load),
    };
    const LATENCY: clap_plugin_latency = clap_plugin_latency {
        get: Some(Self::latency),
    };
    const TAIL: clap_plugin_tail = clap_plugin_tail {
        get: Some(Self::tail),
    };
    unsafe fn instance<'a>(p: *const clap_plugin) -> &'a Instance<D> {
        &*((*p).plugin_data as *const Instance<D>)
    }
    unsafe extern "C" fn init_entry(_: *const c_char) -> bool {
        boundary(false, || {
            let _ = D::descriptor();
            true
        })
    }
    unsafe extern "C" fn deinit_entry() {
        boundary((), || {})
    }
    unsafe extern "C" fn factory(id: *const c_char) -> *const c_void {
        boundary(ptr::null(), || {
            if !id.is_null() && CStr::from_ptr(id) == CLAP_PLUGIN_FACTORY_ID {
                &Self::FACTORY as *const _ as _
            } else {
                ptr::null()
            }
        })
    }
    unsafe extern "C" fn count(_: *const clap_plugin_factory) -> u32 {
        boundary(0, || 1)
    }
    unsafe extern "C" fn descriptor(
        _: *const clap_plugin_factory,
        index: u32,
    ) -> *const clap_plugin_descriptor {
        boundary(ptr::null(), || {
            if index == 0 {
                &D::descriptor().raw
            } else {
                ptr::null()
            }
        })
    }
    unsafe extern "C" fn create(
        _: *const clap_plugin_factory,
        host: *const clap_host,
        id: *const c_char,
    ) -> *const clap_plugin {
        boundary(ptr::null(), || {
            if host.is_null()
                || id.is_null()
                || CStr::from_ptr(id).to_bytes() != D::Plugin::descriptor().id.as_bytes()
                || !clap_version_is_compatible((*host).clap_version)
            {
                return ptr::null();
            }
            let runtime = Runtime::<D::Plugin>::new();
            let mirror = runtime.mirror.clone();
            let mut instance = Box::new(Instance::<D> {
                gate: Default::default(),
                mirror,
                gui: UnsafeCell::new(Gui {
                    options: rvst_platform::EditorOptions {
                        title: D::Plugin::descriptor().name.into(),
                        width: D::Plugin::editor_size().0,
                        height: D::Plugin::editor_size().1,
                        ..Default::default()
                    },
                    ..Default::default()
                }),
                raw: clap_plugin {
                    desc: &D::descriptor().raw,
                    plugin_data: ptr::null_mut(),
                    init: Some(Self::init),
                    destroy: Some(Self::destroy),
                    activate: Some(Self::activate),
                    deactivate: Some(Self::deactivate),
                    start_processing: Some(Self::start),
                    stop_processing: Some(Self::stop),
                    reset: Some(Self::reset),
                    process: Some(Self::process),
                    get_extension: Some(Self::extension),
                    on_main_thread: Some(Self::main),
                },
                runtime: UnsafeCell::new(runtime),
                processing: AtomicBool::new(false),
                missed_audio: AtomicBool::new(false),
                active: AtomicBool::new(false),
                latency: AtomicU32::new(0),
                latency_restart_pending: AtomicBool::new(false),
                host_latency: AtomicPtr::new(ptr::null_mut()),
                tail: AtomicU32::new(0),
                _host: host,
                host_params: AtomicPtr::new(ptr::null_mut()),
                configuration: AtomicU32::new(0),
                _type: PhantomData,
            });
            instance.raw.plugin_data = &mut *instance as *mut _ as _;
            let p = &instance.raw as *const _;
            let _ = Box::into_raw(instance);
            p
        })
    }
    unsafe extern "C" fn init(p: *const clap_plugin) -> bool {
        boundary(false, || {
            let i = Self::instance(p);
            if let Some(get) = (*i._host).get_extension {
                i.host_params.store(
                    get(i._host, CLAP_EXT_PARAMS.as_ptr()) as *mut clap_host_params,
                    Ordering::Release,
                );
                i.host_latency.store(
                    get(i._host, CLAP_EXT_LATENCY.as_ptr()) as *mut clap_host_latency,
                    Ordering::Release,
                );
            }
            true
        })
    }
    unsafe extern "C" fn destroy(p: *const clap_plugin) {
        boundary((), || {
            if !p.is_null() {
                drop(Box::from_raw((*p).plugin_data as *mut Instance<D>));
            }
        })
    }
    unsafe extern "C" fn activate(p: *const clap_plugin, sr: f64, _min: u32, max: u32) -> bool {
        boundary(false, || {
            let i = Self::instance(p);
            if i.active.load(Ordering::Acquire) {
                return false;
            }
            let rt = &mut *i.runtime.get();
            let ok = rt.prepare(sr, max as usize);
            if ok {
                let latency = rt.plugin.latency();
                let previous = i.latency.swap(latency, Ordering::AcqRel);
                i.mirror.latency.store(latency, Ordering::Release);
                i.latency_restart_pending.store(false, Ordering::Release);
                if latency != previous {
                    let host_latency = i.host_latency.load(Ordering::Acquire);
                    if !host_latency.is_null()
                        && let Some(changed) = (*host_latency).changed
                    {
                        // CLAP allows latency.changed only while being activated.
                        changed(i._host);
                    }
                }
                i.tail
                    .store(rvst_adapter::tail(rt.plugin.tail()), Ordering::Release);
                i.active.store(true, Ordering::Release);
            }
            ok
        })
    }
    unsafe extern "C" fn deactivate(p: *const clap_plugin) {
        boundary((), || {
            Self::instance(p).processing.store(false, Ordering::Release);
            Self::instance(p).active.store(false, Ordering::Release);
        })
    }
    unsafe extern "C" fn start(p: *const clap_plugin) -> bool {
        boundary(false, || {
            let i = Self::instance(p);
            if !i.active.load(Ordering::Acquire) {
                return false;
            }
            i.processing.store(true, Ordering::Release);
            if let Some(_access) = i.gate.audio() {
                (&mut *i.runtime.get()).debug_event(2, 0.);
            }
            true
        })
    }
    unsafe extern "C" fn stop(p: *const clap_plugin) {
        boundary((), || {
            let i = Self::instance(p);
            i.processing.store(false, Ordering::Release);
            if let Some(_access) = i.gate.audio() {
                (&mut *i.runtime.get()).debug_event(3, 0.);
            }
        })
    }
    unsafe extern "C" fn reset(p: *const clap_plugin) {
        boundary((), || {
            let i = Self::instance(p);
            if let Some(_access) = i.gate.audio() {
                (&mut *i.runtime.get()).plugin.reset();
            }
        })
    }
    unsafe extern "C" fn main(_: *const clap_plugin) {
        boundary((), || {})
    }
    unsafe extern "C" fn extension(_: *const clap_plugin, id: *const c_char) -> *const c_void {
        boundary(ptr::null(), || {
            if id.is_null() {
                return ptr::null();
            }
            let id = CStr::from_ptr(id);
            if id == CLAP_EXT_GUI {
                &Self::GUI as *const _ as _
            } else if id == CLAP_EXT_PARAMS {
                &Self::PARAMS as *const _ as _
            } else if id == CLAP_EXT_AUDIO_PORTS_CONFIG
                && D::Plugin::descriptor().inputs.iter().any(|b| b.auxiliary)
            {
                &Self::CONFIGS as *const _ as _
            } else if (id == CLAP_EXT_AUDIO_PORTS_CONFIG_INFO
                || id == CLAP_EXT_AUDIO_PORTS_CONFIG_INFO_COMPAT)
                && D::Plugin::descriptor().inputs.iter().any(|b| b.auxiliary)
            {
                &Self::CONFIG_INFO as *const _ as _
            } else if id == CLAP_EXT_AUDIO_PORTS {
                &Self::PORTS as *const _ as _
            } else if id == CLAP_EXT_NOTE_PORTS {
                &Self::NOTES as *const _ as _
            } else if id == CLAP_EXT_STATE {
                &Self::STATE as *const _ as _
            } else if id == CLAP_EXT_LATENCY {
                &Self::LATENCY as *const _ as _
            } else if id == CLAP_EXT_TAIL {
                &Self::TAIL as *const _ as _
            } else {
                ptr::null()
            }
        })
    }
    unsafe extern "C" fn process(
        p: *const clap_plugin,
        data: *const clap_process,
    ) -> clap_process_status {
        boundary(CLAP_PROCESS_ERROR, || {
            if data.is_null() {
                return CLAP_PROCESS_ERROR;
            }
            let i = Self::instance(p);
            if !i.processing.load(Ordering::Acquire) {
                return CLAP_PROCESS_ERROR;
            }
            let Some(_access) = i.gate.audio() else {
                i.missed_audio.store(true, Ordering::Release);
                silence(&*data);
                return CLAP_PROCESS_CONTINUE;
            };
            let rt = &mut *i.runtime.get();
            if i.missed_audio.swap(false, Ordering::AcqRel) {
                rt.plugin.reset();
                rt.output_reset_pending = true;
            }
            #[cfg(feature = "devtools")]
            let debug_start = std::time::Instant::now();
            let d = &*data;
            let desc = D::Plugin::descriptor();
            let n = d.frames_count as usize;
            let configuration = i.configuration.load(Ordering::Acquire);
            if n > rt.max_frames
                || d.audio_inputs_count as usize > compat::input_count::<D::Plugin>(configuration)
                || d.audio_outputs_count as usize != desc.outputs.len()
            {
                return CLAP_PROCESS_ERROR;
            }
            if (d.audio_inputs_count > 0 && d.audio_inputs.is_null())
                || (!desc.outputs.is_empty() && d.audio_outputs.is_null())
            {
                return CLAP_PROCESS_ERROR;
            }
            let ins = if d.audio_inputs_count == 0 {
                &[][..]
            } else {
                std::slice::from_raw_parts(d.audio_inputs, d.audio_inputs_count as usize)
            };
            let outs = if desc.outputs.is_empty() {
                &[][..]
            } else {
                std::slice::from_raw_parts(d.audio_outputs, desc.outputs.len())
            };
            let is64 = ins
                .iter()
                .chain(outs.iter())
                .find(|b| !b.data32.is_null() || !b.data64.is_null())
                .is_some_and(|b| b.data32.is_null() && !b.data64.is_null());
            if is64 && !desc.supports_f64 {
                return CLAP_PROCESS_ERROR;
            }
            for (b, s) in outs.iter().zip(desc.outputs) {
                if b.channel_count != s.channels
                    || if is64 {
                        b.data64.is_null()
                    } else {
                        b.data32.is_null()
                    }
                {
                    return CLAP_PROCESS_ERROR;
                }
                for c in 0..b.channel_count as usize {
                    if if is64 {
                        (*b.data64.add(c)).is_null()
                    } else {
                        (*b.data32.add(c)).is_null()
                    } {
                        return CLAP_PROCESS_ERROR;
                    }
                }
            }
            let copied = if is64 {
                snapshot_inputs::<f64, D::Plugin>(
                    ins,
                    configuration,
                    &mut rt.block_input64[..desc.input_channels() * n],
                    n,
                )
            } else {
                snapshot_inputs::<f32, D::Plugin>(
                    ins,
                    configuration,
                    &mut rt.block_input32[..desc.input_channels() * n],
                    n,
                )
            };
            if !copied {
                return CLAP_PROCESS_ERROR;
            }
            apply_ui(rt, d.out_events);
            let mut output_lost = false;
            if rt.output_reset_pending {
                let mut reset = Output {
                    events: d.out_events,
                    frame: 0,
                    lost: &mut output_lost,
                };
                if reset.push(Event {
                    frame: 0,
                    kind: EventKind::AllNotesOff,
                }) {
                    rt.output_reset_pending = false;
                }
            }
            let mut cursor = 0;
            let count = event_count(d.in_events);
            let mut status = ProcessStatus::Continue;
            let tempo_ramp = !d.transport.is_null()
                && (*d.transport).flags & CLAP_TRANSPORT_HAS_TEMPO != 0
                && (*d.transport).tempo_inc != 0.;
            if n > 0 && count == 0 && !tempo_ramp {
                let t = transport(d.transport, rt.sample_rate, 0);
                let mut sink = Output {
                    events: d.out_events,
                    frame: 0,
                    lost: &mut output_lost,
                };
                status = if is64 {
                    rt.block64(n, &t, &mut sink)
                } else {
                    rt.block32(n, &t, &mut sink)
                };
                let mut channel = 0;
                for b in outs {
                    for c in 0..b.channel_count as usize {
                        if is64 {
                            ptr::copy_nonoverlapping(
                                rt.block_output64.as_ptr().add(channel * n),
                                *b.data64.add(c),
                                n,
                            )
                        } else {
                            ptr::copy_nonoverlapping(
                                rt.block_output32.as_ptr().add(channel * n),
                                *b.data32.add(c),
                                n,
                            )
                        }
                        channel += 1;
                    }
                }
            } else {
                let mut current_transport = d.transport;
                let mut transport_frame = 0;
                for frame in 0..n {
                    while cursor < count {
                        let e = event_at(d.in_events, cursor);
                        if e.is_null() {
                            cursor += 1;
                            continue;
                        }
                        if (*e).time > frame as u32 {
                            break;
                        }
                        if (*e).space_id == CLAP_CORE_EVENT_SPACE_ID
                            && (*e).type_ == CLAP_EVENT_TRANSPORT
                            && (*e).size as usize >= size_of::<clap_event_transport>()
                        {
                            current_transport = e.cast();
                            transport_frame = (*e).time as usize;
                        } else if let Some(e) = convert_event::<D::Plugin>(e) {
                            rt.push(e)
                        }
                        cursor += 1;
                    }
                    if is64 {
                        for c in 0..rt.input64.len() {
                            rt.input64[c] = rt.block_input64[c * n + frame];
                        }
                    } else {
                        for c in 0..rt.input32.len() {
                            rt.input32[c] = rt.block_input32[c * n + frame];
                        }
                    }
                    let transport = transport(
                        current_transport,
                        rt.sample_rate,
                        frame.saturating_sub(transport_frame),
                    );
                    let mut sink = Output {
                        events: d.out_events,
                        frame: frame as u32,
                        lost: &mut output_lost,
                    };
                    status = if is64 {
                        rt.tick64(&transport, &mut sink)
                    } else {
                        rt.tick32(&transport, &mut sink)
                    };
                    let mut channel = 0;
                    for b in outs {
                        for c in 0..b.channel_count as usize {
                            if is64 {
                                *(*b.data64.add(c)).add(frame) = rt.output64[channel]
                            } else {
                                *(*b.data32.add(c)).add(frame) = rt.output32[channel]
                            }
                            channel += 1;
                        }
                    }
                    if status == ProcessStatus::Error {
                        return CLAP_PROCESS_ERROR;
                    }
                }
            }
            for bus in 0..d.audio_outputs_count as usize {
                (*d.audio_outputs.add(bus)).constant_mask = 0;
            }
            rt.output_reset_pending |= output_lost;
            if output_lost {
                rt.debug_event(6, 1.);
            }
            #[cfg(feature = "devtools")]
            rt.debug_callback(n, debug_start);
            i.tail
                .store(rvst_adapter::tail(rt.plugin.tail()), Ordering::Release);
            if rt.plugin.latency() != i.latency.load(Ordering::Acquire)
                && !i.latency_restart_pending.swap(true, Ordering::AcqRel)
            {
                // request_restart is thread-safe. Keep the advertised latency fixed
                // until the host performs its normal deactivate/activate sequence.
                if let Some(restart) = (*i._host).request_restart {
                    restart(i._host);
                }
            }
            match status {
                ProcessStatus::Continue => CLAP_PROCESS_CONTINUE,
                ProcessStatus::Tail => CLAP_PROCESS_TAIL,
                ProcessStatus::Sleep => CLAP_PROCESS_SLEEP,
                ProcessStatus::Error => CLAP_PROCESS_ERROR,
            }
        })
    }
    unsafe extern "C" fn param_count(_: *const clap_plugin) -> u32 {
        boundary(0, || D::Plugin::descriptor().parameters.len() as u32)
    }
    unsafe extern "C" fn param_info(
        _: *const clap_plugin,
        index: u32,
        out: *mut clap_param_info,
    ) -> bool {
        boundary(false, || {
            if out.is_null() {
                return false;
            }
            let Some(p) = D::Plugin::descriptor().parameters.get(index as usize) else {
                return false;
            };
            *out = std::mem::zeroed();
            let o = &mut *out;
            o.id = p.id;
            o.flags = CLAP_PARAM_IS_AUTOMATABLE
                | if D::Plugin::adapter_compatibility().normalized_clap_parameters {
                    CLAP_PARAM_IS_MODULATABLE
                } else {
                    0
                }
                | if p.step.is_some() {
                    CLAP_PARAM_IS_STEPPED
                } else {
                    0
                }
                | if p.bypass { CLAP_PARAM_IS_BYPASS } else { 0 };
            copy_text(p.name, &mut o.name);
            o.min_value = if D::Plugin::adapter_compatibility().normalized_clap_parameters {
                0.
            } else {
                p.min
            };
            o.max_value = compat::maximum::<D::Plugin>(p);
            o.default_value = compat::to_host::<D::Plugin>(p, p.default);
            true
        })
    }
    unsafe extern "C" fn param_value(p: *const clap_plugin, id: u32, out: *mut f64) -> bool {
        boundary(false, || {
            if out.is_null() {
                return false;
            }
            let i = Self::instance(p);
            if let Some(v) = i.mirror.get(id) {
                let spec = D::Plugin::descriptor().parameter(id).unwrap();
                *out = compat::to_host::<D::Plugin>(spec, v);
                true
            } else {
                false
            }
        })
    }
    unsafe extern "C" fn param_text(
        _: *const clap_plugin,
        id: u32,
        v: f64,
        out: *mut c_char,
        cap: u32,
    ) -> bool {
        boundary(false, || {
            if out.is_null()
                || cap == 0
                || !D::Plugin::descriptor()
                    .parameters
                    .iter()
                    .any(|p| p.id == id)
            {
                return false;
            }
            copy_text(
                &format!(
                    "{:.3}",
                    compat::from_host::<D::Plugin>(
                        D::Plugin::descriptor().parameter(id).unwrap(),
                        v
                    )
                ),
                std::slice::from_raw_parts_mut(out, cap as usize),
            );
            true
        })
    }
    unsafe extern "C" fn text_param(
        _: *const clap_plugin,
        id: u32,
        text: *const c_char,
        out: *mut f64,
    ) -> bool {
        boundary(false, || {
            if text.is_null() || out.is_null() {
                return false;
            }
            let Some(p) = D::Plugin::descriptor()
                .parameters
                .iter()
                .find(|p| p.id == id)
            else {
                return false;
            };
            if let Ok(v) = CStr::from_ptr(text).to_string_lossy().parse::<f64>()
                && v.is_finite()
            {
                *out = compat::to_host::<D::Plugin>(p, p.clamp(v));
                return true;
            }
            false
        })
    }
    unsafe extern "C" fn flush(
        p: *const clap_plugin,
        input: *const clap_input_events,
        out: *const clap_output_events,
    ) {
        boundary((), || {
            let i = Self::instance(p);
            let Some(_access) = i.gate.audio() else {
                return;
            };
            let rt = &mut *i.runtime.get();
            apply_ui(rt, out);
            for j in 0..event_count(input) {
                if let Some(Event {
                    kind: EventKind::Parameter { id, value },
                    ..
                }) = convert_event::<D::Plugin>(event_at(input, j))
                {
                    rt.set_parameter_rt(id, value);
                }
            }
        })
    }
    unsafe extern "C" fn port_count(p: *const clap_plugin, input: bool) -> u32 {
        boundary(0, || {
            if input {
                compat::input_count::<D::Plugin>(Self::current_config(p)) as u32
            } else {
                D::Plugin::descriptor().outputs.len() as u32
            }
        })
    }
    unsafe extern "C" fn port_info(
        p: *const clap_plugin,
        index: u32,
        input: bool,
        out: *mut clap_audio_port_info,
    ) -> bool {
        boundary(false, || {
            Self::port_info_for_config(Self::current_config(p), index, input, out)
        })
    }
    unsafe fn port_info_for_config(
        config: u32,
        index: u32,
        input: bool,
        out: *mut clap_audio_port_info,
    ) -> bool {
        if out.is_null() {
            return false;
        }
        let d = D::Plugin::descriptor();
        let bus = if input {
            d.inputs
                .iter()
                .filter(|b| config == 0 || !b.auxiliary)
                .nth(index as usize)
        } else {
            d.outputs.get(index as usize)
        };
        let Some(b) = bus else {
            return false;
        };
        *out = std::mem::zeroed();
        let o = &mut *out;
        let inputs = compat::input_count::<D::Plugin>(config) as u32;
        o.id = index
            + if !input && D::Plugin::adapter_compatibility().clap_output_ids_follow_inputs {
                inputs
            } else {
                0
            };
        o.channel_count = b.channels;
        copy_text(b.name, &mut o.name);
        o.flags = CLAP_AUDIO_PORT_REQUIRES_COMMON_SAMPLE_SIZE
            | if b.auxiliary {
                0
            } else {
                CLAP_AUDIO_PORT_IS_MAIN
            }
            | if d.supports_f64 {
                CLAP_AUDIO_PORT_SUPPORTS_64BITS
            } else {
                0
            };
        o.port_type = port_type(b.channels);
        o.in_place_pair = u32::MAX;
        if !b.auxiliary
            && index == 0
            && d.inputs
                .first()
                .zip(d.outputs.first())
                .is_some_and(|(a, b)| a.channels == b.channels && !a.auxiliary && !b.auxiliary)
        {
            o.in_place_pair = if input {
                if D::Plugin::adapter_compatibility().clap_output_ids_follow_inputs {
                    inputs
                } else {
                    0
                }
            } else {
                0
            };
        }
        true
    }
    unsafe extern "C" fn note_count(_: *const clap_plugin, input: bool) -> u32 {
        boundary(0, || {
            let d = D::Plugin::descriptor();
            u32::from(if input { d.midi_input } else { d.midi_output })
        })
    }
    unsafe extern "C" fn note_info(
        p: *const clap_plugin,
        index: u32,
        input: bool,
        out: *mut clap_note_port_info,
    ) -> bool {
        boundary(false, || {
            if out.is_null() || index >= Self::note_count(p, input) {
                return false;
            }
            *out = std::mem::zeroed();
            (*out).supported_dialects = CLAP_NOTE_DIALECT_CLAP | CLAP_NOTE_DIALECT_MIDI;
            (*out).preferred_dialect = CLAP_NOTE_DIALECT_CLAP;
            copy_text("Notes", &mut (*out).name);
            true
        })
    }
    unsafe extern "C" fn latency(p: *const clap_plugin) -> u32 {
        boundary(0, || Self::instance(p).latency.load(Ordering::Acquire))
    }
    unsafe extern "C" fn tail(p: *const clap_plugin) -> u32 {
        boundary(0, || Self::instance(p).tail.load(Ordering::Acquire))
    }
    unsafe extern "C" fn save(p: *const clap_plugin, s: *const clap_ostream) -> bool {
        boundary(false, || {
            let i = Self::instance(p);
            if s.is_null() {
                return false;
            }
            let Some(_access) = i.gate.control() else {
                return false;
            };
            let Some(write) = (*s).write else {
                return false;
            };
            let rt = &mut *i.runtime.get();
            rt.retired_errors.collect_on_control_thread();
            let Ok(bytes) = rvst_adapter::save_project_state(&mut *rt.plugin, &rt.mirror) else {
                return false;
            };
            drop(_access);
            let bytes = compat::encode_state::<D::Plugin>(bytes);
            let mut pos = 0;
            while pos < bytes.len() {
                let n = write(s, bytes[pos..].as_ptr() as _, (bytes.len() - pos) as u64);
                if n <= 0 || n as usize > bytes.len() - pos {
                    return false;
                }
                pos += n as usize;
            }
            true
        })
    }
    unsafe extern "C" fn load(p: *const clap_plugin, s: *const clap_istream) -> bool {
        boundary(false, || {
            let i = Self::instance(p);
            if s.is_null() {
                return false;
            }
            let Some(read) = (*s).read else { return false };
            let mut bytes = Vec::new();
            let mut block = [0u8; 4096];
            loop {
                let n = read(s, block.as_mut_ptr() as _, block.len() as u64);
                if n < 0 || n as usize > block.len() {
                    return false;
                }
                if n == 0 {
                    break;
                }
                if bytes.len() + n as usize > 64 * 1024 * 1024 {
                    return false;
                }
                bytes.extend_from_slice(&block[..n as usize]);
            }
            let Some(bytes) = compat::decode_state::<D::Plugin>(&bytes) else {
                return false;
            };
            let Some(_access) = i.gate.control() else {
                return false;
            };
            let rt = &mut *i.runtime.get();
            rt.retired_errors.collect_on_control_thread();
            if rvst_adapter::load_project_state(&mut *rt.plugin, &rt.mirror, None, bytes).is_err() {
                return false;
            }
            rt.debug_lifecycle("Native state loaded");
            drop(_access);
            let params = i.host_params.load(Ordering::Acquire);
            if !params.is_null()
                && let Some(rescan) = (*params).rescan
            {
                rescan(i._host, CLAP_PARAM_RESCAN_VALUES | CLAP_PARAM_RESCAN_TEXT);
            }
            true
        })
    }
}
fn port_type(channels: u32) -> *const c_char {
    match channels {
        1 => CLAP_PORT_MONO.as_ptr(),
        2 => CLAP_PORT_STEREO.as_ptr(),
        _ => ptr::null(),
    }
}
trait NativeSample: Copy + Default {
    unsafe fn channels(bus: &clap_sys::audio_buffer::clap_audio_buffer) -> *mut *mut Self;
}
impl NativeSample for f32 {
    unsafe fn channels(b: &clap_sys::audio_buffer::clap_audio_buffer) -> *mut *mut Self {
        b.data32
    }
}
impl NativeSample for f64 {
    unsafe fn channels(b: &clap_sys::audio_buffer::clap_audio_buffer) -> *mut *mut Self {
        b.data64
    }
}
unsafe fn snapshot_inputs<T: NativeSample, P: Plugin>(
    provided: &[clap_sys::audio_buffer::clap_audio_buffer],
    config: u32,
    target: &mut [T],
    frames: usize,
) -> bool {
    target.fill(T::default());
    let mut source = 0;
    let mut base = 0;
    for spec in P::descriptor().inputs {
        let current = base;
        base += spec.channels as usize;
        if config != 0 && spec.auxiliary {
            continue;
        }
        let bus = provided.get(source);
        source += 1;
        let Some(bus) = bus else {
            if spec.auxiliary {
                continue;
            }
            return false;
        };
        if spec.auxiliary
            && (bus.channel_count == 0 || (bus.data32.is_null() && bus.data64.is_null()))
        {
            continue;
        }
        if bus.channel_count != spec.channels {
            return false;
        }
        let pointers = T::channels(bus);
        if pointers.is_null() {
            return false;
        }
        for c in 0..spec.channels as usize {
            let src = *pointers.add(c);
            if src.is_null() {
                return false;
            }
            let output = &mut target[(current + c) * frames..(current + c + 1) * frames];
            if frames > 0 && bus.constant_mask & (1u64 << c) != 0 {
                output.fill(*src);
            } else {
                ptr::copy_nonoverlapping(src, output.as_mut_ptr(), frames);
            }
        }
    }
    true
}
fn copy_text(s: &str, d: &mut [c_char]) {
    d.fill(0);
    let n = d.len().saturating_sub(1);
    for (a, b) in d.iter_mut().take(n).zip(s.bytes()) {
        *a = b as c_char;
    }
}
unsafe fn event_count(e: *const clap_input_events) -> u32 {
    if e.is_null() {
        0
    } else {
        (*e).size.map_or(0, |f| f(e))
    }
}
unsafe fn event_at(e: *const clap_input_events, i: u32) -> *const clap_event_header {
    if e.is_null() {
        ptr::null()
    } else {
        (*e).get.map_or(ptr::null(), |f| f(e, i))
    }
}
unsafe fn convert_event<P: Plugin>(h: *const clap_event_header) -> Option<Event> {
    if h.is_null() || (*h).space_id != CLAP_CORE_EVENT_SPACE_ID {
        return None;
    }
    let h = &*h;
    let kind = match h.type_ {
        CLAP_EVENT_PARAM_VALUE if h.size as usize >= size_of::<clap_event_param_value>() => {
            let e = &*(h as *const _ as *const clap_event_param_value);
            if e.note_id != -1 || e.channel != -1 || e.key != -1 || !e.value.is_finite() {
                return None;
            }
            EventKind::Parameter {
                id: e.param_id,
                value: compat::from_host::<P>(P::descriptor().parameter(e.param_id)?, e.value),
            }
        }
        CLAP_EVENT_PARAM_MOD if h.size as usize >= size_of::<clap_event_param_mod>() => {
            let e = &*(h as *const _ as *const clap_event_param_mod);
            if e.note_id != -1 || e.channel != -1 || e.key != -1 || !e.amount.is_finite() {
                return None;
            }
            EventKind::Modulation {
                id: e.param_id,
                amount: compat::modulation::<P>(P::descriptor().parameter(e.param_id)?, e.amount),
            }
        }
        CLAP_EVENT_NOTE_ON | CLAP_EVENT_NOTE_OFF | CLAP_EVENT_NOTE_CHOKE
            if h.size as usize >= size_of::<clap_event_note>() =>
        {
            let e = &*(h as *const _ as *const clap_event_note);
            if !(-1..16).contains(&e.channel)
                || !(-1..128).contains(&e.key)
                || !e.velocity.is_finite()
            {
                return None;
            }
            if h.type_ == CLAP_EVENT_NOTE_CHOKE
                || (h.type_ == CLAP_EVENT_NOTE_OFF && (e.channel < 0 || e.key < 0))
            {
                EventKind::NoteRelease {
                    channel: (e.channel >= 0).then_some(e.channel as u8),
                    key: (e.key >= 0).then_some(e.key as u8),
                    note_id: (e.note_id >= 0).then_some(e.note_id),
                    velocity: e.velocity,
                    choke: h.type_ == CLAP_EVENT_NOTE_CHOKE,
                }
            } else if h.type_ == CLAP_EVENT_NOTE_ON {
                if e.channel < 0 || e.key < 0 {
                    return None;
                }
                EventKind::NoteOn {
                    channel: e.channel as u8,
                    key: e.key as u8,
                    velocity: e.velocity,
                    note_id: e.note_id,
                }
            } else {
                EventKind::NoteOff {
                    channel: e.channel as u8,
                    key: e.key as u8,
                    velocity: e.velocity,
                    note_id: e.note_id,
                }
            }
        }
        CLAP_EVENT_MIDI if h.size as usize >= size_of::<clap_event_midi>() => {
            rvst_adapter::midi((*(h as *const _ as *const clap_event_midi)).data)?
        }
        _ => return None,
    };
    Some(Event {
        frame: h.time,
        kind,
    })
}
unsafe fn transport(t: *const clap_event_transport, sr: f64, frame: usize) -> Transport {
    let mut out = Transport::default();
    if t.is_null() {
        return out;
    }
    let t = &*t;
    out.playing = t.flags & CLAP_TRANSPORT_IS_PLAYING != 0;
    out.recording = t.flags & CLAP_TRANSPORT_IS_RECORDING != 0;
    if t.flags & CLAP_TRANSPORT_HAS_TEMPO != 0 {
        out.tempo = Some(t.tempo + t.tempo_inc * frame as f64);
    }
    let elapsed = if out.playing { frame as f64 } else { 0. };
    if t.flags & CLAP_TRANSPORT_HAS_BEATS_TIMELINE != 0 {
        let beats = if t.flags & CLAP_TRANSPORT_HAS_TEMPO != 0 {
            // Sum the per-sample tempo ramp from the most recent transport event.
            (elapsed * t.tempo + elapsed * (elapsed - 1.) * 0.5 * t.tempo_inc) / (sr * 60.)
        } else {
            0.
        };
        out.beat_position = Some(t.song_pos_beats as f64 / (1u64 << 31) as f64 + beats);
    }
    if t.flags & CLAP_TRANSPORT_HAS_SECONDS_TIMELINE != 0 {
        out.sample_position = Some(
            ((t.song_pos_seconds as f64 / (1u64 << 31) as f64 * sr) as i64)
                .saturating_add(elapsed as i64),
        );
    }
    if t.flags & CLAP_TRANSPORT_HAS_TIME_SIGNATURE != 0 {
        out.time_signature = Some((t.tsig_num, t.tsig_denom));
    }
    out
}
struct Output<'a> {
    events: *const clap_output_events,
    frame: u32,
    lost: &'a mut bool,
}
impl EventSink for Output<'_> {
    fn push(&mut self, e: Event) -> bool {
        let result = unsafe { self.send(e) };
        if !result
            && matches!(
                e.kind,
                EventKind::NoteOff { .. } | EventKind::NoteRelease { .. } | EventKind::AllNotesOff
            )
        {
            *self.lost = true;
        }
        result
    }
}
impl Output<'_> {
    unsafe fn send(&mut self, e: Event) -> bool {
        if self.events.is_null() {
            return false;
        }
        let Some(push) = (*self.events).try_push else {
            return false;
        };
        let header = clap_event_header {
            size: size_of::<clap_event_note>() as u32,
            time: self.frame + e.frame,
            space_id: CLAP_CORE_EVENT_SPACE_ID,
            type_: CLAP_EVENT_NOTE_ON,
            flags: 0,
        };
        let mut n: clap_event_note = std::mem::zeroed();
        n.header = header;
        match e.kind {
            EventKind::NoteOn {
                channel,
                key,
                velocity,
                note_id,
            } => {
                n.channel = channel as i16;
                n.key = key as i16;
                n.velocity = velocity;
                n.note_id = note_id;
            }
            EventKind::NoteOff {
                channel,
                key,
                velocity,
                note_id,
            } => {
                n.header.type_ = CLAP_EVENT_NOTE_OFF;
                n.channel = channel as i16;
                n.key = key as i16;
                n.velocity = velocity;
                n.note_id = note_id;
            }
            EventKind::NoteRelease {
                channel,
                key,
                note_id,
                velocity,
                choke,
            } => {
                n.header.type_ = if choke {
                    CLAP_EVENT_NOTE_CHOKE
                } else {
                    CLAP_EVENT_NOTE_OFF
                };
                n.channel = channel.map_or(-1, i16::from);
                n.key = key.map_or(-1, i16::from);
                n.note_id = note_id.unwrap_or(-1);
                n.velocity = velocity;
            }
            EventKind::AllNotesOff => {
                n.header.type_ = CLAP_EVENT_NOTE_CHOKE;
                n.channel = -1;
                n.key = -1;
                n.note_id = -1;
            }
            EventKind::ControlChange {
                channel,
                controller,
                value,
            } => {
                let m = clap_event_midi {
                    header: clap_event_header {
                        size: size_of::<clap_event_midi>() as u32,
                        type_: CLAP_EVENT_MIDI,
                        ..header
                    },
                    port_index: 0,
                    data: [
                        0xb0 | channel,
                        controller,
                        (value.clamp(0., 1.) * 127.).round() as u8,
                    ],
                };
                return push(self.events, &m.header);
            }
            EventKind::ChannelPressure { channel, pressure } => {
                let m = clap_event_midi {
                    header: clap_event_header {
                        size: size_of::<clap_event_midi>() as u32,
                        type_: CLAP_EVENT_MIDI,
                        ..header
                    },
                    port_index: 0,
                    data: [
                        0xd0 | channel,
                        (pressure.clamp(0., 1.) * 127.).round() as u8,
                        0,
                    ],
                };
                return push(self.events, &m.header);
            }
            EventKind::PitchBend { channel, value } => {
                let v = ((value.clamp(-1., 1.) + 1.) * 8192.).round().min(16383.) as u16;
                let m = clap_event_midi {
                    header: clap_event_header {
                        size: size_of::<clap_event_midi>() as u32,
                        type_: CLAP_EVENT_MIDI,
                        ..header
                    },
                    port_index: 0,
                    data: [0xe0 | channel, (v & 127) as u8, (v >> 7) as u8],
                };
                return push(self.events, &m.header);
            }
            EventKind::PolyPressure {
                channel,
                key,
                pressure,
                note_id,
            } => {
                let m = clap_event_note_expression {
                    header: clap_event_header {
                        size: size_of::<clap_event_note_expression>() as u32,
                        type_: CLAP_EVENT_NOTE_EXPRESSION,
                        ..header
                    },
                    expression_id: CLAP_NOTE_EXPRESSION_PRESSURE,
                    note_id,
                    port_index: 0,
                    channel: channel as i16,
                    key: key as i16,
                    value: pressure,
                };
                return push(self.events, &m.header);
            }
            _ => return false,
        }
        push(self.events, &n.header)
    }
}

#[macro_export]
macro_rules! export_plugin {
    ($plugin:ty,$editor:path) => {
        struct RvstClapDefinition;
        impl $crate::Definition for RvstClapDefinition {
            type Plugin = $plugin;
            fn descriptor() -> &'static $crate::Descriptor {
                static D: std::sync::OnceLock<$crate::Descriptor> = std::sync::OnceLock::new();
                D.get_or_init(|| {
                    $crate::Descriptor::new(<$plugin as $crate::rvst_core::Plugin>::descriptor())
                })
            }
            fn editor() -> $crate::EditorFn {
                $editor
            }
        }
        #[unsafe(no_mangle)]
        pub static clap_entry: $crate::clap_sys::entry::clap_plugin_entry =
            $crate::Export::<RvstClapDefinition>::ENTRY;
    };
}

#[derive(Default)]
struct Gui {
    created: bool,
    options: rvst_platform::EditorOptions,
    editor: Option<rvst_platform::NativeEditor>,
}
fn gui_api() -> &'static CStr {
    #[cfg(target_os = "macos")]
    return CLAP_WINDOW_API_COCOA;
    #[cfg(target_os = "windows")]
    return CLAP_WINDOW_API_WIN32;
    #[cfg(target_os = "linux")]
    return CLAP_WINDOW_API_X11;
}
struct ClapEditor {
    mirror: std::sync::Arc<rvst_adapter::ParameterMirror>,
    host: usize,
    params: usize,
}
impl ClapEditor {
    fn flush(&self) {
        unsafe {
            let h = self.host as *const clap_host;
            let p = self.params as *const clap_host_params;
            if !p.is_null()
                && let Some(f) = (*p).request_flush
            {
                f(h);
            }
        }
    }
}
impl EditorHost for ClapEditor {
    fn edit_history(&self) -> Option<EditHistory> {
        Some(self.mirror.edit_history.clone())
    }
    fn editor_context(&self) -> Option<EditorContext> {
        self.mirror.editor_context.clone()
    }
    fn descriptor(&self) -> &'static PluginDescriptor {
        self.mirror.descriptor
    }
    fn parameter(&self, id: u32) -> Option<f64> {
        self.mirror.get(id)
    }
    fn begin_edit(&mut self, id: u32) {
        self.mirror.gesture(id, true);
        self.flush()
    }
    fn set_parameter(&mut self, id: u32, v: f64) {
        self.mirror.edit(id, v);
        self.flush()
    }
    fn end_edit(&mut self, id: u32) {
        self.mirror.gesture(id, false);
        self.flush()
    }
    fn save_state(&mut self) -> Result<Vec<u8>, Error> {
        rvst_adapter::save_parameter_preset(&self.mirror)
    }
    fn load_state(&mut self, data: &[u8]) -> Result<(), Error> {
        let values = rvst_adapter::parse_parameter_preset(self.mirror.descriptor, data)?;
        for (id, value) in values {
            self.begin_edit(id);
            self.set_parameter(id, value);
            self.end_edit(id);
        }
        Ok(())
    }
}
unsafe fn apply_ui<P: Plugin>(rt: &mut Runtime<P>, out: *const clap_output_events) {
    for (index, p) in P::descriptor().parameters.iter().enumerate() {
        let gesture = rt.mirror.take_gesture(index);
        let push = if out.is_null() { None } else { (*out).try_push };
        let header = clap_event_header {
            size: size_of::<clap_event_param_gesture>() as u32,
            time: 0,
            space_id: CLAP_CORE_EVENT_SPACE_ID,
            type_: CLAP_EVENT_PARAM_GESTURE_BEGIN,
            flags: CLAP_EVENT_IS_LIVE,
        };
        if gesture & 1 != 0
            && let Some(f) = push
        {
            let e = clap_event_param_gesture {
                header,
                param_id: p.id,
            };
            if !f(out, &e.header) {
                rt.mirror.gesture(p.id, true);
            }
        }
        if let Some((id, value)) = rt.mirror.take_edit(index) {
            let accepted = rt.set_parameter_rt(id, value);
            if let Some(f) = push.filter(|_| accepted) {
                let e = clap_event_param_value {
                    header: clap_event_header {
                        size: size_of::<clap_event_param_value>() as u32,
                        type_: CLAP_EVENT_PARAM_VALUE,
                        ..header
                    },
                    param_id: id,
                    cookie: ptr::null_mut(),
                    note_id: -1,
                    port_index: -1,
                    channel: -1,
                    key: -1,
                    value: compat::to_host::<P>(p, value),
                };
                if !f(out, &e.header) {
                    rt.mirror.retry_edit(index)
                }
            }
        }
        if gesture & 2 != 0
            && let Some(f) = push
        {
            let e = clap_event_param_gesture {
                header: clap_event_header {
                    type_: CLAP_EVENT_PARAM_GESTURE_END,
                    ..header
                },
                param_id: p.id,
            };
            if !f(out, &e.header) {
                rt.mirror.gesture(p.id, false);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[global_allocator]
    static ALLOC: rvst_test::GuardedAllocator<std::alloc::System> =
        rvst_test::GuardedAllocator(std::alloc::System);
    pub(super) mod fixture {
        include!("../../rvst-adapter/test-support/plugin.rs");
    }
    struct Def;
    impl Definition for Def {
        type Plugin = fixture::Gain;
        fn descriptor() -> &'static Descriptor {
            static D: std::sync::OnceLock<Descriptor> = std::sync::OnceLock::new();
            D.get_or_init(|| Descriptor::new(fixture::Gain::descriptor()))
        }
        fn editor() -> EditorFn {
            rvst_ui::default_editor
        }
    }
    unsafe extern "C" fn size(_: *const clap_input_events) -> u32 {
        1
    }
    unsafe extern "C" fn get(list: *const clap_input_events, _: u32) -> *const clap_event_header {
        (*list).ctx as _
    }
    unsafe fn make() -> (*const clap_plugin, Box<clap_host>) {
        let h = Box::new(clap_host {
            clap_version: CLAP_VERSION,
            host_data: ptr::null_mut(),
            name: c"test".as_ptr(),
            vendor: c"RVST".as_ptr(),
            url: c"".as_ptr(),
            version: c"1".as_ptr(),
            get_extension: None,
            request_restart: None,
            request_process: None,
            request_callback: None,
        });
        let f = &Export::<Def>::FACTORY;
        let p = f.create_plugin.unwrap()(f, &*h, c"org.rvst.test".as_ptr());
        assert!(!p.is_null());
        assert!((*p).init.unwrap()(p));
        (p, h)
    }
    #[test]
    fn native_factory_inplace_audio_plain_parameters_offsets_and_lifecycle() {
        unsafe {
            let (p, _host) = make();
            let ext = (*p).get_extension.unwrap()(p, CLAP_EXT_PARAMS.as_ptr())
                as *const clap_plugin_params;
            let mut info: clap_param_info = std::mem::zeroed();
            assert!((*ext).get_info.unwrap()(p, 0, &mut info));
            assert_eq!(
                (info.min_value, info.max_value, info.default_value),
                (0., 2., 1.)
            );
            assert!((*p).activate.unwrap()(p, 48000., 1, 32));
            assert!((*p).start_processing.unwrap()(p));
            let mut l = [1.; 4];
            let mut r = [2.; 4];
            let mut ptrs = [l.as_mut_ptr(), r.as_mut_ptr()];
            let mut audio = clap_sys::audio_buffer::clap_audio_buffer {
                data32: ptrs.as_mut_ptr(),
                data64: ptr::null_mut(),
                channel_count: 2,
                latency: 0,
                constant_mask: 0,
            };
            let event = clap_event_param_value {
                header: clap_event_header {
                    size: size_of::<clap_event_param_value>() as u32,
                    time: 2,
                    space_id: 0,
                    type_: CLAP_EVENT_PARAM_VALUE,
                    flags: 0,
                },
                param_id: 42,
                cookie: ptr::null_mut(),
                note_id: -1,
                port_index: -1,
                channel: -1,
                key: -1,
                value: 0.25,
            };
            let events = clap_input_events {
                ctx: &event as *const _ as _,
                size: Some(size),
                get: Some(get),
            };
            let process = clap_process {
                steady_time: 0,
                frames_count: 4,
                transport: ptr::null(),
                audio_inputs: &audio,
                audio_outputs: &mut audio,
                audio_inputs_count: 1,
                audio_outputs_count: 1,
                in_events: &events,
                out_events: ptr::null(),
            };
            let allocation_guard = rvst_test::AllocationGuard::start();
            let status = (*p).process.unwrap()(p, &process);
            let counts = allocation_guard.finish();
            assert_eq!(counts, rvst_test::AllocationCounts::default());
            assert_eq!(status, CLAP_PROCESS_CONTINUE);
            assert_eq!(l, [1., 1., 0.25, 0.25]);
            assert_eq!(r, [2., 2., 0.5, 0.5]);
            let mut value = 0.;
            assert!((*ext).get_value.unwrap()(p, 42, &mut value));
            assert_eq!(value, 0.25);
            (*p).stop_processing.unwrap()(p);
            (*p).deactivate.unwrap()(p);
            (*p).destroy.unwrap()(p);
        }
    }
    unsafe extern "C" fn write(s: *const clap_ostream, p: *const c_void, n: u64) -> i64 {
        let b = &mut *((*s).ctx as *mut Vec<u8>);
        let n = n.min(3) as usize;
        b.extend_from_slice(std::slice::from_raw_parts(p as *const u8, n));
        n as i64
    }
    pub(super) unsafe extern "C" fn read(s: *const clap_istream, p: *mut c_void, n: u64) -> i64 {
        let state = &mut *((*s).ctx as *mut (Vec<u8>, usize));
        let n = (n as usize).min(3).min(state.0.len() - state.1);
        ptr::copy_nonoverlapping(state.0.as_ptr().add(state.1), p as *mut u8, n);
        state.1 += n;
        n as i64
    }
    #[test]
    fn native_restore_invalidates_shared_history_only_on_success() {
        unsafe {
            let (p, _host) = make();
            let i = Export::<Def>::instance(p);
            let mut editor = rvst_core::HistoryHost::new(ClapEditor {
                mirror: i.mirror.clone(),
                host: 0,
                params: 0,
            });
            editor.begin_edit(42);
            editor.set_parameter(42, 0.25);
            editor.end_edit(42);
            assert!(editor.undo_label().is_some());
            let mut invalid = (vec![0u8], 0usize);
            let input = clap_istream {
                ctx: &mut invalid as *mut _ as _,
                read: Some(read),
            };
            assert!(!Export::<Def>::load(p, &input));
            assert!(editor.undo_label().is_some());
            assert_eq!(i.mirror.get(42), Some(0.25));

            let mut saved = (0.75f64.to_le_bytes().to_vec(), 0usize);
            let input = clap_istream {
                ctx: &mut saved as *mut _ as _,
                read: Some(read),
            };
            assert!(Export::<Def>::load(p, &input));
            assert!(editor.undo_label().is_none());
            assert_eq!(i.mirror.get(42), Some(0.75));
            let rt = &mut *i.runtime.get();
            apply_ui(rt, ptr::null());
            assert_eq!(rt.plugin.parameter(42), Some(0.75));
            drop(editor);
            let reopened = rvst_core::HistoryHost::new(ClapEditor {
                mirror: i.mirror.clone(),
                host: 0,
                params: 0,
            });
            assert!(reopened.undo_label().is_none());
            drop(reopened);
            (*p).destroy.unwrap()(p);
        }
    }
    #[test]
    fn project_state_roundtrip_preserves_undo_redo_and_rejects_corrupt_history() {
        unsafe {
            let (p, _host) = make();
            let i = Export::<Def>::instance(p);
            let mut editor = rvst_core::HistoryHost::new(ClapEditor {
                mirror: i.mirror.clone(),
                host: 0,
                params: 0,
            });
            for value in [0.25, 0.5] {
                editor.begin_edit(42);
                editor.set_parameter(42, value);
                editor.end_edit(42);
            }
            assert!(editor.undo().unwrap());
            let mut saved = Vec::<u8>::new();
            let output = clap_ostream {
                ctx: &mut saved as *mut _ as _,
                write: Some(write),
            };
            assert!(Export::<Def>::save(p, &output));
            let decoded =
                rvst_adapter::decode_native_state(fixture::Gain::descriptor(), &saved).unwrap();
            assert_eq!(decoded.plugin_state, 0.25f64.to_le_bytes());
            assert!(decoded.history.is_some());
            let invalid_plugin = rvst_adapter::encode_native_state(
                fixture::Gain::descriptor(),
                b"invalid plugin state",
                decoded.history.unwrap(),
            )
            .unwrap();
            let invalid_history = rvst_adapter::encode_native_state(
                fixture::Gain::descriptor(),
                &1.75f64.to_le_bytes(),
                b"corrupt history",
            )
            .unwrap();
            editor.set_parameter(42, 1.5);
            let mut invalid = (invalid_history, 0usize);
            let input = clap_istream {
                ctx: &mut invalid as *mut _ as _,
                read: Some(read),
            };
            assert!(!Export::<Def>::load(p, &input));
            assert_eq!(i.mirror.get(42), Some(1.5));
            assert_eq!((&*i.runtime.get()).plugin.parameter(42), Some(0.25));
            assert!(editor.undo_label().is_some());
            let mut invalid = (invalid_plugin, 0usize);
            let input = clap_istream {
                ctx: &mut invalid as *mut _ as _,
                read: Some(read),
            };
            assert!(!Export::<Def>::load(p, &input));
            assert_eq!(i.mirror.get(42), Some(1.5));
            assert!(editor.redo_label().is_none());

            let mut state = (saved, 0usize);
            let input = clap_istream {
                ctx: &mut state as *mut _ as _,
                read: Some(read),
            };
            assert!(Export::<Def>::load(p, &input));
            assert_eq!(i.mirror.get(42), Some(0.25));
            assert!(editor.undo_label().is_some());
            assert!(editor.redo_label().is_some());
            assert!(editor.redo().unwrap());
            assert_eq!(i.mirror.get(42), Some(0.5));
            assert!(editor.undo().unwrap());
            assert!(editor.undo().unwrap());
            assert_eq!(i.mirror.get(42), Some(1.));
            assert!(!editor.undo().unwrap());
            drop(editor);
            (*p).destroy.unwrap()(p);
        }
    }
    #[test]
    fn autosave_midgesture_restores_partial_drag_as_one_undo_in_a_new_instance() {
        unsafe {
            let (source, _source_host) = make();
            let source_instance = Export::<Def>::instance(source);
            let mut editor = rvst_core::HistoryHost::new(ClapEditor {
                mirror: source_instance.mirror.clone(),
                host: 0,
                params: 0,
            });
            editor.begin_edit(42);
            editor.set_parameter(42, 0.25);
            editor.set_parameter(42, 0.4);
            let mut saved = Vec::<u8>::new();
            let output = clap_ostream {
                ctx: &mut saved as *mut _ as _,
                write: Some(write),
            };
            assert!(Export::<Def>::save(source, &output));
            // Saving must leave the pending value available for host notification.
            assert_eq!(source_instance.mirror.take_edit(0), Some((42, 0.4)));
            editor.end_edit(42);
            drop(editor);
            (*source).destroy.unwrap()(source);

            let (restored, _restored_host) = make();
            let restored_instance = Export::<Def>::instance(restored);
            let mut state = (saved, 0usize);
            let input = clap_istream {
                ctx: &mut state as *mut _ as _,
                read: Some(read),
            };
            assert!(Export::<Def>::load(restored, &input));
            let mut reopened = rvst_core::HistoryHost::new(ClapEditor {
                mirror: restored_instance.mirror.clone(),
                host: 0,
                params: 0,
            });
            assert_eq!(reopened.parameter(42), Some(0.4));
            assert!(reopened.undo().unwrap());
            assert_eq!(reopened.parameter(42), Some(1.));
            assert!(!reopened.undo().unwrap());
            assert!(reopened.redo().unwrap());
            assert_eq!(reopened.parameter(42), Some(0.4));
            drop(reopened);
            (*restored).destroy.unwrap()(restored);
        }
    }
    #[test]
    fn active_state_roundtrip_short_stream_reads_and_nonblocking_suspension() {
        unsafe {
            let (p, _h) = make();
            assert!((*p).activate.unwrap()(p, 48000., 1, 8));
            assert!((*p).start_processing.unwrap()(p));
            let i = Export::<Def>::instance(p);
            let mut saved: Vec<u8> = Vec::new();
            let out = clap_ostream {
                ctx: &mut saved as *mut _ as _,
                write: Some(write),
            };
            assert!(Export::<Def>::save(p, &out));
            let decoded =
                rvst_adapter::decode_native_state(fixture::Gain::descriptor(), &saved).unwrap();
            assert_eq!(decoded.plugin_state, 1f64.to_le_bytes());
            assert!(decoded.history.is_some());
            let mut state = (0.5f64.to_le_bytes().to_vec(), 0usize);
            let input = clap_istream {
                ctx: &mut state as *mut _ as _,
                read: Some(read),
            };
            assert!(Export::<Def>::load(p, &input));
            assert_eq!(i.mirror.get(42), Some(0.5));
            let guard = i.gate.control().unwrap();
            let mut samples = [1f32; 4];
            let mut ptrs = [samples.as_mut_ptr(), samples.as_mut_ptr()];
            let mut audio = clap_sys::audio_buffer::clap_audio_buffer {
                data32: ptrs.as_mut_ptr(),
                data64: ptr::null_mut(),
                channel_count: 2,
                latency: 0,
                constant_mask: 0,
            };
            let mut d: clap_process = std::mem::zeroed();
            d.frames_count = 4;
            d.audio_outputs = &mut audio;
            d.audio_outputs_count = 1;
            let allocation_guard = rvst_test::AllocationGuard::start();
            let status = (*p).process.unwrap()(p, &d);
            let counts = allocation_guard.finish();
            assert_eq!(status, CLAP_PROCESS_CONTINUE);
            assert_eq!(samples, [0.; 4]);
            assert_eq!(counts, rvst_test::AllocationCounts::default());
            drop(guard);
            (*p).stop_processing.unwrap()(p);
            (*p).deactivate.unwrap()(p);
            (*p).destroy.unwrap()(p);
        }
    }
    #[test]
    fn rejects_unsupported_f64_and_oversized_blocks() {
        unsafe {
            let (p, _host) = make();
            assert!((*p).activate.unwrap()(p, 48000., 1, 8));
            assert!((*p).start_processing.unwrap()(p));
            let mut process: clap_process = std::mem::zeroed();
            process.frames_count = 9;
            assert_eq!((*p).process.unwrap()(p, &process), CLAP_PROCESS_ERROR);
            (*p).stop_processing.unwrap()(p);
            (*p).deactivate.unwrap()(p);
            (*p).destroy.unwrap()(p);
        }
    }
}

unsafe fn silence(data: &clap_process) {
    if data.audio_outputs.is_null() {
        return;
    }
    for b in 0..data.audio_outputs_count as usize {
        let b = &*data.audio_outputs.add(b);
        for c in 0..b.channel_count as usize {
            if !b.data32.is_null() {
                let p = *b.data32.add(c);
                if !p.is_null() {
                    ptr::write_bytes(p, 0, data.frames_count as usize);
                }
            } else if !b.data64.is_null() {
                let p = *b.data64.add(c);
                if !p.is_null() {
                    ptr::write_bytes(p, 0, data.frames_count as usize);
                }
            }
        }
    }
}

#[cfg(test)]
mod regression_tests {
    use super::*;
    #[test]
    fn targeted_clap_releases_never_become_global() {
        for typ in [CLAP_EVENT_NOTE_OFF, CLAP_EVENT_NOTE_CHOKE] {
            let e = clap_event_note {
                header: clap_event_header {
                    size: size_of::<clap_event_note>() as u32,
                    time: 0,
                    space_id: 0,
                    type_: typ,
                    flags: 0,
                },
                note_id: 123,
                port_index: 0,
                channel: -1,
                key: -1,
                velocity: 0.5,
            };
            let got = unsafe { convert_event::<tests::fixture::Gain>(&e.header) }.unwrap();
            assert_ne!(got.kind, EventKind::AllNotesOff);
        }
    }
}

#[cfg(test)]
mod compatibility_tests {
    use super::*;
    use clap_sys::ext::audio_ports_config::*;
    mod fixture {
        include!("../../rvst-adapter/test-support/plugin.rs");
    }
    struct Legacy(fixture::Gain);
    impl Plugin for Legacy {
        fn adapter_compatibility() -> AdapterCompatibility {
            AdapterCompatibility {
                normalized_clap_parameters: true,
                clap_state_length_prefix: true,
                clap_output_ids_follow_inputs: true,
                ..Default::default()
            }
        }
        fn descriptor() -> &'static PluginDescriptor {
            static D: std::sync::OnceLock<PluginDescriptor> = std::sync::OnceLock::new();
            D.get_or_init(|| {
                let mut d = fixture::Gain::descriptor().clone();
                d.inputs = &[
                    AudioBus {
                        name: "Main",
                        channels: 2,
                        auxiliary: false,
                    },
                    AudioBus {
                        name: "Sidechain",
                        channels: 2,
                        auxiliary: true,
                    },
                ];
                d
            })
        }
        fn new() -> Self {
            Self(fixture::Gain::new())
        }
        fn prepare(&mut self, c: ProcessConfig) -> Result<(), Error> {
            self.0.prepare(c)
        }
        fn process(
            &mut self,
            b: &mut AudioBlock<'_, f32>,
            e: &[Event],
            t: &Transport,
            s: &mut dyn EventSink,
        ) -> ProcessStatus {
            self.0.process(b, e, t, s)
        }
        fn reset(&mut self) {}
        fn parameter(&self, id: u32) -> Option<f64> {
            self.0.parameter(id)
        }
        fn set_parameter(&mut self, id: u32, v: f64) -> Result<(), Error> {
            self.0.set_parameter(id, v)
        }
        fn save_state(&self) -> Result<Vec<u8>, Error> {
            self.0.save_state()
        }
        fn load_state(&mut self, b: &[u8]) -> Result<(), Error> {
            self.0.load_state(b)
        }
    }
    struct Def;
    impl Definition for Def {
        type Plugin = Legacy;
        fn descriptor() -> &'static Descriptor {
            static D: std::sync::OnceLock<Descriptor> = std::sync::OnceLock::new();
            D.get_or_init(|| Descriptor::new(Legacy::descriptor()))
        }
        fn editor() -> EditorFn {
            rvst_ui::default_editor
        }
    }
    unsafe extern "C" fn size(_: *const clap_input_events) -> u32 {
        1
    }
    unsafe extern "C" fn get(e: *const clap_input_events, _: u32) -> *const clap_event_header {
        unsafe { (*e).ctx as _ }
    }
    unsafe extern "C" fn write(s: *const clap_ostream, p: *const c_void, n: u64) -> i64 {
        unsafe {
            let bytes = &mut *((*s).ctx as *mut Vec<u8>);
            bytes.extend_from_slice(std::slice::from_raw_parts(p as _, n as usize));
            n as i64
        }
    }
    #[test]
    fn legacy_normalized_parameters_state_prefix_and_sidechain_config() {
        unsafe {
            let h = clap_host {
                clap_version: CLAP_VERSION,
                host_data: ptr::null_mut(),
                name: c"test".as_ptr(),
                vendor: c"test".as_ptr(),
                url: c"".as_ptr(),
                version: c"1".as_ptr(),
                get_extension: None,
                request_restart: None,
                request_process: None,
                request_callback: None,
            };
            let p = Export::<Def>::create(&Export::<Def>::FACTORY, &h, c"org.rvst.test".as_ptr());
            assert!(!p.is_null());
            assert!(Export::<Def>::init(p));
            let mut info: clap_param_info = std::mem::zeroed();
            assert!(Export::<Def>::param_info(p, 0, &mut info));
            assert_eq!(
                (info.min_value, info.max_value, info.default_value),
                (0., 1., 0.5)
            );
            let e = clap_event_param_value {
                header: clap_event_header {
                    size: size_of::<clap_event_param_value>() as u32,
                    time: 0,
                    space_id: 0,
                    type_: CLAP_EVENT_PARAM_VALUE,
                    flags: 0,
                },
                param_id: 42,
                cookie: ptr::null_mut(),
                note_id: -1,
                port_index: -1,
                channel: -1,
                key: -1,
                value: 0.25,
            };
            let input = clap_input_events {
                ctx: &e as *const _ as _,
                size: Some(size),
                get: Some(get),
            };
            Export::<Def>::flush(p, &input, ptr::null());
            assert_eq!(
                (&*Export::<Def>::instance(p).runtime.get())
                    .plugin
                    .parameter(42),
                Some(0.5)
            );
            let mut value = 0.;
            assert!(Export::<Def>::param_value(p, 42, &mut value));
            assert_eq!(value, 0.25);
            let mut bytes = Vec::<u8>::new();
            let stream = clap_ostream {
                ctx: &mut bytes as *mut _ as _,
                write: Some(write),
            };
            assert!(Export::<Def>::save(p, &stream));
            assert_eq!(&bytes[..8], &((bytes.len() - 8) as u64).to_le_bytes());
            let decoded =
                rvst_adapter::decode_native_state(Legacy::descriptor(), &bytes[8..]).unwrap();
            assert_eq!(decoded.plugin_state, 0.5f64.to_le_bytes());
            assert!(decoded.history.is_some());
            // Existing prefixed raw projects and new prefixed envelopes both load.
            let legacy = compat::encode_state::<Legacy>(0.75f64.to_le_bytes().to_vec());
            for (data, expected) in [(legacy, 0.75), (bytes, 0.5)] {
                let mut state = (data, 0usize);
                let input = clap_istream {
                    ctx: &mut state as *mut _ as _,
                    read: Some(super::tests::read),
                };
                assert!(Export::<Def>::load(p, &input));
                assert_eq!(
                    (&*Export::<Def>::instance(p).runtime.get())
                        .plugin
                        .parameter(42),
                    Some(expected)
                );
            }
            let configs = Export::<Def>::extension(p, CLAP_EXT_AUDIO_PORTS_CONFIG.as_ptr())
                as *const clap_plugin_audio_ports_config;
            assert!(!configs.is_null());
            assert_eq!((*configs).count.unwrap()(p), 2);
            let mut port: clap_audio_port_info = std::mem::zeroed();
            assert!(Export::<Def>::port_info(p, 0, false, &mut port));
            assert_eq!(port.id, 2);
            assert!((*configs).select.unwrap()(p, 1));
            assert_eq!(Export::<Def>::port_count(p, true), 1);
            assert!(Export::<Def>::port_info(p, 0, false, &mut port));
            assert_eq!(port.id, 1);
            for config in [1, 0] {
                assert!((*configs).select.unwrap()(p, config));
                assert!(Export::<Def>::activate(p, 48000., 1, 16));
                assert!(!(*configs).select.unwrap()(p, 1 - config));
                assert!(Export::<Def>::start(p));
                let mut l = [1f32; 4];
                let mut r = [2f32; 4];
                let mut ptrs = [l.as_mut_ptr(), r.as_mut_ptr()];
                let mut bus = clap_sys::audio_buffer::clap_audio_buffer {
                    data32: ptrs.as_mut_ptr(),
                    data64: ptr::null_mut(),
                    channel_count: 2,
                    latency: 0,
                    constant_mask: 0,
                };
                let d = clap_process {
                    steady_time: 0,
                    frames_count: 4,
                    transport: ptr::null(),
                    audio_inputs: &bus,
                    audio_outputs: &mut bus,
                    audio_inputs_count: 1,
                    audio_outputs_count: 1,
                    in_events: ptr::null(),
                    out_events: ptr::null(),
                };
                let guard = rvst_test::AllocationGuard::start();
                let result = Export::<Def>::process(p, &d);
                let counts = guard.finish();
                assert_eq!(counts, rvst_test::AllocationCounts::default());
                assert_eq!(result, CLAP_PROCESS_CONTINUE);
                assert_eq!(l, [0.5; 4]);
                assert_eq!(r, [1.; 4]);
                Export::<Def>::stop(p);
                Export::<Def>::deactivate(p);
            }
            Export::<Def>::destroy(p);
        }
    }
    #[test]
    fn legacy_step_scaling_text_modulation_and_ui_output_use_the_same_wire_domain() {
        unsafe {
            let stepped = ParameterSpec {
                id: 43,
                name: "Steps",
                unit: "st",
                min: -24.,
                max: 24.,
                default: 0.,
                step: Some(1.),
                bypass: false,
            };
            assert_eq!(compat::maximum::<Legacy>(&stepped), 48.);
            assert_eq!(compat::to_host::<Legacy>(&stepped, 0.), 24.);
            assert_eq!(compat::from_host::<Legacy>(&stepped, 12.), -12.);
            let mut text = [0 as c_char; 32];
            assert!(Export::<Def>::param_text(
                ptr::null(),
                42,
                0.25,
                text.as_mut_ptr(),
                32
            ));
            assert_eq!(CStr::from_ptr(text.as_ptr()), c"0.500");
            let mut value = 0.;
            assert!(Export::<Def>::text_param(
                ptr::null(),
                42,
                c"0.500".as_ptr(),
                &mut value
            ));
            assert_eq!(value, 0.25);
            let modulation = clap_event_param_mod {
                header: clap_event_header {
                    size: size_of::<clap_event_param_mod>() as u32,
                    time: 0,
                    space_id: 0,
                    type_: CLAP_EVENT_PARAM_MOD,
                    flags: 0,
                },
                param_id: 42,
                cookie: ptr::null_mut(),
                note_id: -1,
                port_index: -1,
                channel: -1,
                key: -1,
                amount: 0.25,
            };
            assert_eq!(
                convert_event::<Legacy>(&modulation.header).unwrap().kind,
                EventKind::Modulation {
                    id: 42,
                    amount: 0.5
                }
            );
            assert_eq!(
                convert_event::<fixture::Gain>(&modulation.header)
                    .unwrap()
                    .kind,
                EventKind::Modulation {
                    id: 42,
                    amount: 0.25
                }
            );
            unsafe extern "C" fn capture(
                list: *const clap_output_events,
                h: *const clap_event_header,
            ) -> bool {
                unsafe {
                    if (*h).type_ == CLAP_EVENT_PARAM_VALUE {
                        *((*list).ctx as *mut f64) = (*(h as *const clap_event_param_value)).value;
                    }
                    true
                }
            }
            let mut emitted = -1.;
            let output = clap_output_events {
                ctx: &mut emitted as *mut _ as _,
                try_push: Some(capture),
            };
            let mut rt = Runtime::<Legacy>::new();
            rt.mirror.edit(42, 0.5);
            let guard = rvst_test::AllocationGuard::start();
            apply_ui(&mut rt, &output);
            let counts = guard.finish();
            assert_eq!(counts, rvst_test::AllocationCounts::default());
            assert_eq!(rt.plugin.parameter(42), Some(0.5));
            assert_eq!(emitted, 0.25);
            assert!(compat::decode_state::<Legacy>(&[0; 7]).is_none());
            let mut bad = 10u64.to_le_bytes().to_vec();
            bad.extend_from_slice(&[0; 8]);
            assert!(compat::decode_state::<Legacy>(&bad).is_none());
        }
    }
    #[test]
    fn gui_requests_flush_without_querying_extensions_off_main() {
        unsafe {
            static QUERIES: AtomicU32 = AtomicU32::new(0);
            static FLUSHES: AtomicU32 = AtomicU32::new(0);
            unsafe extern "C" fn flush(_: *const clap_host) {
                FLUSHES.fetch_add(1, Ordering::SeqCst);
            }
            static PARAMS: clap_host_params = clap_host_params {
                rescan: None,
                clear: None,
                request_flush: Some(flush),
            };
            unsafe extern "C" fn extension(
                _: *const clap_host,
                id: *const c_char,
            ) -> *const c_void {
                unsafe {
                    QUERIES.fetch_add(1, Ordering::SeqCst);
                    if CStr::from_ptr(id) == CLAP_EXT_PARAMS {
                        &PARAMS as *const _ as _
                    } else {
                        ptr::null()
                    }
                }
            }
            QUERIES.store(0, Ordering::SeqCst);
            FLUSHES.store(0, Ordering::SeqCst);
            let host = Box::new(clap_host {
                clap_version: CLAP_VERSION,
                host_data: ptr::null_mut(),
                name: c"test".as_ptr(),
                vendor: c"test".as_ptr(),
                url: c"".as_ptr(),
                version: c"1".as_ptr(),
                get_extension: Some(extension),
                request_restart: None,
                request_process: None,
                request_callback: None,
            });
            let p =
                Export::<Def>::create(&Export::<Def>::FACTORY, &*host, c"org.rvst.test".as_ptr());
            assert!(Export::<Def>::init(p));
            let i = Export::<Def>::instance(p);
            let mut editor = ClapEditor {
                mirror: i.mirror.clone(),
                host: &*host as *const _ as usize,
                params: i.host_params.load(Ordering::Acquire) as usize,
            };
            std::thread::spawn(move || {
                editor.begin_edit(42);
                editor.set_parameter(42, 0.5);
                editor.end_edit(42);
            })
            .join()
            .unwrap();
            assert_eq!(QUERIES.load(Ordering::SeqCst), 2);
            assert_eq!(FLUSHES.load(Ordering::SeqCst), 3);
            Export::<Def>::destroy(p);
        }
    }
}

#[cfg(test)]
mod latency_tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    mod fixture {
        include!("../../rvst-adapter/test-support/plugin.rs");
    }
    struct Delayed {
        gain: fixture::Gain,
        frames: usize,
        prepares: usize,
        resets: usize,
    }
    impl Plugin for Delayed {
        fn descriptor() -> &'static PluginDescriptor {
            fixture::Gain::descriptor()
        }
        fn new() -> Self {
            Self {
                gain: fixture::Gain::new(),
                frames: 0,
                prepares: 0,
                resets: 0,
            }
        }
        fn prepare(&mut self, c: ProcessConfig) -> Result<(), Error> {
            self.prepares += 1;
            self.gain.prepare(c)
        }
        fn process(
            &mut self,
            b: &mut AudioBlock<'_, f32>,
            e: &[Event],
            t: &Transport,
            s: &mut dyn EventSink,
        ) -> ProcessStatus {
            self.frames += b.frames;
            self.gain.process(b, e, t, s)
        }
        fn latency(&self) -> u32 {
            if self.frames >= 3 { 64 } else { 0 }
        }
        fn reset(&mut self) {
            self.resets += 1;
        }
        fn parameter(&self, id: u32) -> Option<f64> {
            self.gain.parameter(id)
        }
        fn set_parameter(&mut self, id: u32, v: f64) -> Result<(), Error> {
            self.gain.set_parameter(id, v)
        }
        fn save_state(&self) -> Result<Vec<u8>, Error> {
            self.gain.save_state()
        }
        fn load_state(&mut self, b: &[u8]) -> Result<(), Error> {
            self.gain.load_state(b)
        }
    }
    struct Def;
    impl Definition for Def {
        type Plugin = Delayed;
        fn descriptor() -> &'static Descriptor {
            static D: std::sync::OnceLock<Descriptor> = std::sync::OnceLock::new();
            D.get_or_init(|| Descriptor::new(Delayed::descriptor()))
        }
        fn editor() -> EditorFn {
            rvst_ui::default_editor
        }
    }
    #[derive(Default)]
    struct Host {
        restarts: AtomicU32,
        changes: AtomicU32,
        invalid_changes: AtomicU32,
        reported: AtomicU32,
        plugin: AtomicUsize,
    }
    unsafe extern "C" fn restart(h: *const clap_host) {
        (&*((*h).host_data as *const Host))
            .restarts
            .fetch_add(1, Ordering::Relaxed);
    }
    unsafe extern "C" fn changed(h: *const clap_host) {
        let h = &*((*h).host_data as *const Host);
        let p = h.plugin.load(Ordering::Acquire) as *const clap_plugin;
        h.changes.fetch_add(1, Ordering::Relaxed);
        h.reported
            .store(Export::<Def>::latency(p), Ordering::Release);
        if Export::<Def>::instance(p).active.load(Ordering::Acquire) {
            h.invalid_changes.fetch_add(1, Ordering::Relaxed);
        }
    }
    unsafe extern "C" fn extension(_: *const clap_host, id: *const c_char) -> *const c_void {
        static LATENCY: clap_host_latency = clap_host_latency {
            changed: Some(changed),
        };
        if CStr::from_ptr(id) == CLAP_EXT_LATENCY {
            &LATENCY as *const _ as _
        } else {
            ptr::null()
        }
    }
    #[test]
    fn latency_transition_requests_one_restart_and_notifies_only_during_activation() {
        unsafe {
            let state = Host::default();
            let host = clap_host {
                clap_version: CLAP_VERSION,
                host_data: &state as *const _ as _,
                name: c"test".as_ptr(),
                vendor: c"test".as_ptr(),
                url: c"".as_ptr(),
                version: c"1".as_ptr(),
                get_extension: Some(extension),
                request_restart: Some(restart),
                request_process: None,
                request_callback: None,
            };
            let p =
                Export::<Def>::create(&Export::<Def>::FACTORY, &host, c"org.rvst.test".as_ptr());
            state.plugin.store(p as usize, Ordering::Release);
            assert!(Export::<Def>::init(p));
            assert!(Export::<Def>::activate(p, 48000., 1, 16));
            assert!(Export::<Def>::start(p));
            let mut l = [1f32; 2];
            let mut r = [1f32; 2];
            let mut channels = [l.as_mut_ptr(), r.as_mut_ptr()];
            let mut bus = clap_sys::audio_buffer::clap_audio_buffer {
                data32: channels.as_mut_ptr(),
                data64: ptr::null_mut(),
                channel_count: 2,
                latency: 0,
                constant_mask: 0,
            };
            let data = clap_process {
                steady_time: 0,
                frames_count: 2,
                transport: ptr::null(),
                audio_inputs: &bus,
                audio_outputs: &mut bus,
                audio_inputs_count: 1,
                audio_outputs_count: 1,
                in_events: ptr::null(),
                out_events: ptr::null(),
            };
            assert_eq!(Export::<Def>::process(p, &data), CLAP_PROCESS_CONTINUE);
            assert_eq!(state.restarts.load(Ordering::Acquire), 0);
            let guard = rvst_test::AllocationGuard::start();
            let result = Export::<Def>::process(p, &data);
            let counts = guard.finish();
            assert_eq!(result, CLAP_PROCESS_CONTINUE);
            assert_eq!(counts, rvst_test::AllocationCounts::default());
            assert_eq!(state.restarts.load(Ordering::Acquire), 1);
            assert_eq!(Export::<Def>::process(p, &data), CLAP_PROCESS_CONTINUE);
            Export::<Def>::main(p);
            assert_eq!(state.restarts.load(Ordering::Acquire), 1);
            assert_eq!(state.changes.load(Ordering::Acquire), 0);
            assert_eq!(Export::<Def>::latency(p), 0);
            let rt = &*Export::<Def>::instance(p).runtime.get();
            assert_eq!((rt.plugin.prepares, rt.plugin.resets), (1, 0));
            Export::<Def>::stop(p);
            Export::<Def>::deactivate(p);
            assert!(Export::<Def>::activate(p, 48000., 1, 16));
            assert_eq!(Export::<Def>::latency(p), 64);
            assert_eq!(
                Export::<Def>::instance(p)
                    .mirror
                    .latency
                    .load(Ordering::Acquire),
                64
            );
            assert_eq!(state.changes.load(Ordering::Acquire), 1);
            assert_eq!(state.invalid_changes.load(Ordering::Acquire), 0);
            assert_eq!(state.reported.load(Ordering::Acquire), 64);
            assert!(Export::<Def>::start(p));
            assert_eq!(Export::<Def>::process(p, &data), CLAP_PROCESS_CONTINUE);
            assert_eq!(state.restarts.load(Ordering::Acquire), 1);
            Export::<Def>::stop(p);
            Export::<Def>::deactivate(p);
            Export::<Def>::destroy(p);
        }
    }
}

#[cfg(test)]
mod transport_tests {
    use super::*;
    mod fixture {
        include!("../../rvst-adapter/test-support/plugin.rs");
    }
    struct Capture {
        gain: fixture::Gain,
        transports: [Transport; 8],
        count: usize,
    }
    impl Plugin for Capture {
        fn descriptor() -> &'static PluginDescriptor {
            fixture::Gain::descriptor()
        }
        fn new() -> Self {
            Self {
                gain: fixture::Gain::new(),
                transports: [Transport::default(); 8],
                count: 0,
            }
        }
        fn prepare(&mut self, c: ProcessConfig) -> Result<(), Error> {
            self.gain.prepare(c)
        }
        fn process(
            &mut self,
            b: &mut AudioBlock<'_, f32>,
            e: &[Event],
            t: &Transport,
            s: &mut dyn EventSink,
        ) -> ProcessStatus {
            self.transports[self.count] = *t;
            self.count += 1;
            self.gain.process(b, e, t, s)
        }
        fn reset(&mut self) {}
        fn parameter(&self, id: u32) -> Option<f64> {
            self.gain.parameter(id)
        }
        fn set_parameter(&mut self, id: u32, v: f64) -> Result<(), Error> {
            self.gain.set_parameter(id, v)
        }
        fn save_state(&self) -> Result<Vec<u8>, Error> {
            self.gain.save_state()
        }
        fn load_state(&mut self, b: &[u8]) -> Result<(), Error> {
            self.gain.load_state(b)
        }
    }
    struct Def;
    impl Definition for Def {
        type Plugin = Capture;
        fn descriptor() -> &'static Descriptor {
            static D: std::sync::OnceLock<Descriptor> = std::sync::OnceLock::new();
            D.get_or_init(|| Descriptor::new(Capture::descriptor()))
        }
        fn editor() -> EditorFn {
            rvst_ui::default_editor
        }
    }
    unsafe extern "C" fn size(_: *const clap_input_events) -> u32 {
        1
    }
    unsafe extern "C" fn get(list: *const clap_input_events, _: u32) -> *const clap_event_header {
        (*list).ctx as _
    }
    #[test]
    fn transport_events_and_tempo_ramps_keep_exact_frame_context() {
        unsafe {
            let host = clap_host {
                clap_version: CLAP_VERSION,
                host_data: ptr::null_mut(),
                name: c"test".as_ptr(),
                vendor: c"test".as_ptr(),
                url: c"".as_ptr(),
                version: c"1".as_ptr(),
                get_extension: None,
                request_restart: None,
                request_process: None,
                request_callback: None,
            };
            let p =
                Export::<Def>::create(&Export::<Def>::FACTORY, &host, c"org.rvst.test".as_ptr());
            assert!(Export::<Def>::init(p));
            assert!(Export::<Def>::activate(p, 48000., 1, 16));
            assert!(Export::<Def>::start(p));
            let mut initial: clap_event_transport = std::mem::zeroed();
            initial.header = clap_event_header {
                size: size_of::<clap_event_transport>() as u32,
                time: 0,
                space_id: 0,
                type_: CLAP_EVENT_TRANSPORT,
                flags: 0,
            };
            initial.flags = CLAP_TRANSPORT_HAS_TEMPO
                | CLAP_TRANSPORT_HAS_SECONDS_TIMELINE
                | CLAP_TRANSPORT_HAS_BEATS_TIMELINE
                | CLAP_TRANSPORT_IS_PLAYING;
            initial.tempo = 120.;
            initial.song_pos_seconds = 1i64 << 31;
            initial.song_pos_beats = 2i64 << 31;
            let mut stop = initial;
            stop.header.time = 2;
            stop.flags &= !CLAP_TRANSPORT_IS_PLAYING;
            stop.tempo = 90.;
            stop.song_pos_seconds = 2i64 << 31;
            stop.song_pos_beats = 4i64 << 31;
            let list = clap_input_events {
                ctx: &stop as *const _ as _,
                size: Some(size),
                get: Some(get),
            };
            let mut l = [1f32; 4];
            let mut r = [1f32; 4];
            let mut channels = [l.as_mut_ptr(), r.as_mut_ptr()];
            let mut bus = clap_sys::audio_buffer::clap_audio_buffer {
                data32: channels.as_mut_ptr(),
                data64: ptr::null_mut(),
                channel_count: 2,
                latency: 0,
                constant_mask: 0,
            };
            let mut data = clap_process {
                steady_time: 0,
                frames_count: 4,
                transport: &initial,
                audio_inputs: &bus,
                audio_outputs: &mut bus,
                audio_inputs_count: 1,
                audio_outputs_count: 1,
                in_events: &list,
                out_events: ptr::null(),
            };
            let guard = rvst_test::AllocationGuard::start();
            let result = Export::<Def>::process(p, &data);
            let counts = guard.finish();
            assert_eq!(result, CLAP_PROCESS_CONTINUE);
            assert_eq!(counts, rvst_test::AllocationCounts::default());
            let capture = &mut (*Export::<Def>::instance(p).runtime.get()).plugin;
            assert_eq!(capture.count, 4);
            assert_eq!(
                capture.transports[..4]
                    .iter()
                    .map(|t| t.tempo.unwrap())
                    .collect::<Vec<_>>(),
                [120., 120., 90., 90.]
            );
            assert_eq!(
                capture.transports[..4]
                    .iter()
                    .map(|t| t.sample_position.unwrap())
                    .collect::<Vec<_>>(),
                [48000, 48001, 96000, 96000]
            );
            assert_eq!(capture.transports[3].beat_position, Some(4.));
            capture.count = 0;
            initial.tempo = 60.;
            initial.tempo_inc = 60.;
            data.frames_count = 3;
            data.in_events = ptr::null();
            data.transport = &initial;
            assert_eq!(Export::<Def>::process(p, &data), CLAP_PROCESS_CONTINUE);
            let capture = &(*Export::<Def>::instance(p).runtime.get()).plugin;
            assert_eq!(capture.count, 3);
            assert_eq!(
                capture.transports[..3]
                    .iter()
                    .map(|t| t.tempo.unwrap())
                    .collect::<Vec<_>>(),
                [60., 120., 180.]
            );
            assert!(
                (capture.transports[2].beat_position.unwrap() - (2. + 3. / 48000.)).abs() < 1e-12
            );
            Export::<Def>::stop(p);
            Export::<Def>::deactivate(p);
            Export::<Def>::destroy(p);
        }
    }
}
