// web/src/lib/windowManager.ts — 018 Fase 2 US2 (T023) — detach / re-attach por pane.
//
// CAPA FINA sobre los comandos Tauri `window_open_detached` / `window_close` / `window_list`.
// TODA la lógica de mutación del árbol vive en RUST (SSOT unidireccional): el front sólo PIDE
// la acción; Rust valida (T062) + bump revisión (T063) + persiste `LayoutConfigV1` + emite
// `LayoutChanged`; las webviews re-hidratan. El front NUNCA persiste el árbol ni mata procesos.
//
// `resolveWindowLabel()` decide qué window_key renderiza ESTA webview: el `?window_key=` de la
// URL (ventana detached) o `main` (la principal). Es PURA (recibe la search string) → testeable
// sin DOM. Es el espejo front del label que Rust deriva server-side (anti-spoof): el front lo usa
// SÓLO para saber qué subárbol mostrar; el backend nunca confía en él para los leases/guards.

import { invoke } from "@tauri-apps/api/core";
import { MAIN_WINDOW_KEY, type LayoutConfigV1, type PanelLayoutNode } from "./layoutConfig.ts";

/** Una ventana viva (espejo de `WindowReport` de Rust). */
export interface WindowInfo {
  label: string;
  windowKey: string;
  isMain: boolean;
}

interface RawWindowReport {
  label: string;
  window_key: string;
  is_main: boolean;
}

/**
 * Resuelve el `window_key` de ESTA webview a partir de su query string. Una ventana detached
 * se abre con `index.html?window_key=detached-N`; la Main no lleva el param → `main`.
 * PURA: recibe la search string (`location.search`) para ser testeable sin `window`.
 * Sanitiza: sólo acepta keys con el patrón estable (`main` o `detached-<n>`); cualquier otra
 * cosa cae a `main` (defensa anti-URL-manipulada; el backend igual valida server-side).
 */
export function resolveWindowLabel(search: string): string {
  let key: string | null = null;
  try {
    const params = new URLSearchParams(search.startsWith("?") ? search.slice(1) : search);
    key = params.get("window_key");
  } catch {
    key = null;
  }
  if (key && isValidWindowKey(key)) return key;
  return MAIN_WINDOW_KEY;
}

/** ¿`key` tiene el patrón estable de window_key? `main` o `detached-<entero≥1>`. */
export function isValidWindowKey(key: string): boolean {
  if (key === MAIN_WINDOW_KEY) return true;
  return /^detached-[1-9][0-9]*$/.test(key);
}

/** ¿esta webview es la ventana Main? (algunas UI sólo aplican en Main: ⌘K global, agregar panes). */
export function isMainWindow(label: string): boolean {
  return label === MAIN_WINDOW_KEY;
}

/**
 * Subárbol que ESTA webview (`label`) debe renderizar, dado el `LayoutConfigV1`. PURA.
 * INVARIANTE (018 US2 audit): una ventana DETACHED cuyo `window_key` ya no existe en la config
 * (sus panes fueron reatados a Main durante su cierre, antes de que la webview se cierre
 * físicamente) NUNCA cae al árbol de Main — montaría los mismos `panel_id` que Main → doble
 * montaje + guerra de leases (viola invariantes 3 y 6). En ese caso devuelve `null` (render vacío
 * mientras la ventana termina de cerrarse). Sólo la propia Main puede renderizar el árbol Main.
 */
export function windowFor(cfg: LayoutConfigV1, label: string): PanelLayoutNode | null {
  const exact = cfg.windows.find((x) => x.windowKey === label);
  if (exact) return exact.layout;
  if (label !== MAIN_WINDOW_KEY) return null;
  return cfg.windows.find((x) => x.kind === "main")?.layout ?? null;
}

/**
 * DETACH: saca el pane `panelId` (Leaf de Main) a una ventana propia. Devuelve el `window_key`
 * creado, o `null` si el panel ya no estaba en Main (no-op). NUNCA mata el proceso: Rust mueve
 * el descriptor en el árbol y abre la ventana; el proceso sigue vivo, su binding migra vía el
 * lease (force-detach versionado). Sólo tiene sentido invocarlo desde la ventana Main.
 */
export async function detachPane(panelId: string, workspaceId?: string): Promise<string | null> {
  const key = await invoke<string | null>("window_open_detached", { panelId, workspaceId });
  return key ?? null;
}

/**
 * RE-ATTACH / CLOSE: reata los paneles de la ventana `label` a Main y la cierra. Mismo camino
 * que el botón "re-attach" y que el cierre por la X (el backend lo maneja transaccional, sin
 * matar procesos). Idempotente.
 */
export async function closeWindow(label: string, workspaceId?: string): Promise<void> {
  await invoke("window_close", { label, workspaceId });
}

/** Lista las ventanas vivas (label/window_key/isMain). */
export async function listWindows(): Promise<WindowInfo[]> {
  const raw = await invoke<RawWindowReport[]>("window_list");
  return raw.map((r) => ({ label: r.label, windowKey: r.window_key, isMain: r.is_main }));
}
