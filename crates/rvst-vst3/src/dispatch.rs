//! Marshal X11 UI gestures onto the host's VST3 main-thread run loop.
//! Fixed per-parameter mailboxes coalesce values while preserving balanced edits.
use super::*;
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};
use vst3::{ComPtr, Steinberg::Linux::*};
#[derive(Default, Clone, Copy)]
struct Pending {
    touched: bool,
    active: bool,
    value: Option<(u64, u64, f64)>,
}
pub(super) struct Mailbox {
    slots: Mutex<Vec<Pending>>,
    ids: Vec<u32>,
    mirror: Arc<rvst_adapter::ParameterMirror>,
}
impl Mailbox {
    pub fn new(mirror: Arc<rvst_adapter::ParameterMirror>) -> Arc<Self> {
        let d = mirror.descriptor;
        Arc::new(Self {
            slots: Mutex::new(vec![Pending::default(); d.parameters.len()]),
            ids: d.parameters.iter().map(|p| p.id).collect(),
            mirror,
        })
    }
    fn change(&self, id: u32, f: impl FnOnce(&mut Pending)) {
        if let Some(i) = self.ids.iter().position(|p| *p == id) {
            let mut slots = self.slots.lock().unwrap_or_else(|p| p.into_inner());
            slots[i].touched = true;
            f(&mut slots[i]);
        }
    }
    pub fn begin(&self, id: u32) {
        self.change(id, |p| p.active = true);
    }
    pub fn value_at_generation(&self, id: u32, value: f64, generation: u64, token: u64) {
        self.change(id, |p| p.value = Some((generation, token, value)));
    }
    #[cfg(test)]
    pub fn value(&self, id: u32, value: f64) {
        self.value_at_generation(id, value, self.mirror.state_generation(), 0);
    }
    pub fn end(&self, id: u32) {
        self.change(id, |p| p.active = false);
    }
    fn close(&self) {
        let mut slots = self.slots.lock().unwrap_or_else(|p| p.into_inner());
        for p in slots.iter_mut() {
            if p.active {
                p.active = false;
                p.touched = true;
            }
        }
    }
    fn drain(&self, active: &mut [bool], mut emit: impl FnMut(u32, Action)) {
        for (i, &id) in self.ids.iter().enumerate() {
            let p = {
                let mut slots = self.slots.lock().unwrap_or_else(|p| p.into_inner());
                let p = slots[i];
                slots[i].touched = false;
                slots[i].value = None;
                p
            };
            if !p.touched {
                continue;
            }
            if !active[i] {
                emit(id, Action::Begin);
                active[i] = true;
            }
            if let Some((generation, token, value)) = p.value
                && generation == self.mirror.state_generation()
            {
                emit(id, Action::Value(generation, token, value));
            }
            if !p.active {
                emit(id, Action::End);
                active[i] = false;
            }
        }
    }
}
#[derive(Debug, PartialEq)]
enum Action {
    Begin,
    Value(u64, u64, f64),
    End,
}
struct State {
    mailbox: Arc<Mailbox>,
    handler: Handler,
    active: RefCell<Vec<bool>>,
    actions: RefCell<Vec<(u32, Action)>>,
    in_callback: Cell<bool>,
    drain_again: Cell<bool>,
}
impl State {
    fn new(mailbox: Arc<Mailbox>, handler: Handler) -> Rc<Self> {
        let count = mailbox.ids.len();
        Rc::new(Self {
            mailbox,
            handler,
            active: RefCell::new(vec![false; count]),
            actions: RefCell::new(Vec::with_capacity(count * 3)),
            in_callback: Cell::new(false),
            drain_again: Cell::new(false),
        })
    }
    unsafe fn run(&self) {
        if self.in_callback.replace(true) {
            self.drain_again.set(true);
            return;
        }
        struct CallbackGuard<'a>(&'a Cell<bool>);
        impl Drop for CallbackGuard<'_> {
            fn drop(&mut self) {
                self.0.set(false);
            }
        }
        let _guard = CallbackGuard(&self.in_callback);
        let Some(handler) = ComRef::from_raw(self.handler.0) else {
            return;
        };
        let mut actions = self.actions.take();
        loop {
            self.drain_again.set(false);
            // No RefCell borrow or mailbox lock may span a call into the host.
            // A host callback can synchronously remove the view and this timer.
            self.mailbox
                .drain(&mut self.active.borrow_mut(), |id, action| {
                    actions.push((id, action));
                });
            for (id, action) in actions.drain(..) {
                match action {
                    Action::Begin => {
                        handler.beginEdit(id);
                    }
                    Action::Value(generation, token, value) => {
                        // beginEdit may synchronously restore state. Check again
                        // after host callbacks, without holding the mailbox lock.
                        if generation == self.mailbox.mirror.state_generation() {
                            self.mailbox.mirror.host_edit_dispatched(id, token);
                            handler.performEdit(id, value);
                        }
                    }
                    Action::End => {
                        handler.endEdit(id);
                    }
                }
            }
            // A nested close marked the mailbox and returned. Finish its gesture
            // before the retained callback state (and host handler) can be dropped.
            if !self.drain_again.get() {
                break;
            }
        }
        self.actions.replace(actions);
    }
}
struct Timer {
    state: Rc<State>,
}
impl Class for Timer {
    type Interfaces = (ITimerHandler,);
}
impl ITimerHandlerTrait for Timer {
    unsafe fn onTimer(&self) {
        let state = self.state.clone();
        boundary((), || state.run());
    }
}
pub(super) struct Dispatch {
    run_loop: ComPtr<IRunLoop>,
    timer: ComPtr<ITimerHandler>,
    pub mailbox: Arc<Mailbox>,
}
impl Dispatch {
    pub unsafe fn new(
        frame: *mut IPlugFrame,
        handler: Handler,
        mirror: Arc<rvst_adapter::ParameterMirror>,
    ) -> Option<Self> {
        let run_loop = ComRef::from_raw(frame)?.cast::<IRunLoop>()?;
        let mailbox = Mailbox::new(mirror);
        let timer = ComWrapper::new(Timer {
            state: State::new(mailbox.clone(), handler),
        })
        .to_com_ptr::<ITimerHandler>()?;
        if run_loop.registerTimer(timer.as_ptr(), 16) != kResultOk {
            return None;
        }
        Some(Self {
            run_loop,
            timer,
            mailbox,
        })
    }
    pub fn close_gestures(&self) {
        self.mailbox.close();
        unsafe {
            self.timer.onTimer();
        }
    }
}
impl Drop for Dispatch {
    fn drop(&mut self) {
        self.close_gestures();
        unsafe {
            self.run_loop.unregisterTimer(self.timer.as_ptr());
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restored_state_discards_queued_values_but_preserves_balanced_gestures() {
        let mirror =
            rvst_adapter::ParameterMirror::new(super::super::tests::fixture::Gain::descriptor());
        let q = Mailbox::new(mirror.clone());
        let before_restore = mirror.state_generation();
        let mut active = [false];
        let mut events = Vec::new();
        q.begin(42);
        q.value(42, 0.25);
        q.drain(&mut active, |id, action| events.push((id, action)));
        events.clear();
        q.value(42, 0.5);
        mirror.state_restored();
        // Include a UI producer that was suspended before the restore.
        q.value_at_generation(42, 0.6, before_restore, 0);
        q.end(42);
        q.drain(&mut active, |id, action| events.push((id, action)));
        assert_eq!(events, vec![(42, Action::End)]);
        assert!(!active[0]);

        events.clear();
        q.begin(42);
        q.value(42, 0.75);
        q.end(42);
        q.drain(&mut active, |id, action| events.push((id, action)));
        assert_eq!(
            events,
            vec![
                (42, Action::Begin),
                (42, Action::Value(mirror.state_generation(), 0, 0.75)),
                (42, Action::End),
            ]
        );
    }
    #[test]
    fn coalescing_saturation_and_shutdown_keep_edits_balanced() {
        let q = Mailbox::new(rvst_adapter::ParameterMirror::new(
            super::super::tests::fixture::Gain::descriptor(),
        ));
        let mut active = [false];
        let mut events = Vec::new();
        for i in 0..10000 {
            q.begin(42);
            q.value(42, i as f64);
            q.end(42);
        }
        q.drain(&mut active, |id, a| events.push((id, a)));
        assert_eq!(
            events,
            vec![
                (42, Action::Begin),
                (42, Action::Value(1, 0, 9999.)),
                (42, Action::End)
            ]
        );
        events.clear();
        q.begin(42);
        q.value(42, 0.5);
        q.drain(&mut active, |id, a| events.push((id, a)));
        q.close();
        q.drain(&mut active, |id, a| events.push((id, a)));
        assert_eq!(
            events,
            vec![
                (42, Action::Begin),
                (42, Action::Value(1, 0, 0.5)),
                (42, Action::End)
            ]
        );
        assert!(!active[0]);
    }
}

#[cfg(test)]
mod reentrant_tests {
    use super::*;
    use std::{cell::RefCell, rc::Rc};
    struct Host {
        actions: Rc<RefCell<Vec<Action>>>,
        mailbox: Arc<Mailbox>,
        timer: Rc<Cell<*mut ITimerHandler>>,
        close: Cell<bool>,
        restore: Cell<bool>,
    }
    impl Class for Host {
        type Interfaces = (IComponentHandler,);
    }
    impl IComponentHandlerTrait for Host {
        unsafe fn beginEdit(&self, _: u32) -> tresult {
            self.actions.borrow_mut().push(Action::Begin);
            if self.restore.replace(false) {
                self.mailbox.mirror.state_restored();
            }
            if self.close.replace(false) {
                self.mailbox.close();
                ComRef::from_raw(self.timer.get()).unwrap().onTimer();
            }
            kResultOk
        }
        unsafe fn performEdit(&self, _: u32, v: f64) -> tresult {
            self.actions.borrow_mut().push(Action::Value(1, 0, v));
            kResultOk
        }
        unsafe fn endEdit(&self, _: u32) -> tresult {
            self.actions.borrow_mut().push(Action::End);
            kResultOk
        }
        unsafe fn restartComponent(&self, _: i32) -> tresult {
            kResultOk
        }
    }
    #[test]
    fn callback_can_close_gesture_reentrantly_without_delaying_end() {
        unsafe {
            let mailbox = Mailbox::new(rvst_adapter::ParameterMirror::new(
                super::super::tests::fixture::Gain::descriptor(),
            ));
            let actions = Rc::new(RefCell::new(Vec::new()));
            let slot = Rc::new(Cell::new(ptr::null_mut()));
            let host = ComWrapper::new(Host {
                actions: actions.clone(),
                mailbox: mailbox.clone(),
                timer: slot.clone(),
                close: Cell::new(true),
                restore: Cell::new(false),
            })
            .to_com_ptr::<IComponentHandler>()
            .unwrap();
            retain(host.as_ptr() as _);
            let timer = ComWrapper::new(Timer {
                state: State::new(mailbox.clone(), Handler(host.as_ptr())),
            })
            .to_com_ptr::<ITimerHandler>()
            .unwrap();
            slot.set(timer.as_ptr());
            mailbox.begin(42);
            mailbox.value(42, 0.5);
            timer.onTimer();
            assert_eq!(
                *actions.borrow(),
                vec![Action::Begin, Action::Value(1, 0, 0.5), Action::End]
            );
        }
    }
    #[test]
    fn restore_inside_begin_edit_discards_already_drained_value() {
        unsafe {
            let mailbox = Mailbox::new(rvst_adapter::ParameterMirror::new(
                super::super::tests::fixture::Gain::descriptor(),
            ));
            let actions = Rc::new(RefCell::new(Vec::new()));
            let host = ComWrapper::new(Host {
                actions: actions.clone(),
                mailbox: mailbox.clone(),
                timer: Rc::new(Cell::new(ptr::null_mut())),
                close: Cell::new(false),
                restore: Cell::new(true),
            })
            .to_com_ptr::<IComponentHandler>()
            .unwrap();
            retain(host.as_ptr() as _);
            let state = State::new(mailbox.clone(), Handler(host.as_ptr()));
            mailbox.begin(42);
            mailbox.value(42, 0.5);
            mailbox.end(42);
            state.run();
            assert_eq!(*actions.borrow(), vec![Action::Begin, Action::End]);
        }
    }
}

#[cfg(test)]
mod lifetime_tests {
    use super::*;
    struct Host {
        actions: Rc<RefCell<Vec<Action>>>,
        mailbox: Arc<Mailbox>,
        timer: Rc<RefCell<Option<ComPtr<ITimerHandler>>>>,
        destroyed: Rc<Cell<bool>>,
    }
    impl Drop for Host {
        fn drop(&mut self) {
            self.destroyed.set(true);
        }
    }
    impl Class for Host {
        type Interfaces = (IComponentHandler,);
    }
    impl IComponentHandlerTrait for Host {
        unsafe fn beginEdit(&self, _: u32) -> tresult {
            self.actions.borrow_mut().push(Action::Begin);
            self.mailbox.close();
            let timer = self.timer.borrow().as_ref().unwrap().as_ptr();
            ComRef::from_raw(timer).unwrap().onTimer();
            // Model a host removing the view and unregistering the final timer
            // reference from inside beginEdit, before the outer drain resumes.
            let retired = self.timer.borrow_mut().take();
            drop(retired);
            kResultOk
        }
        unsafe fn performEdit(&self, _: u32, v: f64) -> tresult {
            self.actions.borrow_mut().push(Action::Value(1, 0, v));
            kResultOk
        }
        unsafe fn endEdit(&self, _: u32) -> tresult {
            self.actions.borrow_mut().push(Action::End);
            kResultOk
        }
        unsafe fn restartComponent(&self, _: i32) -> tresult {
            kResultOk
        }
    }
    #[test]
    fn callback_survives_final_timer_release_and_closes_its_host_gesture() {
        unsafe {
            let mailbox = Mailbox::new(rvst_adapter::ParameterMirror::new(
                super::super::tests::fixture::Gain::descriptor(),
            ));
            let actions = Rc::new(RefCell::new(Vec::new()));
            let timer = Rc::new(RefCell::new(None));
            let destroyed = Rc::new(Cell::new(false));
            let host = ComWrapper::new(Host {
                actions: actions.clone(),
                mailbox: mailbox.clone(),
                timer: timer.clone(),
                destroyed: destroyed.clone(),
            })
            .to_com_ptr::<IComponentHandler>()
            .unwrap();
            retain(host.as_ptr() as _);
            let native = ComWrapper::new(Timer {
                state: State::new(mailbox.clone(), Handler(host.as_ptr())),
            })
            .to_com_ptr::<ITimerHandler>()
            .unwrap();
            drop(host);
            let raw = native.as_ptr();
            *timer.borrow_mut() = Some(native);
            mailbox.begin(42);
            mailbox.value(42, 0.75);
            ComRef::from_raw(raw).unwrap().onTimer();
            assert!(timer.borrow().is_none());
            assert!(destroyed.get());
            assert_eq!(
                *actions.borrow(),
                vec![Action::Begin, Action::Value(1, 0, 0.75), Action::End]
            );
        }
    }
}
