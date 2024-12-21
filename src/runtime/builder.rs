use crate::driver::IoUringDriver;
use crate::runtime::runtime::Runtime;
use crate::scoped_thread_local;
use crate::utils::thread_id::gen_id;
use std::{io, marker::PhantomData};

// ===== basic builder structure definition =====

/// Runtime builder
pub struct RuntimeBuilder<D> {
    // io_uring entries
    entries: Option<u32>,

    urb: io_uring::Builder,

    // driver mark
    _mark: PhantomData<D>,
}

scoped_thread_local!(pub(crate) static BUILD_THREAD_ID: usize);

impl<T> Default for RuntimeBuilder<T> {
    #[must_use]
    fn default() -> Self {
        RuntimeBuilder::<T>::new()
    }
}

impl<T> RuntimeBuilder<T> {
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: None,

            urb: io_uring::IoUring::builder(),

            _mark: PhantomData,
        }
    }
}

// ===== buildable trait and forward methods =====

/// Buildable trait.
pub trait Buildable: Sized {
    /// Build the runtime.
    fn build(this: RuntimeBuilder<Self>) -> io::Result<Runtime<Self>>;
}

#[allow(unused)]
macro_rules! direct_build {
    ($ty: ty) => {
        impl RuntimeBuilder<$ty> {
            /// Build the runtime.
            pub fn build(self) -> io::Result<Runtime<$ty>> {
                Buildable::build(self)
            }
        }
    };
}

direct_build!(IoUringDriver);

// ===== builder impl =====

impl Buildable for IoUringDriver {
    fn build(this: RuntimeBuilder<Self>) -> io::Result<Runtime<IoUringDriver>> {
        let thread_id = gen_id();

        BUILD_THREAD_ID.set(&thread_id, || {
            let driver = match this.entries {
                Some(entries) => IoUringDriver::new_with_entries(&this.urb, entries)?,
                None => IoUringDriver::new(&this.urb)?,
            };
            let context = crate::runtime::runtime::Context::new();
            Ok(Runtime::new(context, driver))
        })
    }
}

impl<D> RuntimeBuilder<D> {
    const MIN_ENTRIES: u32 = 256;

    /// Set io_uring entries, min size is 256 and the default size is 1024.
    #[must_use]
    pub fn with_entries(mut self, entries: u32) -> Self {
        // If entries is less than 256, it will be 256.
        if entries < Self::MIN_ENTRIES {
            self.entries = Some(Self::MIN_ENTRIES);
            return self;
        }
        self.entries = Some(entries);
        self
    }

    /// Replaces the default [`io_uring::Builder`], which controls the settings for the
    /// inner `io_uring` API.
    ///
    /// Refer to the [`io_uring::Builder`] documentation for all the supported methods.
    #[must_use]
    pub fn uring_builder(mut self, urb: io_uring::Builder) -> Self {
        self.urb = urb;
        self
    }
}

