// spec-022 P0b · REFORMA 3 — Stats accionables (drill-down). CERO stat muerto.
//
// Cada stat del footer del sidebar es una PUERTA a una acción: al click navega a la
// vista que lo origina, y donde aplica aplica un filtro inicial (incidentes→abiertos,
// monitors→down). NO existe ningún literal de estado decorativo ("Schema v3" eliminado).
//
// Lógica PURA y testeable: no toca Tauri ni React. El Shell consume `buildSidebarStats`
// para pintar los controles y `goToStat` (vía su propio router) para navegar+filtrar.

import type { View } from "./router.ts";
import type { LocaleKey } from "../locales/es.ts";

/**
 * Translator i18n inyectado (subset estructural de `t` de lib/i18n). Se pasa para que las etiquetas
 * de los stats salgan del catálogo (US5/FR-008) SIN que esta lógica pura importe React/Tauri.
 * Tipado laxo en params (la paridad de placeholders ya la valida `tsc -b` en el catálogo).
 */
export type StatTranslator = (key: LocaleKey, params?: Record<string, string | number>) => string;

/** Filtro inicial que un stat aplica a su vista destino. */
export type StatFilter =
  | { kind: "incidents"; status: "open" }
  | { kind: "monitors"; status: "down" }
  | { kind: "none" };

/** Filtro vigente de una vista (estado efímero del Shell). `null` = sin filtro. */
export type ViewFilter =
  | { view: "incidents"; status: "open" }
  | { view: "monitors"; status: "down" }
  | null;

/** Un stat accionable: etiqueta + valor (de datos reales) + destino + filtro. */
export interface ActionableStat {
  /** id estable (para keys / tests). */
  id: "incidents" | "panes" | "monitors";
  /** etiqueta legible (sentence-case, ES). */
  label: string;
  /** valor renderizable (string ya formateado de datos reales). */
  value: string;
  /** vista a la que navega el click. */
  destView: View;
  /** filtro inicial que la vista aplica (drill-down a la causa). */
  filter: StatFilter;
  /** aria-label completo del control accionable. */
  ariaLabel: string;
}

/** Datos vivos que alimentan los stats (todos derivados de backend/estado real). */
export interface StatInputs {
  openIncidents: number;
  panes: number;
  monitorsUp: number;
  monitorsTotal: number;
}

/**
 * Translator de fallback (sin i18n) — usado SÓLO por tests/llamadas legacy sin catálogo.
 * Devuelve un texto ES mínimo; producción SIEMPRE pasa el `t` real (US5/FR-008).
 */
const DEFAULT_T: StatTranslator = (key, params) => {
  const p = params ?? {};
  switch (key) {
    case "chrome.stats.incidents": return "Incidentes abiertos";
    case "chrome.stats.panes": return "Paneles";
    case "chrome.stats.monitors": return "Monitors";
    case "chrome.stats.incidentsAria": return `Ver ${p.count} incidentes abiertos`;
    case "chrome.stats.panesAria": return `Ir a los ${p.count} paneles de trabajo`;
    case "chrome.stats.monitorsDownAria": return `Ver ${p.down} monitores caídos de ${p.total}`;
    case "chrome.stats.monitorsUpAria": return `Ver monitores (${p.up} de ${p.total} arriba)`;
    case "chrome.stats.monitorsValue": return `${p.up}/${p.total} arriba`;
    case "chrome.stats.freshNow": return "Recién";
    case "chrome.stats.freshSecs": return `Hace ${p.n}s`;
    case "chrome.stats.freshMins": return `Hace ${p.n}m`;
    case "chrome.stats.freshHrs": return `Hace ${p.n}h`;
    default: return String(key);
  }
};

/**
 * Deriva los stats accionables del footer del sidebar desde datos reales.
 * NO incluye "Schema v3" ni ningún literal de estado decorativo (FR-006).
 * Cada stat SIEMPRE tiene un `destView` real (cobertura por test L1).
 * Las etiquetas/aria salen del catálogo i18n vía el translator inyectado (US5/FR-008); sin él, un
 * fallback ES mínimo (sólo para tests). El `value` deriva de datos (no se traduce).
 */
export function buildSidebarStats(input: StatInputs, t: StatTranslator = DEFAULT_T): ActionableStat[] {
  const monitorsDown = Math.max(0, input.monitorsTotal - input.monitorsUp);
  return [
    {
      id: "incidents",
      label: t("chrome.stats.incidents"),
      value: String(input.openIncidents),
      destView: "incidents",
      filter: { kind: "incidents", status: "open" },
      ariaLabel: t("chrome.stats.incidentsAria", { count: input.openIncidents }),
    },
    {
      id: "panes",
      label: t("chrome.stats.panes"),
      value: String(input.panes),
      destView: "panes",
      filter: { kind: "none" },
      ariaLabel: t("chrome.stats.panesAria", { count: input.panes }),
    },
    {
      id: "monitors",
      label: t("chrome.stats.monitors"),
      // value localizado vía catálogo (NO "up" crudo en locale es — audit MED 1).
      value: t("chrome.stats.monitorsValue", { up: input.monitorsUp, total: input.monitorsTotal }),
      destView: "monitors",
      filter: { kind: "monitors", status: "down" },
      ariaLabel:
        monitorsDown > 0
          ? t("chrome.stats.monitorsDownAria", { down: monitorsDown, total: input.monitorsTotal })
          : t("chrome.stats.monitorsUpAria", { up: input.monitorsUp, total: input.monitorsTotal }),
    },
  ];
}

/** Traduce el `StatFilter` de un stat al `ViewFilter` que el Shell guarda en estado. */
export function statFilterToViewFilter(stat: ActionableStat): ViewFilter {
  switch (stat.filter.kind) {
    case "incidents":
      return { view: "incidents", status: "open" };
    case "monitors":
      return { view: "monitors", status: "down" };
    case "none":
      return null;
  }
}

/** Estado de navegación efímero del Shell: vista activa + filtro de drill-down vigente. */
export interface NavState {
  view: View;
  filter: ViewFilter;
}

/**
 * Reducer PURO de navegación con filtro ONE-SHOT (audit 3-frontera MED).
 *
 * Toda transición de vista pasa por acá. El filtro de drill-down (`incidents→open`,
 * `monitors→down`) es one-shot: SÓLO sobrevive si se setea ATÓMICAMENTE junto a la vista (lo
 * hace `goToStat`, que pasa `filter`). Cualquier nav NORMAL (sidebar, palette `view.*`, deeplinks,
 * atajos) llama sin `filter` (default `null`) → el filtro se LIMPIA. Así, re-entrar a
 * Incidents/Monitors por un camino que no sea el stat ve la vista SIN filtrar.
 *
 * Es deliberadamente trivial (setter atómico view+filter), pero existe como función para fijar
 * el invariante en un test y evitar el orden frágil de un `useEffect` que limpie el filtro.
 */
export function nextNavState(view: View, filter: ViewFilter = null): NavState {
  return { view, filter };
}

/**
 * Freshness relativo barato ("hace 3s" / "3s ago") desde un timestamp. `null`/futuro → vacío.
 * Las etiquetas salen del catálogo i18n vía el translator inyectado (audit MED 1): respeta el
 * locale activo en vez de devolver siempre los literales ES. Sin translator → fallback ES.
 */
export function freshnessLabel(stamp: number | null, now: number, t: StatTranslator = DEFAULT_T): string {
  if (stamp === null || stamp <= 0 || stamp > now) return "";
  const secs = Math.floor((now - stamp) / 1000);
  if (secs < 1) return t("chrome.stats.freshNow");
  if (secs < 60) return t("chrome.stats.freshSecs", { n: secs });
  const mins = Math.floor(secs / 60);
  if (mins < 60) return t("chrome.stats.freshMins", { n: mins });
  const hrs = Math.floor(mins / 60);
  return t("chrome.stats.freshHrs", { n: hrs });
}
