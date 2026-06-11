-- 1.8 — Latency heatmap LLM providers.
-- Cada poll AIE /v1/resilience/state writes 1 row per provider with latency
-- ms (= cached field; AIE doesn't expose raw latency, we approximate by
-- measuring our HTTP round-trip to the endpoint as a coarse proxy).

CREATE TABLE IF NOT EXISTS provider_latency_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    at TEXT NOT NULL DEFAULT (datetime('now')),
    provider TEXT NOT NULL,
    blocked INTEGER NOT NULL DEFAULT 0,
    rtt_ms INTEGER,
    note TEXT
);

CREATE INDEX IF NOT EXISTS idx_latency_at ON provider_latency_history(at DESC);
CREATE INDEX IF NOT EXISTS idx_latency_provider_at ON provider_latency_history(provider, at DESC);
