//! A cascading cancellation flag: zero dependencies, safe to poll from the
//! blocking pipeline thread.
//!
//! A running server holds one [`CancelToken::root`] as its **drain** token; each
//! in-flight request carries a [`CancelToken::child`] of it. Cancelling a child
//! (the client hung up) stops just that request; cancelling the root (a
//! save-and-apply drain, a shutdown) is observed by every in-flight request at
//! once — without threading a signal into each blocking read.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use tokio_util::sync::CancellationToken as AsyncCancellationToken;

/// Why a request stopped. The value is closed so lifecycle, protocol rendering
/// and metrics cannot silently disagree about cancellation semantics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CancelReason {
    ClientDisconnect = 1,
    ServerDrain = 2,
    Deadline = 3,
}

/// A cheap, clonable cancellation flag with a single level of parenting.
///
/// [`is_cancelled`](Self::is_cancelled) is a couple of atomic loads, so the
/// blocking upstream loop can check it between reads without a runtime.
#[derive(Clone, Default)]
pub struct CancelToken {
    reason: Arc<AtomicU8>,
    parent: Option<Arc<AtomicU8>>,
    async_signal: AsyncCancellationToken,
}

impl CancelToken {
    /// A parentless token — the drain token a running server owns.
    #[must_use]
    pub fn root() -> Self {
        Self {
            reason: Arc::new(AtomicU8::new(0)),
            parent: None,
            async_signal: AsyncCancellationToken::new(),
        }
    }

    /// A token that is cancelled when it, or its parent, is cancelled.
    #[must_use]
    pub fn child(&self) -> Self {
        Self {
            reason: Arc::new(AtomicU8::new(0)),
            parent: Some(Arc::clone(&self.reason)),
            async_signal: self.async_signal.child_token(),
        }
    }

    /// Trip this request token because its client disconnected.
    pub fn cancel(&self) {
        self.cancel_with(CancelReason::ClientDisconnect);
    }

    /// Trip this token with an explicit lifecycle reason.
    pub fn cancel_with(&self, reason: CancelReason) {
        let _ = self
            .reason
            .compare_exchange(0, reason as u8, Ordering::SeqCst, Ordering::SeqCst);
        self.async_signal.cancel();
    }

    /// True once this token or its parent has been cancelled.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancel_reason().is_some()
    }

    /// The request-local reason wins over a later parent drain.
    #[must_use]
    pub fn cancel_reason(&self) -> Option<CancelReason> {
        decode(self.reason.load(Ordering::SeqCst)).or_else(|| {
            self.parent
                .as_ref()
                .and_then(|parent| decode(parent.load(Ordering::SeqCst)))
        })
    }

    /// A clone of the same cancellation source for async provider transports.
    /// Root cancellation cascades through Tokio's child-token relationship.
    /// callers must use [`cancel_with`](Self::cancel_with) so the closed reason
    /// and the wake-up remain one operation.
    #[must_use]
    pub fn async_token(&self) -> AsyncCancellationToken {
        self.async_signal.clone()
    }
}

fn decode(value: u8) -> Option<CancelReason> {
    match value {
        1 => Some(CancelReason::ClientDisconnect),
        2 => Some(CancelReason::ServerDrain),
        3 => Some(CancelReason::Deadline),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{CancelReason, CancelToken};

    #[test]
    fn a_fresh_token_is_not_cancelled() {
        assert!(!CancelToken::root().is_cancelled());
    }

    #[test]
    fn cancelling_a_child_leaves_the_parent_and_siblings_alone() {
        let root = CancelToken::root();
        let a = root.child();
        let b = root.child();
        a.cancel();
        assert!(a.is_cancelled());
        assert!(!b.is_cancelled());
        assert!(!root.is_cancelled());
    }

    #[test]
    fn cancelling_the_root_cascades_to_every_child() {
        let root = CancelToken::root();
        let a = root.child();
        let b = root.child();
        root.cancel();
        assert!(root.is_cancelled());
        assert!(a.is_cancelled());
        assert!(b.is_cancelled());
    }

    #[test]
    fn a_server_drain_reason_reaches_children_without_overwriting_a_client_disconnect() {
        let root = CancelToken::root();
        let disconnected = root.child();
        let draining = root.child();
        disconnected.cancel();
        root.cancel_with(CancelReason::ServerDrain);
        assert_eq!(
            disconnected.cancel_reason(),
            Some(CancelReason::ClientDisconnect)
        );
        assert_eq!(draining.cancel_reason(), Some(CancelReason::ServerDrain));
    }

    #[tokio::test]
    async fn async_cancellation_uses_the_same_child_and_root_signal() {
        let root = CancelToken::root();
        let disconnected = root.child();
        let draining = root.child();
        let untouched = root.child();
        let disconnected_async = disconnected.async_token();
        let draining_async = draining.async_token();
        let untouched_async = untouched.async_token();

        disconnected.cancel();
        disconnected_async.cancelled().await;
        assert!(!untouched_async.is_cancelled());

        root.cancel_with(CancelReason::ServerDrain);
        draining_async.cancelled().await;
        untouched_async.cancelled().await;
        assert_eq!(
            disconnected.cancel_reason(),
            Some(CancelReason::ClientDisconnect)
        );
        assert_eq!(draining.cancel_reason(), Some(CancelReason::ServerDrain));
    }
}
