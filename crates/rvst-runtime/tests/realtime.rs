use rvst_core::Transport;
use rvst_runtime::*;
use rvst_test::{AllocationGuard, GuardedAllocator};
#[global_allocator]
static ALLOCATOR: GuardedAllocator<std::alloc::System> = GuardedAllocator(std::alloc::System);
#[test]
fn saturated_telemetry_recording_and_metrics_never_allocate_or_destroy_on_audio_thread() {
    let (mut tx, mut rx) = telemetry_channel(1);
    let (mut recorder, mut drain) = recording_channel(1, 8, 4);
    let metrics = Metrics::default();
    let mut smoother = LinearSmoother::new(0.);
    let guard = AllocationGuard::start();
    smoother.set_target(1., 64);
    for i in 0..1000 {
        tx.emit(TelemetryEvent::counter(1, i));
        recorder.capture_f32(&[1., 2.], &[2., 4.], 2, &[], Transport::default());
        metrics.record_callback(15, 10);
        std::hint::black_box(smoother.next_sample());
    }
    let counts = guard.finish();
    assert_eq!(counts.allocations, 0);
    assert_eq!(counts.reallocations, 0);
    assert_eq!(counts.deallocations, 0);
    assert_eq!(rx.dropped(), 999);
    assert!(rx.pop().is_some());
    assert_eq!(drain.lost_blocks(), 999);
    assert!(drain.pop().is_some());
    let guard = AllocationGuard::start();
    assert!(recorder.capture_f32(&[1., 2.], &[2., 4.], 2, &[], Transport::default()));
    let counts = guard.finish();
    assert_eq!(
        counts.allocations + counts.reallocations + counts.deallocations,
        0
    );
}
