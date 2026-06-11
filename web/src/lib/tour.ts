// web/src/lib/tour.ts — 016 US4 (T077) · máquina de estados FINITA del tour (PURA, testeable).
//
// La UI (Tour.tsx) es un thin render sobre esta FSM. Estados (council T077):
//   idle → running → (waitingTarget ⇄ running) → completed | skipped
// Reglas duras:
//   - target ausente: pasamos a `waitingTarget` con un presupuesto de reintentos ACOTADO (no loop
//     infinito esperando el DOM); agotado → SKIP del paso (fallback), nunca cuelga. FR-016.
//   - skip/finish son terminales (skipped/completed). No auto-relanza (la persistencia la maneja la UI).
//   - persistencia (firstRun done) → la UI llama `markFirstRunDone`; la FSM no toca storage.
//
// `furx.tour.firstRun` en localStorage (con guards) marca que el tour ya se ofreció/corrió.

import { getFlag } from "./flags.ts";
import type { LocaleKey } from "../locales/es.ts"; // 016 S2 — copy keys tipadas (typo → build rojo)

export type TourStatus = "idle" | "running" | "waitingTarget" | "completed" | "skipped";

export interface TourState {
  status: TourStatus;
  /// índice del paso actual (0-based) dentro de los pasos VISIBLES (ya filtrados por condición).
  index: number;
  /// reintentos restantes mientras esperamos el target del paso actual (presupuesto acotado).
  waitBudget: number;
}

/// Un paso del tour (DATOS). El target se resuelve por `[data-tour="<targetId>"]` en el DOM.
export interface TourStep {
  id: string;
  /// valor de `data-tour` del elemento a resaltar. Estable (council T077).
  targetId: string;
  /// dominio (navGroups) al que pertenece — para deep-linkear la vista antes de resaltar.
  domain: string;
  /// deeplink a navegar antes de mostrar el paso (ej furx://<view>). Opcional.
  deeplink?: string;
  /// key i18n del título y cuerpo del paso (tipada: un typo falla el `tsc -b`, no en runtime).
  titleKey: LocaleKey;
  bodyKey: LocaleKey;
  /// flag que debe estar ON para que el paso sea visible (si no, se filtra). Opcional.
  requiresFlag?: string;
}

/// Presupuesto de espera por target ausente. ~12 ticks (la UI tickea cada ~120ms ⇒ ~1.5s). Acotado.
export const WAIT_BUDGET = 12;

/// Filtra los pasos por su condición de visibilidad (flag). Los pasos cuyo `requiresFlag` esté OFF se
/// excluyen ANTES de correr (un paso no-visible nunca entra a la FSM → no hay que esperar su target).
export function visibleSteps(steps: TourStep[]): TourStep[] {
  return steps.filter((s) => {
    if (!s.requiresFlag) return true;
    try {
      return getFlag(s.requiresFlag as never);
    } catch {
      return false;
    }
  });
}

export function initialState(): TourState {
  return { status: "idle", index: 0, waitBudget: WAIT_BUDGET };
}

export type TourAction =
  | { type: "start" }
  | { type: "next"; total: number }
  | { type: "back" }
  | { type: "skip" }
  | { type: "finish" }
  | { type: "targetFound" }
  | { type: "targetMissing" };

/// Reducer PURO de la FSM. `total` = nº de pasos visibles (lo pasa la UI en `next`).
export function tourReducer(state: TourState, action: TourAction): TourState {
  switch (action.type) {
    case "start":
      return { status: "running", index: 0, waitBudget: WAIT_BUDGET };
    case "next": {
      if (state.status !== "running" && state.status !== "waitingTarget") return state;
      const nextIdx = state.index + 1;
      if (nextIdx >= action.total) return { status: "completed", index: state.index, waitBudget: 0 };
      return { status: "running", index: nextIdx, waitBudget: WAIT_BUDGET };
    }
    case "back": {
      if (state.status !== "running" && state.status !== "waitingTarget") return state;
      return { status: "running", index: Math.max(0, state.index - 1), waitBudget: WAIT_BUDGET };
    }
    case "skip":
      return { status: "skipped", index: state.index, waitBudget: 0 };
    case "finish":
      return { status: "completed", index: state.index, waitBudget: 0 };
    case "targetFound":
      // target presente → asegurar `running` y resetear el presupuesto.
      if (state.status !== "running" && state.status !== "waitingTarget") return state;
      return { ...state, status: "running", waitBudget: WAIT_BUDGET };
    case "targetMissing": {
      if (state.status !== "running" && state.status !== "waitingTarget") return state;
      const budget = state.waitBudget - 1;
      // Presupuesto agotado → SKIP del paso (fallback): pasamos al siguiente sin colgar.
      // (La UI traduce esto a un dispatch `next` cuando recibe `waitBudget<=0` en waitingTarget.)
      return { status: "waitingTarget", index: state.index, waitBudget: Math.max(0, budget) };
    }
    default:
      return state;
  }
}

/// True si el paso actual agotó su presupuesto de espera del target → la UI debe avanzar (fallback).
export function shouldFallbackAdvance(state: TourState): boolean {
  return state.status === "waitingTarget" && state.waitBudget <= 0;
}

/* ── Persistencia del primer arranque (guards localStorage, T071) ────────────────────────────── */

const FIRST_RUN_KEY = "furx.tour.firstRun";

/// True si el tour de primeros pasos YA se ofreció/corrió (no auto-relanzar). FR-017.
export function isFirstRunDone(): boolean {
  try {
    if (typeof localStorage === "undefined") return false;
    return localStorage.getItem(FIRST_RUN_KEY) === "done";
  } catch {
    // sin storage no podemos saber → tratamos como "ya hecho" para NO molestar repetidamente.
    return true;
  }
}

export function markFirstRunDone(): void {
  try {
    if (typeof localStorage !== "undefined") localStorage.setItem(FIRST_RUN_KEY, "done");
  } catch {
    /* sin storage: no es fatal */
  }
}
