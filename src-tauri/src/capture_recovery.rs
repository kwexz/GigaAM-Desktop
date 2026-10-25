//! Bounded device recovery policy, separate from platform resource ownership.
use std::time::Duration;

#[derive(Debug, PartialEq, Eq)]
pub enum RecoveryAction {
    KeepCurrent,
    RetryAfter(Duration),
    Fail,
}

pub fn device_recovery_action(invalidated: bool, attempts: usize, explicitly_selected: bool) -> RecoveryAction {
    if !invalidated { return RecoveryAction::KeepCurrent; }
    // Never replace a user-selected microphone with a different endpoint.
    if explicitly_selected || attempts >= 3 { return RecoveryAction::Fail; }
    RecoveryAction::RetryAfter(Duration::from_millis(100 * (1 << attempts)))
}

#[cfg(test)]
mod tests {
    use super::*;
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
