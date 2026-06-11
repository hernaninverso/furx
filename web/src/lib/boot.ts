// 042 FR-001 / FR-005 — lógica PURA del gate del wizard en el boot, extraída de Shell para testear
// sin montar la shell pesada (xterm, vistas). El boot resuelve DOS invokes con `Promise.allSettled`
// (un fallo no tumba el boot) y decide: ¿mostrar el wizard? ¿en qué `settingsState` quedamos?

/** Resultado de un `Promise.allSettled` item (forma mínima que usamos). */
export type Settled<T> = { status: "fulfilled"; value: T } | { status: "rejected" };

export interface BootDecision {
  settingsState: "loaded" | "error";
  needsWizard: boolean;
  tmuxAvailable: boolean;
}

/**
 * Decide el estado del boot a partir de los resultados de `settings_get(first_run)` + `tmux_available`
 * y del fallsafe local (flag en localStorage que el wizard escribe si la DB falla — FR-005).
 *
 * Reglas:
 *  - `tmux` es INDEPENDIENTE del wizard: si falla → false (la EmptyShellState avisa), sin wizard espurio.
 *  - first_run OK: wizard SÓLO si NI la DB (`=== true`) NI el fallsafe local lo dan por completado.
 *  - first_run FALLA: no sabemos si es primer arranque → `error`; respetamos el fallsafe local para no
 *    re-abrir el wizard en bucle; si tampoco está, lo mostramos.
 */
export function decideBoot(
  firstRun: Settled<unknown>,
  tmux: Settled<boolean>,
  firstRunCompletedLocal: boolean,
): BootDecision {
  const tmuxAvailable = tmux.status === "fulfilled" ? Boolean(tmux.value) : false;
  if (firstRun.status === "fulfilled") {
    const doneInDb = firstRun.value === true;
    return {
      settingsState: "loaded",
      needsWizard: !(doneInDb || firstRunCompletedLocal),
      tmuxAvailable,
    };
  }
  return {
    settingsState: "error",
    needsWizard: !firstRunCompletedLocal,
    tmuxAvailable,
  };
}

/** Decisión cuando el boot supera el timeout DURO (8s) sin que los invokes respondan (FR-001). */
export function decideBootTimeout(firstRunCompletedLocal: boolean): Pick<BootDecision, "settingsState" | "needsWizard"> {
  return { settingsState: "error", needsWizard: !firstRunCompletedLocal };
}

// ── 042 FR-005 — fallsafe local del wizard (anti-loop) ─────────────────────────────────────────
// Si `settings_set("app.first_run_completed")` falla (DB locked / disco / modo privado), el wizard
// marca el flag acá para que el boot NO re-abra el wizard en bucle. Helpers compartidos por el boot
// (lee) y el wizard (escribe). `safe*` NUNCA tiran (try/catch sobre localStorage).
export const FIRST_RUN_LOCAL_FLAG = "furx.first_run_completed";

/** Lee de localStorage sin tirar nunca (quota / modo privado / SSR). */
export function safeLocalGet(key: string): string | null {
  try { return localStorage.getItem(key); } catch { return null; }
}

/** Escribe en localStorage y VERIFICA que quedó (devuelve false si no se pudo persistir). No tira. */
export function safeLocalSet(key: string, value: string): boolean {
  try {
    localStorage.setItem(key, value);
    return localStorage.getItem(key) === value;
  } catch {
    return false;
  }
}

/** ¿El primer arranque está marcado como completado en el fallsafe local? */
export function firstRunCompletedLocal(): boolean {
  return safeLocalGet(FIRST_RUN_LOCAL_FLAG) === "true";
}

/** Marca el primer arranque como completado en el fallsafe local. Devuelve si se pudo persistir. */
export function markFirstRunCompletedLocal(): boolean {
  return safeLocalSet(FIRST_RUN_LOCAL_FLAG, "true");
}
