//! Blocking tasks related.

use std::{future::Future, task::Poll};
use threadpool::{Builder as ThreadPoolBuilder, ThreadPool as ThreadPoolImpl};

/// Users may implement a ThreadPool and attach it to runtime.
/// We also provide an implementation based on threadpool crate, you can use DefaultThreadPool.
pub trait ThreadPool {
    /// Monoio runtime will call `schedule_task` on `spawn_blocking`.
    /// ThreadPool impl must execute it now or later.
    fn schedule_task(&self, task: BlockingTask);
}

/// Error on waiting blocking task.
#[derive(Debug, Clone, Copy)]
pub enum JoinError {
    /// Task is canceled.
    Canceled,
}

/// BlockingTask is contrusted by monoio, ThreadPool impl
/// will execute it with `.run()`.
pub struct BlockingTask {
    task: Option<crate::task::Task<NoopScheduler>>,
    blocking_vtable: &'static BlockingTaskVtable,
}

unsafe impl Send for BlockingTask {}

struct BlockingTaskVtable {
    pub(crate) drop: unsafe fn(&mut crate::task::Task<NoopScheduler>),
}





impl Drop for BlockingTask {
    fn drop(&mut self) {
        if let Some(task) = self.task.as_mut() {
            unsafe { (self.blocking_vtable.drop)(task) };
        }
    }
}

impl BlockingTask {
    /// Run task.
    #[inline]
    pub fn run(mut self) {
        let task = self.task.take().unwrap();
        task.run();
        // // if we are within a runtime, just run it.
        // if crate::runtime::CURRENT.is_set() {
        //     task.run();
        //     return;
        // }
        // // if we are on a standalone thread, we will use thread local ctx as Context.
        // crate::runtime::DEFAULT_CTX.with(|ctx| {
        //     crate::runtime::CURRENT.set(ctx, || task.run());
        // });
    }
}

/// BlockingStrategy can be set if there is no ThreadPool attached.
/// It controls how to handle `spawn_blocking` without thread pool.
#[derive(Clone, Copy, Debug)]
pub enum BlockingStrategy {
    /// Panic when `spawn_blocking`.
    Panic,
    /// Execute with current thread when `spawn_blocking`.
    ExecuteLocal,
}

/// DefaultThreadPool is a simple wrapped `threadpool::ThreadPool` that implement
/// `monoio::blocking::ThreadPool`. You may use this implementation, or you can use your own thread
/// pool implementation.
#[derive(Clone)]
pub struct DefaultThreadPool {
    pool: ThreadPoolImpl,
}

impl DefaultThreadPool {
    /// Create a new DefaultThreadPool.
    pub fn new(num_threads: usize) -> Self {
        let pool = ThreadPoolBuilder::default()
            .num_threads(num_threads)
            .build();
        Self { pool }
    }
}

impl ThreadPool for DefaultThreadPool {
    #[inline]
    fn schedule_task(&self, task: BlockingTask) {
        self.pool.execute(move || task.run());
    }
}

pub(crate) struct NoopScheduler;

impl crate::task::Schedule for NoopScheduler {
    fn schedule(&self, _task: crate::task::Task<Self>) {
        unreachable!()
    }

    fn yield_now(&self, _task: crate::task::Task<Self>) {
        unreachable!()
    }
}

pub(crate) enum BlockingHandle {
    Attached(Box<dyn ThreadPool + Send + 'static>),
    Empty(BlockingStrategy),
}

impl From<BlockingStrategy> for BlockingHandle {
    fn from(value: BlockingStrategy) -> Self {
        Self::Empty(value)
    }
}

struct BlockingFuture<F>(Option<F>);

impl<T> Unpin for BlockingFuture<T> {}

impl<F, R> Future for BlockingFuture<F>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    type Output = Result<R, JoinError>;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let me = &mut *self;
        let func = me.0.take().expect("blocking task ran twice.");
        Poll::Ready(Ok(func()))
    }
}
