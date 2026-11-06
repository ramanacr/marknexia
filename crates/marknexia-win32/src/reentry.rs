//! Reentrancy rules for the shell state.
//!
//! The shell keeps its state in one `Rc<RefCell<_>>`. Window messages, UIA
//! calls and COM callbacks can re-enter the window procedure while an outer
//! frame holds a borrow (for example inside a nested message pump of an
//! outgoing COM call, or a message sent synchronously to the window). A
//! `RefCell` borrow conflict there would panic. Entry points therefore check
//! [`is_free`] first and, while the state is busy, queue their work in a
//! [`Deferred`] list kept outside the state; the message loop runs it once
//! the outer frame has returned. No entry point panics on a reentrant path.

use std::{cell::RefCell, collections::VecDeque};

/// True when no borrow of `cell` is outstanding, so an entry point may
/// borrow it (shared or mutable) without a conflict.
#[must_use]
pub fn is_free<T>(cell: &RefCell<T>) -> bool {
    cell.try_borrow_mut().is_ok()
}

/// Runs `work` with a shared borrow, or returns `None` while a mutable
/// borrow is outstanding.
pub fn read<T, R>(cell: &RefCell<T>, work: impl FnOnce(&T) -> R) -> Option<R> {
    cell.try_borrow().ok().map(|value| work(&value))
}

/// Runs `work` with a mutable borrow, or returns `None` while any borrow is
/// outstanding.
pub fn write<T, R>(cell: &RefCell<T>, work: impl FnOnce(&mut T) -> R) -> Option<R> {
    cell.try_borrow_mut().ok().map(|mut value| work(&mut value))
}

/// Work that arrived while the state was busy, in arrival order, bounded.
/// The queue lives outside the state, so queueing never conflicts with the
/// outer borrow.
#[derive(Debug)]
pub struct Deferred<T> {
    items: RefCell<VecDeque<T>>,
    limit: usize,
}

impl<T> Deferred<T> {
    #[must_use]
    pub const fn new(limit: usize) -> Self {
        Self {
            items: RefCell::new(VecDeque::new()),
            limit,
        }
    }

    /// Queues `item`; returns false (and drops it) when the queue is full.
    pub fn push(&self, item: T) -> bool {
        match self.items.try_borrow_mut() {
            Ok(mut items) if items.len() < self.limit => {
                items.push_back(item);
                true
            }
            _ => false,
        }
    }

    /// Takes everything queued so far. Work queued while the taken items run
    /// waits for the next call.
    pub fn take(&self) -> Vec<T> {
        self.items
            .try_borrow_mut()
            .map(|mut items| items.drain(..).collect())
            .unwrap_or_default()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.items.try_borrow().map_or(0, |items| items.len())
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct State {
        closes: Vec<u64>,
    }

    /// The shape of the shell's crash: a handler that needs the state runs
    /// while an outer frame holds a shared borrow (a nested pump). It must
    /// defer, not panic, and the work runs once the outer frame returns.
    fn close_handler(state: &RefCell<State>, deferred: &Deferred<u64>, tab: u64) {
        if !is_free(state) {
            assert!(deferred.push(tab));
            return;
        }
        write(state, |state| state.closes.push(tab)).expect("free");
    }

    #[test]
    fn a_reentrant_handler_defers_instead_of_panicking() {
        let state = RefCell::new(State::default());
        let deferred = Deferred::new(8);
        {
            let _outer = state.borrow();
            close_handler(&state, &deferred, 7);
            assert_eq!(read(&state, |state| state.closes.len()), Some(0));
            assert_eq!(write(&state, |_| ()), None, "shared borrow outstanding");
        }
        {
            let _outer = state.borrow_mut();
            close_handler(&state, &deferred, 8);
            assert_eq!(read(&state, |_| ()), None, "mutable borrow outstanding");
        }
        assert_eq!(deferred.len(), 2);
        // Back at the top of the message loop: the queue runs in order.
        for tab in deferred.take() {
            close_handler(&state, &deferred, tab);
        }
        assert_eq!(state.borrow().closes, [7, 8]);
        assert!(deferred.is_empty());
    }

    #[test]
    fn the_deferred_queue_is_bounded() {
        let deferred = Deferred::new(2);
        assert!(deferred.push(1));
        assert!(deferred.push(2));
        assert!(!deferred.push(3));
        assert_eq!(deferred.take(), [1, 2]);
        assert!(deferred.take().is_empty());
    }
}
