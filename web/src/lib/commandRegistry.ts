// 015 US1 — Command Registry (espejo TS del registry tipado de Rust).
//
// Fuente ÚNICA de comandos en el front. NO se duplica la tabla acá: el backend
// Rust (`services/command_registry.rs`) es el dueño de los datos y este loader
// los trae vía el comando Tauri `command_registry_list`. Los tipos de abajo son
// el espejo 1:1 del `CommandDef` de Rust (serde `rename_all = "lowercase"` en los
// enums) para que el front tenga tipado fuerte sin re-declarar la tabla.
//
// US2 (⌘K palette) consumirá esto; US1 sólo provee tipos + loader.
// BYOK (constitución F-I): el registry NO trae secrets; `risk: "credential"` es
// SÓLO metadata para que el gate (US4) exija aprobación.

import { invoke } from "@tauri-apps/api/core";

/** Alcance de un comando en el árbol de UI. Espejo de `Scope` (Rust). */
export type CommandScope = "app" | "window" | "view" | "pane";

/** Clasificación de riesgo para el capability/approval gate (US4). */
export type CommandRisk = "safe" | "destructive" | "credential" | "external";

/** Dónde se muestra el comando. Espejo de `Visibility` (Rust). */
export type CommandVisibility = "primary" | "palette" | "internal" | "hidden";

/** Definición tipada de un comando. Espejo de `CommandDef` (Rust). */
export interface CommandDef {
  /** Identificador estable = nombre del comando Tauri (snake_case). Único. */
  id: string;
  /** Etiqueta legible para UI. */
  label: string;
  /** Descripción corta (puede venir vacía en el baseline auto-derivado). */
  description: string;
  /** Dominio/categoría para agrupar en nav/palette. */
  category: string;
  scope: CommandScope;
  risk: CommandRisk;
  visibility: CommandVisibility;
  /** Shortcut declarativo (no hardcodeado). `null` por defecto. */
  shortcut: string | null;
  /** Si el gate (US4) debe pedir confirmación antes de ejecutar. */
  requires_confirmation: boolean;
  /** Si la acción es reversible (informativo para gate/undo). */
  reversible: boolean;
  /** Deep-link a UI propia (US9 router: furx://...). `null` por defecto. */
  deeplink: string | null;
  /** Metadata extensible. Objeto JSON arbitrario (default `{}`). */
  extra: Record<string, unknown>;
}

/**
 * Trae el registry completo desde el backend Rust.
 *
 * Es la ÚNICA forma de obtener comandos en el front: no hay tabla local. Lanza
 * si el comando Tauri falla (p.ej. fuera del runtime Tauri en tests web puros).
 */
export async function loadCommandRegistry(): Promise<CommandDef[]> {
  return invoke<CommandDef[]>("command_registry_list");
}

/** Sólo los comandos visibles en la palette ⌘K (US2). Helper de conveniencia. */
export function paletteCommands(all: CommandDef[]): CommandDef[] {
  return all.filter(
    (c) => c.visibility === "palette" || c.visibility === "primary",
  );
}

/**
 * True si el comando es SÓLO de desarrollo (`extra.dev_only === true`).
 *
 * P0b (audit 3-frontera HIGH 2): el backend marca herramientas de dev (p.ej.
 * `seed_demo_cards`) con este flag. La palette universal NO debe listarlos en
 * builds de producción (`!import.meta.env.DEV`). El backend además los rechaza en
 * release, así que esto es defensa en capas (ocultar + rechazar).
 */
export function isDevOnly(c: CommandDef): boolean {
  return c.extra?.dev_only === true;
}

/**
 * Comandos que una superficie de usuario (palette universal) puede listar.
 *
 * P0b (audit 3-frontera HIGH 2): en builds de PRODUCCIÓN (`devVisible === false`) se
 * EXCLUYEN los comandos dev-only (`extra.dev_only === true`, p.ej. `seed_demo_cards`). En
 * desarrollo se devuelven todos. Lógica pura (sin React / sin `import.meta`) para que sea
 * testeable: el componente pasa `import.meta.env.DEV` como `devVisible`.
 */
export function usablePaletteCommands(
  cmds: CommandDef[],
  devVisible: boolean,
): CommandDef[] {
  return devVisible ? cmds : cmds.filter((c) => !isDevOnly(c));
}

/** True si el comando debe pasar por el gate de aprobación (US4). */
export function needsApproval(c: CommandDef): boolean {
  return (
    c.requires_confirmation ||
    c.risk === "destructive" ||
    c.risk === "credential"
  );
}
