use rvst_core::*;

fn gain() -> ParameterSpec {
    ParameterSpec {
        id: 1,
        name: "Gain",
        unit: "dB",
        min: -60.0,
        max: 12.0,
        default: 0.0,
        step: None,
        bypass: false,
    }
}
#[test]
fn parameter_roundtrips_and_rejects_nonfinite_input() {
    let p = gain();
    for value in [-60.0, -20.0, 0.0, 12.0] {
        assert!((p.denormalize(p.normalize(value)) - value).abs() < 1e-10);
    }
    assert_eq!(p.clamp(f64::NAN), 0.0);
    assert_eq!(p.clamp(900.0), 12.0);
    assert_eq!(p.denormalize(-1.0), -60.0);
}
#[test]
fn discrete_parameter_rounds_in_plain_domain() {
    let p = ParameterSpec {
        step: Some(1.0),
        min: 0.0,
        max: 3.0,
        default: 0.0,
        ..gain()
    };
    assert_eq!(p.clamp(1.6), 2.0);
    assert_eq!(p.denormalize(0.5), 2.0);
}
#[test]
fn planar_channels_stay_separate_and_invalid_shapes_fail() {
    let input = [1.0, 2.0, 3.0, 4.0];
    let mut out = [0.0; 4];
    let mut b = AudioBlock::new(&input, &mut out, 2, 2, 2).unwrap();
    assert_eq!(b.input(1).unwrap(), &[3.0, 4.0]);
    b.output_mut(0).unwrap().copy_from_slice(&[8.0, 9.0]);
    assert_eq!(out, [8.0, 9.0, 0.0, 0.0]);
    assert!(AudioBlock::new(&input, &mut out, 3, 2, 2).is_err());
    assert!(AudioBlock::new(&input, &mut out, usize::MAX, 2, 2).is_err());
}
#[test]
fn no_audio_bus_is_valid_for_midi_processor() {
    let mut out: [f32; 0] = [];
    let block = AudioBlock::new(&[], &mut out, 128, 0, 0).unwrap();
    assert_eq!(block.frames, 128);
}
#[test]
fn normalized_ramps_retain_previous_point_and_clamp_time() {
    let ramp = ParameterRamp {
        id: 1,
        start_frame: 2,
        end_frame: 6,
        start_normalized: 0.0,
        end_normalized: 1.0,
    };
    assert_eq!(ramp.normalized_at(0), 0.0);
    assert_eq!(ramp.normalized_at(4), 0.5);
    assert_eq!(ramp.normalized_at(8), 1.0);
}
