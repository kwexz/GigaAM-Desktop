//! Bounded device recovery policy, separate from platform resource ownership.
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(Debug, PartialEq, Eq)]
pub enum RecoveryAction {
    KeepCurrent,
    RetryAfter(Duration),
    Fail,
}

pub fn is_device_invalidation(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("88890004") || message.contains("88890026")
        || message.contains("device_invalidated")
}

pub fn reset_after_healthy_capture(attempts: &mut usize, healthy_audio_samples: usize) {
    if healthy_audio_samples >= 48_000 * 2 { *attempts = 0; }
}

pub fn device_recovery_action(invalidated: bool, attempts: usize, explicitly_selected: bool) -> RecoveryAction {
    if !invalidated { return RecoveryAction::KeepCurrent; }
    // Never replace a user-selected microphone with a different endpoint.
    if explicitly_selected || attempts >= 3 { return RecoveryAction::Fail; }
    RecoveryAction::RetryAfter(Duration::from_millis(100 * (1 << attempts)))
}

/// One acquisition attempt of a capture source. `consecutive_failures` may be
/// reset by the attempt once it has demonstrably captured healthy audio, so
/// independent recovery episodes share one bounded budget only while recent
/// failures keep happening.
pub trait CaptureAttempt {
    fn run(&mut self, consecutive_failures: &mut usize) -> Result<(), String>;
}

#[derive(Debug, PartialEq, Eq)]
pub enum SupervisorEvent<'a> {
    AttemptStarted { resumed: bool },
    Reconnecting { attempt: usize, delay: Duration },
    Failed { error: &'a str },
    Stopped,
}

fn wait_for_retry(stop: &AtomicBool, delay: Duration) -> bool {
    let deadline = Instant::now() + delay;
    loop {
        if stop.load(Ordering::Relaxed) { return false; }
        let now = Instant::now();
        if now >= deadline { return true; }
        std::thread::sleep((deadline - now).min(Duration::from_millis(10)));
    }
}

/// Shared production control loop for capture recovery. The Windows platform
/// supplies a real WASAPI attempt; recovery tests supply scripted attempts so
/// device-add, invalidation, exhaustion and selected-device policy all run
/// through this same code.
pub fn supervise_capture(
    attempt: &mut dyn CaptureAttempt,
    stop: &AtomicBool,
    explicitly_selected: bool,
    emit: &mut dyn FnMut(SupervisorEvent<'_>),
) {
    let mut failures = 0usize;
    loop {
        if stop.load(Ordering::Relaxed) {
            emit(SupervisorEvent::Stopped);
            return;
        }
        emit(SupervisorEvent::AttemptStarted { resumed: failures > 0 });
        match attempt.run(&mut failures) {
            // Attempts return normally on user stop or a clean end of input.
            Ok(()) => return,
            Err(message) => match device_recovery_action(
                is_device_invalidation(&message),
                failures,
                explicitly_selected,
            ) {
                RecoveryAction::RetryAfter(delay) => {
                    failures += 1;
                    emit(SupervisorEvent::Reconnecting { attempt: failures, delay });
                    if !wait_for_retry(stop, delay) {
                        emit(SupervisorEvent::Stopped);
                        return;
                    }
                }
                RecoveryAction::KeepCurrent | RecoveryAction::Fail => {
                    emit(SupervisorEvent::Failed { error: &message });
                    return;
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    enum Step {
        Frames(u32),
        Stop,
        Fail(&'static str),
    }

    struct ScriptedAttempt {
        script: VecDeque<Step>,
        starts: usize,
        frames: Vec<u32>,
    }

    impl ScriptedAttempt {
        fn new(script: Vec<Step>) -> Self {
            Self { script: script.into(), starts: 0, frames: Vec::new() }
        }
    }

    impl CaptureAttempt for ScriptedAttempt {
        fn run(&mut self, _failures: &mut usize) -> Result<(), String> {
            self.starts += 1;
            loop {
                match self.script.pop_front() {
                    None | Some(Step::Stop) => return Ok(()),
                    Some(Step::Frames(count)) => {
                        for index in 0..count {
                            self.frames.push(self.starts as u32 * 1000 + index);
                            std::thread::sleep(Duration::from_millis(2));
                        }
                    }
                    Some(Step::Fail(message)) => return Err(message.into()),
                }
            }
        }
    }

    fn record(events: &mut Vec<String>) -> impl FnMut(SupervisorEvent<'_>) + '_ {
        move |event| match event {
            SupervisorEvent::AttemptStarted { resumed } => events.push(format!("started:{resumed}")),
            SupervisorEvent::Reconnecting { attempt, delay } => {
                events.push(format!("reconnecting:{attempt}:{}", delay.as_millis()))
            }
            SupervisorEvent::Failed { error } => events.push(format!("failed:{error}")),
            SupervisorEvent::Stopped => events.push("stopped".into()),
        }
    }

    #[test]
    fn added_device_frames_do_not_restart_capture() {
        let mut attempt = ScriptedAttempt::new(vec![Step::Frames(3), Step::Stop]);
        let mut events = Vec::new();
        supervise_capture(&mut attempt, &AtomicBool::new(false), false, &mut record(&mut events));
        assert_eq!(attempt.starts, 1);
        assert_eq!(attempt.frames.len(), 3);
        assert_eq!(events, ["started:false"]);
    }

    #[test]
    fn invalidation_recovers_through_shared_supervisor() {
        let mut attempt = ScriptedAttempt::new(vec![
            Step::Frames(1),
            Step::Fail("AUDCLNT_E_DEVICE_INVALIDATED"),
            Step::Frames(2),
            Step::Stop,
        ]);
        let mut events = Vec::new();
        supervise_capture(&mut attempt, &AtomicBool::new(false), false, &mut record(&mut events));
        assert_eq!(attempt.starts, 2);
        assert_eq!(attempt.frames.len(), 3);
        assert_eq!(events, ["started:false", "reconnecting:1:100", "started:true"]);
    }

    #[test]
    fn repeated_invalidations_end_with_visible_failure() {
        let mut attempt = ScriptedAttempt::new(vec![
            Step::Fail("88890004"),
            Step::Fail("88890004"),
            Step::Fail("88890004"),
            Step::Fail("88890004"),
        ]);
        let mut events = Vec::new();
        supervise_capture(&mut attempt, &AtomicBool::new(false), false, &mut record(&mut events));
        assert_eq!(attempt.starts, 4);
        assert_eq!(events.iter().filter(|event| event.starts_with("reconnecting")).count(), 3);
        assert!(events.last().unwrap().starts_with("failed:"));
    }

    #[test]
    fn explicitly_selected_microphone_fails_without_fallback() {
        let mut attempt = ScriptedAttempt::new(vec![
            Step::Fail("88890004"),
            Step::Frames(1),
            Step::Stop,
        ]);
        let mut events = Vec::new();
        supervise_capture(&mut attempt, &AtomicBool::new(false), true, &mut record(&mut events));
        assert_eq!(attempt.starts, 1);
        assert!(attempt.frames.is_empty());
        assert_eq!(events, ["started:false", "failed:88890004"]);
    }

    #[test]
    fn unrelated_failure_is_terminal_without_reconnect() {
        let mut attempt = ScriptedAttempt::new(vec![Step::Fail("permission denied")]);
        let mut events = Vec::new();
        supervise_capture(&mut attempt, &AtomicBool::new(false), false, &mut record(&mut events));
        assert_eq!(attempt.starts, 1);
        assert_eq!(events, ["started:false", "failed:permission denied"]);
    }

    #[test]
    fn stop_during_backoff_returns_promptly_without_failure() {
        let stop = AtomicBool::new(false);
        let mut attempt = ScriptedAttempt::new(vec![Step::Fail("88890004")]);
        let mut events = Vec::new();
        let mut emit = |event: SupervisorEvent<'_>| {
            if matches!(event, SupervisorEvent::Reconnecting { .. }) {
                stop.store(true, Ordering::Relaxed);
            }
            record(&mut events)(event);
        };
        let started = Instant::now();
        supervise_capture(&mut attempt, &stop, false, &mut emit);
        assert!(started.elapsed() < Duration::from_millis(90));
        assert_eq!(attempt.starts, 1);
        assert_eq!(events, ["started:false", "reconnecting:1:100", "stopped"]);
    }

    #[test]
    fn healthy_track_continues_while_peer_reconnects() {
        let mut healthy = ScriptedAttempt::new(vec![Step::Frames(40), Step::Stop]);
        let mut broken = ScriptedAttempt::new(vec![
            Step::Fail("88890004"),
            Step::Fail("88890004"),
            Step::Stop,
        ]);
        let (healthy_result, broken_result) = std::thread::scope(|scope| {
            let healthy_handle = scope.spawn(|| {
                let mut events = Vec::new();
                supervise_capture(&mut healthy, &AtomicBool::new(false), false, &mut record(&mut events));
                (healthy.starts, healthy.frames.len(), events)
            });
            let broken_handle = scope.spawn(|| {
                let mut events = Vec::new();
                supervise_capture(&mut broken, &AtomicBool::new(false), false, &mut record(&mut events));
                (broken.starts, broken.frames.len(), events)
            });
            (healthy_handle.join().unwrap(), broken_handle.join().unwrap())
        });
        assert_eq!(healthy_result.0, 1);
        assert_eq!(healthy_result.1, 40, "healthy track must keep capturing during peer recovery");
        assert_eq!(broken_result.0, 3);
        assert_eq!(
            broken_result.2.iter().filter(|event| event.starts_with("reconnecting")).count(),
            2
        );
        assert_eq!(broken_result.2.last().unwrap(), "started:true");
    }
    #[test]
    fn separate_healthy_recovery_episodes_do_not_exhaust_lifetime_budget() {
        let mut attempts = 0;
        for _ in 0..4 {
            assert!(matches!(device_recovery_action(true, attempts, false), RecoveryAction::RetryAfter(_)));
            attempts += 1;
            reset_after_healthy_capture(&mut attempts, 96_000);
            assert_eq!(attempts, 0);
        }
        attempts = 3;
        reset_after_healthy_capture(&mut attempts, 100);
        assert_eq!(device_recovery_action(true, attempts, false), RecoveryAction::Fail);
    }
    #[test]
    fn invalidation_classification_does_not_retry_unrelated_failures() {
        assert!(is_device_invalidation("HRESULT 0x88890004"));
        assert!(is_device_invalidation("AUDCLNT_E_DEVICE_INVALIDATED"));
        assert!(!is_device_invalidation("permission denied"));
        assert!(!is_device_invalidation("device added"));
    }
    #[test]
    fn added_device_does_not_restart_healthy_capture() {
        assert_eq!(device_recovery_action(false, 0, false), RecoveryAction::KeepCurrent);
    }
    #[test]
    fn invalidation_has_bounded_backoff_and_no_selected_device_fallback() {
        assert_eq!(device_recovery_action(true, 0, false), RecoveryAction::RetryAfter(Duration::from_millis(100)));
        assert_eq!(device_recovery_action(true, 2, false), RecoveryAction::RetryAfter(Duration::from_millis(400)));
        assert_eq!(device_recovery_action(true, 3, false), RecoveryAction::Fail);
        assert_eq!(device_recovery_action(true, 0, true), RecoveryAction::Fail);
    }
}
