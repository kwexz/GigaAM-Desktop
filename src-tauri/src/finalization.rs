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
    fn acknowledgement_does_not_wait_for_slow_archive() {
        let (ack_tx, ack_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (finished_tx, finished_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            ack_tx.send(()).unwrap();
            // Deterministic blocked-storage simulation, released by caller.
            release_rx.recv().unwrap();
            finished_tx.send(()).unwrap();
        });
        ack_rx.recv_timeout(std::time::Duration::from_millis(500)).unwrap();
        assert!(finished_rx.try_recv().is_err());
        release_tx.send(()).unwrap();
        worker.join().unwrap();
        finished_rx.recv().unwrap();
    }

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
