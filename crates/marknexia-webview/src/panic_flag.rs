//! Records a panic caught inside a COM callback so it is not lost.
//!
//! Release builds unwind, so callbacks wrap application work in
//! `catch_unwind` (a panic must not cross the COM boundary). A caught panic
//! means a failure notification (process failure, browser exit, page
//! message, accelerator) may have been dropped; the flag lets the next
//! `WebViewSession::poll` report it as an error instead.

use std::{
    cell::Cell,
    panic::{AssertUnwindSafe, catch_unwind},
    rc::Rc,
};

#[derive(Clone, Debug, Default)]
pub(crate) struct PanicFlag(Rc<Cell<bool>>);

impl PanicFlag {
    /// Runs `work`; a panic is caught and recorded.
    pub(crate) fn guard(&self, work: impl FnOnce()) {
        if catch_unwind(AssertUnwindSafe(work)).is_err() {
            self.0.set(true);
        }
    }

    /// True once per recorded panic.
    pub(crate) fn take(&self) -> bool {
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
