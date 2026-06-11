// 019 F2 T020 — lógica PURA de filtrado para el kit (`FilterBar.tsx` es sólo presentación).
// Destilada de patrones que ya existen: AuditDrawer (filtra kind/actor), CommandPalette015
// (texto + faceta de categoría), CouncilModal/AgentGallery (subconjuntos por campo). Acá vive el
// matcher; el componente sólo lo invoca → testeable sin DOM y reusable en ≥3 superficies.

/** Una faceta = un campo discreto por el que se puede filtrar (estado, agente, categoría…). */
export interface Facet {
  /** id estable de la faceta (clave en el predicado del item). */
  id: string;
  /** etiqueta visible. */
  label: string;
  /** valores posibles (ej los estados de una tarea). `null` = "todos". */
  options: { value: string; label: string; count?: number }[];
}

/** Estado de filtro: texto libre + faceta activa por id (null = sin faceta). */
export interface FilterState {
  /** búsqueda de texto (case-insensitive, trim). */
  query: string;
  /** facetId → valor seleccionado (string) o null (todos). */
  facets: Record<string, string | null>;
}

export function emptyFilter(): FilterState {
  return { query: "", facets: {} };
}

/**
 * Devuelve true si `haystacks` (los campos buscables de un item) matchean la query.
 * Match = TODOS los tokens de la query aparecen en ALGÚN haystack (AND de tokens, OR de campos).
 * Pura: no toca DOM, no captura nada externo.
 */
export function matchesQuery(query: string, haystacks: (string | null | undefined)[]): boolean {
  const q = query.trim().toLowerCase();
  if (!q) return true;
  const tokens = q.split(/\s+/).filter(Boolean);
  const hay = haystacks.filter((h): h is string => !!h).map((h) => h.toLowerCase());
  return tokens.every((t) => hay.some((h) => h.includes(t)));
}

/** True si el item pasa TODAS las facetas activas. `getFacet` mapea facetId → valor del item. */
export function matchesFacets(
  facets: Record<string, string | null>,
  getFacet: (facetId: string) => string | null | undefined,
): boolean {
  for (const [facetId, want] of Object.entries(facets)) {
    if (want == null) continue; // "todos"
    if ((getFacet(facetId) ?? null) !== want) return false;
  }
  return true;
}

/**
 * Filtra una lista genérica con texto + facetas. `accessors` describe cómo leer cada item:
 * - `text(item)` → campos buscables
 * - `facet(item, facetId)` → valor de esa faceta para el item
 * Devuelve un array NUEVO (no muta). Pura y O(n·m).
 */
export function applyFilter<T>(
  items: readonly T[],
  filter: FilterState,
  accessors: {
    text: (item: T) => (string | null | undefined)[];
    facet?: (item: T, facetId: string) => string | null | undefined;
  },
): T[] {
  return items.filter(
    (it) =>
      matchesQuery(filter.query, accessors.text(it)) &&
      matchesFacets(filter.facets, (fid) => accessors.facet?.(it, fid)),
  );
}

/** Cuenta items por valor de una faceta (para los badges de conteo en los chips). */
export function facetCounts<T>(
  items: readonly T[],
  facetId: string,
  facet: (item: T, facetId: string) => string | null | undefined,
): Record<string, number> {
  const out: Record<string, number> = {};
  for (const it of items) {
    const v = facet(it, facetId);
    if (v == null) continue;
    out[v] = (out[v] ?? 0) + 1;
  }
  return out;
}
