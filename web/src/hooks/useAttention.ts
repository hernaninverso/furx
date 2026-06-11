// 047 FR-002 — hook compartido de la cola de atención por-pane. UN solo poll de `attention_list`
// (~2s) para TODA la app (singleton de módulo con fan-out a los suscriptores), en vez de que cada
// PaneCard pollinee por su cuenta (N panes × 2s = ruido de IPC). Devuelve la prioridad del pane
// pedido (`needs_input` | `has_result` | null). Fail-safe: si el backend falla, todos quedan en null
// (NUNCA mostramos datos obsoletos). La fuente es la MISMA que el AttentionBadge (030-034): la cola
// la puebla el poller backend; acá sólo la leemos para reflejar el estado, sin tocar el foco del mic.
import { useEffect, useState } from "react";
import { invoke } from "../lib/invoke";

export type AttentionPriority = "needs_input" | "has_result";

interface AttentionEntry {
  seq: number;
  pane_id: string;
  priority: AttentionPriority;
  attended: boolean;
}

// ── Singleton de módulo: un solo timer + un Map paneId→priority compartido. ──
let priorities = new Map<string, AttentionPriority>();
const listeners = new Set<() => void>();
let timer: ReturnType<typeof setInterval> | null = null;

function notify() {
  for (const l of listeners) l();
}

async function poll() {
  try {
    const list = await invoke<AttentionEntry[]>("attention_list", {});
    const next = new Map<string, AttentionPriority>();
    if (Array.isArray(list)) {
      for (const e of list) {
        // needs_input gana sobre has_result si un pane apareciera con ambos (no debería).
        const prev = next.get(e.pane_id);
        if (prev === "needs_input") continue;
        next.set(e.pane_id, e.priority);
      }
    }
    priorities = next;
  } catch {
    priorities = new Map(); // fail-safe: nunca datos obsoletos
  } finally {
    // audit-3 (codex/nvidia MED) — notify SIEMPRE (en éxito o error), y SÓLO si quedan suscriptores:
    // si un poll estaba en vuelo cuando maybeStop() limpió el timer (listeners vacíos), su notify
    // tardío es inocuo (no hay callbacks) — el guard lo hace explícito.
    if (listeners.size > 0) notify();
  }
}

function ensureRunning() {
  if (timer !== null) return;
  void poll(); // primer fetch inmediato
  timer = setInterval(() => void poll(), 2000);
}

function maybeStop() {
  if (listeners.size === 0 && timer !== null) {
    clearInterval(timer);
    timer = null;
  }
}

/** Suscribe el componente a la cola compartida y devuelve la prioridad del pane (o null). */
export function usePaneAttention(paneId: string): AttentionPriority | null {
  const [, force] = useState(0);
  useEffect(() => {
    const cb = () => force((n) => n + 1);
    listeners.add(cb);
    ensureRunning();
    return () => {
      listeners.delete(cb);
      maybeStop();
    };
  }, []);
  return priorities.get(paneId) ?? null;
}

// Sólo para tests: resetea el singleton (sin esto el estado del módulo persiste entre tests).
export function __resetAttentionForTest() {
  priorities = new Map();
  listeners.clear();
  if (timer !== null) {
    clearInterval(timer);
    timer = null;
  }
}
