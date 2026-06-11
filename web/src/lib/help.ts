// web/src/lib/help.ts — 016 US2 (T020 + T075) · índice de Help DERIVADO (SSOT, cero duplicación).
//
// Construye `HelpEntry[]` desde:
//   - el Command Registry (`loadCommandRegistry()`), filtrando `internal`/`hidden` (sólo lo que el
//     usuario puede descubrir), agrupado por `category` (dominio). FR-006.
//   - los 6 dominios de `NAV_GROUPS` → entradas de "ir a la vista" (deeplink furx://<view>).
// Búsqueda fuzzy (reusa el scorer del CommandPalette015, replicado acá para no acoplar UI↔lib) que
// indexa label + description + category + `extra.keywords` (council T075: enriquecer SIN tocar Rust,
// usando el campo `extra` que YA existe en CommandDef). FR-007.
//
// `buildHelpIndex()` es PURO sobre sus inputs (cmds + nav) y MEMOIZADO por identidad de `cmds`
// (council T075) — el Help no recomputa el índice en cada keystroke.

import type { CommandDef } from "./commandRegistry.ts";
import { NAV_GROUPS } from "./navGroups.ts";
import { buildRoute } from "./router.ts";

/// Una entrada de Help (derivada, NO almacenada duplicada). FR Key Entities.
export interface HelpEntry {
  /// id estable (id de comando, o `nav.<view>` para entradas de navegación).
  id: string;
  /// dominio para agrupar (categoría del comando o etiqueta del grupo de nav).
  domain: string;
  label: string;
  description: string;
  /// deeplink `furx://…` para "ir/ejecutar" (el del comando, o la vista). `null` = sin acción directa.
  deeplink: string | null;
  /// id de comando real a ejecutar vía el gate del kernel (sólo si NO tiene deeplink). `null` = nav.
  commandId: string | null;
  /// metadata del riesgo para el gate (sólo informativo en Help; el gate real vive en `invoke`).
  risk: CommandDef["risk"];
  /// términos extra para el fuzzy (extra.keywords del registry). No se muestra.
  keywords: string[];
}

/// Extrae `extra.keywords` de un CommandDef de forma segura (council T075: el campo es opcional y de
/// tipo `unknown` en `extra`). Devuelve sólo strings.
function extractKeywords(extra: Record<string, unknown>): string[] {
  const kw = (extra as { keywords?: unknown }).keywords;
  if (!Array.isArray(kw)) return [];
  return kw.filter((x): x is string => typeof x === "string");
}

/**
 * Construye el índice de Help. MEMOIZADO por identidad del array `cmds` (council T075): si el caller
 * pasa el MISMO array (referencia), reusa el resultado. El Help carga el registry una vez y lo guarda
 * en state → la identidad es estable entre renders.
 */
let _memoCmds: CommandDef[] | null = null;
let _memoIndex: HelpEntry[] | null = null;

export function buildHelpIndex(cmds: CommandDef[]): HelpEntry[] {
  if (_memoCmds === cmds && _memoIndex) return _memoIndex;
  const out: HelpEntry[] = [];

  // 1) Comandos descubribles (no internal/hidden). FR-006.
  for (const c of cmds) {
    if (c.visibility === "internal" || c.visibility === "hidden") continue;
    out.push({
      id: c.id,
      domain: c.category || "general",
      label: c.label,
      // FR Edge: comando sin description → Help muestra label + categoría, NUNCA entrada vacía.
      description: c.description || "",
      deeplink: c.deeplink,
      commandId: c.deeplink ? null : c.id,
      risk: c.risk,
      keywords: extractKeywords(c.extra),
    });
  }

  // 2) Entradas de navegación a las vistas (los 6 dominios de navGroups). Deeplink furx://<view>.
  for (const g of NAV_GROUPS) {
    for (const item of g.items) {
      out.push({
        id: `nav.${item.view}`,
        domain: g.label,
        label: item.label,
        description: "",
        deeplink: buildRoute(item.view),
        commandId: null,
        risk: "safe",
        keywords: [g.label, item.view],
      });
    }
  }

  _memoCmds = cmds;
  _memoIndex = out;
  return out;
}

/// Sólo para tests: limpia la memoización entre suites.
export function __resetHelpMemo(): void {
  _memoCmds = null;
  _memoIndex = null;
}

/* ── Fuzzy match (subsequence scorer, idéntico al de CommandPalette015 para coherencia de UX). ── */
function fuzzyScore(query: string, text: string): number | null {
  if (query.length === 0) return 0;
  const q = query.toLowerCase();
  const tx = text.toLowerCase();
  let qi = 0, score = 0, prevIdx = -1;
  for (let ti = 0; ti < tx.length && qi < q.length; ti++) {
    if (tx[ti] === q[qi]) {
      if (prevIdx === ti - 1) score += 6;
      const atBoundary = ti === 0 || /[\s/_.\-:]/.test(tx[ti - 1]);
      if (atBoundary) score += 4;
      score += 1;
      prevIdx = ti;
      qi++;
    }
  }
  return qi === q.length ? score : null;
}

/// Mejor score de una entrada sobre label/description/domain/keywords (el campo más fuerte gana).
/// keywords/description/domain pesan menos que el label (igual estrategia que el palette).
export function helpEntryScore(query: string, e: HelpEntry): number | null {
  if (query.trim() === "") return 0;
  const candidates: (number | null)[] = [
    fuzzyScore(query, e.label),
    e.description ? penalize(fuzzyScore(query, e.description), 3) : null,
    penalize(fuzzyScore(query, e.domain), 4),
    ...e.keywords.map((k) => penalize(fuzzyScore(query, k), 1)),
  ];
  const scores = candidates.filter((s): s is number => s != null);
  return scores.length ? Math.max(...scores) : null;
}

function penalize(s: number | null, by: number): number | null {
  return s == null ? null : s - by;
}

/// Filtra + rankea por query. Vacío = todas (orden por dominio, luego label). FR-007.
export function searchHelp(index: HelpEntry[], query: string): HelpEntry[] {
  const scored: { e: HelpEntry; score: number }[] = [];
  for (const e of index) {
    const s = helpEntryScore(query, e);
    if (s != null) scored.push({ e, score: s });
  }
  scored.sort((a, b) => {
    if (query.trim() !== "" && b.score !== a.score) return b.score - a.score;
    if (a.e.domain !== b.e.domain) return a.e.domain.localeCompare(b.e.domain);
    return a.e.label.localeCompare(b.e.label);
  });
  return scored.map((x) => x.e);
}

/// Agrupa entradas por dominio (preservando el orden de entrada). Para render por secciones. FR-007.
export function groupByDomain(entries: HelpEntry[]): { domain: string; entries: HelpEntry[] }[] {
  const map = new Map<string, HelpEntry[]>();
  for (const e of entries) {
    const arr = map.get(e.domain) ?? [];
    arr.push(e);
    map.set(e.domain, arr);
  }
  return [...map.entries()].map(([domain, es]) => ({ domain, entries: es }));
}
