//! Per-request lifecycle: the cancel signal, the overall deadline and the
//! per-attempt timeout that let the blocking pipeline stop instead of running
//! an abandoned request to completion.
//!
//! The server layer builds one of these per request as a child of the running
//! server's drain token, then hands it to [`crate::gateway::Gateway::chat`]. The
//! gateway consumes it: it polls [`is_cancelled`](RequestContext::is_cancelled)
//! between upstream reads, so a client that hangs up (or a drain that fires)
//! stops the exchange rather than paying for output nobody will read.

use std::time::{Duration, Instant};

use crate::cancel::{CancelReason, CancelToken};

/// The lifetime and stop conditions of a single in-flight request.
pub struct RequestContext {
    cancel: CancelToken,
    deadline: Instant,
    per_attempt_timeout: Duration,
}

impl RequestContext {
    /// Build a request-scoped context under a server's drain token.
    #[must_use]
    pub fn new(drain: &CancelToken, total: Duration, per_attempt: Duration) -> Self {
        Self {
            cancel: drain.child(),
            deadline: Instant::now() + total,
            per_attempt_timeout: per_attempt,
        }
    }

    /// A standalone context with no drain parent — for tests and callers that do
    /// not (yet) run under a supervised server.
    #[must_use]
    pub fn detached(total: Duration, per_attempt: Duration) -> Self {
        Self::new(&CancelToken::root(), total, per_attempt)
    }

    /// Trip this request's cancel — the client disconnected.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// True once the client hung up, the drain fired, or the deadline passed.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancel_reason().is_some()
    }

    /// The exact lifecycle reason, including a deadline that is derived from
    /// the monotonic clock rather than written into the cancellation token.
    #[must_use]
    pub fn cancel_reason(&self) -> Option<CancelReason> {
        self.cancel
            .cancel_reason()
            .or_else(|| self.remaining().is_zero().then_some(CancelReason::Deadline))
    }

    /// Time left before the overall deadline (zero once it has passed).
    #[must_use]
    pub fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }

    /// The cap on a single upstream attempt.
    #[must_use]
    pub fn per_attempt_timeout(&self) -> Duration {
        self.per_attempt_timeout
    }

    /// A handle to hand an upstream client for abort/read-timeout slicing.
    #[must_use]
    pub fn token(&self) -> CancelToken {
        self.cancel.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::RequestContext;
    use crate::cancel::{CancelReason, CancelToken};
    use std::time::Duration;

    #[test]
    fn a_cancelled_client_cancels_the_context() {
        let ctx = RequestContext::detached(Duration::from_mins(1), Duration::from_secs(30));
        assert!(!ctx.is_cancelled());
        ctx.cancel();
        assert!(ctx.is_cancelled());
        assert_eq!(ctx.cancel_reason(), Some(CancelReason::ClientDisconnect));
    }

    #[test]
    fn a_zero_deadline_reads_as_cancelled() {
        let ctx = RequestContext::detached(Duration::ZERO, Duration::from_secs(30));
        assert!(ctx.is_cancelled());
        assert_eq!(ctx.cancel_reason(), Some(CancelReason::Deadline));
    }

    #[test]
    fn a_drain_cascades_into_live_contexts() {
        let drain = CancelToken::root();
        let ctx = RequestContext::new(&drain, Duration::from_mins(1), Duration::from_secs(30));
        assert!(!ctx.is_cancelled());
        drain.cancel_with(CancelReason::ServerDrain);
        assert!(ctx.is_cancelled());
        assert_eq!(ctx.cancel_reason(), Some(CancelReason::ServerDrain));
    }
}
