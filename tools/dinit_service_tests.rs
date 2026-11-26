//! Host tests for the currently in-kernel service supervisor policy.
extern crate alloc;
#[path = "../src/dinit/service.rs"]
mod service;
use service::{RestartPolicy, ServiceDescriptor, ServiceStatus};

#[test]
fn restart_waits_for_backoff_for_all_policies() {
    for policy in [RestartPolicy::Always, RestartPolicy::OnFailure, RestartPolicy::UnlessStopped] {
        let mut s = ServiceDescriptor::new("svc", "/svc.mex", 3);
        s.restart_policy = policy;
        s.mark_started(1, 100);
        assert!(s.mark_crashed(-1, 200));
        assert!(!s.can_restart(1199));
        assert!(s.can_restart(1200));
        s.mark_stopped();
        assert!(!s.can_restart(u64::MAX));
    }
}

#[test]
fn retries_are_bounded_and_waiting_is_not_healthy_uptime() {
    let mut s = ServiceDescriptor::new("svc", "/svc.mex", 3);
    s.mark_started(1, 0);
    assert!(s.mark_crashed(-1, 1));
    s.mark_started(2, 40_000); // long wait, not long uptime
    assert!(s.mark_crashed(-1, 40_001));
    assert_eq!(s.crash_count, 2);
    assert!(s.mark_crashed(-1, 50_000)); // launch failure
    assert!(s.mark_crashed(-1, 60_000));
    assert!(!s.mark_crashed(-1, 70_000));
    assert_eq!(s.status, ServiceStatus::Crashed);
    assert!(!s.can_restart(u64::MAX));
}

#[test]
fn healthy_running_service_resets_crash_history() {
    let mut s = ServiceDescriptor::new("svc", "/svc.mex", 3);
    s.mark_started(1, 0);
    s.mark_crashed(-1, 1);
    s.mark_started(2, 2_000);
    s.mark_crashed(-1, 32_000);
    assert_eq!(s.crash_count, 1);
    assert_eq!(s.restart_backoff_ms, 1_000);
}

#[test]
fn reconciliation_requires_exact_image_and_authority() {
    let mut service = ServiceDescriptor::new("svc", "/system/svc.elf", 3);
    assert!(service.can_reconcile("/system/svc.elf", 0, 7, 7));
    assert!(!service.can_reconcile("/system/other.elf", 0, 7, 7));
    assert!(!service.can_reconcile("/system/svc.elf", 1000, 7, 7));
    assert!(!service.can_reconcile("/system/svc.elf", 0, 15, 7));
    service.binary_path.clear();
    assert!(!service.can_reconcile("", 0, 7, 7));
    let builtin = ServiceDescriptor::builtin("svc", RestartPolicy::Always, true);
    assert!(!builtin.can_reconcile("", 0, 7, 7));
}
