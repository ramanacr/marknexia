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

/// True when neither `outer` nor the cell that `inner` selects inside it is
/// borrowed: the shell state and the portable state it shares
/// (`Rc<RefCell<_>>` inside the outer `RefCell`). An entry point that may
/// touch either checks this, because an outer frame can hold the inner
/// cell while the outer one is free (CI run 42356283206: a reentrant tab
/// close read the portable state while an outer frame held it mutably).
#[must_use]
pub fn is_free_nested<T, U>(outer: &RefCell<T>, inner: fn(&T) -> &RefCell<U>) -> bool {
    is_free(outer) && read(outer, |value| is_free(inner(value))).unwrap_or(false)
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

/// How a deferred item joins the queue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Admission {
    /// Idempotent work (a repaint, a layout): an item of the same kind that
    /// is already queued is replaced, so it is queued at most once.
    Coalesce,
    /// Work that must never be lost (a close, a selection): it is queued
    /// even past the limit. An identical item already queued moves to the
    /// back, so the latest request wins (selections A, B, A end on A). Its
    /// count is bounded by what it refers to (tabs).
    Critical,
    /// Work that must never be lost and never merge with an identical item
    /// (a button-up: two clicks need two). Queued even past the limit.
    Keep,
    /// Other input: queued while there is room, otherwise dropped (and the
    /// caller logs it).
    Bounded,
}

/// A deferred item's queueing rules.
pub trait Queued {
    fn admission(&self) -> Admission;
    /// Same kind for [`Admission::Coalesce`], identical otherwise.
    fn same(&self, other: &Self) -> bool;
}

/// What [`Deferred::push`] did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Pushed {
    Queued,
    /// Replaced or matched an item already queued.
    Coalesced,
    /// The queue was full (only for [`Admission::Bounded`]).
    Dropped,
}

/// Work that arrived while the state was busy, in arrival order. The queue
/// lives outside the state, so queueing never conflicts with the outer
/// borrow.
#[derive(Debug)]
pub struct Deferred<T> {
    items: RefCell<VecDeque<T>>,
    limit: usize,
}

impl<T: Queued> Deferred<T> {
    #[must_use]
    pub const fn new(limit: usize) -> Self {
        Self {
            items: RefCell::new(VecDeque::new()),
            limit,
        }
    }

    /// Queues `item` by its [`Admission`].
    pub fn push(&self, item: T) -> Pushed {
        let Ok(mut items) = self.items.try_borrow_mut() else {
            // Only reachable if pushing re-entered a push; never panics.
            return Pushed::Dropped;
        };
        match item.admission() {
            Admission::Coalesce => {
                if let Some(slot) = items.iter_mut().find(|queued| queued.same(&item)) {
                    *slot = item;
                    return Pushed::Coalesced;
                }
            }
            Admission::Critical => {
                if let Some(index) = items.iter().position(|queued| queued.same(&item)) {
                    items.remove(index);
                    items.push_back(item);
                    return Pushed::Coalesced;
                }
            }
            Admission::Keep => {}
            Admission::Bounded => {
                if items.len() >= self.limit {
                    return Pushed::Dropped;
                }
            }
        }
        items.push_back(item);
        Pushed::Queued
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

    #[derive(Debug, PartialEq)]
    enum Item {
        Close(u64),
        Select(u64),
        Up(u32),
        Repaint,
        Layout(u32),
        Click(u32),
    }

    impl Queued for Item {
        fn admission(&self) -> Admission {
            match self {
                Self::Close(_) | Self::Select(_) => Admission::Critical,
                Self::Up(_) => Admission::Keep,
                Self::Repaint | Self::Layout(_) => Admission::Coalesce,
                Self::Click(_) => Admission::Bounded,
            }
        }

        fn same(&self, other: &Self) -> bool {
            match (self, other) {
                (Self::Layout(_), Self::Layout(_)) => true,
                _ => self == other,
            }
        }
    }

    #[derive(Default)]
    struct State {
        closes: Vec<u64>,
    }

    /// The shape of the shell's crash: a handler that needs the state runs
    /// while an outer frame holds a borrow (a nested pump). It must defer,
    /// not panic, and the work runs once the outer frame returns.
    fn close_handler(state: &RefCell<State>, deferred: &Deferred<Item>, tab: u64) {
        if !is_free(state) {
            assert_eq!(deferred.push(Item::Close(tab)), Pushed::Queued);
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
        for item in deferred.take() {
            let Item::Close(tab) = item else { panic!() };
            close_handler(&state, &deferred, tab);
        }
        assert_eq!(state.borrow().closes, [7, 8]);
        assert!(deferred.is_empty());
    }

    /// The shell's shape: the portable tab state is an `Rc<RefCell<_>>`
    /// inside the outer state. CI run 42356283206 crashed when a close
    /// read the inner state while an outer frame held it mutably, with
    /// the outer state free. Close and select must defer, then run.
    struct Shell {
        portable: std::rc::Rc<RefCell<crate::app::AppState>>,
    }

    #[derive(Debug, PartialEq)]
    enum ShellWork {
        Close(crate::tabs::TabId),
        Select(crate::tabs::TabId),
    }

    impl Queued for ShellWork {
        fn admission(&self) -> Admission {
            Admission::Critical
        }

        fn same(&self, other: &Self) -> bool {
            self == other
        }
    }

    fn portable(shell: &Shell) -> &RefCell<crate::app::AppState> {
        &shell.portable
    }

    fn run_shell(shell: &RefCell<Shell>, deferred: &Deferred<ShellWork>, work: ShellWork) {
        if !is_free_nested(shell, portable) {
            let _ = deferred.push(work);
            return;
        }
        let app = std::rc::Rc::clone(&shell.borrow().portable);
        let mut app = app.borrow_mut();
        match work {
            ShellWork::Close(tab) => assert!(app.close_tab(tab)),
            ShellWork::Select(tab) => assert!(app.select_tab(tab)),
        }
    }

    #[test]
    fn close_and_select_defer_while_the_inner_portable_state_is_borrowed() {
        let mut app = crate::app::AppState::new();
        let first = app.open_tab("first.md").unwrap();
        let second = app.open_tab("second.md").unwrap();
        let third = app.open_tab("third.md").unwrap();
        let inner = std::rc::Rc::new(RefCell::new(app));
        let shell = RefCell::new(Shell {
            portable: std::rc::Rc::clone(&inner),
        });
        let deferred = Deferred::new(8);
        {
            // An outer frame holds the inner state mutably; the outer
            // state is free (the crash's shape).
            let _held = inner.borrow_mut();
            assert!(is_free(&shell));
            assert!(!is_free_nested(&shell, portable));
            run_shell(&shell, &deferred, ShellWork::Close(second));
            run_shell(&shell, &deferred, ShellWork::Select(first));
        }
        {
            // A shared inner borrow defers too: the handlers write.
            let _held = inner.borrow();
            run_shell(&shell, &deferred, ShellWork::Select(third));
        }
        assert_eq!(deferred.len(), 3);
        assert!(is_free_nested(&shell, portable));
        for work in deferred.take() {
            run_shell(&shell, &deferred, work);
        }
        let app = inner.borrow();
        let ids: Vec<_> = app.tabs().tabs().iter().map(|tab| tab.id()).collect();
        assert_eq!(ids, [first, third]);
        assert_eq!(app.active_tab(), Some(third));
        assert!(deferred.is_empty());
    }

    #[test]
    fn idempotent_work_coalesces_and_critical_work_is_never_dropped() {
        let deferred = Deferred::new(2);
        assert_eq!(deferred.push(Item::Click(1)), Pushed::Queued);
        assert_eq!(deferred.push(Item::Layout(96)), Pushed::Queued);
        // Full: bounded input is dropped, the rest is not.
        assert_eq!(deferred.push(Item::Click(2)), Pushed::Dropped);
        assert_eq!(deferred.push(Item::Layout(144)), Pushed::Coalesced);
        assert_eq!(deferred.push(Item::Repaint), Pushed::Queued);
        assert_eq!(deferred.push(Item::Repaint), Pushed::Coalesced);
        assert_eq!(deferred.push(Item::Close(3)), Pushed::Queued);
        assert_eq!(deferred.push(Item::Close(3)), Pushed::Coalesced);
        assert_eq!(deferred.push(Item::Close(4)), Pushed::Queued);
        // Order is kept; a coalesced layout keeps its place with the latest
        // value.
        assert_eq!(
            deferred.take(),
            [
                Item::Click(1),
                Item::Layout(144),
                Item::Repaint,
                Item::Close(3),
                Item::Close(4)
            ]
        );
        assert!(deferred.take().is_empty());
    }

    #[test]
    fn a_repeated_critical_request_moves_to_the_back() {
        let deferred = Deferred::new(1);
        for item in [Item::Select(1), Item::Select(2), Item::Select(1)] {
            let _ = deferred.push(item);
        }
        // Selections A, B, A end on A.
        assert_eq!(deferred.take(), [Item::Select(2), Item::Select(1)]);
    }

    #[test]
    fn identical_button_ups_are_kept_for_a_double_click() {
        let deferred = Deferred::new(1);
        assert_eq!(deferred.push(Item::Click(5)), Pushed::Queued);
        assert_eq!(deferred.push(Item::Up(5)), Pushed::Queued);
        // Full for bounded input, but button-ups are kept, and never merged.
        assert_eq!(deferred.push(Item::Click(5)), Pushed::Dropped);
        assert_eq!(deferred.push(Item::Up(5)), Pushed::Queued);
        assert_eq!(deferred.take(), [Item::Click(5), Item::Up(5), Item::Up(5)]);
    }
}
