//! Load an actual CLAP shared library. Optional second argument keeps its
//! processing instance alive for N seconds for browser-inspector validation.
use clap_sys::{
    audio_buffer::*, entry::*, ext::audio_ports::*, factory::plugin_factory::*, host::*,
    process::*, version::*,
};
use std::{
    ffi::CStr,
    ptr,
    time::{Duration, Instant},
};
fn main() {
    unsafe { run().unwrap() }
}
unsafe fn run() -> Result<(), Box<dyn std::error::Error>> {
    unsafe {
        let path = std::env::args()
            .nth(1)
            .ok_or("usage: host-smoke <shared-library> [seconds]")?;
        let seconds = std::env::args()
            .nth(2)
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        let lib = libloading::Library::new(path)?;
        let entry = lib.get::<*const clap_plugin_entry>(b"clap_entry\0")?;
        let entry = &**entry;
        if !(entry.init.ok_or("missing entry init")?)(c"".as_ptr()) {
            return Err("entry init failed".into());
        }
        let factory = (entry.get_factory.ok_or("missing factory")?)(CLAP_PLUGIN_FACTORY_ID.as_ptr())
            as *const clap_plugin_factory;
        if factory.is_null() {
            return Err("no CLAP factory".into());
        }
        let f = &*factory;
        let desc = (f.get_plugin_descriptor.ok_or("missing descriptor")?)(factory, 0);
        if desc.is_null() {
            return Err("no descriptor".into());
        }
        let host = clap_host {
            clap_version: CLAP_VERSION,
            host_data: ptr::null_mut(),
            name: c"RVST ABI smoke host".as_ptr(),
            vendor: c"RVST".as_ptr(),
            url: c"".as_ptr(),
            version: c"0.1".as_ptr(),
            get_extension: None,
            request_restart: None,
            request_process: None,
            request_callback: None,
        };
        let p = (f.create_plugin.ok_or("missing create")?)(factory, &host, (*desc).id);
        if p.is_null() {
            return Err("creation failed".into());
        }
        if !((*p).init.unwrap())(p) {
            return Err("plugin init failed".into());
        }
        let ports = ((*p).get_extension.unwrap())(p, CLAP_EXT_AUDIO_PORTS.as_ptr())
            as *const clap_plugin_audio_ports;
        if ports.is_null() {
            return Err("no audio ports".into());
        }
        let mut storage: Vec<Vec<Vec<f32>>> = Vec::new();
        for input in [true, false] {
            let count = ((*ports).count.unwrap())(p, input);
            let mut side = Vec::new();
            for b in 0..count {
                let mut info = std::mem::zeroed();
                assert!(((*ports).get.unwrap())(p, b, input, &mut info));
                for _ in 0..info.channel_count {
                    side.push(vec![if input { 0.25 } else { 0. }; 64]);
                }
            }
            storage.push(side);
        }
        let mut pointers: Vec<Vec<*mut f32>> = storage
            .iter_mut()
            .map(|side| side.iter_mut().map(|c| c.as_mut_ptr()).collect())
            .collect();
        let mut buffers: Vec<Vec<clap_audio_buffer>> = Vec::new();
        for (side, input) in [true, false].into_iter().enumerate() {
            let mut offset = 0;
            let mut buses = Vec::new();
            for b in 0..((*ports).count.unwrap())(p, input) {
                let mut info = std::mem::zeroed();
                assert!(((*ports).get.unwrap())(p, b, input, &mut info));
                buses.push(clap_audio_buffer {
                    data32: pointers[side].as_mut_ptr().add(offset),
                    data64: ptr::null_mut(),
                    channel_count: info.channel_count,
                    latency: 0,
                    constant_mask: 0,
                });
                offset += info.channel_count as usize;
            }
            buffers.push(buses);
        }
        assert!(((*p).activate.unwrap())(p, 48000., 1, 64));
        assert!(((*p).start_processing.unwrap())(p));
        let mut d = clap_process {
            steady_time: 0,
            frames_count: 64,
            transport: ptr::null(),
            audio_inputs: buffers[0].as_ptr(),
            audio_outputs: buffers[1].as_mut_ptr(),
            audio_inputs_count: buffers[0].len() as u32,
            audio_outputs_count: buffers[1].len() as u32,
            in_events: ptr::null(),
            out_events: ptr::null(),
        };
        let end = Instant::now() + Duration::from_secs(seconds);
        let mut blocks = 0;
        loop {
            let status = ((*p).process.unwrap())(p, &d);
            if status == CLAP_PROCESS_ERROR {
                return Err("CLAP process returned error".into());
            }
            blocks += 1;
            d.steady_time += 64;
            if blocks >= 8 && Instant::now() >= end {
                break;
            }
            if seconds > 0 {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        for c in &storage[1] {
            assert!(c.iter().all(|v| v.is_finite()));
        }
        println!(
            "Loaded {}: {} real ABI blocks, {} input buses, {} output buses; finite f32 output",
            CStr::from_ptr((*desc).name).to_string_lossy(),
            blocks,
            d.audio_inputs_count,
            d.audio_outputs_count
        );
        ((*p).stop_processing.unwrap())(p);
        ((*p).deactivate.unwrap())(p);
        ((*p).destroy.unwrap())(p);
        if let Some(f) = entry.deinit {
            f();
        }
        Ok(())
    }
}
