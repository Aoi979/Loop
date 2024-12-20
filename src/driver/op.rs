use std::{
    future::Future,
    io,
    pin::Pin,
    task::{Context, Poll},
};
use std::task::ready;
use crate::driver;



/// In-flight operation
pub(crate) struct Op<T: 'static + OpAble> {
    // Driver running the operation
    pub(super) driver: driver::Inner,

    // Operation index in the slab(useless for legacy)
    pub(super) index: usize,

    // Per-operation data
    pub(super) data: Option<T>,
}

/// Operation completion. Returns stored state with the result of the operation.
#[derive(Debug)]
pub(crate) struct Completion<T> {
    pub(crate) data: T,
    pub(crate) meta: CompletionMeta,
}

/// Operation completion meta info.
#[derive(Debug)]
pub(crate) struct CompletionMeta {
    pub(crate) result: io::Result<MaybeFd>,
    #[allow(unused)]
    pub(crate) flags: u32,
}

/// MaybeFd is a wrapper for fd or a normal number. If it is marked as fd, it will close the fd when
/// dropped.
/// Use `into_inner` to take the inner fd or number and skip the drop.
///
/// This wrapper is designed to be used in the syscall return value. It can prevent fd leak when the
/// operation is cancelled.
#[derive(Debug)]
pub(crate) struct MaybeFd {
    is_fd: bool,
    fd: u32,
}

impl MaybeFd {
    #[inline]
    pub(crate) unsafe fn new_result(fdr: io::Result<u32>, is_fd: bool) -> io::Result<Self> {
        fdr.map(|fd| Self { is_fd, fd })
    }

    #[inline]
    pub(crate) unsafe fn new_fd_result(fdr: io::Result<u32>) -> io::Result<Self> {
        fdr.map(|fd| Self { is_fd: true, fd })
    }

    #[inline]
    pub(crate) fn new_non_fd_result(fdr: io::Result<u32>) -> io::Result<Self> {
        fdr.map(|fd| Self { is_fd: false, fd })
    }

    #[inline]
    pub(crate) const unsafe fn new_fd(fd: u32) -> Self {
        Self { is_fd: true, fd }
    }

    #[inline]
    pub(crate) const fn new_non_fd(fd: u32) -> Self {
        Self { is_fd: false, fd }
    }

    #[inline]
    pub(crate) const fn into_inner(self) -> u32 {
        let fd = self.fd;
        std::mem::forget(self);
        fd
    }

    #[inline]
    pub(crate) const fn zero() -> Self {
        Self {
            is_fd: false,
            fd: 0,
        }
    }

    #[inline]
    pub(crate) fn fd(&self) -> u32 {
        self.fd
    }
}

impl Drop for MaybeFd {
    fn drop(&mut self) {
        // The fd close only executed when:
        // 1. the operation is cancelled
        // 2. the cancellation failed
        // 3. the returned result is a fd
        // So this is a relatively cold path. For simplicity, we just do a close syscall here
        // instead of pushing close op.
        if self.is_fd {
            unsafe {
                libc::close(self.fd as libc::c_int);
            }
        }
    }
}

pub(crate) trait OpAble {
    const RET_IS_FD: bool = false;
    const SKIP_CANCEL: bool = false;
    fn uring_op(&mut self) -> io_uring::squeue::Entry;

}



impl<T: OpAble> Op<T> {
    /// Submit an operation to uring.
    ///
    /// `state` is stored during the operation tracking any state submitted to
    /// the kernel.
    pub(super) fn submit_with(data: T) -> io::Result<Op<T>> {
        driver::CURRENT.with(|this| this.submit_with(data))
    }

    /// Try submitting an operation to uring
    #[allow(unused)]
    pub(super) fn try_submit_with(data: T) -> io::Result<Op<T>> {
        if driver::CURRENT.is_set() {
            Op::submit_with(data)
        } else {
            Err(io::ErrorKind::Other.into())
        }
    }

    pub(crate) fn op_canceller(&self) -> OpCanceller {
        OpCanceller {
            index: self.index,
        }
    }
}

impl<T> Future for Op<T>
where
    T: Unpin + OpAble + 'static,
{
    type Output = Completion<T>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let me = &mut *self;
        let data_mut = me.data.as_mut().expect("unexpected operation state");
        let meta = ready!(me.driver.poll_op::<T>(data_mut, me.index, cx));
        me.index = usize::MAX;
        let data = me.data.take().expect("unexpected operation state");
        Poll::Ready(Completion { data, meta })
    }
}

impl<T: OpAble> Drop for Op<T> {
    #[inline]
    fn drop(&mut self) {
        self.driver
            .drop_op(self.index, &mut self.data, T::SKIP_CANCEL);
    }
}

#[derive(Debug, Eq, PartialEq, Clone, Hash)]
pub(crate) struct OpCanceller {
    pub(super) index: usize,
}

impl OpCanceller {
    pub(crate) unsafe fn cancel(&self) {
        super::CURRENT.with(|inner| inner.cancel_op(self))
    }
}
