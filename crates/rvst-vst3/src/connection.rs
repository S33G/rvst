//! Per-instance component/controller association. Messages contain opaque IDs,
//! never pointers. A proxy in another process safely falls back to normal VST3
//! parameter/state behavior when its local weak registry has no matching entry.
use super::*;
use rvst_adapter::ParameterMirror;
use std::{
    collections::HashMap,
    ffi::CStr,
    sync::{Arc, Mutex, OnceLock, Weak, atomic::AtomicU64},
};
type Token = [u8; 16];
fn registry() -> &'static Mutex<HashMap<Token, Weak<ParameterMirror>>> {
    static R: OnceLock<Mutex<HashMap<Token, Weak<ParameterMirror>>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}
pub(super) struct Registration {
    pub(super) token: Token,
}
impl Registration {
    pub(super) fn new(mirror: &Arc<ParameterMirror>) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let mut token = [0; 16];
        let epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        token[..8].copy_from_slice(&(epoch ^ (std::process::id() as u64)).to_le_bytes());
        token[8..].copy_from_slice(&NEXT.fetch_add(1, Ordering::Relaxed).to_le_bytes());
        registry()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(token, Arc::downgrade(mirror));
        Self { token }
    }
}
impl Drop for Registration {
    fn drop(&mut self) {
        registry()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.token);
    }
}
#[derive(Default)]
pub(super) struct Endpoint(Cell<*mut IConnectionPoint>);
impl Endpoint {
    unsafe fn set(&self, peer: *mut IConnectionPoint) -> tresult {
        if peer.is_null() {
            return kInvalidArgument;
        }
        if !self.0.get().is_null() {
            return kResultFalse;
        }
        retain(peer as _);
        self.0.set(peer);
        kResultOk
    }
    unsafe fn clear(&self, peer: *mut IConnectionPoint) -> tresult {
        if self.0.get() != peer {
            return kResultFalse;
        }
        self.0.set(ptr::null_mut());
        if !peer.is_null() {
            release(peer as _);
        }
        kResultOk
    }
    unsafe fn send(&self, token: Option<Token>) -> tresult {
        let Some(peer) = ComRef::from_raw(self.0.get()) else {
            return kResultFalse;
        };
        let attrs = ComWrapper::new(Attributes {
            token: token.unwrap_or([0; 16]),
        })
        .to_com_ptr::<IAttributeList>()
        .unwrap();
        let message = ComWrapper::new(Message {
            attributes: attrs,
            kind: if token.is_some() {
                c"rvst.context.v1"
            } else {
                c"rvst.context.request.v1"
            },
        })
        .to_com_ptr::<IMessage>()
        .unwrap();
        peer.notify(message.as_ptr())
    }
}
impl Drop for Endpoint {
    fn drop(&mut self) {
        unsafe {
            let p = self.0.replace(ptr::null_mut());
            if !p.is_null() {
                release(p as _);
            }
        }
    }
}
impl<P: Plugin> IConnectionPointTrait for Processor<P> {
    unsafe fn connect(&self, other: *mut IConnectionPoint) -> tresult {
        boundary(kInternalError, || {
            let result = self.connection.set(other);
            if result == kResultOk {
                let _ = self.connection.send(Some(self.registration.token));
            }
            result
        })
    }
    unsafe fn disconnect(&self, other: *mut IConnectionPoint) -> tresult {
        boundary(kInternalError, || self.connection.clear(other))
    }
    unsafe fn notify(&self, message: *mut IMessage) -> tresult {
        boundary(kInternalError, || {
            let Some(m) = ComRef::from_raw(message) else {
                return kInvalidArgument;
            };
            let id = m.getMessageID();
            if !id.is_null() && CStr::from_ptr(id) == c"rvst.context.request.v1" {
                self.connection.send(Some(self.registration.token))
            } else {
                kResultFalse
            }
        })
    }
}
impl<P: Plugin> IConnectionPointTrait for Controller<P> {
    unsafe fn connect(&self, other: *mut IConnectionPoint) -> tresult {
        boundary(kInternalError, || {
            let result = self.connection.set(other);
            if result == kResultOk {
                let _ = self.connection.send(None);
            }
            result
        })
    }
    unsafe fn disconnect(&self, other: *mut IConnectionPoint) -> tresult {
        boundary(kInternalError, || {
            let result = self.connection.clear(other);
            if result == kResultOk {
                *self.linked.get() = None;
                self.restart_timer();
            }
            result
        })
    }
    unsafe fn notify(&self, message: *mut IMessage) -> tresult {
        boundary(kInternalError, || {
            let Some(m) = ComRef::from_raw(message) else {
                return kInvalidArgument;
            };
            let id = m.getMessageID();
            if id.is_null() || CStr::from_ptr(id) != c"rvst.context.v1" {
                return kResultFalse;
            }
            let Some(attributes) = ComRef::from_raw(m.getAttributes()) else {
                return kResultFalse;
            };
            let mut data = ptr::null();
            let mut size = 0;
            if attributes.getBinary(c"instance".as_ptr(), &mut data, &mut size) != kResultOk
                || size != 16
                || data.is_null()
            {
                return kResultFalse;
            }
            let token = ptr::read_unaligned(data as *const Token);
            let mirror = registry()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&token)
                .and_then(Weak::upgrade);
            if let Some(mirror) = mirror {
                if mirror.descriptor.id != P::descriptor().id {
                    return kResultFalse;
                }
                *self.linked.get() = Some(mirror);
                self.restart_timer();
                kResultOk
            } else {
                kResultFalse
            }
        })
    }
}
struct Message {
    attributes: vst3::ComPtr<IAttributeList>,
    kind: &'static CStr,
}
impl Class for Message {
    type Interfaces = (IMessage,);
}
impl IMessageTrait for Message {
    unsafe fn getMessageID(&self) -> FIDString {
        self.kind.as_ptr()
    }
    unsafe fn setMessageID(&self, _: FIDString) {}
    unsafe fn getAttributes(&self) -> *mut IAttributeList {
        self.attributes.as_ptr()
    }
}
struct Attributes {
    token: Token,
}
impl Class for Attributes {
    type Interfaces = (IAttributeList,);
}
impl IAttributeListTrait for Attributes {
    unsafe fn setInt(&self, _: *const c_char, _: i64) -> tresult {
        kNotImplemented
    }
    unsafe fn getInt(&self, _: *const c_char, _: *mut i64) -> tresult {
        kResultFalse
    }
    unsafe fn setFloat(&self, _: *const c_char, _: f64) -> tresult {
        kNotImplemented
    }
    unsafe fn getFloat(&self, _: *const c_char, _: *mut f64) -> tresult {
        kResultFalse
    }
    unsafe fn setString(&self, _: *const c_char, _: *const TChar) -> tresult {
        kNotImplemented
    }
    unsafe fn getString(&self, _: *const c_char, _: *mut TChar, _: u32) -> tresult {
        kResultFalse
    }
    unsafe fn setBinary(&self, _: *const c_char, _: *const c_void, _: u32) -> tresult {
        kNotImplemented
    }
    unsafe fn getBinary(
        &self,
        id: *const c_char,
        data: *mut *const c_void,
        size: *mut u32,
    ) -> tresult {
        if id.is_null() || data.is_null() || size.is_null() || CStr::from_ptr(id) != c"instance" {
            return kInvalidArgument;
        }
        *data = self.token.as_ptr() as _;
        *size = 16;
        kResultOk
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::fixture::Gain;
    fn controller() -> ComWrapper<Controller<Gain>> {
        ComWrapper::new(Controller {
            mirror: ParameterMirror::new(Gain::descriptor()),
            plugin: UnsafeCell::new(rvst_adapter::DropShield::new(Gain::new())),
            handler: Cell::new(ptr::null_mut()),
            host_context: Cell::new(ptr::null_mut()),
            timer: UnsafeCell::new(None),
            editor: rvst_ui::default_editor,
            connection: Endpoint::default(),
            linked: UnsafeCell::new(None),
        })
    }
    #[test]
    fn exact_instance_handshake_disconnect_and_remote_fallback() {
        unsafe {
            let a = Processor::<Gain>::new();
            (&*a.runtime.get()).mirror.publish(42, 0.25);
            let at = a.registration.token;
            let a_context = (&*a.runtime.get()).mirror.editor_context.clone().unwrap();
            let a = ComWrapper::new(a).to_com_ptr::<IConnectionPoint>().unwrap();
            let b = Processor::<Gain>::new();
            (&*b.runtime.get()).mirror.publish(42, 1.5);
            let b = ComWrapper::new(b).to_com_ptr::<IConnectionPoint>().unwrap();
            let ca_owner = controller();
            let ca_edit = ca_owner.to_com_ptr::<IEditController>().unwrap();
            let ca = ca_owner.to_com_ptr::<IConnectionPoint>().unwrap();
            let cb = controller();
            let cb_edit = cb.to_com_ptr::<IEditController>().unwrap();
            let cb = cb.to_com_ptr::<IConnectionPoint>().unwrap();
            assert_eq!(ca.connect(a.as_ptr()), kResultOk);
            assert_eq!(b.connect(cb.as_ptr()), kResultOk);
            assert_eq!(a.connect(ca.as_ptr()), kResultOk);
            assert_eq!(cb.connect(b.as_ptr()), kResultOk);
            assert_eq!(ca_edit.getParamNormalized(42), 0.125);
            let linked_context = ca_owner.editor_mirror().editor_context.clone().unwrap();
            assert!(Arc::ptr_eq(&a_context, &linked_context));
            assert!(ca_owner.editor_mirror().inspector_url.is_some() == cfg!(feature = "devtools"));
            assert_eq!(cb_edit.getParamNormalized(42), 0.75);
            assert_eq!(ca.disconnect(a.as_ptr()), kResultOk);
            assert_eq!(a.disconnect(ca.as_ptr()), kResultOk);
            assert_eq!(ca_edit.getParamNormalized(42), 0.5);
            assert_eq!(cb_edit.getParamNormalized(42), 0.75);
            assert_eq!(cb.disconnect(b.as_ptr()), kResultOk);
            assert_eq!(b.disconnect(cb.as_ptr()), kResultOk);
            drop(a);
            assert!(registry().lock().unwrap().get(&at).is_none());
            let attrs = ComWrapper::new(Attributes { token: [0xff; 16] })
                .to_com_ptr::<IAttributeList>()
                .unwrap();
            let message = ComWrapper::new(Message {
                attributes: attrs,
                kind: c"rvst.context.v1",
            })
            .to_com_ptr::<IMessage>()
            .unwrap();
            assert_eq!(ca.notify(message.as_ptr()), kResultFalse);
        }
    }
}
