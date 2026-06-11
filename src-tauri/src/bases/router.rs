// PaneInputRouter — idempotencia por (correlation_id, action_id).
// Si el frontend reintenta una acción con el mismo par, devolvemos el resultado
// cacheado en lugar de re-ejecutar — protege contra dobles spawns/writes ante red flaky.

use lru::LruCache;
use parking_lot::Mutex;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ActionKey {
    pub correlation_id: String,
    pub action_id: String,
}

#[derive(Debug, Clone)]
pub struct ActionRecord {
    pub at: Instant,
    pub outcome: ActionOutcome,
}

#[derive(Debug, Clone)]
pub enum ActionOutcome {
    Ok(String),
    Err(String),
}

const TTL: Duration = Duration::from_secs(300);

#[derive(Clone)]
pub struct InputRouter {
    cache: Arc<Mutex<LruCache<ActionKey, ActionRecord>>>,
}

impl InputRouter {
    pub fn new() -> Self {
        Self {
            cache: Arc::new(Mutex::new(LruCache::new(NonZeroUsize::new(4096).unwrap()))),
        }
    }

    /// Returns cached outcome if the action ran in the last TTL window; otherwise None.
    pub fn check(&self, key: &ActionKey) -> Option<ActionOutcome> {
        let mut cache = self.cache.lock();
        if let Some(rec) = cache.get(key) {
            if rec.at.elapsed() < TTL {
                return Some(rec.outcome.clone());
            }
        }
        cache.pop(key);
        None
    }

    pub fn record(&self, key: ActionKey, outcome: ActionOutcome) {
        let mut cache = self.cache.lock();
        cache.put(
            key,
            ActionRecord {
                at: Instant::now(),
                outcome,
            },
        );
    }
}

impl Default for InputRouter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedups_within_ttl() {
        let r = InputRouter::new();
        let k = ActionKey {
            correlation_id: "c1".into(),
            action_id: "a1".into(),
        };
        assert!(r.check(&k).is_none());
        r.record(k.clone(), ActionOutcome::Ok("done".into()));
        match r.check(&k) {
            Some(ActionOutcome::Ok(v)) => assert_eq!(v, "done"),
            other => panic!("expected Ok(done), got {:?}", other),
        }
    }
}
