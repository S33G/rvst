# Real-time rules and test recipes

[Documentation](README.md) / Real-time rules and test recipes

## Audio-thread rules

Inside a callback: no heap allocation/reallocation, destruction of heap-owned
values, locks, file or socket I/O, formatting or blocking. Allocate buffers,
voices and queues in prepare. Retire old state to a control-thread owner. Do not
replace the last `Arc` or `Box` on the audio thread. Bounds-check before constructing
safe slices from host pointers; the host may supply in-place/aliased buffers.

`LinearSmoother` handles exact sample-count ramps. `Metrics` accepts nanosecond
callback duration and deadline values into atomics. `RtTelemetry` accepts
fixed-size numeric events into a bounded SPSC queue. Dropped telemetry is counted.
Keep musical output and begin/end edit recovery separate from telemetry.

## Detect allocations and destruction

Use the test allocator in a test binary:

```rust
use rvst_test::{AllocationGuard, GuardedAllocator};
#[global_allocator]
static ALLOCATOR: GuardedAllocator<std::alloc::System> =
    GuardedAllocator(std::alloc::System);

let guard = AllocationGuard::start();
// Call the already-prepared processor here.
let counts = guard.finish();
assert_eq!(counts.allocations + counts.reallocations + counts.deallocations, 0);
```

Counters are per-thread; they detect work on the guarded callback thread only.
Do not install a panic-on-allocation allocator in a user's DAW.
`OfflineHost<P>` provides preallocated output/event buffers and returns callback
allocation counts. Its instrumentation requires installing `GuardedAllocator`.

## Exercise host and state boundaries

Test zero, one, maximum and changing block lengths; multiple instances; omitted
sidechains; f32/f64 if advertised; parameter steps/ramps; note-on/off and saturated
queues; state corruption and previous-version fixtures; latency, tail and bypass;
reset, deactivate/reactivate and destruction. Dynamic CLAP/VST3 smoke hosts in
the adapter examples exercise the actual exported binary, independently of a DAW.

## Record without blocking

Recording uses a preallocated slot pool. The audio producer never frees slots
when full; the off-thread drain copies completed blocks and returns slots. A lost
block or output event marks the capture incomplete. Offline replay checks schema,
identity, shape, event timing, numerical output and recorded musical outputs.

## Check generated projects

Run `python3 scripts/check-generated.py` to check fresh generated workspaces,
or add `--template gain` for one starter. The script isolates each project's
Cargo target directory, including both MIDI variants, and keeps the CLI build
separate. A caller-provided `CARGO_TARGET_DIR` is used only as the parent of the
CLI cache. When building multiple generated projects manually, keep separate
target directories: they reuse package names such as `xtask` and
`plugin-native`, so sharing output paths can select another project's binary.
