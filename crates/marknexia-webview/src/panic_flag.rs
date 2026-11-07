//! Records a panic caught inside a WebView2 callback so it is not lost.
//!
//! Release builds unwind, and a panic must never unwind across an
//! `extern "system"` COM callback. Every WebView2 event handler body runs
//! under [`PanicFlag::call`]: web resource requests, navigation starting
//! and completed, web messages (including parsing the untrusted JSON), new
//! window requests, accelerator keys, process failures, browser exit, and the
//! environment- and controller-created completions (a panic there delivers
//! an `Err(CallbackPanicked)` result).
//! Each handler fails closed on a panic (deny and close the controller,
//! cancel the navigation, mark the new window handled). The session also
//! guards the application's page observer. A caught panic means a
//! notification may have been dropped, so `WebViewSession::poll` reports it
//! once as `SessionError::CallbackPanicked`, including panics recorded by a
//! controller or browser-exit subscription that has since been retired.

use std::{
    cell::Cell,
    panic::{AssertUnwindSafe, catch_unwind},
    rc::Rc,
};

/// Shared record of panics caught in WebView2 callbacks. The session owns
/// one and passes it to every environment, controller and subscription
/// callback it registers.
#[derive(Clone, Debug, Default)]
pub struct PanicFlag(Rc<Cell<bool>>);

impl PanicFlag {
    /// Runs `work`; a panic is caught and recorded.
    pub(crate) fn guard(&self, work: impl FnOnce()) {
        let _ = self.call(work);
    }

    /// Runs `work` and returns its value, or `None` after recording a
    /// caught panic. Callers decide the fail-closed result.
    pub(crate) fn call<T>(&self, work: impl FnOnce() -> T) -> Option<T> {
        match catch_unwind(AssertUnwindSafe(work)) {
            Ok(value) => Some(value),
            Err(_) => {
                self.0.set(true);
                None
            }
        }
    }

    /// True once per recorded panic.
    pub fn take(&self) -> bool {
        self.0.replace(false)
    }
}

#[cfg(test)]
mod tests {
    use super::PanicFlag;

    #[test]
    fn a_caught_panic_is_reported_once() {
        let flag = PanicFlag::default();
        flag.guard(|| {});
        assert!(!flag.take());
        let shared = flag.clone();
        shared.guard(|| panic!("callback failure"));
        assert!(flag.take());
        assert!(!flag.take());
    }
}
