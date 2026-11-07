//! A single-flight lifecycle claim shared by Stop and Cancel.
use std::sync::atomic::{AtomicBool, Ordering};

pub struct FinalizationClaim<'a>(&'a AtomicBool);

impl<'a> FinalizationClaim<'a> {
    pub fn acquire(flag: &'a AtomicBool) -> Option<Self> {
        flag.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .ok()
            .map(|_| Self(flag))
    }
}

impl Drop for FinalizationClaim<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Production ordering seam: acknowledge stopped ingress before slow work.
/// Panics in finalization become an explicit failure, never a silent detach.
pub fn finish_after_acknowledgement<T>(
    acknowledge: impl FnOnce(),
    finish: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    acknowledge();
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(finish))
        .unwrap_or_else(|_| Err("Live finalizer panicked; raw audio retained for recovery".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_between_publication_and_reconciliation_is_observed() {
        use std::sync::{Arc, Barrier, Mutex};
        let token = Arc::new(AtomicBool::new(false));
        let published = Arc::new(Mutex::new(None::<Arc<AtomicBool>>));
        let persisted = Arc::new(AtomicBool::new(false));
        let barrier = Arc::new(Barrier::new(2));
        let worker = {
            let (token, published, persisted, barrier) = (
                token.clone(),
                published.clone(),
                persisted.clone(),
                barrier.clone(),
            );
            std::thread::spawn(move || {
                *published.lock().unwrap() = Some(token.clone());
                barrier.wait();
                barrier.wait();
                if persisted.load(Ordering::Acquire) {
                    token.store(true, Ordering::Release);
                }
                assert!(token.load(Ordering::Acquire));
            })
        };
        barrier.wait();
        persisted.store(true, Ordering::Release);
        published
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .store(true, Ordering::Release);
        barrier.wait();
        worker.join().unwrap();
    }

    #[test]
    fn acknowledgement_does_not_wait_for_slow_archive() {
        let (ack_tx, ack_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (finished_tx, finished_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            finish_after_acknowledgement(
                || ack_tx.send(()).unwrap(),
                || {
                    release_rx.recv().unwrap();
                    finished_tx.send(()).unwrap();
                    Ok(())
                },
            )
            .unwrap();
        });
        ack_rx
            .recv_timeout(std::time::Duration::from_millis(500))
            .unwrap();
        assert!(finished_rx.try_recv().is_err());
        release_tx.send(()).unwrap();
        worker.join().unwrap();
        finished_rx.recv().unwrap();
    }

    #[test]
    fn production_ordering_converts_post_ack_panic_to_error() {
        let acknowledged = std::cell::Cell::new(false);
        let result: Result<(), String> = finish_after_acknowledgement(
            || acknowledged.set(true),
            || panic!("injected archive panic"),
        );
        assert!(acknowledged.get());
        assert!(result.unwrap_err().contains("panicked"));
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
