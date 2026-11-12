//! One-way cancellation shared by turn runners and provider streams.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

/// A clonable, one-way cancellation flag with wakeups.
///
/// The primitive lives in `cowbot-core` because both the turn runner and the
/// LLM stream observe it; `cowbot-llm` re-exports it as `CancelSignal`.
/// Cancelling is idempotent and wakes every waiter, so a stopped turn aborts
/// the upstream request instead of waiting for the next chunk (ADR-0039).
#[derive(Clone, Default)]
pub struct CancelSignal {
    state: Arc<CancelState>,
}

#[derive(Default)]
struct CancelState {
    cancelled: AtomicBool,
    waiters: Mutex<Vec<Waker>>,
}

impl CancelSignal {
    pub fn new() -> Self {
        Self::default()
    }

    /// Stop the work this signal guards. Repeated calls are no-ops.
    pub fn cancel(&self) {
        if self.state.cancelled.swap(true, Ordering::SeqCst) {
            return;
        }
        let waiters = self
            .state
            .waiters
            .lock()
            .map(|mut waiters| std::mem::take(&mut *waiters))
            .unwrap_or_default();
        for waker in waiters {
            waker.wake();
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.state.cancelled.load(Ordering::SeqCst)
    }

    /// Resolve as soon as the signal is cancelled.
    pub fn cancelled(&self) -> CancelWait<'_> {
        CancelWait { signal: self }
    }
}

impl std::fmt::Debug for CancelSignal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CancelSignal")
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}

/// The future returned by [`CancelSignal::cancelled`].
pub struct CancelWait<'a> {
    signal: &'a CancelSignal,
}

impl Future for CancelWait<'_> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<()> {
        if self.signal.is_cancelled() {
            return Poll::Ready(());
        }
        // Register first, then re-check, so a cancellation that lands between
        // the two cannot be missed.
        if let Ok(mut waiters) = self.signal.state.waiters.lock() {
            waiters.push(context.waker().clone());
        }
        if self.signal.is_cancelled() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn cancel_wakes_a_waiter_and_is_idempotent() {
        let signal = CancelSignal::new();
        assert!(!signal.is_cancelled());

        let waiter = signal.clone();
        let task = tokio::spawn(async move {
            waiter.cancelled().await;
            true
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        signal.cancel();
        signal.cancel();

        assert!(signal.is_cancelled());
        assert!(task.await.unwrap());
    }

    #[tokio::test]
    async fn an_already_cancelled_signal_resolves_immediately() {
        let signal = CancelSignal::new();
        signal.cancel();
        tokio::time::timeout(Duration::from_millis(50), signal.cancelled())
            .await
            .expect("a cancelled signal resolves without waiting");
    }
}
