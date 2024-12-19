use std::io;
use crate::driver::uring::lifecycle::MaybeFdLifecycle;
use crate::utils::slab::Slab;

mod lifecycle;
// When dropping the driver, all in-flight operations must have completed. This
// type wraps the slab and ensures that, on drop, the slab is empty.
pub struct Ops {
    pub(crate) slab: Slab<MaybeFdLifecycle>,
}
impl Ops {
    pub(crate) const fn new() -> Self {
        Ops { slab: Slab::new() }
    }

    // Insert a new operation
    #[inline]
    pub(crate) fn insert(&mut self, is_fd: bool) -> usize {
        self.slab.insert(MaybeFdLifecycle::new(is_fd))
    }

    // Complete an operation
    // # Safety
    // Caller must make sure the result is valid.
    #[inline]
    pub(crate) unsafe fn complete(&mut self, index: usize, result: io::Result<u32>, flags: u32) {
        let lifecycle = unsafe { self.slab.get(index).unwrap_unchecked() };
        lifecycle.complete(result, flags);
    }
}