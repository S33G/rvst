//! Shared history for editor actions and native project persistence.
use crate::{EditorContext, EditorHost, Error, PluginDescriptor};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
    },
};

const MAX_ENTRIES: usize = 128;
const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_LABEL_BYTES: usize = 256;
const RESTORING: u64 = 1 << 63;

mod persistence;
pub use persistence::{HistorySnapshot, RestoredHistory};

/// Instance-owned undo storage. Clones share one bounded history. Native hosts
/// can persist validated snapshots alongside their project state.
/// Only [`Self::invalidate`] is realtime-safe; all other history work happens in
/// [`HistoryHost`] on an editor/control thread. At most 128 entries and 8 MiB of
/// entry data are retained across the undo and redo stacks together.
#[derive(Clone, Default)]
pub struct EditHistory(Arc<Shared>);

#[derive(Default)]
struct Shared {
    generation: AtomicU64,
    snapshot_revision: AtomicU64,
    semantic_revision: AtomicU64,
    activity: AtomicU64,
    next_owner: AtomicU64,
    state: Mutex<History>,
}

#[derive(Clone, Default)]
struct History {
    generation: u64,
    revision: u64,
    undo: VecDeque<Entry>,
    redo: Vec<Entry>,
    bytes: usize,
    pending: Vec<Pending>,
}

#[derive(Clone)]
struct Pending {
    owner: u64,
    order: u64,
    entry: Option<Entry>,
}

/// Excludes actual UI host mutations during a native project restore. An open,
/// paused gesture does not prevent acquiring this guard. Dropping the guard
/// without installing state preserves history; UI value mutations blocked while
/// it is held are not forwarded. Editor destruction may still send terminal
/// `end_edit` notifications on the original owner thread to balance gestures.
pub struct HistoryRestoreGuard(EditHistory);

impl Drop for HistoryRestoreGuard {
    fn drop(&mut self) {
        self.0.0.snapshot_revision.fetch_add(1, Ordering::AcqRel);
        self.0.0.activity.fetch_and(!RESTORING, Ordering::AcqRel);
    }
}

struct ActivityGuard(EditHistory);
impl Drop for ActivityGuard {
    fn drop(&mut self) {
        self.0.0.snapshot_revision.fetch_add(1, Ordering::AcqRel);
        self.0.0.activity.fetch_sub(1, Ordering::AcqRel);
    }
}

impl EditHistory {
    /// Invalidate past actions after successful external state restoration.
    /// This only changes an atomic: no allocation, destruction, or locking.
    /// The next editor operation releases old entries and abandons open edits.
    pub fn invalidate(&self) {
        self.0.generation.fetch_add(1, Ordering::AcqRel);
        self.0.snapshot_revision.fetch_add(1, Ordering::AcqRel);
    }

    /// Exclude UI mutations while loading plugin state and installing the
    /// matching validated history. Busy callers may retry outside audio.
    pub fn try_restore(&self) -> Result<HistoryRestoreGuard, Error> {
        self.0
            .activity
            .compare_exchange(0, RESTORING, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| Error::from("editor action in progress"))?;
        self.0.snapshot_revision.fetch_add(1, Ordering::AcqRel);
        Ok(HistoryRestoreGuard(self.clone()))
    }

    fn begin_mutation(&self) -> Option<ActivityGuard> {
        let mut activity = self.0.activity.load(Ordering::Acquire);
        loop {
            if activity & RESTORING != 0 {
                return None;
            }
            match self.0.activity.compare_exchange_weak(
                activity,
                activity + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    self.0.snapshot_revision.fetch_add(1, Ordering::AcqRel);
                    return Some(ActivityGuard(self.clone()));
                }
                Err(current) => activity = current,
            }
        }
    }

    fn begin_teardown(&self) -> ActivityGuard {
        // Closing an editor cannot postpone host-affine gesture notifications
        // to a potentially different native state-loading thread. Allow only
        // terminal notifications during restore, while keeping snapshots and
        // new restores excluded until this teardown completes.
        self.0.activity.fetch_add(1, Ordering::AcqRel);
        self.0.snapshot_revision.fetch_add(1, Ordering::AcqRel);
        ActivityGuard(self.clone())
    }

    fn generation(&self) -> u64 {
        self.0.generation.load(Ordering::Acquire)
    }

    fn lock(&self) -> MutexGuard<'_, History> {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        let generation = self.generation();
        if state.generation != generation {
            state.clear();
            state.generation = generation;
        }
        state
    }
}

impl History {
    fn clear(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.undo.clear();
        self.redo.clear();
        self.pending.clear();
        self.bytes = 0;
    }

    fn clear_redo(&mut self) {
        self.bytes -= self.redo.iter().map(Entry::bytes).sum::<usize>();
        self.redo.clear();
    }

    fn push(&mut self, entry: Entry) {
        self.revision = self.revision.wrapping_add(1);
        self.clear_redo();
        let bytes = entry.bytes();
        if bytes > MAX_BYTES {
            // An unrecordable action is a boundary: old history cannot cross it.
            self.clear();
            return;
        }
        while self.undo.len() >= MAX_ENTRIES || self.bytes + bytes > MAX_BYTES {
            let Some(oldest) = self.undo.pop_front() else {
                break;
            };
            self.bytes -= oldest.bytes();
        }
        self.bytes += bytes;
        self.undo.push_back(entry);
    }
}

#[derive(Clone)]
struct Change {
    id: u32,
    before: f64,
    after: f64,
}

#[derive(Clone)]
enum Action {
    Parameters(Vec<Change>),
    State { before: Vec<u8>, after: Vec<u8> },
}

#[derive(Clone)]
struct Entry {
    label: String,
    action: Action,
}

impl Entry {
    fn bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.label.capacity()
            + match &self.action {
                Action::Parameters(changes) => changes.capacity() * std::mem::size_of::<Change>(),
                Action::State { before, after } => before.capacity() + after.capacity(),
            }
    }
}

struct Gesture {
    id: u32,
    before: f64,
    deferred_end: bool,
}

/// Records UI actions while forwarding the original host's normal edit path.
/// Keep one wrapper alive across frames. Its inner host may return a shared
/// [`EditHistory`] so completed history survives closing/reopening the editor.
///
/// Parameter undo touches only parameters explicitly edited by the UI. Host
/// automation published outside this wrapper is never recorded. Preset loads
/// retain opaque before/after state, so plugin-owned non-parameter data survives
/// undo too. A successful preset that cannot be snapshotted within the storage
/// budget remains successful and clears history; a failed load preserves it.
/// Successful preset loads finish open parameter gestures/groups first, then
/// become a separate undo step.
pub struct HistoryHost<H: EditorHost> {
    inner: H,
    history: EditHistory,
    generation: u64,
    active: Vec<Gesture>,
    changes: Vec<Change>,
    group_depth: usize,
    label: Option<String>,
    owner: u64,
    deferred_group_ends: usize,
}

impl<H: EditorHost> HistoryHost<H> {
    pub fn new(inner: H) -> Self {
        let history = inner.edit_history().unwrap_or_default();
        let generation = history.generation();
        let owner = history.0.next_owner.fetch_add(1, Ordering::Relaxed);
        Self {
            inner,
            history,
            generation,
            active: Vec::new(),
            changes: Vec::new(),
            group_depth: 0,
            label: None,
            owner,
            deferred_group_ends: 0,
        }
    }

    fn publish_pending(&self) {
        let changes: Vec<_> = self
            .changes
            .iter()
            .filter(|change| change.before != change.after)
            .cloned()
            .collect();
        let mut history = self.history.lock();
        history
            .pending
            .retain(|pending| pending.owner != self.owner);
        if history.generation == self.generation
            && (!changes.is_empty() || !self.active.is_empty() || self.group_depth != 0)
        {
            let entry = (!changes.is_empty()).then(|| Entry {
                label: self.label.clone().unwrap_or_else(|| "Parameters".into()),
                action: Action::Parameters(changes),
            });
            history.pending.push(Pending {
                owner: self.owner,
                order: self
                    .history
                    .0
                    .snapshot_revision
                    .fetch_add(1, Ordering::AcqRel),
                entry,
            });
        }
    }

    fn foreign_pending(&self) -> bool {
        self.history
            .lock()
            .pending
            .iter()
            .any(|pending| pending.owner != self.owner)
    }

    fn synchronize(&mut self) {
        let generation = self.history.generation();
        if generation != self.generation {
            self.generation = generation;
            self.changes.clear();
            self.group_depth = 0;
            self.deferred_group_ends = 0;
            self.label = None;
            // No history lock spans a host call, including gesture teardown.
            for gesture in self.active.drain(..) {
                self.inner.end_edit(gesture.id);
            }
        }
        // A release can arrive while native state loading excludes UI calls.
        // If that load failed, preserve the existing transaction but complete
        // its queued releases before starting another user action.
        let deferred =
            self.deferred_group_ends != 0 || self.active.iter().any(|gesture| gesture.deferred_end);
        if deferred {
            self.group_depth = self.group_depth.saturating_sub(self.deferred_group_ends);
            self.deferred_group_ends = 0;
            let mut index = 0;
            while index < self.active.len() {
                if self.active[index].deferred_end {
                    let gesture = self.active.remove(index);
                    self.inner.end_edit(gesture.id);
                } else {
                    index += 1;
                }
            }
            if self.history.generation() != self.generation {
                self.synchronize();
            } else if self.active.is_empty() && self.group_depth == 0 {
                self.commit();
            }
        }
    }

    fn commit(&mut self) {
        self.changes.retain(|change| change.before != change.after);
        if !self.changes.is_empty() {
            let entry = Entry {
                label: self.label.take().unwrap_or_else(|| "Parameters".into()),
                action: Action::Parameters(std::mem::take(&mut self.changes)),
            };
            self.push(entry);
        }
        self.label = None;
    }

    fn push(&mut self, entry: Entry) {
        if entry.bytes() > MAX_BYTES {
            self.unrecordable_boundary();
            return;
        }
        let mut history = self.history.lock();
        if history.generation == self.generation {
            history.push(entry);
        }
    }

    fn unrecordable_boundary(&mut self) {
        // Other open editors must abandon in-flight edits at this boundary too.
        self.history.invalidate();
        self.synchronize();
        drop(self.history.lock());
    }

    /// Close open host gestures and commit any completed changes. Called on
    /// drop as well; an interrupted drag remains undoable after reopening.
    pub fn finish_edits(&mut self) {
        let Some(_activity) = self.history.begin_mutation() else {
            for gesture in &mut self.active {
                gesture.deferred_end = true;
            }
            self.deferred_group_ends = self.group_depth;
            return;
        };
        self.synchronize();
        for gesture in self.active.drain(..) {
            self.inner.end_edit(gesture.id);
        }
        self.synchronize();
        self.group_depth = 0;
        self.commit();
        self.publish_pending();
    }

    fn replay(&mut self, redo: bool) -> Result<bool, Error> {
        if self.foreign_pending() {
            return Ok(false);
        }
        self.finish_edits();
        let (entry, generation, revision) = {
            let mut history = self.history.lock();
            let entry = if redo {
                history.redo.pop()
            } else {
                history.undo.pop_back()
            };
            if let Some(entry) = &entry {
                history.bytes -= entry.bytes();
                history.revision = history.revision.wrapping_add(1);
            }
            (entry, history.generation, history.revision)
        };
        let Some(entry) = entry else { return Ok(false) };
        let stamp = self.history.0.semantic_revision.load(Ordering::Acquire);
        let current = || {
            let history = self.history.lock();
            self.history.generation() == generation
                && self.history.0.semantic_revision.load(Ordering::Acquire) == stamp
                && history.revision == revision
                && !history
                    .pending
                    .iter()
                    .any(|pending| pending.owner != self.owner)
        };
        if self.history.generation() != generation {
            self.synchronize();
            return Ok(false);
        }
        let mut completed = true;
        let mut applied = false;
        let result = match &entry.action {
            Action::Parameters(changes) => {
                for change in changes {
                    if !current() {
                        completed = false;
                        break;
                    }
                    self.inner.begin_edit(change.id);
                    if current() {
                        applied = true;
                        self.inner.set_parameter(
                            change.id,
                            if redo { change.after } else { change.before },
                        );
                    } else {
                        completed = false;
                    }
                    self.inner.end_edit(change.id);
                    if !current() {
                        completed = false;
                        break;
                    }
                }
                Ok(())
            }
            Action::State { before, after } => {
                applied = true;
                self.inner.load_state(if redo { after } else { before })
            }
        };
        {
            let mut history = self.history.lock();
            // A reentrant edit in another view takes precedence. Reinserting
            // this older command would cross that action or exceed the budget.
            if history.generation == generation
                && history.revision == revision
                && self.history.0.semantic_revision.load(Ordering::Acquire) == stamp
            {
                history.bytes += entry.bytes();
                history.revision = history.revision.wrapping_add(1);
                if redo == (result.is_ok() && completed) {
                    history.undo.push_back(entry);
                } else {
                    history.redo.push(entry);
                }
            }
        }
        self.synchronize();
        result.map(|()| completed || applied)
    }
}

fn label(text: &str) -> String {
    let mut end = text.len().min(MAX_LABEL_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].into()
}

impl<H: EditorHost> Drop for HistoryHost<H> {
    fn drop(&mut self) {
        let _activity = self.history.begin_teardown();
        self.synchronize();
        for gesture in self.active.drain(..) {
            self.inner.end_edit(gesture.id);
        }
        self.synchronize();
        self.group_depth = 0;
        self.commit();
        self.publish_pending();
    }
}

impl<H: EditorHost> EditorHost for HistoryHost<H> {
    fn edit_history(&self) -> Option<EditHistory> {
        Some(self.history.clone())
    }
    fn editor_context(&self) -> Option<EditorContext> {
        self.inner.editor_context()
    }
    fn descriptor(&self) -> &'static PluginDescriptor {
        self.inner.descriptor()
    }
    fn parameter(&self, id: u32) -> Option<f64> {
        self.inner.parameter(id)
    }
    fn undo_label(&self) -> Option<String> {
        if self.foreign_pending() {
            return None;
        }
        if self.generation == self.history.generation()
            && self.changes.iter().any(|c| c.before != c.after)
        {
            return Some(self.label.clone().unwrap_or_else(|| "Parameters".into()));
        }
        self.history
            .lock()
            .undo
            .back()
            .map(|entry| entry.label.clone())
    }
    fn redo_label(&self) -> Option<String> {
        if self.foreign_pending() {
            return None;
        }
        if self.generation == self.history.generation()
            && self.changes.iter().any(|c| c.before != c.after)
        {
            return None;
        }
        self.history
            .lock()
            .redo
            .last()
            .map(|entry| entry.label.clone())
    }
    fn undo(&mut self) -> Result<bool, Error> {
        let _activity = self
            .history
            .begin_mutation()
            .ok_or_else(|| Error::from("project state restore in progress"))?;
        self.replay(false)
    }
    fn redo(&mut self) -> Result<bool, Error> {
        let _activity = self
            .history
            .begin_mutation()
            .ok_or_else(|| Error::from("project state restore in progress"))?;
        self.replay(true)
    }
    fn begin_edit_group(&mut self, text: &str) {
        let Some(_activity) = self.history.begin_mutation() else {
            return;
        };
        self.synchronize();
        if self.group_depth == 0 {
            self.label = Some(label(text));
        }
        self.group_depth = self.group_depth.saturating_add(1);
        self.publish_pending();
    }
    fn end_edit_group(&mut self) {
        let Some(_activity) = self.history.begin_mutation() else {
            self.deferred_group_ends = self
                .deferred_group_ends
                .saturating_add(1)
                .min(self.group_depth);
            return;
        };
        self.synchronize();
        self.group_depth = self.group_depth.saturating_sub(1);
        if self.group_depth == 0 && self.active.is_empty() {
            self.commit();
        }
        self.publish_pending();
    }
    fn begin_edit(&mut self, id: u32) {
        let Some(_activity) = self.history.begin_mutation() else {
            return;
        };
        self.synchronize();
        if self.active.iter().any(|gesture| gesture.id == id) {
            return;
        }
        if let Some(before) = self.inner.parameter(id) {
            self.active.push(Gesture {
                id,
                before,
                deferred_end: false,
            });
            self.inner.begin_edit(id);
            self.synchronize();
        }
        self.publish_pending();
    }
    fn set_parameter(&mut self, id: u32, value: f64) {
        let Some(_activity) = self.history.begin_mutation() else {
            return;
        };
        self.synchronize();
        let generation = self.generation;
        let current = self.inner.parameter(id);
        let before = self
            .active
            .iter()
            .find(|gesture| gesture.id == id)
            .map(|gesture| gesture.before)
            .or(current);
        self.inner.set_parameter(id, value);
        let after = self.inner.parameter(id);
        self.synchronize();
        if generation != self.generation {
            self.publish_pending();
            return;
        }
        if let (Some(before), Some(after)) = (before, after)
            && current != Some(after)
        {
            self.history
                .0
                .semantic_revision
                .fetch_add(1, Ordering::AcqRel);
            if let Some(change) = self.changes.iter_mut().find(|change| change.id == id) {
                change.after = after;
            } else {
                self.changes.push(Change { id, before, after });
            }
            if self.label.is_none() {
                self.label = Some(label(
                    self.descriptor()
                        .parameter(id)
                        .map_or("Parameters", |p| p.name),
                ));
            }
        }
        if self.active.is_empty() && self.group_depth == 0 {
            self.commit();
        }
        self.publish_pending();
    }
    fn end_edit(&mut self, id: u32) {
        let Some(_activity) = self.history.begin_mutation() else {
            if let Some(gesture) = self.active.iter_mut().find(|gesture| gesture.id == id) {
                gesture.deferred_end = true;
            }
            return;
        };
        self.synchronize();
        if let Some(index) = self.active.iter().position(|gesture| gesture.id == id) {
            self.active.remove(index);
            self.inner.end_edit(id);
            self.synchronize();
        }
        if self.active.is_empty() && self.group_depth == 0 {
            self.commit();
        }
        self.publish_pending();
    }
    fn save_state(&mut self) -> Result<Vec<u8>, Error> {
        let _activity = self
            .history
            .begin_mutation()
            .ok_or_else(|| Error::from("project state restore in progress"))?;
        self.inner.save_state()
    }
    fn load_state(&mut self, data: &[u8]) -> Result<(), Error> {
        let _activity = self
            .history
            .begin_mutation()
            .ok_or_else(|| Error::from("project state restore in progress"))?;
        self.synchronize();
        let before = if data.len() <= MAX_BYTES {
            self.inner.save_state().ok()
        } else {
            None
        };
        // Preserve both completed and open edits when the load fails.
        self.inner.load_state(data)?;
        self.finish_edits();
        // Accepted input is a valid redo snapshot even if serializing the newly
        // loaded state fails. Never report a failed load after it has succeeded.
        if let Some(before) = before {
            let after = self.inner.save_state().unwrap_or_else(|_| data.to_vec());
            if before == after {
                return Ok(());
            }
            self.push(Entry {
                label: "Preset".into(),
                action: Action::State { before, after },
            });
        } else {
            self.unrecordable_boundary();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ParameterSpec, PluginDescriptor, PluginKind};
    use std::sync::{Arc, Mutex};

    static PARAMETERS: [ParameterSpec; 2] = [
        ParameterSpec {
            id: 1,
            name: "Gain",
            unit: "",
            min: 0.0,
            max: 1.0,
            default: 0.0,
            step: None,
            bypass: false,
        },
        ParameterSpec {
            id: 2,
            name: "Mix",
            unit: "",
            min: 0.0,
            max: 1.0,
            default: 0.0,
            step: None,
            bypass: false,
        },
    ];
    pub(super) static DESCRIPTOR: PluginDescriptor = PluginDescriptor {
        id: "test",
        name: "test",
        vendor: "test",
        version: "1",
        url: "",
        email: "",
        vst3_id: [1; 16],
        kind: PluginKind::Effect,
        parameters: &PARAMETERS,
        inputs: &[],
        outputs: &[],
        midi_input: false,
        midi_output: false,
        supports_f64: false,
    };
    #[derive(Default)]
    struct Values {
        parameters: [f64; 2],
        opaque: u8,
        calls: Vec<String>,
        fail_save: bool,
        fail_load: bool,
        fail_save_after_load: bool,
        padding: usize,
        invalidate_on_begin: bool,
        reentrant_edit_on_begin: bool,
        reentrant_open_on_begin: bool,
        restore_callback_on_end: bool,
        nested_restore_blocked: bool,
    }
    #[derive(Clone, Default)]
    pub(super) struct Host {
        values: Arc<Mutex<Values>>,
        pub(super) history: EditHistory,
    }
    impl Host {
        pub(super) fn calls(&self) -> Vec<String> {
            self.values.lock().unwrap().calls.clone()
        }
    }
    thread_local! {
        static OPEN_EDITOR: std::cell::RefCell<Option<HistoryHost<Host>>> = const { std::cell::RefCell::new(None) };
    }
    impl EditorHost for Host {
        fn edit_history(&self) -> Option<EditHistory> {
            Some(self.history.clone())
        }
        fn descriptor(&self) -> &'static PluginDescriptor {
            &DESCRIPTOR
        }
        fn parameter(&self, id: u32) -> Option<f64> {
            self.values
                .lock()
                .unwrap()
                .parameters
                .get(id.wrapping_sub(1) as usize)
                .copied()
        }
        fn begin_edit(&mut self, id: u32) {
            assert!(
                self.history.0.state.try_lock().is_ok(),
                "history mutex held over host call"
            );
            let mut values = self.values.lock().unwrap();
            values.calls.push(format!("begin {id}"));
            if values.invalidate_on_begin {
                self.history.invalidate();
            }
            if values.reentrant_edit_on_begin {
                values.reentrant_edit_on_begin = false;
                drop(values);
                HistoryHost::new(self.clone()).set_parameter(2, 0.25);
            } else if values.reentrant_open_on_begin {
                values.reentrant_open_on_begin = false;
                drop(values);
                let mut editor = HistoryHost::new(self.clone());
                editor.begin_edit(2);
                OPEN_EDITOR.with(|slot| *slot.borrow_mut() = Some(editor));
            }
        }
        fn set_parameter(&mut self, id: u32, value: f64) {
            let mut v = self.values.lock().unwrap();
            if let Some(parameter) = v.parameters.get_mut(id.wrapping_sub(1) as usize) {
                *parameter = value.clamp(0.0, 1.0);
            }
            v.calls.push(format!("set {id} {value}"));
        }
        fn end_edit(&mut self, id: u32) {
            assert!(
                self.history.0.state.try_lock().is_ok(),
                "history mutex held over host call"
            );
            let callback = {
                let mut values = self.values.lock().unwrap();
                values.calls.push(format!("end {id}"));
                std::mem::take(&mut values.restore_callback_on_end)
            };
            if callback {
                let blocked = self.history.try_restore().is_err();
                HistoryHost::new(self.clone()).set_parameter(2, 0.8);
                self.history.invalidate();
                self.values.lock().unwrap().nested_restore_blocked = blocked;
            }
        }
        fn save_state(&mut self) -> Result<Vec<u8>, Error> {
            let v = self.values.lock().unwrap();
            if v.fail_save {
                return Err("snapshot unavailable".into());
            }
            let mut bytes = Vec::from(v.parameters[0].to_le_bytes());
            bytes.extend_from_slice(&v.parameters[1].to_le_bytes());
            bytes.push(v.opaque);
            bytes.resize(17 + v.padding, 0);
            Ok(bytes)
        }
        fn load_state(&mut self, data: &[u8]) -> Result<(), Error> {
            assert!(
                self.history.0.state.try_lock().is_ok(),
                "history mutex held over host call"
            );
            if data.len() < 17 {
                return Err("bad preset".into());
            }
            let mut v = self.values.lock().unwrap();
            if v.fail_load {
                return Err("restore rejected".into());
            }
            v.parameters[0] = f64::from_le_bytes(data[..8].try_into().unwrap());
            v.parameters[1] = f64::from_le_bytes(data[8..16].try_into().unwrap());
            v.opaque = data[16];
            if v.fail_save_after_load {
                v.fail_save = true;
            }
            Ok(())
        }
    }
    pub(super) fn edit(host: &mut impl EditorHost, id: u32, value: f64) {
        host.begin_edit(id);
        host.set_parameter(id, value);
        host.end_edit(id);
    }
    #[test]
    fn a_drag_is_one_step_and_replay_forwards_balanced_gestures() {
        let raw = Host::default();
        let mut host = HistoryHost::new(raw.clone());
        host.begin_edit(1);
        host.set_parameter(1, 0.25);
        host.set_parameter(1, 0.75);
        host.end_edit(1);
        assert_eq!(host.undo_label().as_deref(), Some("Gain"));
        raw.values.lock().unwrap().calls.clear();
        assert!(host.undo().unwrap());
        assert_eq!(host.parameter(1), Some(0.0));
        assert_eq!(
            raw.values.lock().unwrap().calls,
            ["begin 1", "set 1 0", "end 1"]
        );
        assert!(!host.undo().unwrap());
        assert!(host.redo().unwrap());
        assert_eq!(host.parameter(1), Some(0.75));
    }
    #[test]
    fn groups_merge_repeated_parameters_and_preserve_unrelated_automation() {
        let raw = Host::default();
        let mut host = HistoryHost::new(raw.clone());
        host.begin_edit_group("Reset");
        edit(&mut host, 1, 0.2);
        edit(&mut host, 1, 0.4);
        host.end_edit_group();
        raw.values.lock().unwrap().parameters[1] = 0.9;
        assert_eq!(host.undo_label().as_deref(), Some("Reset"));
        assert!(host.undo().unwrap());
        assert_eq!(host.parameter(1), Some(0.0));
        assert_eq!(host.parameter(2), Some(0.9));
        assert!(!host.undo().unwrap());
        assert!(host.redo().unwrap());
        assert_eq!(host.parameter(1), Some(0.4));
    }
    #[test]
    fn noops_keep_redo_but_effective_new_edits_clear_it() {
        let mut host = HistoryHost::new(Host::default());
        edit(&mut host, 1, 0.5);
        host.undo().unwrap();
        edit(&mut host, 1, 0.0);
        assert!(host.redo_label().is_some());
        edit(&mut host, 2, 0.5);
        assert!(host.redo_label().is_none());
        assert!(host.undo().unwrap());
        assert!(!host.undo().unwrap());
    }
    #[test]
    fn drag_returning_to_start_preserves_redo() {
        let mut host = HistoryHost::new(Host::default());
        edit(&mut host, 1, 0.5);
        host.undo().unwrap();
        host.begin_edit(2);
        host.set_parameter(2, 0.5);
        assert!(host.redo_label().is_none());
        host.set_parameter(2, 0.0);
        host.end_edit(2);
        assert!(host.undo_label().is_none());
        assert!(host.redo_label().is_some());
        assert!(host.redo().unwrap());
        assert_eq!(host.parameter(1), Some(0.5));
    }
    #[test]
    fn opaque_preset_roundtrips_and_invalid_presets_leave_history_intact() {
        let raw = Host::default();
        let mut host = HistoryHost::new(raw.clone());
        let mut preset = host.save_state().unwrap();
        preset[16] = 42;
        host.load_state(&preset).unwrap();
        assert_eq!(raw.values.lock().unwrap().opaque, 42);
        assert!(host.load_state(&[]).is_err());
        assert!(host.undo().unwrap());
        assert_eq!(raw.values.lock().unwrap().opaque, 0);
        assert!(host.redo().unwrap());
        assert_eq!(raw.values.lock().unwrap().opaque, 42);
    }
    #[test]
    fn successful_unrecordable_preset_is_a_history_boundary() {
        let raw = Host::default();
        let mut host = HistoryHost::new(raw.clone());
        edit(&mut host, 1, 0.5);
        let mut preset = host.save_state().unwrap();
        preset[16] = 42;
        raw.values.lock().unwrap().fail_save = true;
        host.load_state(&preset).unwrap();
        assert_eq!(raw.values.lock().unwrap().opaque, 42);
        assert!(!host.undo().unwrap());
        raw.values.lock().unwrap().fail_save = false;
        edit(&mut host, 1, 0.75);
        preset.resize(9 * 1024 * 1024, 0);
        host.load_state(&preset).unwrap();
        assert!(!host.undo().unwrap());
    }
    #[test]
    fn window_close_finishes_drag_and_reopen_reuses_instance_history() {
        let raw = Host::default();
        {
            let mut host = HistoryHost::new(raw.clone());
            host.begin_edit(1);
            host.set_parameter(1, 0.5);
        }
        assert_eq!(raw.values.lock().unwrap().calls.last().unwrap(), "end 1");
        let mut host = HistoryHost::new(raw);
        assert!(host.undo().unwrap());
        assert_eq!(host.parameter(1), Some(0.0));
        assert!(!HistoryHost::new(Host::default()).undo().unwrap());
    }
    #[test]
    fn invalidate_discards_inflight_group_and_old_state_even_across_reopen() {
        let raw = Host::default();
        let mut host = HistoryHost::new(raw.clone());
        host.begin_edit_group("Group");
        host.begin_edit(1);
        host.set_parameter(1, 0.5);
        raw.history.invalidate();
        raw.values.lock().unwrap().parameters[0] = 0.8;
        host.end_edit(1);
        host.end_edit_group();
        assert!(!host.undo().unwrap());
        drop(host);
        let mut host = HistoryHost::new(raw);
        edit(&mut host, 1, 0.9);
        host.undo().unwrap();
        assert_eq!(host.parameter(1), Some(0.8));
    }
    #[test]
    fn history_evicts_oldest_entries_at_capacity() {
        let mut host = HistoryHost::new(Host::default());
        for i in 1..=130 {
            edit(&mut host, 1, f64::from(i) / 200.0);
        }
        let mut count = 0;
        while host.undo().unwrap() {
            count += 1;
        }
        assert_eq!(count, 128);
        assert_eq!(host.parameter(1), Some(0.01));
    }
    #[test]
    fn nested_groups_and_standalone_sets_are_single_transactions() {
        let mut host = HistoryHost::new(Host::default());
        host.begin_edit_group("All controls");
        edit(&mut host, 1, 0.3);
        host.begin_edit_group("Inner label");
        host.set_parameter(2, 0.6);
        host.end_edit_group();
        edit(&mut host, 1, 0.7);
        host.end_edit_group();
        assert_eq!(host.undo_label().as_deref(), Some("All controls"));
        host.undo().unwrap();
        assert_eq!(host.parameter(1), Some(0.0));
        assert_eq!(host.parameter(2), Some(0.0));
        assert!(!host.undo().unwrap());
        host.redo().unwrap();
        assert_eq!(host.parameter(1), Some(0.7));
        assert_eq!(host.parameter(2), Some(0.6));
        host.set_parameter(1, 0.9);
        host.undo().unwrap();
        assert_eq!(host.parameter(1), Some(0.7));
    }
    #[test]
    fn state_undo_failure_keeps_entry_retryable_and_stacks_ordered() {
        let raw = Host::default();
        let mut host = HistoryHost::new(raw.clone());
        edit(&mut host, 1, 0.5);
        let mut preset = host.save_state().unwrap();
        preset[16] = 42;
        host.load_state(&preset).unwrap();
        raw.values.lock().unwrap().fail_load = true;
        assert!(host.undo().is_err());
        assert_eq!(host.undo_label().as_deref(), Some("Preset"));
        assert!(host.redo_label().is_none());
        assert_eq!(raw.values.lock().unwrap().opaque, 42);
        raw.values.lock().unwrap().fail_load = false;
        assert!(host.undo().unwrap());
        assert_eq!(raw.values.lock().unwrap().opaque, 0);
        assert_eq!(host.parameter(1), Some(0.5));
        assert!(host.undo().unwrap());
        assert_eq!(host.parameter(1), Some(0.0));
        assert!(host.redo().unwrap());
        assert!(host.redo().unwrap());
        assert_eq!(host.parameter(1), Some(0.5));
        assert_eq!(raw.values.lock().unwrap().opaque, 42);
    }
    #[test]
    fn failed_load_preserves_open_edit_group_and_redo() {
        let mut host = HistoryHost::new(Host::default());
        edit(&mut host, 2, 0.5);
        host.undo().unwrap();
        assert!(host.load_state(&[]).is_err());
        assert!(host.redo_label().is_some());
        host.begin_edit_group("Gesture");
        host.begin_edit(1);
        host.set_parameter(1, 0.2);
        assert!(host.load_state(&[]).is_err());
        host.set_parameter(1, 0.4);
        host.end_edit(1);
        host.end_edit_group();
        assert_eq!(host.undo_label().as_deref(), Some("Gesture"));
        host.undo().unwrap();
        assert_eq!(host.parameter(1), Some(0.0));
        assert!(!host.undo().unwrap());
    }
    #[test]
    fn accepted_input_supplies_redo_when_post_load_serialization_fails() {
        let raw = Host::default();
        let mut host = HistoryHost::new(raw.clone());
        let mut preset = host.save_state().unwrap();
        preset[16] = 42;
        raw.values.lock().unwrap().fail_save_after_load = true;
        host.load_state(&preset).unwrap();
        assert!(host.undo().unwrap());
        assert_eq!(raw.values.lock().unwrap().opaque, 0);
        assert!(host.redo().unwrap());
        assert_eq!(raw.values.lock().unwrap().opaque, 42);
    }
    #[test]
    fn byte_budget_evicts_old_state_entries() {
        let raw = Host::default();
        raw.values.lock().unwrap().padding = 1024 * 1024;
        let mut host = HistoryHost::new(raw.clone());
        let mut preset = host.save_state().unwrap();
        for value in 1..=5 {
            preset[16] = value;
            host.load_state(&preset).unwrap();
        }
        let history = raw.history.lock();
        assert!(history.bytes <= MAX_BYTES);
        assert!(history.undo.len() < 5);
        let retained = history.undo.len();
        drop(history);
        let mut count = 0;
        while host.undo().unwrap() {
            count += 1;
        }
        assert_eq!(count, retained);
        assert!(count > 0);
    }
    #[test]
    fn invalidation_is_atomic_only_and_clears_lazily() {
        let raw = Host::default();
        let mut host = HistoryHost::new(raw.clone());
        edit(&mut host, 1, 0.5);
        let history = raw.history.0.state.lock().unwrap();
        raw.history.invalidate(); // Would deadlock if invalidation acquired the history mutex.
        assert_eq!(history.undo.len(), 1);
        drop(history);
        assert!(host.undo_label().is_none());
        assert!(raw.history.lock().undo.is_empty());
    }
    #[test]
    fn replay_stops_if_a_host_callback_restores_external_state() {
        let raw = Host::default();
        let mut host = HistoryHost::new(raw.clone());
        host.begin_edit_group("Both");
        edit(&mut host, 1, 0.5);
        edit(&mut host, 2, 0.7);
        host.end_edit_group();
        raw.values.lock().unwrap().invalidate_on_begin = true;
        host.undo().unwrap();
        assert_eq!(host.parameter(1), Some(0.5));
        assert_eq!(host.parameter(2), Some(0.7));
        assert!(host.undo_label().is_none());
        assert!(host.redo_label().is_none());
    }
    #[test]
    fn reentrant_edit_does_not_reinsert_stale_redo() {
        let raw = Host::default();
        let mut host = HistoryHost::new(raw.clone());
        edit(&mut host, 1, 0.5);
        raw.values.lock().unwrap().reentrant_edit_on_begin = true;
        host.undo().unwrap();
        assert_eq!(host.parameter(1), Some(0.5));
        assert_eq!(host.parameter(2), Some(0.25));
        assert_eq!(host.undo_label().as_deref(), Some("Mix"));
        assert!(host.redo_label().is_none());
        host.undo().unwrap();
        assert_eq!(host.parameter(2), Some(0.0));
    }
    #[test]
    fn grouped_replay_stops_before_overwriting_reentrant_edit() {
        let raw = Host::default();
        let mut host = HistoryHost::new(raw.clone());
        host.begin_edit_group("Both");
        edit(&mut host, 1, 0.5);
        edit(&mut host, 2, 0.7);
        host.end_edit_group();
        raw.values.lock().unwrap().reentrant_edit_on_begin = true;
        host.undo().unwrap();
        assert_eq!(host.parameter(1), Some(0.5));
        assert_eq!(host.parameter(2), Some(0.25));
        assert_eq!(host.undo_label().as_deref(), Some("Mix"));
        host.undo().unwrap();
        assert_eq!(host.parameter(2), Some(0.7));
    }
    #[test]
    fn another_open_editor_cannot_replay_across_an_active_gesture() {
        let raw = Host::default();
        let mut first = HistoryHost::new(raw.clone());
        let mut second = HistoryHost::new(raw);
        edit(&mut first, 1, 0.2);
        first.begin_edit(1);
        assert!(second.undo_label().is_none());
        assert!(!second.undo().unwrap());
        first.set_parameter(1, 0.4);
        assert!(!second.undo().unwrap());
        first.end_edit(1);
        assert!(second.undo().unwrap());
        assert_eq!(second.parameter(1), Some(0.2));
        assert!(second.undo().unwrap());
        assert_eq!(second.parameter(1), Some(0.0));
    }
    #[test]
    fn terminal_callback_during_restore_cannot_mutate_or_reintroduce_stale_history() {
        let raw = Host::default();
        let mut host = HistoryHost::new(raw.clone());
        host.begin_edit(1);
        host.set_parameter(1, 0.5);
        let guard = raw.history.try_restore().unwrap();
        raw.values.lock().unwrap().restore_callback_on_end = true;
        drop(host);
        assert!(raw.values.lock().unwrap().nested_restore_blocked);
        assert_eq!(raw.parameter(2), Some(0.0));
        drop(guard);
        let mut reopened = HistoryHost::new(raw);
        assert!(reopened.undo_label().is_none());
        assert!(!reopened.undo().unwrap());
    }
    #[test]
    fn reentrant_noop_does_not_abort_or_discard_undo() {
        let raw = Host::default();
        let mut host = HistoryHost::new(raw.clone());
        edit(&mut host, 2, 0.25);
        edit(&mut host, 1, 0.5);
        raw.values.lock().unwrap().reentrant_edit_on_begin = true;
        assert!(host.undo().unwrap());
        assert_eq!(host.parameter(1), Some(0.0));
        assert_eq!(host.parameter(2), Some(0.25));
        assert_eq!(host.redo_label().as_deref(), Some("Gain"));
        assert!(host.redo().unwrap());
        assert_eq!(host.parameter(1), Some(0.5));
    }
    #[test]
    fn reentrant_open_gesture_defers_replay_without_losing_entry() {
        let raw = Host::default();
        let mut host = HistoryHost::new(raw.clone());
        edit(&mut host, 1, 0.5);
        raw.values.lock().unwrap().reentrant_open_on_begin = true;
        assert!(!host.undo().unwrap());
        assert_eq!(host.parameter(1), Some(0.5));
        assert!(host.undo_label().is_none());
        OPEN_EDITOR.with(|slot| {
            slot.borrow_mut().take();
        });
        assert_eq!(host.undo_label().as_deref(), Some("Gain"));
        assert!(host.undo().unwrap());
        assert_eq!(host.parameter(1), Some(0.0));
    }
}
