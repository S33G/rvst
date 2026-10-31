use super::*;

const MAGIC: &[u8; 4] = b"RVUH";
const VERSION: u32 = 1;

/// Bounded, versioned history bytes and the revision they describe. A native
/// host must recheck [`EditHistory::snapshot_is_current`] after capturing plugin
/// state so a concurrent editor action cannot produce a mixed project save.
pub struct HistorySnapshot {
    bytes: Vec<u8>,
    stamp: u64,
    generation: u64,
    history: EditHistory,
}
impl HistorySnapshot {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}
/// Fully validated import data, safe to install after plugin-state loading has
/// succeeded. Validation alone never changes the current history.
#[derive(Clone)]
pub struct RestoredHistory {
    state: History,
}

impl EditHistory {
    /// Save committed undo/redo plus stable, unfinished parameter gestures.
    /// Including a pending gesture does not close or alter the live gesture.
    /// Actual in-flight host mutations return an error; a paused drag is safe.
    pub fn snapshot(&self) -> Result<HistorySnapshot, Error> {
        if self.0.activity.load(Ordering::Acquire) != 0 {
            return Err("editor action in progress".into());
        }
        let stamp = self.0.snapshot_revision.load(Ordering::Acquire);
        let generation = self.generation();
        let mut state = self.lock().clone();
        let mut pending = std::mem::take(&mut state.pending);
        pending.sort_by_key(|pending| pending.order);
        for pending in pending {
            if let Some(entry) = pending.entry {
                state.push(entry);
            }
        }
        let mut bytes = Vec::new();
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&VERSION.to_le_bytes());
        bytes.extend_from_slice(&(state.undo.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&(state.redo.len() as u16).to_le_bytes());
        for entry in state.undo.iter().chain(&state.redo) {
            encode_entry(&mut bytes, entry);
        }
        if bytes.len() > MAX_BYTES {
            return Err("undo history exceeds storage limit".into());
        }
        let snapshot = HistorySnapshot {
            bytes,
            stamp,
            generation,
            history: self.clone(),
        };
        if !self.snapshot_is_current(&snapshot) {
            return Err("editor changed during history snapshot".into());
        }
        Ok(snapshot)
    }

    /// Recheck after plugin-state serialization and before returning a project
    /// save. This check is lock-free and does not allocate.
    pub fn snapshot_is_current(&self, snapshot: &HistorySnapshot) -> bool {
        Arc::ptr_eq(&self.0, &snapshot.history.0)
            && self.0.activity.load(Ordering::Acquire) == 0
            && self.0.snapshot_revision.load(Ordering::Acquire) == snapshot.stamp
            && self.generation() == snapshot.generation
    }

    /// Validate the complete serialized history without touching live state.
    /// Entry counts, data size, version, labels, parameter identities, finite
    /// values, and descriptor ranges are checked before installation is possible.
    pub fn decode(data: &[u8], descriptor: &PluginDescriptor) -> Result<RestoredHistory, Error> {
        if data.len() > MAX_BYTES {
            return Err("undo history exceeds storage limit".into());
        }
        let mut cursor = Cursor(data);
        if cursor.take(4)? != MAGIC || cursor.u32()? != VERSION {
            return Err("unsupported undo history format".into());
        }
        let undo = cursor.u16()? as usize;
        let redo = cursor.u16()? as usize;
        if undo + redo > MAX_ENTRIES {
            return Err("too many undo history entries".into());
        }
        let mut state = History::default();
        for index in 0..undo + redo {
            let entry = decode_entry(&mut cursor, descriptor)?;
            state.bytes += entry.bytes();
            if state.bytes > MAX_BYTES {
                return Err("undo history exceeds storage limit".into());
            }
            if index < undo {
                state.undo.push_back(entry);
            } else {
                state.redo.push(entry);
            }
        }
        if !cursor.0.is_empty() {
            return Err("trailing undo history data".into());
        }
        Ok(RestoredHistory { state })
    }

    /// Install prevalidated history after successful plugin-state loading.
    /// Hold [`Self::try_restore`]'s guard across both operations in native hosts.
    /// The generation and stacks change together, so an existing editor drops
    /// its old pending gesture without clearing the newly restored history.
    pub fn restore(&self, restored: RestoredHistory) {
        let mut old = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        let mut state = restored.state;
        state.generation = self
            .0
            .generation
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        state.revision = old.revision.wrapping_add(1);
        *old = state;
        self.0.snapshot_revision.fetch_add(1, Ordering::AcqRel);
    }
}

fn encode_entry(bytes: &mut Vec<u8>, entry: &Entry) {
    bytes.extend_from_slice(&(entry.label.len() as u16).to_le_bytes());
    bytes.extend_from_slice(entry.label.as_bytes());
    match &entry.action {
        Action::Parameters(changes) => {
            bytes.push(0);
            bytes.extend_from_slice(&(changes.len() as u32).to_le_bytes());
            for change in changes {
                bytes.extend_from_slice(&change.id.to_le_bytes());
                bytes.extend_from_slice(&change.before.to_le_bytes());
                bytes.extend_from_slice(&change.after.to_le_bytes());
            }
        }
        Action::State { before, after } => {
            bytes.push(1);
            for state in [before, after] {
                bytes.extend_from_slice(&(state.len() as u32).to_le_bytes());
                bytes.extend_from_slice(state);
            }
        }
    }
}

fn decode_entry(cursor: &mut Cursor<'_>, descriptor: &PluginDescriptor) -> Result<Entry, Error> {
    let label_len = cursor.u16()? as usize;
    if label_len > MAX_LABEL_BYTES {
        return Err("undo label exceeds storage limit".into());
    }
    let label = String::from_utf8(cursor.take(label_len)?.to_vec())
        .map_err(|_| Error::from("invalid undo label"))?;
    let action = match cursor.take(1)?[0] {
        0 => {
            let count = cursor.u32()? as usize;
            if count == 0 || count > descriptor.parameters.len() || count > cursor.0.len() / 20 {
                return Err("invalid undo parameter count".into());
            }
            let mut seen = std::collections::HashSet::with_capacity(count);
            let mut changes = Vec::with_capacity(count);
            for _ in 0..count {
                let id = cursor.u32()?;
                let before = cursor.f64()?;
                let after = cursor.f64()?;
                let Some(parameter) = descriptor.parameter(id) else {
                    return Err("unknown undo parameter".into());
                };
                if !seen.insert(id)
                    || !before.is_finite()
                    || !after.is_finite()
                    || !(parameter.min..=parameter.max).contains(&before)
                    || !(parameter.min..=parameter.max).contains(&after)
                    || before == after
                {
                    return Err("invalid undo parameter value".into());
                }
                changes.push(Change { id, before, after });
            }
            Action::Parameters(changes)
        }
        1 => {
            let before_len = cursor.u32()? as usize;
            let before = cursor.take(before_len)?.to_vec();
            let after_len = cursor.u32()? as usize;
            let after = cursor.take(after_len)?.to_vec();
            if before == after {
                return Err("empty undo state change".into());
            }
            Action::State { before, after }
        }
        _ => return Err("unknown undo action".into()),
    };
    Ok(Entry { label, action })
}

struct Cursor<'a>(&'a [u8]);
impl<'a> Cursor<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], Error> {
        if count > self.0.len() {
            return Err("truncated undo history".into());
        }
        let (value, rest) = self.0.split_at(count);
        self.0 = rest;
        Ok(value)
    }
    fn u16(&mut self) -> Result<u16, Error> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn f64(&mut self) -> Result<f64, Error> {
        Ok(f64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{DESCRIPTOR, Host, edit};
    use super::*;
    #[test]
    fn project_roundtrip_preserves_undo_and_redo() {
        let raw = Host::default();
        let mut host = HistoryHost::new(raw.clone());
        edit(&mut host, 1, 0.2);
        edit(&mut host, 2, 0.7);
        host.undo().unwrap();
        let snapshot = raw.history.snapshot().unwrap();
        let restored = EditHistory::decode(snapshot.bytes(), &DESCRIPTOR).unwrap();
        let target = Host::default();
        let mut next = HistoryHost::new(target.clone());
        next.load_state(&host.save_state().unwrap()).unwrap();
        target.history.restore(restored);
        assert!(next.redo().unwrap());
        assert_eq!(next.parameter(2), Some(0.7));
        next.undo().unwrap();
        next.undo().unwrap();
        assert_eq!(next.parameter(1), Some(0.0));
    }
    #[test]
    fn paused_drag_is_saved_as_one_step_without_ending_live_gesture() {
        let raw = Host::default();
        let mut host = HistoryHost::new(raw.clone());
        host.begin_edit_group("Drag group");
        host.begin_edit(1);
        host.set_parameter(1, 0.4);
        let snapshot = raw.history.snapshot().unwrap();
        let target = Host::default();
        let mut next = HistoryHost::new(target.clone());
        next.load_state(&host.save_state().unwrap()).unwrap();
        target
            .history
            .restore(EditHistory::decode(snapshot.bytes(), &DESCRIPTOR).unwrap());
        assert_eq!(next.undo_label().as_deref(), Some("Drag group"));
        next.undo().unwrap();
        assert_eq!(next.parameter(1), Some(0.0));
        host.set_parameter(1, 0.8);
        host.end_edit(1);
        host.end_edit_group();
        host.undo().unwrap();
        assert_eq!(host.parameter(1), Some(0.0));
        assert!(!host.undo().unwrap());
    }
    #[test]
    fn malformed_import_never_changes_existing_history() {
        let raw = Host::default();
        let mut host = HistoryHost::new(raw.clone());
        edit(&mut host, 1, 0.5);
        let snapshot = raw.history.snapshot().unwrap();
        assert!(EditHistory::decode(&snapshot.bytes()[..5], &DESCRIPTOR).is_err());
        let mut unknown = snapshot.bytes().to_vec();
        unknown[4] = 99;
        assert!(EditHistory::decode(&unknown, &DESCRIPTOR).is_err());
        assert!(EditHistory::decode(&vec![0; MAX_BYTES + 1], &DESCRIPTOR).is_err());
        assert!(host.undo().unwrap());
        assert_eq!(host.parameter(1), Some(0.0));
    }

    #[test]
    fn opaque_preset_history_survives_serialization() {
        let raw = Host::default();
        let mut host = HistoryHost::new(raw.clone());
        let original = host.save_state().unwrap();
        let mut preset = original.clone();
        preset[16] = 42;
        host.load_state(&preset).unwrap();
        let snapshot = raw.history.snapshot().unwrap();
        let target = Host::default();
        let mut next = HistoryHost::new(target.clone());
        next.load_state(&preset).unwrap();
        target
            .history
            .restore(EditHistory::decode(snapshot.bytes(), &DESCRIPTOR).unwrap());
        next.undo().unwrap();
        assert_eq!(next.save_state().unwrap(), original);
        next.redo().unwrap();
        assert_eq!(next.save_state().unwrap(), preset);
    }

    #[test]
    fn imported_history_survives_synchronization_of_an_old_open_gesture() {
        let source = Host::default();
        let mut source_host = HistoryHost::new(source.clone());
        edit(&mut source_host, 2, 0.7);
        let snapshot = source.history.snapshot().unwrap();
        let imported = EditHistory::decode(snapshot.bytes(), &DESCRIPTOR).unwrap();
        let target = Host::default();
        let mut target_host = HistoryHost::new(target.clone());
        target_host.begin_edit(1);
        target_host.set_parameter(1, 0.4);
        let guard = target.history.try_restore().unwrap();
        let mut raw_target = target.clone();
        raw_target
            .load_state(&source_host.save_state().unwrap())
            .unwrap();
        target.history.restore(imported);
        drop(guard);
        target_host.end_edit(1);
        assert_eq!(target_host.undo_label().as_deref(), Some("Mix"));
        target_host.undo().unwrap();
        assert_eq!(target_host.parameter(2), Some(0.0));
        assert_eq!(target_host.parameter(1), Some(0.0));
        assert!(!target_host.undo().unwrap());
    }

    #[test]
    fn failed_restore_guard_preserves_history_and_blocks_ui_forwarding() {
        let raw = Host::default();
        let mut host = HistoryHost::new(raw.clone());
        edit(&mut host, 1, 0.5);
        let guard = raw.history.try_restore().unwrap();
        assert!(raw.history.snapshot().is_err());
        assert!(raw.history.try_restore().is_err());
        host.begin_edit(2);
        host.set_parameter(2, 0.8);
        host.end_edit(2);
        assert_eq!(host.parameter(2), Some(0.0));
        assert!(host.undo().is_err());
        drop(guard);
        assert_eq!(host.undo_label().as_deref(), Some("Gain"));
        host.undo().unwrap();
        assert_eq!(host.parameter(1), Some(0.0));
    }

    #[test]
    fn snapshot_stamp_covers_inflight_actions_and_instance_identity() {
        let raw = Host::default();
        let mut host = HistoryHost::new(raw.clone());
        let snapshot = raw.history.snapshot().unwrap();
        assert!(raw.history.snapshot_is_current(&snapshot));
        assert!(!EditHistory::default().snapshot_is_current(&snapshot));
        let action = raw.history.begin_mutation().unwrap();
        assert!(raw.history.snapshot().is_err());
        assert!(raw.history.try_restore().is_err());
        assert!(!raw.history.snapshot_is_current(&snapshot));
        drop(action);
        assert!(!raw.history.snapshot_is_current(&snapshot));
        let snapshot = raw.history.snapshot().unwrap();
        host.begin_edit(1);
        host.set_parameter(1, 0.25);
        assert!(!raw.history.snapshot_is_current(&snapshot));
        let paused = raw.history.snapshot().unwrap();
        assert!(raw.history.snapshot_is_current(&paused));
        raw.history.invalidate();
        assert!(!raw.history.snapshot_is_current(&paused));
    }

    #[test]
    fn decoder_rejects_nonfinite_out_of_range_duplicate_and_unknown_parameters() {
        for changes in [
            vec![Change {
                id: 1,
                before: f64::NAN,
                after: 0.5,
            }],
            vec![Change {
                id: 1,
                before: 0.0,
                after: 1.5,
            }],
            vec![Change {
                id: 999,
                before: 0.0,
                after: 0.5,
            }],
            vec![
                Change {
                    id: 1,
                    before: 0.0,
                    after: 0.5,
                },
                Change {
                    id: 1,
                    before: 0.5,
                    after: 0.8,
                },
            ],
        ] {
            let mut bytes = Vec::from(*MAGIC);
            bytes.extend_from_slice(&VERSION.to_le_bytes());
            bytes.extend_from_slice(&1u16.to_le_bytes());
            bytes.extend_from_slice(&0u16.to_le_bytes());
            encode_entry(
                &mut bytes,
                &Entry {
                    label: "Bad".into(),
                    action: Action::Parameters(changes),
                },
            );
            assert!(EditHistory::decode(&bytes, &DESCRIPTOR).is_err());
        }
    }

    #[test]
    fn decoder_rejects_invalid_labels_excess_entries_and_trailing_data() {
        let raw = Host::default();
        let mut host = HistoryHost::new(raw.clone());
        edit(&mut host, 1, 0.5);
        let snapshot = raw.history.snapshot().unwrap();
        let mut label = snapshot.bytes().to_vec();
        label[14] = 0xff;
        assert!(EditHistory::decode(&label, &DESCRIPTOR).is_err());
        let mut count = snapshot.bytes().to_vec();
        count[8..10].copy_from_slice(&129u16.to_le_bytes());
        assert!(EditHistory::decode(&count, &DESCRIPTOR).is_err());
        let mut trailing = snapshot.bytes().to_vec();
        trailing.push(1);
        assert!(EditHistory::decode(&trailing, &DESCRIPTOR).is_err());
    }

    #[test]
    fn pending_noop_snapshot_preserves_redo_and_effective_pending_action_replaces_it() {
        let raw = Host::default();
        let mut host = HistoryHost::new(raw.clone());
        edit(&mut host, 1, 0.5);
        host.undo().unwrap();
        host.begin_edit(2);
        host.set_parameter(2, 0.6);
        let snapshot = raw.history.snapshot().unwrap();
        let state = EditHistory::decode(snapshot.bytes(), &DESCRIPTOR)
            .unwrap()
            .state;
        assert_eq!(state.undo.len(), 1);
        assert!(state.redo.is_empty());
        host.set_parameter(2, 0.0);
        let snapshot = raw.history.snapshot().unwrap();
        let state = EditHistory::decode(snapshot.bytes(), &DESCRIPTOR)
            .unwrap()
            .state;
        assert!(state.undo.is_empty());
        assert_eq!(state.redo.len(), 1);
    }

    #[test]
    fn release_during_failed_restore_finishes_old_transaction_before_new_edit() {
        let raw = Host::default();
        let mut host = HistoryHost::new(raw.clone());
        host.begin_edit_group("First gesture");
        host.begin_edit(1);
        host.set_parameter(1, 0.25);
        let guard = raw.history.try_restore().unwrap();
        host.end_edit(1);
        host.end_edit_group();
        drop(guard); // Plugin loading failed; no generation/history replacement.
        edit(&mut host, 1, 0.75);
        host.undo().unwrap();
        assert_eq!(host.parameter(1), Some(0.25));
        assert_eq!(host.undo_label().as_deref(), Some("First gesture"));
        host.undo().unwrap();
        assert_eq!(host.parameter(1), Some(0.0));
        assert!(!host.undo().unwrap());
    }

    #[test]
    fn close_during_failed_restore_balances_and_commits_interrupted_gesture() {
        let raw = Host::default();
        let mut host = HistoryHost::new(raw.clone());
        host.begin_edit(1);
        host.set_parameter(1, 0.4);
        let guard = raw.history.try_restore().unwrap();
        drop(host);
        assert_eq!(raw.calls().last().unwrap(), "end 1");
        assert!(raw.history.lock().pending.is_empty());
        drop(guard);
        let mut reopened = HistoryHost::new(raw.clone());
        assert_eq!(reopened.undo_label().as_deref(), Some("Gain"));
        reopened.undo().unwrap();
        assert_eq!(reopened.parameter(1), Some(0.0));
        let snapshot = raw.history.snapshot().unwrap();
        let state = EditHistory::decode(snapshot.bytes(), &DESCRIPTOR)
            .unwrap()
            .state;
        assert!(state.undo.is_empty());
        assert_eq!(state.redo.len(), 1);
    }

    #[test]
    fn close_during_successful_restore_never_reintroduces_old_pending_action() {
        let source = Host::default();
        let mut source_host = HistoryHost::new(source.clone());
        edit(&mut source_host, 2, 0.7);
        let imported =
            EditHistory::decode(source.history.snapshot().unwrap().bytes(), &DESCRIPTOR).unwrap();
        let target = Host::default();
        let mut host = HistoryHost::new(target.clone());
        host.begin_edit(1);
        host.set_parameter(1, 0.4);
        let guard = target.history.try_restore().unwrap();
        drop(host);
        assert_eq!(target.calls().last().unwrap(), "end 1");
        target
            .clone()
            .load_state(&source_host.save_state().unwrap())
            .unwrap();
        target.history.restore(imported);
        drop(guard);
        let mut reopened = HistoryHost::new(target);
        assert_eq!(reopened.undo_label().as_deref(), Some("Mix"));
        reopened.undo().unwrap();
        assert_eq!(reopened.parameter(2), Some(0.0));
        assert!(!reopened.undo().unwrap());
    }

    #[test]
    fn restore_guard_release_preserves_inflight_terminal_activity() {
        let history = EditHistory::default();
        let restore = history.try_restore().unwrap();
        let teardown = history.begin_teardown();
        drop(restore);
        assert!(history.snapshot().is_err());
        assert!(history.try_restore().is_err());
        drop(teardown);
        assert!(history.snapshot().is_ok());
        assert!(history.try_restore().is_ok());
    }
}
