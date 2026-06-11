// PLAN_CLOSE J — extracted polling hook. Stable callback via ref to avoid
// stale-closure (Block-A council MUST-FIX, unanimous 5/5 + Codex).
//
// API kept tiny so it doesn't grow into "half of Shell.tsx as args" (codex
// nice-to-have "shell-refactor").

import { useEffect, useRef } from "react";

interface Options {
  /** Polling interval in ms. Re-creating the timer only when this changes. */
  intervalMs: number;
  /** When true, no timer is installed (e.g. drawer closed). */
  enabled?: boolean;
  /** Whether to fire `fn` immediately on mount before the first interval tick. */
  runOnMount?: boolean;
}

// `fn` recibe `isCancelled()` — true tras el unmount (o re-arm del effect). Una callback con awaits
// SECUENCIALES (ej: fetch A → si A.ready → fetch B) debe chequearlo entre awaits para no disparar el 2º
// fetch tras el unmount. El `inFlight` sólo evita SOLAPES de ticks; no corta una callback ya en vuelo.
export function usePolling(fn: (isCancelled: () => boolean) => void | Promise<void>, opts: Options): void {
  const { intervalMs, enabled = true, runOnMount = true } = opts;
  // Stable ref: every render writes the latest fn; the timer reads the latest.
  const fnRef = useRef(fn);
  fnRef.current = fn;
  // Re-arm in-flight flag survives across renders so a slow fn doesn't pile up.
  const inFlight = useRef(false);

  useEffect(() => {
    if (!enabled) return;
    let cancelled = false;

    const tick = async () => {
      if (cancelled || inFlight.current) return;
      inFlight.current = true;
      try {
        await fnRef.current(() => cancelled);
      } catch {
        // fn is responsible for its own logging; we never swallow silently here
        // because polling continues even on transient errors.
      } finally {
        inFlight.current = false;
      }
    };

    if (runOnMount) void tick();
    const id = window.setInterval(() => { void tick(); }, intervalMs);
    return () => { cancelled = true; window.clearInterval(id); };
  }, [intervalMs, enabled, runOnMount]);
}
