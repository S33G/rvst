//! Stable MIDI controller parameter identities and platform-neutral class IDs.
use super::*;
pub(crate) const MIDI_COUNT: u32 = 16 * 130;
pub(crate) fn midi_base<P: Plugin>() -> Option<u32> {
    if !P::descriptor().midi_input {
        return None;
    }
    let base = P::adapter_compatibility()
        .vst3_midi_parameter_base
        .unwrap_or((1 << 31) - MIDI_COUNT);
    let end = base.checked_add(MIDI_COUNT)?;
    if end > 1 << 31
        || P::descriptor()
            .parameters
            .iter()
            .any(|p| (base..end).contains(&p.id))
    {
        return None;
    }
    Some(base)
}
pub(crate) fn midi_address<P: Plugin>(id: u32) -> Option<(u8, u8)> {
    let relative = id.checked_sub(midi_base::<P>()?)?;
    (relative < MIDI_COUNT).then_some(((relative / 130) as u8, (relative % 130) as u8))
}
pub(crate) fn midi_event<P: Plugin>(id: u32, value: f64) -> Option<EventKind> {
    if !value.is_finite() {
        return None;
    }
    let (channel, controller) = midi_address::<P>(id)?;
    let value = value.clamp(0., 1.);
    Some(match controller {
        120 | 123 => EventKind::NoteRelease {
            channel: Some(channel),
            key: None,
            note_id: None,
            velocity: 0.,
            choke: controller == 120,
        },
        128 => EventKind::ChannelPressure {
            channel,
            pressure: value,
        },
        129 => EventKind::PitchBend {
            channel,
            value: value * 2. - 1.,
        },
        controller => EventKind::ControlChange {
            channel,
            controller,
            value,
        },
    })
}
pub(crate) fn class_id(mut id: [u8; 16], windows: bool) -> TUID {
    if windows {
        id[..4].reverse();
        id[4..6].reverse();
        id[6..8].reverse();
    }
    id.map(|b| b as i8)
}
pub(crate) fn midi_info<P: Plugin>(index: usize, out: &mut ParameterInfo) -> bool {
    let Some(relative) = index.checked_sub(P::descriptor().parameters.len()) else {
        return false;
    };
    if relative >= MIDI_COUNT as usize {
        return false;
    }
    let Some(base) = midi_base::<P>() else {
        return false;
    };
    let channel = relative / 130 + 1;
    let cc = relative % 130;
    let label = match cc {
        128 => format!("MIDI Ch. {channel} Channel Pressure"),
        129 => format!("MIDI Ch. {channel} Pitch Bend"),
        _ => format!("MIDI Ch. {channel} CC {cc}"),
    };
    *out = unsafe { std::mem::zeroed() };
    out.id = base + relative as u32;
    wide(&label, &mut out.title);
    wide(&label, &mut out.shortTitle);
    // These transport MIDI to the processor. They are not user automation controls.
    out.flags =
        ParameterInfo_::ParameterFlags_::kIsReadOnly | ParameterInfo_::ParameterFlags_::kIsHidden;
    true
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn midi_mapping_has_stable_ids_and_plain_internal_domains() {
        type P = super::super::tests::fixture::Gain;
        let base = midi_base::<P>().unwrap();
        assert_eq!(base, 2147481568);
        assert_eq!(midi_address::<P>(base + 130 * 15 + 129), Some((15, 129)));
        assert_eq!(midi_address::<P>(base - 1), None);
        assert_eq!(midi_address::<P>(base + MIDI_COUNT), None);
        assert_eq!(
            midi_event::<P>(base + 129, 0.5),
            Some(EventKind::PitchBend {
                channel: 0,
                value: 0.
            })
        );
        assert_eq!(
            midi_event::<P>(base + 128, 0.75),
            Some(EventKind::ChannelPressure {
                channel: 0,
                pressure: 0.75
            })
        );
        assert_eq!(
            midi_event::<P>(base + 130 + 74, 0.2),
            Some(EventKind::ControlChange {
                channel: 1,
                controller: 74,
                value: 0.2
            })
        );
        assert!(midi_event::<P>(base, f64::NAN).is_none());
        assert_eq!(
            midi_event::<P>(base + 130 + 123, 0.),
            Some(EventKind::NoteRelease {
                channel: Some(1),
                key: None,
                note_id: None,
                velocity: 0.,
                choke: false
            })
        );
        assert_eq!(
            midi_event::<P>(base + 120, 0.),
            Some(EventKind::NoteRelease {
                channel: Some(0),
                key: None,
                note_id: None,
                velocity: 0.,
                choke: true
            })
        );
    }
    #[test]
    fn guid_wire_order_preserves_cross_platform_identity() {
        let id = *b"HollowPointFx001";
        assert_eq!(class_id(id, false), id.map(|v| v as i8));
        assert_eq!(class_id(id, true), b"lloHwooPintFx001".map(|v| v as i8));
    }
}
