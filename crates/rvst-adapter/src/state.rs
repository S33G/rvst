//! Versioned native project state. Editor presets remain plugin-owned raw data.
use crate::ParameterMirror;
use rvst_core::{EditHistory, Error, Plugin, PluginDescriptor};

const MAGIC: &[u8; 16] = b"RVST\0STATE\0HIST\0";
const VERSION: u32 = 1;
const HEADER_BYTES: usize = 40;
// Leave room for the optional legacy CLAP length prefix inside its stream cap.
const MAX_ENVELOPE_BYTES: usize = 64 * 1024 * 1024 - 8;

pub struct NativeState<'a> {
    pub plugin_state: &'a [u8],
    pub history: Option<&'a [u8]>,
}

/// Capture on a control thread with exclusive access to the plugin. Complete
/// pending UI values without consuming their host-notification markers. The
/// history stamp rejects a concurrent editor mutation across either snapshot.
pub fn save_project_state<P: Plugin>(
    plugin: &mut P,
    mirror: &ParameterMirror,
) -> Result<Vec<u8>, Error> {
    let mut snapshot_error = None;
    for _ in 0..3 {
        let history = match mirror.edit_history.snapshot() {
            Ok(history) => history,
            Err(error) => {
                snapshot_error = Some(error);
                continue;
            }
        };
        snapshot_error = None;
        for parameter in P::descriptor().parameters {
            if let Some(value) = mirror.get(parameter.id)
                && plugin.parameter(parameter.id) != Some(value)
            {
                plugin.set_parameter(parameter.id, value)?;
            }
        }
        let plugin_state = plugin.save_state()?;
        if mirror.edit_history.snapshot_is_current(&history) {
            return encode_native_state(P::descriptor(), &plugin_state, history.bytes());
        }
    }
    Err(snapshot_error.unwrap_or_else(|| {
        Error("editor changed while saving project state; retry the save".into())
    }))
}

/// Validate the complete project before touching live state. The caller must
/// exclude audio access; history guards also exclude concurrent UI mutations.
/// A controller can provide its distinct fallback mirror so old views and a
/// later disconnect observe the same restored state and history.
pub fn load_project_state<P: Plugin>(
    plugin: &mut P,
    mirror: &ParameterMirror,
    fallback: Option<&ParameterMirror>,
    bytes: &[u8],
) -> Result<(), Error> {
    let state = decode_native_state(P::descriptor(), bytes)?;
    let history = state
        .history
        .map(|bytes| EditHistory::decode(bytes, P::descriptor()))
        .transpose()?;
    let fallback_history = fallback.and_then(|_| history.clone());
    let _history_access = mirror.edit_history.try_restore()?;
    let _fallback_access = fallback
        .map(|mirror| mirror.edit_history.try_restore())
        .transpose()?;
    plugin.load_state(state.plugin_state)?;
    mirror.discard_pending_edits();
    if let Some(fallback) = fallback {
        fallback.discard_pending_edits();
    }
    for parameter in P::descriptor().parameters {
        if let Some(value) = plugin.parameter(parameter.id) {
            mirror.publish(parameter.id, value);
            if let Some(fallback) = fallback {
                fallback.publish(parameter.id, value);
            }
        }
    }
    if let Some(history) = history {
        mirror.edit_history.restore(history);
    } else {
        mirror.edit_history.invalidate();
    }
    if let Some(fallback) = fallback {
        if let Some(history) = fallback_history {
            fallback.edit_history.restore(history);
        } else {
            fallback.edit_history.invalidate();
        }
    }
    Ok(())
}

pub fn encode_native_state(
    descriptor: &PluginDescriptor,
    plugin_state: &[u8],
    history: &[u8],
) -> Result<Vec<u8>, Error> {
    let size = HEADER_BYTES
        .checked_add(descriptor.id.len())
        .and_then(|n| n.checked_add(plugin_state.len()))
        .and_then(|n| n.checked_add(history.len()))
        .and_then(|n| n.checked_add(4))
        .filter(|&n| n <= MAX_ENVELOPE_BYTES)
        .ok_or_else(|| Error("native project state exceeds storage limit".into()))?;
    let id_size = u32::try_from(descriptor.id.len())
        .map_err(|_| Error("plugin ID exceeds storage limit".into()))?;
    let mut out = Vec::with_capacity(size);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&id_size.to_le_bytes());
    out.extend_from_slice(&(plugin_state.len() as u64).to_le_bytes());
    out.extend_from_slice(&(history.len() as u64).to_le_bytes());
    out.extend_from_slice(descriptor.id.as_bytes());
    out.extend_from_slice(plugin_state);
    out.extend_from_slice(history);
    out.extend_from_slice(&crc32fast::hash(&out).to_le_bytes());
    Ok(out)
}

pub fn decode_native_state<'a>(
    descriptor: &PluginDescriptor,
    bytes: &'a [u8],
) -> Result<NativeState<'a>, Error> {
    if !bytes.starts_with(MAGIC) {
        return Ok(NativeState {
            plugin_state: bytes,
            history: None,
        });
    }
    let invalid = || Error("invalid native project state".into());
    if bytes.len() < HEADER_BYTES + 4 || bytes.len() > MAX_ENVELOPE_BYTES {
        return Err(invalid());
    }
    let version = u32::from_le_bytes(bytes[16..20].try_into().map_err(|_| invalid())?);
    if version != VERSION {
        return Err(Error("unsupported native project state version".into()));
    }
    let id_size = u32::from_le_bytes(bytes[20..24].try_into().map_err(|_| invalid())?) as usize;
    let plugin_size = usize::try_from(u64::from_le_bytes(
        bytes[24..32].try_into().map_err(|_| invalid())?,
    ))
    .map_err(|_| invalid())?;
    let history_size = usize::try_from(u64::from_le_bytes(
        bytes[32..40].try_into().map_err(|_| invalid())?,
    ))
    .map_err(|_| invalid())?;
    let id_end = HEADER_BYTES.checked_add(id_size).ok_or_else(invalid)?;
    let plugin_end = id_end.checked_add(plugin_size).ok_or_else(invalid)?;
    let history_end = plugin_end.checked_add(history_size).ok_or_else(invalid)?;
    if history_end.checked_add(4) != Some(bytes.len()) {
        return Err(invalid());
    }
    if &bytes[HEADER_BYTES..id_end] != descriptor.id.as_bytes() {
        return Err(Error(
            "native project state belongs to another plugin".into(),
        ));
    }
    let expected = u32::from_le_bytes(bytes[history_end..].try_into().map_err(|_| invalid())?);
    if crc32fast::hash(&bytes[..history_end]) != expected {
        return Err(Error("native project state checksum mismatch".into()));
    }
    Ok(NativeState {
        plugin_state: &bytes[id_end..plugin_end],
        history: Some(&bytes[plugin_end..history_end]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rvst_core::Plugin;
    mod fixture {
        include!("../test-support/plugin.rs");
    }

    #[test]
    fn envelope_preserves_raw_sections_and_accepts_legacy_state() {
        let d = fixture::Gain::descriptor();
        let encoded = encode_native_state(d, b"plugin", b"history").unwrap();
        let decoded = decode_native_state(d, &encoded).unwrap();
        assert_eq!(decoded.plugin_state, b"plugin");
        assert_eq!(decoded.history, Some(b"history".as_slice()));
        let legacy = decode_native_state(d, b"old plugin bytes").unwrap();
        assert_eq!(legacy.plugin_state, b"old plugin bytes");
        assert_eq!(legacy.history, None);
    }

    #[test]
    fn malformed_envelopes_are_rejected_before_sections_are_exposed() {
        let d = fixture::Gain::descriptor();
        let encoded = encode_native_state(d, b"plugin", b"history").unwrap();
        for length in MAGIC.len()..encoded.len() {
            assert!(decode_native_state(d, &encoded[..length]).is_err());
        }
        for offset in MAGIC.len()..encoded.len() {
            let mut corrupt = encoded.clone();
            corrupt[offset] ^= 1;
            assert!(decode_native_state(d, &corrupt).is_err());
        }
        let mut trailing = encoded.clone();
        trailing.push(0);
        assert!(decode_native_state(d, &trailing).is_err());
        let mut other = d.clone();
        other.id = "other.plugin";
        assert!(decode_native_state(&other, &encoded).is_err());
    }
}
