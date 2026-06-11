// G — Toast notification hook. Council MUST-FIX 4/5: feedback on success AND
// failure (silent failure is the worst UX for a critical action like
// snapshot_take). Uses ARIA role=status (success) / role=alert (error).

import { useCallback, useEffect, useRef, useState } from "react";

export type ToastKind = "success" | "error" | "info";

export interface ToastSpec {
  id: number;
  kind: ToastKind;
  message: string;
}

export interface UseToastApi {
  toasts: ToastSpec[];
  show: (kind: ToastKind, message: string, ttlMs?: number) => void;
  dismiss: (id: number) => void;
}

const DEFAULT_TTL_MS = 5000;

export function useToast(): UseToastApi {
  const [toasts, setToasts] = useState<ToastSpec[]>([]);
  // Codex audit LOW #3: track auto-dismiss timers so unmount can clear them
  // before they call setState on an unmounted hook (React 18 warning + leak).
  const timersRef = useRef<Map<number, number>>(new Map());

  const dismiss = useCallback((id: number) => {
    const handle = timersRef.current.get(id);
    if (handle !== undefined) {
      window.clearTimeout(handle);
      timersRef.current.delete(id);
    }
    setToasts((prev) => prev.filter((t) => t.id !== id));
  }, []);

  const show = useCallback((kind: ToastKind, message: string, ttlMs: number = DEFAULT_TTL_MS) => {
    const id = Date.now() + Math.floor(Math.random() * 1000);
    setToasts((prev) => [...prev, { id, kind, message }]);
    const handle = window.setTimeout(() => dismiss(id), ttlMs);
    timersRef.current.set(id, handle);
  }, [dismiss]);

  useEffect(() => {
    const timers = timersRef.current;
    return () => {
      for (const h of timers.values()) window.clearTimeout(h);
      timers.clear();
    };
  }, []);

  return { toasts, show, dismiss };
}
