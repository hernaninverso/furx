// PaneStateModel — FSM con heartbeat-is-truth.
//
// Estados:
//   Idle    — pane sin actividad reciente.
//   Busy    — recibió input, esperando output (LLM thinking, shell ejecutando).
//   Ready   — output disponible / prompt activo / esperando user input.
//   Error   — falla del proceso, requiere atención.
//
// Truth source (priority decreasing): manual override > OSC 133 marker > heartbeat
// ticker (last 60s evidencia output) > prompt detector heurístico > idle timer.
// El heartbeat ticker corre en background y marca Idle si pasaron >5min sin output.

use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PaneState {
    Idle,
    Busy,
    Ready,
    Error,
}

#[derive(Debug, Clone)]
pub struct PaneStateRecord {
    pub state: PaneState,
    pub since: Instant,
    pub last_output_at: Option<Instant>,
    pub last_input_at: Option<Instant>,
    pub override_until: Option<Instant>,
}

impl Default for PaneStateRecord {
    fn default() -> Self {
        Self {
            state: PaneState::Idle,
            since: Instant::now(),
            last_output_at: None,
            last_input_at: None,
            override_until: None,
        }
    }
}

#[derive(Clone, Default)]
pub struct PaneStateModel {
    panes: Arc<Mutex<HashMap<String, PaneStateRecord>>>,
}

impl PaneStateModel {
    pub fn new() -> Self {
        Self {
            panes: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn on_input(&self, pane_id: &str) {
        let mut g = self.panes.lock();
        let rec = g.entry(pane_id.to_string()).or_default();
        rec.last_input_at = Some(Instant::now());
        rec.state = PaneState::Busy;
        rec.since = Instant::now();
    }

    pub fn on_output(&self, pane_id: &str) {
        let mut g = self.panes.lock();
        let rec = g.entry(pane_id.to_string()).or_default();
        rec.last_output_at = Some(Instant::now());
        // Si estaba Busy, una nueva ola de output mantiene Busy; el ticker decidirá cuándo a Ready.
        if rec.state != PaneState::Error {
            rec.state = PaneState::Busy;
            rec.since = Instant::now();
        }
    }

    pub fn on_error(&self, pane_id: &str) {
        let mut g = self.panes.lock();
        let rec = g.entry(pane_id.to_string()).or_default();
        rec.state = PaneState::Error;
        rec.since = Instant::now();
    }

    pub fn manual_override(&self, pane_id: &str, target: PaneState, duration: Duration) {
        let mut g = self.panes.lock();
        let rec = g.entry(pane_id.to_string()).or_default();
        rec.state = target;
        rec.since = Instant::now();
        rec.override_until = Some(Instant::now() + duration);
    }

    pub fn get(&self, pane_id: &str) -> Option<PaneStateRecord> {
        self.panes.lock().get(pane_id).cloned()
    }

    /// Tick — corre cada 60s, decide transiciones automáticas.
    /// Busy → Ready si no hay output en últimos 3s.
    /// Ready → Idle si no hay actividad en últimos 5min.
    pub fn tick(&self) {
        let now = Instant::now();
        let mut g = self.panes.lock();
        for rec in g.values_mut() {
            if let Some(until) = rec.override_until {
                if now < until {
                    continue;
                }
                rec.override_until = None;
            }
            if rec.state == PaneState::Error {
                continue;
            }
            let last_out = rec.last_output_at.unwrap_or(rec.since);
            let last_in = rec.last_input_at.unwrap_or(rec.since);
            let last_activity = last_out.max(last_in);
            let idle_for = now.duration_since(last_activity);

            rec.state = match rec.state {
                PaneState::Busy if idle_for > Duration::from_secs(3) => PaneState::Ready,
                PaneState::Ready if idle_for > Duration::from_secs(300) => PaneState::Idle,
                other => other,
            };
        }
    }

    pub fn forget(&self, pane_id: &str) {
        self.panes.lock().remove(pane_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    #[test]
    fn transitions_busy_to_ready_after_quiet() {
        let m = PaneStateModel::new();
        m.on_input("p1");
        assert_eq!(m.get("p1").unwrap().state, PaneState::Busy);
        sleep(Duration::from_millis(3100));
        m.tick();
        assert_eq!(m.get("p1").unwrap().state, PaneState::Ready);
    }
}
