//! One browser slot with bounded FIFO admission. Waiting requests own no browser resources.

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;

    fn wait_for_size(queue: &SearchQueue, expected: usize) {
        let state = queue.state.lock().unwrap();
        let (state, result) = queue
            .changed
            .wait_timeout_while(state, Duration::from_secs(3), |state| {
                state.waiters.len() != expected
            })
            .unwrap();
        assert!(
            !result.timed_out(),
            "Expected {expected} waiters, got {}",
            state.waiters.len()
        );
    }

    #[test]
    fn concurrent_requests_wait_in_fifo_order_and_reuse_capacity() {
        let queue = SearchQueue::default();
        let first = queue.acquire(&|| false, 3, Duration::from_secs(3)).unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::scope(|scope| {
            for index in 0..3 {
                let queue = &queue;
                let tx = tx.clone();
                scope.spawn(move || {
                    let _permit = queue.acquire(&|| false, 3, Duration::from_secs(3)).unwrap();
                    tx.send(index).unwrap();
                });
                wait_for_size(queue, index + 1);
            }
            assert!(queue.busy());
            assert!(
                queue
                    .acquire(&|| false, 3, Duration::from_secs(3))
                    .err()
                    .unwrap()
                    .contains("full")
            );
            drop(first);
            for index in 0..3 {
                assert_eq!(rx.recv_timeout(Duration::from_secs(3)).unwrap(), index);
            }
        });
        assert!(!queue.busy());
        assert!(queue.acquire(&|| false, 3, Duration::ZERO).is_ok());
    }

    #[test]
    fn cancellation_removes_a_waiter_without_releasing_the_active_slot() {
        let queue = SearchQueue::default();
        let first = queue.acquire(&|| false, 1, Duration::from_secs(3)).unwrap();
        let cancelled = AtomicBool::new(false);
        std::thread::scope(|scope| {
            let waiting = scope.spawn(|| {
                queue
                    .acquire(
                        &|| cancelled.load(Ordering::Acquire),
                        1,
                        Duration::from_secs(3),
                    )
                    .err()
                    .unwrap()
            });
            wait_for_size(&queue, 1);
            cancelled.store(true, Ordering::Release);
            assert!(waiting.join().unwrap().contains("cancelled"));
            wait_for_size(&queue, 0);
            assert!(queue.busy());
        });
        drop(first);
        assert!(!queue.busy());
    }

    #[test]
    fn timeout_removes_its_ticket_and_later_requests_can_run() {
        let queue = SearchQueue::default();
        let first = queue.acquire(&|| false, 1, Duration::from_secs(1)).unwrap();
        assert!(
            queue
                .acquire(&|| false, 1, Duration::ZERO)
                .err()
                .unwrap()
                .contains("timed out")
        );
        wait_for_size(&queue, 0);
        assert!(queue.busy());
        drop(first);
        assert!(queue.acquire(&|| false, 1, Duration::ZERO).is_ok());
    }

    #[test]
    fn cancellation_takes_precedence_over_available_capacity() {
        let queue = SearchQueue::default();
        assert!(
            queue
                .acquire(&|| true, 1, Duration::ZERO)
                .err()
                .unwrap()
                .contains("cancelled")
        );
        assert!(!queue.busy());
    }

    #[test]
    fn unwinding_removes_waiting_and_active_permits() {
        let queue = SearchQueue::default();
        let first = queue.acquire(&|| false, 1, Duration::from_secs(3)).unwrap();
        let invoked = AtomicBool::new(false);
        let result = std::panic::catch_unwind(|| {
            let _permit = queue.acquire(
                &|| {
                    assert!(
                        !invoked.swap(true, Ordering::Relaxed),
                        "Simulated cancellation callback panic"
                    );
                    false
                },
                1,
                Duration::from_secs(3),
            );
        });
        assert!(result.is_err());
        wait_for_size(&queue, 0);
        assert!(queue.busy());
        drop(first);
        let result = std::panic::catch_unwind(|| {
            let _permit = queue.acquire(&|| false, 1, Duration::ZERO).unwrap();
            panic!("Simulated browser failure");
        });
        assert!(result.is_err());
        assert!(!queue.busy());
    }
}

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
struct State {
    active: bool,
    waiters: VecDeque<Arc<()>>,
}

#[derive(Default)]
pub(super) struct SearchQueue {
    state: Mutex<State>,
    changed: Condvar,
}

pub(super) struct Permit<'a> {
    queue: &'a SearchQueue,
    ticket: Arc<()>,
    registered: bool,
    active: bool,
}

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        if !self.registered {
            return;
        }
        let mut state = self
            .queue
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.active {
            state.active = false;
        } else {
            state
                .waiters
                .retain(|ticket| !Arc::ptr_eq(ticket, &self.ticket));
        }
        self.queue.changed.notify_all();
    }
}

impl SearchQueue {
    pub(super) fn busy(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .active
    }

    pub(super) fn acquire(
        &self,
        cancelled: &dyn Fn() -> bool,
        capacity: usize,
        timeout: Duration,
    ) -> Result<Permit<'_>, String> {
        if cancelled() {
            return Err("Browser search was cancelled.".into());
        }
        let started = Instant::now();
        let mut permit = Permit {
            queue: self,
            ticket: Arc::new(()),
            registered: false,
            active: false,
        };
        {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !state.active && state.waiters.is_empty() {
                state.active = true;
                permit.active = true;
                permit.registered = true;
                return Ok(permit);
            }
            if state.waiters.len() >= capacity {
                return Err("Browser search queue is full. Retry later.".into());
            }
            state.waiters.push_back(Arc::clone(&permit.ticket));
            permit.registered = true;
            self.changed.notify_all();
        }
        loop {
            // Invoke external callbacks without the queue lock. Permit cleanup also handles unwinding.
            if cancelled() {
                return Err("Browser search was cancelled while waiting.".into());
            }
            let remaining = timeout.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                return Err("Browser search queue wait timed out. Retry later.".into());
            }
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !state.active
                && state
                    .waiters
                    .front()
                    .is_some_and(|ticket| Arc::ptr_eq(ticket, &permit.ticket))
            {
                state.waiters.pop_front();
                state.active = true;
                permit.active = true;
                return Ok(permit);
            }
            drop(
                self.changed
                    .wait_timeout(state, remaining.min(Duration::from_millis(50)))
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            );
        }
    }
}
