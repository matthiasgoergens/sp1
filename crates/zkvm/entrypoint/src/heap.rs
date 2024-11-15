use core::alloc::{GlobalAlloc, Layout};

use crate::syscalls::sys_alloc_aligned;

/// A simple heap allocator.
///
/// Allocates memory from left to right, without any deallocation.
pub struct SimpleAlloc;

unsafe impl GlobalAlloc for SimpleAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        sys_alloc_aligned(layout.size(), layout.align())
    }

    // TODO(Matthias): why does this not implement zeroed alloc?
    // Ok, looking at 'read_vec' it looks like the prover has total control over uninitialized memory
    // So we can't assume it's zero.

    unsafe fn dealloc(&self, _: *mut u8, _: Layout) {}
}
