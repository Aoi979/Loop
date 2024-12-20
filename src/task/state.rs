use std::{
    fmt,
    sync::atomic::{
        AtomicUsize,
        Ordering::{AcqRel, Acquire},
    },
};
use log::trace;

pub(crate) struct State(AtomicUsize);

/// Current state value
#[derive(Copy, Clone)]
pub(crate) struct Snapshot(usize);

type UpdateResult = Result<Snapshot, Snapshot>;

/// The task is currently being run.
const RUNNING: usize = 0b0001;

/// The task is complete.
///
/// Once this bit is set, it is never unset
const COMPLETE: usize = 0b0010;

/// Extracts the task's lifecycle value from the state
const LIFECYCLE_MASK: usize = 0b11;

/// Flag tracking if the task has been pushed into a run queue.
const NOTIFIED: usize = 0b100;

/// The join handle is still around
#[allow(clippy::unusual_byte_groupings)] // https://github.com/rust-lang/rust-clippy/issues/6556
const JOIN_INTEREST: usize = 0b1_000;

/// A join handle waker has been set
#[allow(clippy::unusual_byte_groupings)] // https://github.com/rust-lang/rust-clippy/issues/6556
const JOIN_WAKER: usize = 0b10_000;

/// All bits
const STATE_MASK: usize = LIFECYCLE_MASK | NOTIFIED | JOIN_INTEREST | JOIN_WAKER;

/// Bits used by the ref count portion of the state.
const REF_COUNT_MASK: usize = !STATE_MASK;

/// Number of positions to shift the ref count
const REF_COUNT_SHIFT: usize = REF_COUNT_MASK.count_zeros() as usize;

/// One ref count
const REF_ONE: usize = 1 << REF_COUNT_SHIFT;

/// State a task is initialized with
///
/// A task is initialized with two references:
///
///  * A reference for Task.
///  * A reference for the JoinHandle.
///
/// As the task starts with a `JoinHandle`, `JOIN_INTEREST` is set.
/// As the task starts with a `Notified`, `NOTIFIED` is set.
const INITIAL_STATE: usize = (REF_ONE * 2) | JOIN_INTEREST | NOTIFIED;

#[must_use]
pub(super) enum TransitionToIdle {
    Ok,
    OkNotified,
}

#[must_use]
pub(super) enum TransitionToNotified {
    DoNothing,
    Submit,
}

impl State {
    pub(crate) fn new() -> Self {
        State(AtomicUsize::new(INITIAL_STATE))
    }

    pub(crate) fn load(&self) -> Snapshot {
        Snapshot(self.0.load(Acquire))
    }

    /// Attempt to transition the lifecycle to `Running`. This sets the
    /// notified bit to false so notifications during the poll can be detected.
    pub(super) fn transition_to_running(&self) {
        self.fetch_update_action(|mut curr| {
            debug_assert!(curr.is_notified());
            debug_assert!(curr.is_idle());
            curr.set_running();
            curr.unset_notified();
            ((), Some(curr))
        });
    }

    /// Transitions the task from `Running` -> `Idle`.
    pub(super) fn transition_to_idle(&self) -> TransitionToIdle {
        self.fetch_update_action(|mut curr| {
            debug_assert!(curr.is_running());
            curr.unset_running();
            let action = if curr.is_notified() {
                TransitionToIdle::OkNotified
            } else {
                TransitionToIdle::Ok
            };
            (action, Some(curr))
        })
    }

    /// Transitions the task from `Running` -> `Complete`.
    pub(super) fn transition_to_complete(&self) -> Snapshot {
        const DELTA: usize = RUNNING | COMPLETE;

        let prev = Snapshot(self.0.fetch_xor(DELTA, AcqRel));
        debug_assert!(prev.is_running());
        debug_assert!(!prev.is_complete());

        Snapshot(prev.0 ^ DELTA)
    }

    /// Try transitions the state to `NOTIFIED`, but if it cannot do it without submitting, it will
    /// return false. In another word, if it returns true, it means we have marked the task notified
    /// and do not have to do anything.
    pub(crate) fn transition_to_notified_without_submit(&self) -> bool {
        self.fetch_update_action(|mut curr| {
            if curr.is_running() {
                curr.set_notified();
                (true, Some(curr))
            } else if curr.is_complete() || curr.is_notified() {
                (true, Some(curr))
            } else {
                (false, Some(curr))
            }
        })
    }

    /// Transitions the state to `NOTIFIED`.
    pub(super) fn transition_to_notified(&self) -> TransitionToNotified {
        self.fetch_update_action(|mut curr| {
            let action = if curr.is_running() {
                curr.set_notified();
                TransitionToNotified::DoNothing
            } else if curr.is_complete() || curr.is_notified() {
                TransitionToNotified::DoNothing
            } else {
                curr.set_notified();
                TransitionToNotified::Submit
            };
            (action, Some(curr))
        })
    }

    /// Optimistically tries to swap the state assuming the join handle is
    /// __immediately__ dropped on spawn
    pub(super) fn drop_join_handle_fast(&self) -> Result<(), ()> {
        match self.fetch_update(|curr| {
            if *curr == INITIAL_STATE {
                Some(Snapshot((INITIAL_STATE - REF_ONE) & !JOIN_INTEREST))
            } else {
                None
            }
        }) {
            Ok(_) => {
                trace!(" DEBUG[State]: drop_join_handle_fast");
                Ok(())
            }
            Err(_) => Err(()),
        }
    }

    /// Try to unset the JOIN_INTEREST flag.
    ///
    /// Returns `Ok` if the operation happens before the task transitions to a
    /// completed state, `Err` otherwise.
    pub(super) fn unset_join_interested(&self) -> UpdateResult {
        self.fetch_update(|curr| {
            assert!(curr.is_join_interested());

            if curr.is_complete() {
                return None;
            }

            let mut next = curr;
            next.unset_join_interested();

            Some(next)
        })
    }

    /// Set the `JOIN_WAKER` bit.
    ///
    /// Returns `Ok` if the bit is set, `Err` otherwise. This operation fails if
    /// the task has completed.
    pub(super) fn set_join_waker(&self) -> UpdateResult {
        self.fetch_update(|curr| {
            assert!(curr.is_join_interested());
            assert!(!curr.has_join_waker());

            if curr.is_complete() {
                return None;
            }

            let mut next = curr;
            next.set_join_waker();

            Some(next)
        })
    }

    /// Unsets the `JOIN_WAKER` bit.
    ///
    /// Returns `Ok` has been unset, `Err` otherwise. This operation fails if
    /// the task has completed.
    pub(super) fn unset_waker(&self) -> UpdateResult {
        self.fetch_update(|curr| {
            assert!(curr.is_join_interested());
            assert!(curr.has_join_waker());

            if curr.is_complete() {
                return None;
            }

            let mut next = curr;
            next.unset_join_waker();

            Some(next)
        })
    }

    pub(crate) fn ref_inc(&self) {
        use std::{process, sync::atomic::Ordering::Relaxed};

        let prev = Snapshot(self.0.fetch_add(REF_ONE, Relaxed));

        trace!(
            " DEBUG[State]: ref_inc {}, ptr: {:p}",
            prev.ref_count() + 1,
            self
        );

        // If the reference count overflowed, abort.
        if prev.0 > isize::MAX as usize {
            process::abort();
        }
    }

    /// Returns `true` if the task should be released.
    pub(crate) fn ref_dec(&self) -> bool {
        let prev = Snapshot(self.0.fetch_sub(REF_ONE, AcqRel));
        debug_assert!(prev.ref_count() >= 1);
        trace!(
            " DEBUG[State]: ref_dec {}, ptr: {:p}",
            prev.ref_count() - 1,
            self
        );
        prev.ref_count() == 1
    }

    fn fetch_update_action<F, T>(&self, mut f: F) -> T
    where
        F: FnMut(Snapshot) -> (T, Option<Snapshot>),
    {
        let mut curr = self.load();

        loop {
            let (output, next) = f(curr);
            let next = match next {
                Some(next) => next,
                None => return output,
            };

            let res = self.0.compare_exchange(curr.0, next.0, AcqRel, Acquire);

            match res {
                Ok(_) => return output,
                Err(actual) => curr = Snapshot(actual),
            }
        }
    }

    fn fetch_update<F>(&self, mut f: F) -> Result<Snapshot, Snapshot>
    where
        F: FnMut(Snapshot) -> Option<Snapshot>,
    {
        let mut curr = self.load();

        loop {
            let next = match f(curr) {
                Some(next) => next,
                None => return Err(curr),
            };

            let res = self.0.compare_exchange(curr.0, next.0, AcqRel, Acquire);

            match res {
                Ok(_) => return Ok(next),
                Err(actual) => curr = Snapshot(actual),
            }
        }
    }
}

impl std::ops::Deref for Snapshot {
    type Target = usize;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Snapshot {
    /// Returns `true` if the task is in an idle state.
    pub(super) fn is_idle(self) -> bool {
        self.0 & (RUNNING | COMPLETE) == 0
    }

    /// Returns `true` if the task has been flagged as notified.
    pub(super) fn is_notified(self) -> bool {
        self.0 & NOTIFIED == NOTIFIED
    }

    fn unset_notified(&mut self) {
        self.0 &= !NOTIFIED
    }

    fn set_notified(&mut self) {
        self.0 |= NOTIFIED
    }

    pub(super) fn is_running(self) -> bool {
        self.0 & RUNNING == RUNNING
    }

    fn set_running(&mut self) {
        self.0 |= RUNNING;
    }

    fn unset_running(&mut self) {
        self.0 &= !RUNNING;
    }

    /// Returns `true` if the task's future has completed execution.
    pub(super) fn is_complete(self) -> bool {
        self.0 & COMPLETE == COMPLETE
    }

    pub(super) fn is_join_interested(self) -> bool {
        self.0 & JOIN_INTEREST == JOIN_INTEREST
    }

    fn unset_join_interested(&mut self) {
        self.0 &= !JOIN_INTEREST
    }

    pub(super) fn has_join_waker(self) -> bool {
        self.0 & JOIN_WAKER == JOIN_WAKER
    }

    fn set_join_waker(&mut self) {
        self.0 |= JOIN_WAKER;
    }

    fn unset_join_waker(&mut self) {
        self.0 &= !JOIN_WAKER
    }

    pub(super) fn ref_count(self) -> usize {
        (self.0 & REF_COUNT_MASK) >> REF_COUNT_SHIFT
    }
}

impl fmt::Debug for State {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        let snapshot = self.load();
        snapshot.fmt(fmt)
    }
}

impl fmt::Debug for Snapshot {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt.debug_struct("Snapshot")
            .field("is_running", &self.is_running())
            .field("is_complete", &self.is_complete())
            .field("is_notified", &self.is_notified())
            .field("is_join_interested", &self.is_join_interested())
            .field("has_join_waker", &self.has_join_waker())
            .field("ref_count", &self.ref_count())
            .finish()
    }
}
