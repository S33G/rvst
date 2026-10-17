//! Main/UI-thread timers. Construction and destruction stay on the owning thread.
use super::*;
#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    type Ref = *mut c_void;
    type Callback = std::rc::Rc<std::cell::RefCell<Box<dyn FnMut()>>>;
    #[repr(C)]
    struct Context {
        version: isize,
        info: Ref,
        retain: Ref,
        release: Ref,
        description: Ref,
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFAbsoluteTimeGetCurrent() -> f64;
        fn CFRunLoopGetCurrent() -> Ref;
        fn CFRunLoopTimerCreate(
            allocator: Ref,
            fire: f64,
            interval: f64,
            flags: usize,
            order: isize,
            callback: unsafe extern "C" fn(Ref, Ref),
            context: *mut Context,
        ) -> Ref;
        fn CFRunLoopAddTimer(run_loop: Ref, timer: Ref, mode: Ref);
        fn CFRunLoopTimerInvalidate(timer: Ref);
        fn CFRelease(value: Ref);
        static kCFRunLoopCommonModes: Ref;
    }
    pub(super) struct Timer {
        raw: Ref,
        callback: *mut Callback,
    }
    impl Timer {
        pub unsafe fn new(callback: Box<dyn FnMut()>) -> Option<Self> {
            unsafe extern "C" fn fire(_: Ref, info: Ref) {
                boundary((), || unsafe {
                    let callback = (&*info.cast::<Callback>()).clone();
                    (callback.borrow_mut())();
                });
            }
            let callback = Box::into_raw(Box::new(std::rc::Rc::new(std::cell::RefCell::new(
                callback,
            ))));
            let mut context = Context {
                version: 0,
                info: callback.cast(),
                retain: ptr::null_mut(),
                release: ptr::null_mut(),
                description: ptr::null_mut(),
            };
            let raw = CFRunLoopTimerCreate(
                ptr::null_mut(),
                CFAbsoluteTimeGetCurrent() + 0.02,
                0.02,
                0,
                0,
                fire,
                &mut context,
            );
            if raw.is_null() {
                drop(Box::from_raw(callback));
                return None;
            }
            CFRunLoopAddTimer(CFRunLoopGetCurrent(), raw, kCFRunLoopCommonModes);
            Some(Self { raw, callback })
        }
    }
    impl Drop for Timer {
        fn drop(&mut self) {
            unsafe {
                CFRunLoopTimerInvalidate(self.raw);
                CFRelease(self.raw);
                drop(Box::from_raw(self.callback));
            }
        }
    }
}
#[cfg(target_os = "windows")]
mod platform {
    use super::*;
    use std::{cell::RefCell, collections::HashMap};
    use windows_sys::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};
    thread_local! {static CALLBACKS:RefCell<HashMap<usize,std::rc::Rc<RefCell<Box<dyn FnMut()>>>>>=RefCell::new(HashMap::new());}
    pub(super) struct Timer(usize);
    impl Timer {
        pub unsafe fn new(callback: Box<dyn FnMut()>) -> Option<Self> {
            unsafe extern "system" fn fire(_: *mut c_void, _: u32, id: usize, _: u32) {
                boundary((), || {
                    let callback = CALLBACKS.with(|all| all.borrow().get(&id).cloned());
                    if let Some(callback) = callback {
                        (callback.borrow_mut())();
                    }
                });
            }
            let id = SetTimer(ptr::null_mut(), 0, 20, Some(fire));
            if id == 0 {
                return None;
            }
            CALLBACKS.with(|all| {
                all.borrow_mut()
                    .insert(id, std::rc::Rc::new(RefCell::new(callback)))
            });
            Some(Self(id))
        }
    }
    impl Drop for Timer {
        fn drop(&mut self) {
            unsafe {
                KillTimer(ptr::null_mut(), self.0);
            }
            CALLBACKS.with(|all| all.borrow_mut().remove(&self.0));
        }
    }
}
#[cfg(target_os = "linux")]
mod platform {
    use super::*;
    use vst3::{ComPtr, Steinberg::Linux::*};
    struct Callback(std::rc::Rc<std::cell::RefCell<Box<dyn FnMut()>>>);
    impl Class for Callback {
        type Interfaces = (ITimerHandler,);
    }
    impl ITimerHandlerTrait for Callback {
        unsafe fn onTimer(&self) {
            boundary((), || {
                let callback = self.0.clone();
                (callback.borrow_mut())();
            });
        }
    }
    pub(super) struct Timer {
        run_loop: ComPtr<IRunLoop>,
        handler: ComPtr<ITimerHandler>,
    }
    impl Timer {
        pub unsafe fn new(context: *mut FUnknown, callback: Box<dyn FnMut()>) -> Option<Self> {
            let run_loop = ComRef::from_raw(context)?.cast::<IRunLoop>()?;
            let handler = ComWrapper::new(Callback(std::rc::Rc::new(std::cell::RefCell::new(
                callback,
            ))))
            .to_com_ptr::<ITimerHandler>()?;
            if run_loop.registerTimer(handler.as_ptr(), 20) != kResultOk {
                return None;
            }
            Some(Self { run_loop, handler })
        }
    }
    impl Drop for Timer {
        fn drop(&mut self) {
            unsafe {
                self.run_loop.unregisterTimer(self.handler.as_ptr());
            }
        }
    }
}
pub(super) struct MainTimer {
    _timer: platform::Timer,
    _thread_bound: PhantomData<std::rc::Rc<()>>,
}
impl MainTimer {
    pub unsafe fn new(_context: *mut FUnknown, callback: impl FnMut() + 'static) -> Option<Self> {
        #[cfg(target_os = "linux")]
        let timer = platform::Timer::new(_context, Box::new(callback))?;
        #[cfg(not(target_os = "linux"))]
        let timer = platform::Timer::new(Box::new(callback))?;
        Some(Self {
            _timer: timer,
            _thread_bound: PhantomData,
        })
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::sync::{Arc, atomic::AtomicUsize};
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRunLoopRunInMode(mode: *mut c_void, seconds: f64, return_after_source: bool) -> i32;
        static kCFRunLoopDefaultMode: *mut c_void;
    }
    struct Host {
        calls: Arc<AtomicUsize>,
        thread: std::thread::ThreadId,
    }
    impl Class for Host {
        type Interfaces = (IComponentHandler,);
    }
    impl IComponentHandlerTrait for Host {
        unsafe fn beginEdit(&self, _: u32) -> tresult {
            kResultOk
        }
        unsafe fn performEdit(&self, _: u32, _: f64) -> tresult {
            kResultOk
        }
        unsafe fn endEdit(&self, _: u32) -> tresult {
            kResultOk
        }
        unsafe fn restartComponent(&self, flags: i32) -> tresult {
            assert_eq!(std::thread::current().id(), self.thread);
            assert_eq!(flags, RestartFlags_::kLatencyChanged);
            self.calls.fetch_add(1, Ordering::Relaxed);
            kResultOk
        }
    }
    #[test]
    fn native_latency_notification_runs_on_controller_thread_and_stops_on_terminate() {
        unsafe {
            type P = super::super::tests::fixture::Gain;
            let calls = Arc::new(AtomicUsize::new(0));
            let host = ComWrapper::new(Host {
                calls: calls.clone(),
                thread: std::thread::current().id(),
            })
            .to_com_ptr::<IComponentHandler>()
            .unwrap();
            let mirror = rvst_adapter::ParameterMirror::new(P::descriptor());
            let controller = Controller::<P> {
                connection: connection::Endpoint::default(),
                linked: UnsafeCell::new(None),
                mirror: mirror.clone(),
                plugin: UnsafeCell::new(rvst_adapter::DropShield::new(P::new())),
                handler: Cell::new(ptr::null_mut()),
                host_context: Cell::new(ptr::null_mut()),
                timer: UnsafeCell::new(None),
                editor: rvst_ui::default_editor,
            };
            controller.setComponentHandler(host.as_ptr());
            mirror.latency.store(512, Ordering::Release);
            CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.08, false);
            assert_eq!(calls.load(Ordering::Relaxed), 1);
            controller.terminate();
            mirror.latency.store(1024, Ordering::Release);
            CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.04, false);
            assert_eq!(calls.load(Ordering::Relaxed), 1);
        }
    }
}
