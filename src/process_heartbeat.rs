//! Monotonic heartbeat accounting. Policy such as restart belongs to the
//! supervisor; this module only reports the process's observed state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Health {
    /// No heartbeat has arrived, but the initial grace period has not elapsed.
    Starting,
    /// A heartbeat was received within the timeout, including its boundary.
    Alive,
    /// No heartbeat has arrived in time (or the initial grace period expired).
    TimedOut,
}
#[derive(Clone, Copy, Debug)]
pub struct Heartbeat { started_ms: u64, last_ms: Option<u64> }
impl Heartbeat {
    pub const fn new(started_ms: u64) -> Self { Self { started_ms, last_ms: None } }
    pub fn record(&mut self, now_ms: u64) { self.last_ms = Some(now_ms); }
    pub fn last_ms(&self) -> Option<u64> { self.last_ms }
    pub fn health(&self, now_ms: u64, timeout_ms: u64) -> Health {
        let reference = self.last_ms.unwrap_or(self.started_ms);
        if now_ms.saturating_sub(reference) > timeout_ms { Health::TimedOut }
        else if self.last_ms.is_some() { Health::Alive }
        else { Health::Starting }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn checks_startup_timeout_recovery_and_boundary() {
        let mut hb = Heartbeat::new(100);
        assert_eq!(hb.health(200, 100), Health::Starting);
        assert_eq!(hb.health(201, 100), Health::TimedOut);
        hb.record(210);
        assert_eq!(hb.last_ms(), Some(210));
        assert_eq!(hb.health(310, 100), Health::Alive);
        assert_eq!(hb.health(311, 100), Health::TimedOut);
        hb.record(320);
        assert_eq!(hb.health(320, 100), Health::Alive);
    }
    #[test] fn backwards_clock_saturates_and_zero_is_not_missing() {
        let mut hb = Heartbeat::new(0);
        hb.record(0);
        assert_eq!(hb.last_ms(), Some(0));
        assert_eq!(hb.health(0, 0), Health::Alive);
        hb.record(100);
        assert_eq!(hb.health(99, 0), Health::Alive);
    }
}
