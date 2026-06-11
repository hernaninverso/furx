// web/src/lib/processManager.ts — 015-frontend-reform-kernel · US5
// Espejo TS del registro de procesos de Rust (services/process_manager.rs) +
// loaders vía los comandos Tauri `process_list` / `process_cancel` / `process_attach`.
//
// CIMIENTO de la reforma: el proceso es PROPIEDAD del backend y SOBREVIVE a
// unmount/reload/cierre de ventana. La UI es un VIEWPORT que lo observa/controla:
//   - `loadProcesses()` lista lo VIVO al re-montar (reattach masivo).
//   - `attachProcess(id)` rehidrata UN proceso al re-suscribirse un viewport.
//   - `cancelProcess(id)` es la ÚNICA cancelación (explícita). Cerrar la ventana /
//     desmontar un pane NO cancela — eso es puro lifecycle de UI, no toca el backend.
//
// La rehidratación reactiva (cuando el estado cambia mientras la UI está montada)
// llega por el event bus (eventBus.ts → AppEvent.TaskChanged con el process_id).
// Este módulo NO toca Shell.tsx; sólo expone tipos + loaders para que las olas
// siguientes (palette/router/nav) los consuman.

import { invoke } from "@tauri-apps/api/core";

/** Clase de proceso. Espejo de `ProcessKind` (Rust). */
export type ProcessKind = "pty" | "job" | "agent";

/** Estado del proceso en el registro. */
export type ProcessStatus = "running" | "done" | "failed" | "canceled";

/** Una fila del registro = un proceso que vive en el backend. Espejo de `ProcessInfo`. */
export interface ProcessInfo {
  /** Id estable del proceso (puede ser el pane_id, un UUID, etc.). */
  process_id: string;
  kind: ProcessKind;
  /** Contexto de origen (window_id/pane_id/task_id). Informativo: su muerte NO cancela. */
  owner_context: string | null;
  /** Referencia al recurso real (pane_id del PtyManager / task_id / job_id). */
  external_ref: string | null;
  status: ProcessStatus;
  /** Progreso 0.0..1.0 (opcional). */
  progress: number | null;
  label: string | null;
  started_at: string;
  updated_at: string;
  /** Generación del run vigente (sync interno backend; el front no necesita usarla). */
  run_token: number | null;
}

/** Estados terminales: un proceso así no se puede cancelar (la cancelación es no-op). */
const TERMINAL: ReadonlySet<ProcessStatus> = new Set(["done", "failed", "canceled"]);

/** True si el proceso sigue vivo (la UI puede mostrar controles de cancel/attach). */
export function isAlive(p: ProcessInfo): boolean {
  return !TERMINAL.has(p.status);
}

/**
 * Lista los procesos VIVOS del registro. Es lo que un viewport pide al re-montar
 * (tras reload / cerrar y reabrir): los procesos siguieron corriendo en el backend.
 */
export async function loadProcesses(): Promise<ProcessInfo[]> {
  return invoke<ProcessInfo[]>("process_list");
}

/**
 * CANCELLATION EXPLÍCITA del proceso `id`. ÚNICA forma de terminarlo desde la UI.
 * El backend marca `canceled` y mata el recurso real (PTY vía PtyManager para
 * kind=pty). Idempotente: cancelar uno ya terminal devuelve su estado sin error.
 * Devuelve el `ProcessInfo` resultante.
 */
export async function cancelProcess(processId: string): Promise<ProcessInfo> {
  return invoke<ProcessInfo>("process_cancel", { processId });
}

/**
 * ATTACH/reattach: rehidrata UN proceso al re-suscribirse un viewport. Lanza si el
 * proceso no existe. Para el listado masivo usar `loadProcesses()`.
 */
export async function attachProcess(processId: string): Promise<ProcessInfo> {
  return invoke<ProcessInfo>("process_attach", { processId });
}
