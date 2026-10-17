//! Load an actual VST3 binary, instantiate both COM classes and process audio.
use std::{ffi::c_void, ptr};
use vst3::{
    ComPtr, Interface,
    Steinberg::{Vst::*, *},
};
fn main() {
    unsafe { run().unwrap() }
}
unsafe fn run() -> Result<(), Box<dyn std::error::Error>> {
    unsafe {
        let path = std::env::args()
            .nth(1)
            .ok_or("usage: host-smoke <VST3 binary>")?;
        let lib = libloading::Library::new(path)?;
        #[cfg(target_os = "macos")]
        if let Ok(entry) =
            lib.get::<unsafe extern "system" fn(*mut c_void) -> bool>(b"bundleEntry\0")
        {
            assert!(entry(ptr::null_mut()));
        }
        let get =
            lib.get::<unsafe extern "system" fn() -> *mut IPluginFactory>(b"GetPluginFactory\0")?;
        let f = ComPtr::from_raw(get()).ok_or("null factory")?;
        assert_eq!(f.countClasses(), 2);
        let mut info: PClassInfo = std::mem::zeroed();
        assert_eq!(f.getClassInfo(0, &mut info), kResultOk);
        let mut p = ptr::null_mut();
        assert_eq!(
            f.createInstance(info.cid.as_ptr(), IComponent::IID.as_ptr() as _, &mut p),
            kResultOk
        );
        let component = ComPtr::<IComponent>::from_raw(p as _).ok_or("null component")?;
        assert_eq!(component.initialize(ptr::null_mut()), kResultOk);
        let processor = component
            .cast::<IAudioProcessor>()
            .ok_or("no processor interface")?;
        let mut setup = ProcessSetup {
            processMode: 0,
            symbolicSampleSize: 0,
            maxSamplesPerBlock: 64,
            sampleRate: 48000.,
        };
        assert_eq!(processor.setupProcessing(&mut setup), kResultOk);
        let mut storage: Vec<Vec<Vec<f32>>> = Vec::new();
        let mut sizes = Vec::new();
        for dir in [0, 1] {
            let count = component.getBusCount(0, dir);
            let mut side = Vec::new();
            let mut side_sizes = Vec::new();
            for bus in 0..count {
                let mut b = std::mem::zeroed();
                assert_eq!(component.getBusInfo(0, dir, bus, &mut b), kResultOk);
                side_sizes.push(b.channelCount);
                for _ in 0..b.channelCount {
                    side.push(vec![if dir == 0 { 0.25 } else { 0. }; 64]);
                }
                component.activateBus(0, dir, bus, 1);
            }
            storage.push(side);
            sizes.push(side_sizes);
        }
        let mut pointers: Vec<Vec<*mut f32>> = storage
            .iter_mut()
            .map(|side| side.iter_mut().map(|c| c.as_mut_ptr()).collect())
            .collect();
        let mut buses = Vec::new();
        for side in 0..2 {
            let mut offset = 0;
            let mut list = Vec::new();
            for size in &sizes[side] {
                list.push(AudioBusBuffers {
                    numChannels: *size,
                    silenceFlags: 0,
                    __field0: AudioBusBuffers__type0 {
                        channelBuffers32: pointers[side].as_mut_ptr().add(offset),
                    },
                });
                offset += *size as usize;
            }
            buses.push(list);
        }
        assert_eq!(component.setActive(1), kResultOk);
        assert_eq!(processor.setProcessing(1), kResultOk);
        let mut d: ProcessData = std::mem::zeroed();
        d.numSamples = 64;
        d.numInputs = buses[0].len() as i32;
        d.numOutputs = buses[1].len() as i32;
        d.inputs = buses[0].as_mut_ptr();
        d.outputs = buses[1].as_mut_ptr();
        for _ in 0..8 {
            assert_eq!(processor.process(&mut d), kResultOk);
        }
        for c in &storage[1] {
            assert!(c.iter().all(|v| v.is_finite()));
        }
        let mut cid = [0; 16];
        assert_eq!(component.getControllerClassId(&mut cid), kResultOk);
        let mut p = ptr::null_mut();
        assert_eq!(
            f.createInstance(cid.as_ptr(), IEditController::IID.as_ptr() as _, &mut p),
            kResultOk
        );
        let controller = ComPtr::<IEditController>::from_raw(p as _).ok_or("null controller")?;
        assert_eq!(controller.initialize(ptr::null_mut()), kResultOk);
        println!(
            "Loaded VST3: 8 real ABI blocks, {} parameters, finite f32 output",
            controller.getParameterCount()
        );
        controller.terminate();
        processor.setProcessing(0);
        component.setActive(0);
        component.terminate();
        drop(controller);
        drop(processor);
        drop(component);
        drop(f);
        #[cfg(target_os = "macos")]
        if let Ok(exit) = lib.get::<unsafe extern "system" fn() -> bool>(b"bundleExit\0") {
            assert!(exit());
        }
        Ok(())
    }
}
