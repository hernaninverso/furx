// spec-022 P0b · REFORMA 4 — Shortcuts del sidebar derivados del registry real.
//
// UNA sola fuente de verdad: el panel de shortcuts del sidebar DERIVA del mismo
// registro que `ShortcutSheet` (`buildActions()` en `actions.ts`), en vez de una
// lista literal en JSX que se desincroniza. Si un shortcut cambia o desaparece del
// registry, el sidebar se actualiza solo (no hay literales que mantener a mano).
//
// "Destacados": un subconjunto CURADO de los más usados, elegido por id (orden
// curado). Cualquier id ausente del registry simplemente NO se muestra (nunca se
// inventa un literal). El ⌘/ "ver todos" sigue abriendo el ShortcutSheet completo.
//
// Lógica PURA y testeable: no toca React/Tauri.

import type { ActionEntry } from "../actions.ts";

/** Una entrada de shortcut lista para pintar en el sidebar. */
export interface SidebarShortcut {
  /** id del action en el registry (estable). */
  id: string;
  /** combinación de teclas (del registry — nunca un literal hardcodeado). */
  shortcut: string;
  /** etiqueta legible (del registry). */
  label: string;
}

/**
 * Orden CURADO de los shortcuts "destacados" del sidebar, por id del registry.
 * Se eligen los más usados (focus de panes, nuevo/cerrar, cambiar modo, palettes).
 * El orden de este array = orden de render. Un id que no exista (o no tenga
 * `shortcut`) en el registry se omite — sin literales, sin drift.
 */
export const FEATURED_SHORTCUT_IDS: readonly string[] = [
  "pane.focus.1",
  "pane.add",
  "pane.close",
  "pane.cycle-mode",
  "modal.voice",
  "modal.broadcast",
  "modal.council",
] as const;

/**
 * Deriva los shortcuts destacados del sidebar desde el registry real (`buildActions()`).
 * - Sólo incluye acciones con `shortcut` real (las que no tienen binding se omiten).
 * - Respeta `FEATURED_SHORTCUT_IDS` como orden curado.
 * - Cada entrada existe DE VERDAD en el registry pasado (cobertura verificable).
 */
export function featuredSidebarShortcuts(
  actions: ActionEntry[],
  featuredIds: readonly string[] = FEATURED_SHORTCUT_IDS,
): SidebarShortcut[] {
  const byId = new Map(actions.map((a) => [a.id, a] as const));
  const out: SidebarShortcut[] = [];
  for (const id of featuredIds) {
    const a = byId.get(id);
    if (!a || !a.shortcut) continue; // sin binding → se omite, nunca un literal
    out.push({ id: a.id, shortcut: a.shortcut, label: a.label });
  }
  return out;
}
