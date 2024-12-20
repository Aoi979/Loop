use crate::driver::Driver;
use crate::runtime::scheduler::{LocalScheduler, TaskQueue};
use crate::scoped_thread_local;
use crate::task::waker_fn::{dummy_waker, set_poll, should_poll};
use crate::task::{new_task, JoinHandle};
use std::future::Future;

scoped_thread_local!(pub(crate) static CURRENT: Context);

pub(crate) struct Context {
    pub tasks : TaskQueue,
    pub thread_id: usize,
}

impl Context {
    pub(crate) fn new() -> Self {
        let thread_id = crate::runtime::builder::BUILD_THREAD_ID.with(|id| *id);

        Self {
            thread_id,
            tasks: TaskQueue::default(),
        }
    }

}


pub struct Runtime<D> {
    pub(crate) context: Context,
    pub(crate) driver: D,
}

impl<D> Runtime<D> {
    pub(crate) fn new(context: Context, driver: D) -> Self {
        Self { context, driver }
    }

    /// Block on
    pub fn block_on<F>(&mut self, future: F) -> F::Output
    where
        F: Future,
        D: Driver,
    {
        assert!(
            !CURRENT.is_set(),
            "Can not start a runtime inside a runtime"
        );

        let waker = dummy_waker();
        let cx = &mut std::task::Context::from_waker(&waker);

        self.driver.with(|| {
            CURRENT.set(&self.context, || {
                let join = future;

                let mut join = std::pin::pin!(join);
                set_poll();
                loop {
                    loop {
                        // Consume all tasks(with max round to prevent io starvation)
                        let mut max_round = self.context.tasks.len() * 2;
                        while let Some(t) = self.context.tasks.pop() {
                            t.run();
                            if max_round == 0 {
                                // maybe there's a looping task
                                break;
                            } else {
                                max_round -= 1;
                            }
                        }

                        // Check main future
                        while should_poll() {
                            // check
                            if let std::task::Poll::Ready(t) = join.as_mut().poll(cx) {
                                let mut max_round = self.context.tasks.len() * 2;
                                while let Some(t) = self.context.tasks.pop() {
                                    t.run();
                                    if max_round == 0 {
                                        // maybe there's a looping task
                                        break;
                                    } else {
                                        max_round -= 1;
                                    }
                                }
                                return t;
                            }
                        }
                        if self.context.tasks.is_empty() {
                            // No task to execute, we should wait for io blockingly
                            // Hot path
                            break;
                        }
                        // Cold path
                        let _ = self.driver.submit();
                    }
                    // Wait and Process CQ(the error is ignored for not debug mode)
                    let _ = self.driver.park();
                }
            })
        })
    }
}
pub fn spawn<T>(future: T) -> JoinHandle<T::Output>
where
    T: Future + 'static,
    T::Output: 'static,
{
    let (task, join) = new_task(
        crate::utils::thread_id::get_current_thread_id(),
        future,
        LocalScheduler,
    );

    CURRENT.with(|ctx| {
        ctx.tasks.push(task);
    });
    join
}