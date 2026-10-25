//! A single-flight lifecycle claim shared by Stop and Cancel.
use std::sync::atomic::{AtomicBool, Ordering};

pub struct FinalizationClaim<'a>(&'a AtomicBool);

impl<'a> FinalizationClaim<'a> {
    pub fn acquire(flag: &'a AtomicBool) -> Option<Self> {
        flag.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .ok().map(|_| Self(flag))
    }
}

impl Drop for FinalizationClaim<'_> {
    fn drop(&mut self) { self.0.store(false, Ordering::SeqCst); }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_stop_cannot_claim_same_session() {
        let flag = AtomicBool::new(false);
        let first = FinalizationClaim::acquire(&flag).unwrap();
        assert!(FinalizationClaim::acquire(&flag).is_none());
        drop(first);
        assert!(FinalizationClaim::acquire(&flag).is_some());
    }

    #[test]
    fn panic_releases_claim() {
        let flag = AtomicBool::new(false);
        let result = std::panic::catch_unwind(|| {
            let _claim = FinalizationClaim::acquire(&flag).unwrap();
            panic!("injected finalizer failure");
        });
        assert!(result.is_err());
        assert!(FinalizationClaim::acquire(&flag).is_some());
    }
}
