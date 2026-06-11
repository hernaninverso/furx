// events.js — 017 T042 · typed AppEvent client over the WS bridge.
//
// Mirrors the kernel event bus contract (web/src/lib/eventBus.ts): apply an
// AppEvent ONLY if its `seq` is strictly greater than the last applied (a stale
// or replayed seq never overwrites newer state — FR-010). Dedup is implicit in
// the seq monotonicity (an exact replay has seq <= lastSeq → dropped). The frame
// is already HMAC-verified by the caller (defense-in-depth, T063) before reaching
// applyEvent.

let lastSeq = 0;
const handlers = new Set();

/** Cursor of the last applied seq (debug/tests). */
export function lastSeenSeq() {
  return lastSeq;
}

/**
 * Apply an AppEvent envelope { event:{tag,data}, seq }. Returns true if applied
 * (and advances the cursor), false if dropped as stale/replay.
 */
export function applyEvent(env) {
  const seq = Number(env && env.seq);
  if (!Number.isFinite(seq) || seq <= lastSeq) return false; // stale/replay
  lastSeq = seq;
  const ev = env.event || {};
  for (const h of handlers) {
    try {
      h(ev, env);
    } catch (e) {
      // A throwing handler must not kill the others.
      console.warn("[events] handler error", e);
    }
  }
  return true;
}

/** Subscribe to applied AppEvents. Returns an unsubscribe fn. */
export function onAppEvent(handler) {
  handlers.add(handler);
  return () => handlers.delete(handler);
}

/**
 * Reset the seq cursor — call on (re)connect so a fresh connection re-syncs from
 * the server's current seq without a stale floor blocking new events (FR-011).
 * The bridge restarts seq per-connection conceptually; the snapshot/HelloAck is
 * the resync point.
 */
export function resetSeq() {
  lastSeq = 0;
}

// Full reset (tests only).
export function __reset() {
  lastSeq = 0;
  handlers.clear();
}
