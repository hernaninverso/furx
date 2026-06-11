// web/src/lib/incidents.ts — 022 P1 · US6 — lógica PURA del inbox de incidentes accionable.
//
// Todo lo testeable del inbox vive acá, SIN tocar Tauri ni React: agrupación, filtro "solo
// accionables", cálculo de `snooze_until` (1h/4h/mañana), auto-unsnooze derivado, y el mapeo
// fuente→destino para "ir al origen". El Shell consume estas funciones y aplica los efectos.

import type { Card } from "../types.ts";
import type { View } from "./router.ts";
import { safeLocalGet, safeLocalSet } from "./boot.ts";

// ── Estado de inbox derivado ──────────────────────────────────────────────────────────────────

/** Estado efímero de una card en el inbox (derivado de sus campos, no persistido aparte). */
export type InboxState = "actionable" | "snoozed" | "read" | "dismissed" | "closed";

/**
 * Deriva el estado de inbox de una card a partir de sus campos + el "ahora".
 *   - closed     : status === 'closed' (decidida: approved/rejected/needs-changes).
 *   - dismissed  : descartada (dismissed_at presente).
 *   - snoozed    : snooze_until en el futuro Y no reabierta por actividad.
 *   - read        : marcada leída (read_at) y aún no accionada (sigue open, visible pero atenuada).
 *   - actionable : el resto (open, visible, requiere triaje).
 *
 * `nowIso` es el "ahora" en formato comparable lexicográficamente con los timestamps SQLite
 * (`YYYY-MM-DD HH:MM:SS`, UTC). Se inyecta para que la función sea pura/determinista en tests.
 */
export function inboxState(card: Card, nowIso: string): InboxState {
  if (card.status === "closed") return "closed";
  if (card.dismissed_at) return "dismissed";
  // Reabierta por nueva actividad → vuelve a ser accionable aunque tuviera snooze.
  if (card.snooze_until && !card.reopened && card.snooze_until > nowIso) return "snoozed";
  if (card.read_at) return "read";
  return "actionable";
}

/** ¿La card está accionable en el inbox AHORA? (visible y pendiente de triaje, incl. reabiertas). */
export function isActionable(card: Card, nowIso: string): boolean {
  const s = inboxState(card, nowIso);
  return s === "actionable" || s === "read";
}

/** ¿La card está snoozeada (oculta del inbox activo) AHORA? */
export function isSnoozed(card: Card, nowIso: string): boolean {
  return inboxState(card, nowIso) === "snoozed";
}

/**
 * Filtra las cards visibles del inbox. Por defecto excluye closed/dismissed/snoozed.
 * Con `onlyActionable` excluye además las "read" (deja sólo lo que requiere triaje activo).
 */
export function inboxCards(cards: Card[], nowIso: string, onlyActionable = false): Card[] {
  return cards.filter((c) => {
    const s = inboxState(c, nowIso);
    if (s === "closed" || s === "dismissed" || s === "snoozed") return false;
    if (onlyActionable && s === "read") return false;
    return true;
  });
}

// ── Snooze: cálculo de `snooze_until` ───────────────────────────────────────────────────────────

/** Opciones de snooze ofrecidas en la UI (no es un literal fijo de 1h). */
export type SnoozeOption = "1h" | "4h" | "tomorrow";

export const SNOOZE_OPTIONS: readonly SnoozeOption[] = ["1h", "4h", "tomorrow"] as const;

/** Formatea un `Date` (UTC) al timestamp `YYYY-MM-DD HH:MM:SS` que usa SQLite (`datetime('now')`). */
function toSqliteUtc(d: Date): string {
  const p = (n: number, w = 2) => String(n).padStart(w, "0");
  return (
    `${d.getUTCFullYear()}-${p(d.getUTCMonth() + 1)}-${p(d.getUTCDate())} ` +
    `${p(d.getUTCHours())}:${p(d.getUTCMinutes())}:${p(d.getUTCSeconds())}`
  );
}

/**
 * Calcula el `snooze_until` (en UTC, formato SQLite) para una opción de snooze desde un `now` (ms).
 *   - 1h        : ahora + 1 hora.
 *   - 4h        : ahora + 4 horas.
 *   - tomorrow  : mañana a las 09:00 LOCAL (convertido a UTC) — "posponer hasta mañana".
 *
 * El resultado es comparable lexicográficamente con `datetime('now')` del backend.
 */
export function computeSnoozeUntil(option: SnoozeOption, nowMs: number): string {
  const now = new Date(nowMs);
  if (option === "1h") return toSqliteUtc(new Date(nowMs + 60 * 60 * 1000));
  if (option === "4h") return toSqliteUtc(new Date(nowMs + 4 * 60 * 60 * 1000));
  // tomorrow: 09:00 hora local del día siguiente.
  const t = new Date(now.getFullYear(), now.getMonth(), now.getDate() + 1, 9, 0, 0, 0);
  return toSqliteUtc(t);
}

// ── Agrupación ──────────────────────────────────────────────────────────────────────────────────

/** Criterio de agrupación del inbox. */
export type GroupBy = "project" | "severity" | "source";

/** Un grupo de cards del inbox (clave + miembros), con conteo accionable. */
export interface IncidentGroup {
  key: string;
  cards: Card[];
  /** cuántas del grupo son accionables AHORA (para el badge del grupo). */
  actionableCount: number;
}

/** Orden de severidad (más urgente primero) para ordenar grupos/cards. */
const SEVERITY_RANK: Record<string, number> = { critical: 0, warning: 1, info: 2 };

/**
 * Agrupa las cards por el criterio dado. Los grupos se ordenan:
 *   - por severidad: critical → warning → info.
 *   - por proyecto/fuente: alfabético, pero los grupos con más accionables primero (desempate).
 * Las cards dentro de cada grupo se ordenan por severidad y luego por recencia (created_at desc).
 */
export function groupIncidents(cards: Card[], by: GroupBy, nowIso: string): IncidentGroup[] {
  const byKey = new Map<string, Card[]>();
  for (const c of cards) {
    const key = by === "project" ? c.project : by === "severity" ? c.severity : c.source;
    const k = key || "—";
    const arr = byKey.get(k);
    if (arr) arr.push(c);
    else byKey.set(k, [c]);
  }
  const groups: IncidentGroup[] = [];
  for (const [key, list] of byKey) {
    const sorted = [...list].sort((a, b) => {
      const sr = (SEVERITY_RANK[a.severity] ?? 3) - (SEVERITY_RANK[b.severity] ?? 3);
      if (sr !== 0) return sr;
      return a.created_at < b.created_at ? 1 : a.created_at > b.created_at ? -1 : 0;
    });
    groups.push({
      key,
      cards: sorted,
      actionableCount: sorted.filter((c) => isActionable(c, nowIso)).length,
    });
  }
  groups.sort((a, b) => {
    if (by === "severity") {
      return (SEVERITY_RANK[a.key] ?? 3) - (SEVERITY_RANK[b.key] ?? 3);
    }
    // más accionables primero; desempate alfabético estable.
    if (b.actionableCount !== a.actionableCount) return b.actionableCount - a.actionableCount;
    return a.key < b.key ? -1 : a.key > b.key ? 1 : 0;
  });
  return groups;
}

// ── 044 FR-002 — colapso/expansión de grupos de incidentes (persistido) ─────────────────────────

/** Clave de `localStorage` para el estado de colapso de los grupos del inbox (sufijo `_v1`). */
export const INCIDENT_GROUPS_COLLAPSED_KEY = "furx_incident_groups_v1_collapsed";

/** Cap duro de cards renderizadas por grupo en el DOM (perf). El "ver más" sube hasta este tope. */
export const INCIDENT_GROUP_DOM_CAP = 200;
/** Primeras N cards visibles por grupo antes del "ver N más". */
export const INCIDENT_GROUP_INITIAL_VISIBLE = 5;
/** Cuánto incrementa "ver más" cada click. */
export const INCIDENT_GROUP_VISIBLE_STEP = 50;

/** ¿El grupo contiene al menos una card `critical`? (para el badge de emergencia, aunque colapse). */
export function groupHasCritical(group: IncidentGroup): boolean {
  return group.cards.some((c) => c.severity === "critical");
}

/**
 * Lee el estado de colapso persistido de `localStorage`. Devuelve un mapa `groupKey → collapsed`,
 * o `null` si no hay nada guardado / el JSON es inválido (nunca tira). Sólo conserva valores boolean.
 */
export function loadCollapsedState(): Record<string, boolean> | null {
  const raw = safeLocalGet(INCIDENT_GROUPS_COLLAPSED_KEY);
  if (!raw) return null;
  try {
    const parsed = JSON.parse(raw) as unknown;
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return null;
    const out: Record<string, boolean> = {};
    for (const [k, v] of Object.entries(parsed as Record<string, unknown>)) {
      if (typeof v === "boolean") out[k] = v;
    }
    return out;
  } catch {
    return null;
  }
}

/** Persiste el estado de colapso. Devuelve `false` si no se pudo guardar (no tira). */
export function saveCollapsedState(state: Record<string, boolean>): boolean {
  return safeLocalSet(INCIDENT_GROUPS_COLLAPSED_KEY, JSON.stringify(state));
}

// ── 050 Ola 8 P2 (FR-004) — modo compacto de incidentes (densidad alta, persistido) ─────────────

/** Clave de `localStorage` para el toggle de modo compacto del inbox (sufijo `_v1`). */
export const INCIDENT_COMPACT_KEY = "furx_incident_compact_v1";

/** Lee el toggle de modo compacto persistido. Default `false` (vista normal → cero regresión). */
export function loadCompactIncidents(): boolean {
  return safeLocalGet(INCIDENT_COMPACT_KEY) === "1";
}

/** Persiste el toggle de modo compacto. Devuelve `false` si no se pudo guardar (no tira). */
export function saveCompactIncidents(on: boolean): boolean {
  return safeLocalSet(INCIDENT_COMPACT_KEY, on ? "1" : "0");
}

/**
 * Estado de colapso INICIAL para los grupos dados, combinando lo persistido con el default de primer
 * arranque. Reglas (FR-002):
 *   - Si un grupo YA tiene un valor persistido (el usuario lo expandió/colapsó), ese valor manda.
 *   - Si NO (grupo nuevo / primer arranque): el grupo `critical` (o cualquiera que contenga una card
 *     critical) arranca EXPANDIDO (collapsed=false); el resto arranca COLAPSADO (collapsed=true).
 * `persisted` es lo que devuelve `loadCollapsedState()` (puede ser null en el primer arranque).
 */
export function initialCollapsedState(
  groups: IncidentGroup[],
  persisted: Record<string, boolean> | null,
): Record<string, boolean> {
  const out: Record<string, boolean> = {};
  for (const g of groups) {
    if (persisted && Object.prototype.hasOwnProperty.call(persisted, g.key)) {
      out[g.key] = persisted[g.key];
    } else {
      // Primer arranque para este grupo: expandido sólo si contiene un critical.
      out[g.key] = !groupHasCritical(g);
    }
  }
  return out;
}

// ── "Ir al origen": mapeo fuente → destino ──────────────────────────────────────────────────────

/** Destino de "ir al origen" de una card. Si no hay vista clara, `view` es null → abrir slide-over. */
export interface SourceTarget {
  /** vista a la que navegar, o null si la fuente no tiene una vista canónica. */
  view: View | null;
  /** filtro inicial (hoy sólo monitors→down). */
  drilldown: "monitors-down" | null;
  /** clave i18n del label del botón "ir al origen" (según la fuente). */
  labelKey: "incidents.source.monitor" | "incidents.source.worktree" | "incidents.source.generic";
}

/**
 * Mapea la `source` de una card a un destino navegable. Conservador: SÓLO mapea fuentes con una
 * vista canónica real. Cualquier otra fuente → `view: null` (el caller abre el slide-over de detalle,
 * cumpliendo el requisito "si no hay destino claro, al menos detalle").
 *   - monitor          → vista monitors filtrada a caídos.
 *   - merge / worktree → vista panes (el trabajo del worktree vive en los paneles).
 *   - resto            → null (slide-over).
 */
export function sourceTarget(card: Card): SourceTarget {
  switch (card.source) {
    case "monitor":
      return { view: "monitors", drilldown: "monitors-down", labelKey: "incidents.source.monitor" };
    case "merge":
    case "worktree":
      return { view: "panes", drilldown: null, labelKey: "incidents.source.worktree" };
    default:
      return { view: null, drilldown: null, labelKey: "incidents.source.generic" };
  }
}

/** ¿"Ir al origen" navega a una vista (true) o sólo abre el slide-over de detalle (false)? */
export function hasNavigableSource(card: Card): boolean {
  return sourceTarget(card).view !== null;
}
