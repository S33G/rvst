use rvst_test::{AllocationGuard, GuardedAllocator};
#[global_allocator]
static ALLOCATOR: GuardedAllocator<std::alloc::System> = GuardedAllocator(std::alloc::System);
#[test]
fn detects_both_allocation_and_destruction() {
    let prior = Box::new(42u64);
    let guard = AllocationGuard::start();
    let created = std::hint::black_box(Box::new(99u64));
    drop(prior);
    let count = guard.finish();
    drop(created);
    assert_eq!(count.allocations, 1);
    assert_eq!(count.deallocations, 1);
}
#[test]
fn empty_scope_has_no_allocation() {
    let guard = AllocationGuard::start();
    std::hint::black_box(42u64);
    assert_eq!(guard.finish().allocations, 0);
}
