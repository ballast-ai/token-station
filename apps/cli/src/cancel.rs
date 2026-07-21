//! A cascading cancellation flag: zero dependencies, safe to poll from the
//! blocking pipeline thread.
//!
//! A running server holds one [`CancelToken::root`] as its **drain** token; each
//! in-flight request carries a [`CancelToken::child`] of it. Cancelling a child
//! (the client hung up) stops just that request; cancelling the root (a
//! save-and-apply drain, a shutdown) is observed by every in-flight request at
//! once — without threading a signal into each blocking read.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// A cheap, clonable cancellation flag with a single level of parenting.
///
/// [`is_cancelled`](Self::is_cancelled) is a couple of atomic loads, so the
/// blocking upstream loop can check it between reads without a runtime.
#[derive(Clone, Default)]
pub struct CancelToken {
    flag: Arc<AtomicBool>,
    parent: Option<Arc<AtomicBool>>,
}

impl CancelToken {
    /// A parentless token — the drain token a running server owns.
    pub fn root() -> Self {
        Self {
            flag: Arc::new(AtomicBool::new(false)),
            parent: None,
        }
    }

    /// A token that is cancelled when it, or its parent, is cancelled.
    pub fn child(&self) -> Self {
        Self {
            flag: Arc::new(AtomicBool::new(false)),
            parent: Some(Arc::clone(&self.flag)),
        }
    }

    /// Trip this token (and, by parenting, everything below it).
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
    }

    /// True once this token or its parent has been cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
            || self
                .parent
                .as_ref()
                .is_some_and(|parent| parent.load(Ordering::SeqCst))
    }
}

#[cfg(test)]
mod tests {
    use super::CancelToken;

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
}
