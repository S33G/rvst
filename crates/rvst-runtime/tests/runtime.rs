use rvst_runtime::*;
#[test]
fn smoothing_lands_exactly_and_zero_length_is_immediate() {
    let mut s = LinearSmoother::new(0.0);
    s.set_target(1.0, 4);
    assert_eq!(
        (0..5).map(|_| s.next_sample()).collect::<Vec<_>>(),
        vec![0.25, 0.5, 0.75, 1.0, 1.0]
    );
    s.set_target(-2.0, 0);
    assert_eq!(s.next_sample(), -2.0);
}
#[test]
fn telemetry_is_bounded_and_reports_loss() {
    let (mut tx, mut rx) = telemetry_channel(2);
    assert!(tx.emit(TelemetryEvent::counter(7, 1)));
    assert!(tx.emit(TelemetryEvent::counter(7, 2)));
    assert!(!tx.emit(TelemetryEvent::counter(7, 3)));
    assert_eq!(rx.dropped(), 1);
    assert_eq!(rx.pop().unwrap().value, 1.0);
    assert_eq!(rx.pop().unwrap().value, 2.0);
    assert!(rx.pop().is_none());
}
#[test]
fn metrics_separate_deadline_misses_and_ui_time() {
    let m = Metrics::default();
    m.record_callback(500, 1000);
    m.record_callback(2000, 1000);
    m.record_ui_frame(40);
    let snapshot = m.snapshot();
    assert_eq!(snapshot.callbacks, 2);
    assert_eq!(snapshot.deadline_misses, 1);
    assert_eq!(snapshot.max_callback_ns, 2000);
    assert_eq!(snapshot.ui_frames, 1);
}
#[test]
fn recorder_recovers_pool_and_marks_overflow_incomplete() {
    let (mut rt, mut drain) = recording_channel(1, 4, 2);
    let transport = rvst_core::Transport::default();
    assert!(rt.capture_f32(&[1., 2.], &[2., 4.], 2, &[], transport));
    assert!(!rt.capture_f32(&[3., 4.], &[6., 8.], 2, &[], transport));
    let first = drain.pop().unwrap();
    assert_eq!(first.input, vec![1., 2.]);
    assert!(rt.capture_f32(&[3., 4.], &[6., 8.], 2, &[], transport));
    assert_eq!(drain.lost_blocks(), 1);
}
