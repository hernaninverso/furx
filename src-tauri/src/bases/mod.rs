// 4 bases transversales (Plan v4.2):
//   1. PaneInputRouter — idempotencia (correlation_id, action_id) con LRU.
//   2. PaneStateModel — FSM Idle/Busy/Ready/Error + heartbeat-is-truth.
//   3. Audit — append-only schema + triggers (db migration 001) + writer helpers.
//   4. Scheduler — backpressure global con rate-limit por sujeto.

pub mod allowlist;
pub mod audit;
pub mod guardrail;
pub mod router;
pub mod scheduler;
pub mod state;
