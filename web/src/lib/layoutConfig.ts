// web/src/lib/layoutConfig.ts — 015-frontend-reform-kernel · US6
// Espejo TS del schema de layout versionado de Rust (services/layout_config.rs)
// + loader/saver vía los comandos Tauri `layout_config_get`/`layout_config_save`.
//
// CIMIENTO: vive EN PARALELO al layout legacy (get_layout/save_layout). La nav
// lo adopta en otra ola; este módulo NO reescribe Shell.tsx, sólo expone el tipo
// y el loader para que la próxima ola los consuma.
//
// Invariantes que reflejan el lado Rust:
//   - panelType (CLASE de panel) ≠ panelId (INSTANCIA concreta).
//   - windowKey/monitor existen desde el día 1 aunque la UI use una sola ventana.
//   - displayHint son PISTAS (no monitor-IDs absolutos): si el monitor no existe
//     al rehidratar, se ignora.

import { invoke } from "@tauri-apps/api/core";

export const CURRENT_LAYOUT_VERSION = 1;
export const DEFAULT_WORKSPACE = "default";
export const MAIN_WINDOW_KEY = "main";

/** Clase de ventana. `main` = raíz; `detached` = ventana secundaria (Fase 2). */
export type WindowKind = "main" | "detached";

/** Dirección de un split. */
export type SplitDirection = "horizontal" | "vertical";

/**
 * Descriptor de un panel. `panelType` = CLASE (qué clase de panel:
 * "terminal"/"claude"/"codex"/...); `panelId` = INSTANCIA (id único de ESTE
 * pane). `params` = metadata libre extensible por tipo de panel.
 *
 * OJO serialización: el backend Rust usa snake_case (`panel_type`/`panel_id`).
 * Acá exponemos camelCase y el loader/saver hace el mapeo en los bordes.
 */
export interface PanelDescriptor {
  panelType: string;
  panelId: string;
  params?: unknown;
}

/**
 * Árbol de paneles de una ventana. Discriminado por `node`.
 *   - leaf:  un panel concreto.
 *   - split: división horizontal/vertical en sub-árboles.
 *   - tabs:  pestañas con índice activo.
 */
export type PanelLayoutNode =
  | { node: "leaf"; panel: PanelDescriptor }
  | { node: "split"; direction: SplitDirection; children: PanelLayoutNode[] }
  | { node: "tabs"; active: number; children: PanelLayoutNode[] };

/** Pista de posición/tamaño de una ventana. Todos los campos opcionales. */
export interface DisplayHint {
  monitorId?: string;
  x?: number;
  y?: number;
  width?: number;
  height?: number;
}

/** Layout de una ventana: clave estable, clase, hint y árbol de paneles. */
export interface WindowLayout {
  windowKey: string;
  kind: WindowKind;
  displayHint?: DisplayHint;
  layout: PanelLayoutNode;
}

/** Config de layout versionada de un workspace. */
export interface LayoutConfigV1 {
  version: number;
  workspaceId: string;
  windows: WindowLayout[];
  /** 018 Fase 2 B0 (T063) — revisión monotónica (optimistic concurrency). Espejo del
   *  `revision: u64` de Rust. `save` exige `stored + 1`; un write stale → `stale_layout`.
   *  Filas v1 viejas (sin el campo) llegan con 0 (serde default). */
  revision: number;
}

// ── Mapeo de bordes snake_case (Rust) ↔ camelCase (TS) ────────────────────────
// El backend serde usa snake_case. Mantenemos el JSON de Tauri tal cual y
// traducimos sólo las claves que difieren, recursivamente sobre el árbol.

type RawPanelNode =
  | { node: "leaf"; panel: { panel_type: string; panel_id: string; params?: unknown } }
  | { node: "split"; direction: SplitDirection; children: RawPanelNode[] }
  | { node: "tabs"; active: number; children: RawPanelNode[] };

// Audit fix codex US6: el JSON de Rust usa snake_case `monitor_id`; el tipo TS usa
// `monitorId`. Sin un tipo raw + conversión explícita, el hint de monitor se PIERDE en
// el roundtrip (justo el dato que la Fase 2 / detach necesita).
interface RawDisplayHint {
  monitor_id?: string;
  x?: number;
  y?: number;
  width?: number;
  height?: number;
}

interface RawWindow {
  window_key: string;
  kind: WindowKind;
  display_hint?: RawDisplayHint;
  layout: RawPanelNode;
}

function hintFromRaw(h?: RawDisplayHint): DisplayHint | undefined {
  if (!h) return undefined;
  return { monitorId: h.monitor_id, x: h.x, y: h.y, width: h.width, height: h.height };
}

function hintToRaw(h?: DisplayHint): RawDisplayHint | undefined {
  if (!h) return undefined;
  return { monitor_id: h.monitorId, x: h.x, y: h.y, width: h.width, height: h.height };
}

interface RawConfig {
  version: number;
  workspace_id: string;
  windows: RawWindow[];
  /** serde default 0 si la fila v1 es vieja (schema 026 sin revision). */
  revision?: number;
}

function nodeFromRaw(n: RawPanelNode): PanelLayoutNode {
  switch (n.node) {
    case "leaf":
      return {
        node: "leaf",
        panel: { panelType: n.panel.panel_type, panelId: n.panel.panel_id, params: n.panel.params },
      };
    case "split":
      return { node: "split", direction: n.direction, children: n.children.map(nodeFromRaw) };
    case "tabs":
      return { node: "tabs", active: n.active, children: n.children.map(nodeFromRaw) };
  }
}

function nodeToRaw(n: PanelLayoutNode): RawPanelNode {
  switch (n.node) {
    case "leaf":
      return {
        node: "leaf",
        panel: {
          panel_type: n.panel.panelType,
          panel_id: n.panel.panelId,
          params: n.panel.params ?? null,
        },
      };
    case "split":
      return { node: "split", direction: n.direction, children: n.children.map(nodeToRaw) };
    case "tabs":
      return { node: "tabs", active: n.active, children: n.children.map(nodeToRaw) };
  }
}

export function configFromRaw(raw: RawConfig): LayoutConfigV1 {
  return {
    version: raw.version,
    workspaceId: raw.workspace_id,
    revision: raw.revision ?? 0,
    windows: raw.windows.map((w) => ({
      windowKey: w.window_key,
      kind: w.kind,
      displayHint: hintFromRaw(w.display_hint),
      layout: nodeFromRaw(w.layout),
    })),
  };
}

export function configToRaw(cfg: LayoutConfigV1): RawConfig {
  return {
    version: cfg.version,
    workspace_id: cfg.workspaceId,
    revision: cfg.revision,
    windows: cfg.windows.map((w) => ({
      window_key: w.windowKey,
      kind: w.kind,
      display_hint: hintToRaw(w.displayHint),
      layout: nodeToRaw(w.layout),
    })),
  };
}

// ── Loader / saver ────────────────────────────────────────────────────────────

/** Carga la config de layout versionada de un workspace (default si se omite). */
export async function loadLayoutConfig(workspaceId = DEFAULT_WORKSPACE): Promise<LayoutConfigV1> {
  const raw = await invoke<RawConfig>("layout_config_get", { workspaceId });
  return configFromRaw(raw);
}

/** Persiste la config de layout versionada. */
export async function saveLayoutConfig(cfg: LayoutConfigV1): Promise<void> {
  await invoke("layout_config_save", { config: configToRaw(cfg) });
}

/** Config vacía válida (1 ventana Main sin paneles) para un workspace nuevo. */
export function emptyLayoutConfig(workspaceId = DEFAULT_WORKSPACE): LayoutConfigV1 {
  return {
    version: CURRENT_LAYOUT_VERSION,
    workspaceId,
    revision: 0,
    windows: [
      {
        windowKey: MAIN_WINDOW_KEY,
        kind: "main",
        layout: { node: "split", direction: "horizontal", children: [] },
      },
    ],
  };
}
