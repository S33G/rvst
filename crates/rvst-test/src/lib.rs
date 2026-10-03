//! Offline host and per-thread allocation instrumentation for plugin authors.
//! Install [`GuardedAllocator`] as the test binary's global allocator before
//! using [`AllocationGuard`]. Never install a panic-on-allocation hook in a DAW.
use rvst_core::{
    AudioBlock, Error, Event, EventSink, Plugin, ProcessConfig, ProcessStatus, Transport,
};
use std::{
    alloc::{GlobalAlloc, Layout},
    cell::Cell,
    marker::PhantomData,
    rc::Rc,
};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct AllocationCounts {
    pub allocations: usize,
    pub deallocations: usize,
    pub reallocations: usize,
    pub allocated_bytes: usize,
}
#[derive(Default, Clone, Copy)]
struct GuardState {
    enabled: bool,
    counts: AllocationCounts,
}
thread_local! { static STATE:Cell<GuardState> = const { Cell::new(GuardState { enabled:false, counts:AllocationCounts{allocations:0,deallocations:0,reallocations:0,allocated_bytes:0} }) }; }
fn record(f: impl FnOnce(&mut AllocationCounts)) {
    let _ = STATE.try_with(|state| {
        let mut s = state.get();
        if s.enabled {
            f(&mut s.counts);
            state.set(s);
        }
    });
}
/// Transparent wrapper around an allocator. Counting uses const-initialized TLS
/// with no allocation, locks or formatted logging in the allocator path.
pub struct GuardedAllocator<A>(pub A);
// SAFETY: every operation forwards its original pointer, layout and size to A.
unsafe impl<A: GlobalAlloc> GlobalAlloc for GuardedAllocator<A> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record(|c| {
            c.allocations += 1;
            c.allocated_bytes += layout.size();
        });
        unsafe { self.0.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record(|c| {
            c.allocations += 1;
            c.allocated_bytes += layout.size();
        });
        unsafe { self.0.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        record(|c| c.deallocations += 1);
        unsafe { self.0.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        record(|c| {
            c.reallocations += 1;
            c.allocated_bytes += new_size;
        });
        unsafe { self.0.realloc(ptr, layout, new_size) }
    }
}
/// Scoped counting on the calling thread. Nested guards are rejected. The guard
/// is deliberately !Send: finishing on another thread would invalidate counts.
pub struct AllocationGuard {
    finished: bool,
    _thread: PhantomData<Rc<()>>,
}
impl AllocationGuard {
    pub fn start() -> Self {
        STATE.with(|s| {
            assert!(!s.get().enabled, "nested allocation guard");
            s.set(GuardState {
                enabled: true,
                counts: AllocationCounts::default(),
            });
        });
        Self {
            finished: false,
            _thread: PhantomData,
        }
    }
    pub fn finish(mut self) -> AllocationCounts {
        let result = STATE.with(|s| {
            let old = s.replace(GuardState::default());
            old.counts
        });
        self.finished = true;
        result
    }
}
impl Drop for AllocationGuard {
    fn drop(&mut self) {
        if !self.finished {
            let _ = STATE.try_with(|s| s.set(GuardState::default()));
        }
    }
}

/// Bounded event sink. Saturation is visible; callers must test their recovery
/// strategy for note releases rather than silently treating drops as success.
pub struct EventBuffer {
    events: Vec<Event>,
    dropped: usize,
}
impl EventBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            events: Vec::with_capacity(capacity),
            dropped: 0,
        }
    }
    pub fn events(&self) -> &[Event] {
        &self.events
    }
    pub fn dropped(&self) -> usize {
        self.dropped
    }
    pub fn clear(&mut self) {
        self.events.clear();
        self.dropped = 0;
    }
}
impl EventSink for EventBuffer {
    fn push(&mut self, event: Event) -> bool {
        if self.events.len() == self.events.capacity() {
            self.dropped += 1;
            false
        } else {
            self.events.push(event);
            true
        }
    }
}

/// Minimal deterministic host using the same processor contract as adapters.
/// This harness intentionally performs no audio-device or GUI initialization.
pub struct OfflineHost<P: Plugin> {
    pub plugin: P,
    pub config: ProcessConfig,
    output: Vec<f32>,
    events: EventBuffer,
}
pub struct BlockResult<'a> {
    pub audio: &'a [f32],
    pub events: &'a [Event],
    pub dropped_events: usize,
    pub status: ProcessStatus,
    pub allocations: AllocationCounts,
}
impl<P: Plugin> OfflineHost<P> {
    pub fn new(config: ProcessConfig, event_capacity: usize) -> Result<Self, Error> {
        P::descriptor().validate()?;
        config.validate()?;
        let mut plugin = P::new();
        plugin.prepare(config)?;
        plugin.reset();
        Ok(Self {
            plugin,
            config,
            output: vec![0.; config.max_frames * config.output_channels],
            events: EventBuffer::new(event_capacity),
        })
    }
    /// Input/event shape checks occur before instrumentation. The callback and
    /// output event sink are counted; assertion formatting remains outside it.
    pub fn process(
        &mut self,
        input: &[f32],
        frames: usize,
        events: &[Event],
        transport: Transport,
    ) -> Result<BlockResult<'_>, Error> {
        if frames > self.config.max_frames
            || events.iter().any(|e| e.frame as usize >= frames)
            || events.windows(2).any(|w| w[0].frame > w[1].frame)
        {
            return Err("invalid offline block size or event timing".into());
        }
        self.events.clear();
        let output = &mut self.output[..frames * self.config.output_channels];
        output.fill(0.);
        let mut block = AudioBlock::new(
            input,
            output,
            frames,
            self.config.input_channels,
            self.config.output_channels,
        )?;
        let guard = AllocationGuard::start();
        let status = self
            .plugin
            .process(&mut block, events, &transport, &mut self.events);
        let allocations = guard.finish();
        Ok(BlockResult {
            audio: output,
            events: self.events.events(),
            dropped_events: self.events.dropped(),
            status,
            allocations,
        })
    }
}
