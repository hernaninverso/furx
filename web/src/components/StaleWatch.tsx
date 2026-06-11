// F26 — StaleWatch wrapper. Applies a "stale" CSS class to its child when the
// last successful update is older than `2 × intervalMs`. Avoids re-render loops
// by only setting state when the stale boolean actually flips (council perf
// must-fix; Codex edge-case for lastUpdate=0 = "unknown, treat as fresh").

import { useEffect, useState, type ReactNode } from "react";

const CHECK_MS = 1000;

interface Props {
  /** Date.now() of last successful update. 0 = unknown (treated as fresh). */
  lastUpdate: number;
  /** Polling interval of the upstream data source, ms. */
  intervalMs: number;
  /** Optional class merged with `stale` when applicable. */
  className?: string;
  children: ReactNode;
}

export function StaleWatch({ lastUpdate, intervalMs, className, children }: Props) {
  const [isStale, setIsStale] = useState<boolean>(false);

  useEffect(() => {
    let cancelled = false;

    const compute = () => {
      // Codex edge-case: lastUpdate === 0 means "we haven't received any update
      // yet" (e.g. backend hasn't replied). Treat as fresh — don't mislead the
      // user into thinking valid data went stale.
      if (lastUpdate === 0) return false;
      const age = Date.now() - lastUpdate;
      return age > intervalMs * 2;
    };

    const check = () => {
      if (cancelled) return;
      const next = compute();
      setIsStale((prev) => (prev === next ? prev : next));
    };

    check();
    const id = window.setInterval(check, CHECK_MS);
    return () => { cancelled = true; window.clearInterval(id); };
  }, [lastUpdate, intervalMs]);

  const cls = [isStale ? "stale" : "", className].filter(Boolean).join(" ").trim();
  return (
    <span
      className={cls || undefined}
      data-stale={isStale ? "true" : "false"}
      aria-live={isStale ? "polite" : undefined}
    >
      {children}
    </span>
  );
}
