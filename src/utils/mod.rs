//! Common utils

pub(crate) mod box_into_inner;
pub(crate) mod linked_list;
#[allow(dead_code)]
pub(crate) mod slab;
#[allow(dead_code)]
pub(crate) mod thread_id;
pub(crate) mod uring_detect;

mod rand;
pub use rand::thread_rng_n;

pub use crate::driver::op::is_legacy;



