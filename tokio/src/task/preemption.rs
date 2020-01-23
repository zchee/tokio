//! Opt-in preeemption points for improved cooperative scheduling.
//!
//! A single call to `poll` on a top-level task may potentially do a lot of work before it returns
//! `Poll::Pending`. If a task runs for a long period of time without yielding back to the
//! executor, it can starve other tasks waiting on that executor to execute them, or drive
//! underlying resources. Since Rust does not have a runtime, it is difficult to forcibly preempt a
//! long-running task. Instead, this module provides an opt-in mechanism for futures to collaborate
//! with the executor to avoid starvation.
//!
//! Consider a future like this one:
//!
//! ```
//! # use tokio::stream::StreamExt;
//! async fn drop_all<I: Stream>(input: I) {
//!     while let Some(_) = input.next().await {}
//! }
//! ```
//!
//! It may look harmless, but consider what happens under heavy load if the input stream is
//! _always_ ready. If we spawn `drop_all`, the task will never yield, and will starve other tasks
//! and resources on the same executor. With opt-in preemption, this problem is alleviated:
//!
//! ```
//! # use tokio::stream::StreamExt;
//! async fn drop_all<I: Stream>(input: I) {
//!     while let Some(_) = input.next().await {
//!         tokio::preemption::check().await;
//!     }
//! }
//! ```
//!
//! The [`check`] future will coordinate with the executor to make sure that every so often control
//! is yielded back to the executor so it can run other tasks.
//!
//! # Placing preemption points
//!
//! Preemption points should be placed _after_ at least some work has been done. If they are not, a
//! future sufficiently deep in the task hierarchy may end up _never_ getting to run because of the
//! number of preemption points that inevitably appear before it is reached.

use std::cell::Cell;
use std::task::{Context, Poll};

/// Constant used to determine how much "work" a task is allowed to do without yielding.
///
/// The value itself is chosen somewhat arbitrarily. It needs to be high enough to amortize wakeup
/// and scheduling costs, but low enough that we do not starve other tasks for too long. The value
/// also needs to be high enough that particularly deep tasks are able to do at least some useful
/// work at all.
///
/// Note that as more preemption points are added in the ecosystem, this value will probably also
/// have to be raised.
const BUDGET: usize = 128;

thread_local! {
    static HITS: Cell<usize> = Cell::new(0);
}

/// Mark that the top-level task yielded, and that the preemption budget should be reset.
pub(crate) fn yielded() {
    HITS.with(|hits| {
        hits.set(BUDGET);
    });
}

/// Returns `Poll::Pending` if the current task has exceeded its preemption budget and should yield.
#[allow(unreachable_pub)]
pub fn poll(cx: &mut Context<'_>) -> Poll<()> {
    HITS.with(|hits| {
        let n = hits.get();
        if n == 0 {
            cx.waker().wake_by_ref();
            Poll::Pending
        } else {
            hits.set(n.saturating_sub(1));
            Poll::Ready(())
        }
    })
}

/// Resolves immediately unless the current task has already exceeded its preemption budget.
///
/// This should be placed after at least some work has been done. Otherwise a future sufficiently
/// deep in the task hierarchy may end up never getting to run because of the number of preemption
/// points that inevitably appear before it is even reached. For example:
///
/// ```
/// # use tokio::stream::StreamExt;
/// async fn drop_all<I: Stream>(input: I) {
///     while let Some(_) = input.next().await {
///         tokio::preemption::check().await;
///     }
/// }
/// ```
#[allow(unreachable_pub)]
pub async fn check() {
    use crate::future::poll_fn;
    poll_fn(|cx| poll(cx)).await;
}