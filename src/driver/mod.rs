pub mod file_io;
pub(crate) mod op;
mod uring;
mod util;

use crate::driver::op::{CompletionMeta, Mappable, Op};
use crate::driver::uring::Ops;
use crate::driver::util::timespec;
use crate::scoped_thread_local;
use io_uring::types::Timespec;
use io_uring::{cqueue, opcode, IoUring};
use std::cell::UnsafeCell;
use std::io;
use std::mem::ManuallyDrop;
use std::rc::Rc;
use std::task::{Context, Poll};
use std::time::Duration;

#[allow(unused)]
pub(crate) const CANCEL_USERDATA: u64 = u64::MAX;
pub(crate) const TIMEOUT_USERDATA: u64 = u64::MAX - 1;

pub(crate) const MIN_REVERSED_USERDATA: u64 = u64::MAX - 3;

pub struct IoUringDriver {
    inner: Rc<UnsafeCell<UringInner>>,

    // Used as timeout buffer
    timespec: *mut Timespec,
}

pub(crate) struct UringInner {
    /// Record Submitted Operations
    ops: Ops,

    /// IoUring bindings
    uring: ManuallyDrop<IoUring>,

    // Uring support ext_arg
    ext_arg: bool,
}
pub trait Driver {
    /// Run with driver TLS.
    fn with<R>(&self, f: impl FnOnce() -> R) -> R;
    /// Submit ops to kernel and process returned events.
    fn submit(&self) -> io::Result<()>;
    /// Wait infinitely and process returned events.
    fn park(&self) -> io::Result<()>;
    /// Wait with timeout and process returned events.
    fn park_timeout(&self, duration: Duration) -> io::Result<()>;
}
scoped_thread_local!(pub(crate) static CURRENT: Inner);
#[derive(Clone)]
pub(crate) enum Inner {
    Uring(Rc<UnsafeCell<UringInner>>),
}
impl Inner {
    fn submit_with<T: Mappable>(&self, data: T) -> io::Result<Op<T>> {
        match self {
            Inner::Uring(this) => UringInner::submit_with_data(this, data),
        }
    }

    #[allow(unused)]
    fn poll_op<T: Mappable>(
        &self,
        data: &mut T,
        index: usize,
        cx: &mut Context<'_>,
    ) -> Poll<CompletionMeta> {
        match self {
            Inner::Uring(this) => UringInner::poll_op(this, index, cx),
        }
    }

    #[inline]
    fn drop_op<T: 'static>(&self, index: usize, data: &mut Option<T>, skip_cancel: bool) {
        match self {
            Inner::Uring(this) => UringInner::drop_op(this, index, data, skip_cancel),
        }
    }

    #[allow(unused)]
    pub(super) unsafe fn cancel_op(&self, op_canceller: &op::OpCanceller) {
        match self {
            Inner::Uring(this) => UringInner::cancel_op(this, op_canceller.index),
        }
    }
    fn is_legacy(&self) -> bool {
        false
    }
}

impl IoUringDriver {
    const DEFAULT_ENTRIES: u32 = 1024;

    pub(crate) fn new(b: &io_uring::Builder) -> io::Result<IoUringDriver> {
        Self::new_with_entries(b, Self::DEFAULT_ENTRIES)
    }

    pub(crate) fn new_with_entries(
        urb: &io_uring::Builder,
        entries: u32,
    ) -> io::Result<IoUringDriver> {
        let uring = ManuallyDrop::new(urb.build(entries)?);

        let inner = Rc::new(UnsafeCell::new(UringInner {
            ops: Ops::new(),
            ext_arg: uring.params().is_feature_ext_arg(),
            uring,
        }));

        Ok(IoUringDriver {
            inner,
            timespec: Box::leak(Box::new(Timespec::new())) as *mut Timespec,
        })
    }

    #[allow(unused)]
    fn num_operations(&self) -> usize {
        let inner = self.inner.get();
        unsafe { (*inner).ops.slab.len() }
    }

    // Flush to make enough space
    fn flush_space(inner: &mut UringInner, need: usize) -> io::Result<()> {
        let sq = inner.uring.submission();
        debug_assert!(sq.capacity() >= need);
        if sq.len() + need > sq.capacity() {
            drop(sq);
            inner.submit()?;
        }
        Ok(())
    }

    fn install_timeout(&self, inner: &mut UringInner, duration: Duration) {
        let timespec = timespec(duration);
        unsafe {
            std::ptr::replace(self.timespec, timespec);
        }
        let entry = opcode::Timeout::new(self.timespec as *const Timespec)
            .build()
            .user_data(TIMEOUT_USERDATA);

        let mut sq = inner.uring.submission();
        let _ = unsafe { sq.push(&entry) };
    }

    fn inner_park(&self, timeout: Option<Duration>) -> io::Result<()> {
        let inner = unsafe { &mut *self.inner.get() };

        if timeout.is_some() {
            Self::flush_space(inner, 1)?;
        }

        if let Some(duration) = timeout {
            match inner.ext_arg {
                // Submit and Wait with timeout in an TimeoutOp way.
                // Better compatibility(5.4+).
                false => {
                    self.install_timeout(inner, duration);
                    inner.uring.submit_and_wait(1)?;
                }
                // Submit and Wait with enter args.
                // Better performance(5.11+).
                true => {
                    let timespec = timespec(duration);
                    let args = io_uring::types::SubmitArgs::new().timespec(&timespec);
                    if let Err(e) = inner.uring.submitter().submit_with_args(1, &args) {
                        if e.raw_os_error() != Some(libc::ETIME) {
                            return Err(e);
                        }
                    }
                }
            }
        } else {
            inner.uring.submit_and_wait(1)?;
        }
        // Process CQ
        inner.tick()?;

        Ok(())
    }
}

impl Driver for IoUringDriver {
    /// Enter the driver context. This enables using uring types.
    fn with<R>(&self, f: impl FnOnce() -> R) -> R {
        let inner = Inner::Uring(self.inner.clone());
        CURRENT.set(&inner, f)
    }

    fn submit(&self) -> io::Result<()> {
        let inner = unsafe { &mut *self.inner.get() };
        inner.submit()?;
        inner.tick()?;
        Ok(())
    }

    fn park(&self) -> io::Result<()> {
        self.inner_park(None)
    }

    fn park_timeout(&self, duration: Duration) -> io::Result<()> {
        self.inner_park(Some(duration))
    }
}

impl UringInner {
    fn tick(&mut self) -> io::Result<()> {
        let cq = self.uring.completion();

        for cqe in cq {
            let index = cqe.user_data();
            match index {
                _ if index >= MIN_REVERSED_USERDATA => (),
                // # Safety
                // Here we can make sure the result is valid.
                _ => unsafe { self.ops.complete(index as _, unwrap_to_result(&cqe), cqe.flags()) },
            }
        }
        Ok(())
    }

    fn submit(&mut self) -> io::Result<()> {
        loop {
            match self.uring.submit() {
                Err(ref e)
                    if matches!(e.raw_os_error(), Some(libc::EAGAIN) | Some(libc::EBUSY)) =>
                {
                    // This error is constructed with io::Error::last_os_error():
                    // https://github.com/tokio-rs/io-uring/blob/01c83bbce965d4aaf93ebfaa08c3aa8b7b0f5335/src/sys/mod.rs#L32
                    // So we can use https://doc.rust-lang.org/nightly/std/io/struct.Error.html#method.raw_os_error
                    // to get the raw error code.
                    self.tick()?;
                }
                e => return e.map(|_| ()),
            }
        }
    }

    fn new_op<T: Mappable>(data: T, inner: &mut UringInner, driver: Inner) -> Op<T> {
        Op {
            driver,
            index: inner.ops.insert(T::RET_IS_FD),
            data: Some(data),
        }
    }

    pub(crate) fn submit_with_data<T>(
        this: &Rc<UnsafeCell<UringInner>>,
        data: T,
    ) -> io::Result<Op<T>>
    where
        T: Mappable,
    {
        let inner = unsafe { &mut *this.get() };
        // If the submission queue is full, flush it to the kernel
        if inner.uring.submission().is_full() {
            inner.submit()?;
        }

        // Create the operation
        let mut op = Self::new_op(data, inner, Inner::Uring(this.clone()));

        // Configure the SQE
        let data_mut = unsafe { op.data.as_mut().unwrap_unchecked() };
        let sqe = Mappable::uring_op(data_mut).user_data(op.index as _);

        {
            let mut sq = inner.uring.submission();

            // Push the new operation
            if unsafe { sq.push(&sqe).is_err() } {
                unimplemented!("when is this hit?");
            }
        }
        Ok(op)
    }

    pub(crate) fn poll_op(
        this: &Rc<UnsafeCell<UringInner>>,
        index: usize,
        cx: &mut Context<'_>,
    ) -> Poll<CompletionMeta> {
        let inner = unsafe { &mut *this.get() };
        let lifecycle = unsafe { inner.ops.slab.get(index).unwrap_unchecked() };
        lifecycle.poll_op(cx)
    }

    pub(crate) fn drop_op<T: 'static>(
        this: &Rc<UnsafeCell<UringInner>>,
        index: usize,
        data: &mut Option<T>,
        _skip_cancel: bool,
    ) {
        let inner = unsafe { &mut *this.get() };
        if index == usize::MAX {
            // already finished
            return;
        }
        if let Some(lifecycle) = inner.ops.slab.get(index) {
            let _must_finished = lifecycle.drop_op(data);
            if !_must_finished && !_skip_cancel {
                unsafe {
                    let cancel = opcode::AsyncCancel::new(index as u64)
                        .build()
                        .user_data(u64::MAX);

                    // Try push cancel, if failed, will submit and re-push.
                    if inner.uring.submission().push(&cancel).is_err() {
                        let _ = inner.submit();
                        let _ = inner.uring.submission().push(&cancel);
                    }
                }
            }
        }
    }

    pub(crate) unsafe fn cancel_op(this: &Rc<UnsafeCell<UringInner>>, index: usize) {
        let inner = &mut *this.get();
        let cancel = opcode::AsyncCancel::new(index as u64)
            .build()
            .user_data(u64::MAX);
        if inner.uring.submission().push(&cancel).is_err() {
            let _ = inner.submit();
            let _ = inner.uring.submission().push(&cancel);
        }
    }
}
fn unwrap_to_result(cqe: &cqueue::Entry) -> io::Result<u32> {
    let res = cqe.result();

    if res >= 0 {
        Ok(res as u32)
    } else {
        Err(io::Error::from_raw_os_error(-res))
    }
}
