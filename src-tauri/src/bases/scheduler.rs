// Backpressure Scheduler — rate-limit global por sujeto (provider, comando, etc.).
// Usa governor crate (token bucket). Si un sujeto satura su quota, el scheduler
// encola la acción y aplica delay con jitter.

use governor::{
    clock::{Clock, DefaultClock},
    state::{InMemoryState, NotKeyed},
    Quota, RateLimiter,
};
use nonzero_ext::nonzero;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

type Limiter = RateLimiter<NotKeyed, InMemoryState, DefaultClock>;

#[derive(Clone)]
pub struct Scheduler {
    limiters: Arc<Mutex<HashMap<String, Arc<Limiter>>>>,
}

impl Scheduler {
    pub fn new() -> Self {
        Self {
            limiters: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Get-or-create a limiter for `subject`. Default: 30 req/min (burst 10).
    /// Custom quotas can be added via `set_quota`.
    pub fn limiter(&self, subject: &str) -> Arc<Limiter> {
        let mut g = self.limiters.lock();
        g.entry(subject.to_string())
            .or_insert_with(|| {
                Arc::new(RateLimiter::direct(
                    Quota::per_minute(nonzero!(30u32)).allow_burst(nonzero!(10u32)),
                ))
            })
            .clone()
    }

    pub fn set_quota(&self, subject: &str, quota: Quota) {
        self.limiters
            .lock()
            .insert(subject.to_string(), Arc::new(RateLimiter::direct(quota)));
    }

    /// Try acquire one token. Returns None if available, Some(wait) if must wait.
    pub fn try_acquire(&self, subject: &str) -> Option<Duration> {
        let lim = self.limiter(subject);
        match lim.check() {
            Ok(_) => None,
            Err(neg) => Some(neg.wait_time_from(DefaultClock::default().now())),
        }
    }
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_burst_then_blocks() {
        let s = Scheduler::new();
        // Default burst: 10. First 10 should pass.
        let mut blocked = false;
        for i in 0..20 {
            if s.try_acquire("provider:test").is_some() {
                blocked = true;
                assert!(i >= 10, "blocked too early at {}", i);
                break;
            }
        }
        assert!(blocked, "expected backpressure after burst");
    }
}
