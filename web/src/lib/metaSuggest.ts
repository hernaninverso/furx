// 020 meta-orchestrator US2/US3 — lógica PURA del cableado de sugerencias advisory del AIE.
//
// Backend (commiteado en master):
//   - meta_suggest_variant_ranking(group_id) -> Option<Vec<usize>>  (US2): orden sugerido de las
//     variantes best-of-N, índices contra el orden de variantes (variant_index ASC). None si OFF /
//     sin diffs / AIE caído.
//   - meta_suggest_agent(objective) -> Option<String>  (US3): categoría sugerida (bugfix/feature/…).
//     None si OFF / AIE caído.
// Ambos son ADVISORY: NUNCA mutan estado. Acá vive el parseo del ranking → labels y el mapping
// categoría → display (sin tocar React/Tauri) para poder testearlo con `npm test`.

// ───────────────────────── US2: ranking de variantes ─────────────────────────

/** Etiqueta visible de una variante por su índice en el orden de variantes (0-based → "v1"). */
export function variantLabel(index: number): string {
  return `v${index + 1}`;
}

/**
 * Resultado normalizado de un ranking sugerido por el AIE.
 * - `bestIndex`: índice (en el orden de variantes) de la variante sugerida como mejor (orden[0]).
 * - `order`: el ranking completo, saneado.
 * - `summary`: línea legible "v2 > v1 > v3" para mostrar como hint.
 */
export interface RankingSuggestion {
  bestIndex: number;
  order: number[];
  summary: string;
}

/**
 * Normaliza el `Option<Vec<usize>>` que devuelve `meta_suggest_variant_ranking` a una sugerencia
 * lista para mostrar, validándola contra la cantidad real de variantes renderizadas.
 *
 * Devuelve `null` (NO mostrar nada) cuando:
 *   - el ranking es `null`/`undefined` (feature OFF, sin diffs, o AIE caído);
 *   - está vacío;
 *   - no es un array de enteros ≥0;
 *   - contiene índices fuera de `[0, variantCount)` o repetidos (ranking corrupto);
 *   - su longitud no coincide con `variantCount` (desalineado con lo renderizado).
 * Cualquier inconsistencia ⇒ `null`: una sugerencia advisory dudosa no se muestra (no rompe el picker).
 */
export function parseRankingSuggestion(
  ranking: number[] | null | undefined,
  variantCount: number,
): RankingSuggestion | null {
  if (!Array.isArray(ranking) || ranking.length === 0) return null;
  if (variantCount <= 0 || ranking.length !== variantCount) return null;
  const seen = new Set<number>();
  for (const idx of ranking) {
    if (!Number.isInteger(idx) || idx < 0 || idx >= variantCount) return null;
    if (seen.has(idx)) return null;
    seen.add(idx);
  }
  const summary = ranking.map(variantLabel).join(" › ");
  return { bestIndex: ranking[0], order: [...ranking], summary };
}

/**
 * Clave estable derivada del CONTENIDO de las variantes (no sólo del count).
 *
 * Motivo (codex/deepseek HIGH 1): el ranking sugerido depende de los DIFFS de las variantes. Si los
 * diffs se refrescan pero la cantidad de variantes no cambia, una dep `variants.length` NO re-dispara
 * el efecto → la sugerencia queda STALE (basada en diffs viejos). Usando esta clave como dep, un
 * refresh de diffs con el mismo count cambia la clave y re-pide la sugerencia.
 *
 * Incluye, por variante, su `task_id`, `state` y `diff_stat` (el resumen del diff que ve el AIE).
 * `OrchVariantDiff` no expone `revision`/`updated_at`, así que el contenido del diff + el estado son
 * la señal de cambio disponible.
 */
export function variantsContentKey(
  variants: ReadonlyArray<{ task_id: string; state: string; diff_stat: string }>,
): string {
  return variants.map((v) => `${v.task_id}\x00${v.state}\x00${v.diff_stat}`).join("");
}

// ───────────────────────── US3: sugerencia de agente ─────────────────────────

// Mapa categoría cruda (lo que clasifica el AIE) → display legible. Claves en minúscula.
// Set CERRADO y conocido: el output del LLM NO se confía como texto arbitrario en la UI.
const CATEGORY_DISPLAY: Record<string, string> = {
  bugfix: "bugfix",
  feature: "feature",
  refactor: "refactor",
  docs: "docs",
  test: "test",
  chore: "chore",
  perf: "performance",
  style: "estilo",
};

// Defensa en profundidad (deepseek HIGH 2): si el AIE devuelve una categoría FUERA del set conocido,
// no la mostramos cruda. La saneamos a un slug corto y conservador (alfanumérico + espacio/guion,
// ≤ MAX_RAW_CATEGORY_LEN). React ya escapa el texto (no hay innerHTML → no hay XSS real), pero igual
// no dejamos basura/inyección de longitud arbitraria del LLM en la UI. Si tras sanear no queda nada
// útil, descartamos la sugerencia (null).
const MAX_RAW_CATEGORY_LEN = 24;

function sanitizeRawCategory(key: string): string | null {
  // Sólo letras (incl. acentos comunes), números, espacio y guion. El resto se descarta.
  const cleaned = key
    .replace(/[^\p{L}\p{N} -]+/gu, "")
    .replace(/\s+/g, " ")
    .trim()
    .slice(0, MAX_RAW_CATEGORY_LEN)
    .trim();
  return cleaned === "" ? null : cleaned;
}

/**
 * Normaliza la categoría cruda que devuelve `meta_suggest_agent` a un display.
 * Devuelve `null` (no mostrar hint) si viene `null`/vacía.
 *
 * Endurecido (deepseek HIGH 2): la categoría se limita a un set CONOCIDO → display amigable. Si el
 * AIE devuelve algo FUERA del set, se sanitiza (truncado ~24 chars, sólo alfanumérico + espacio/guion);
 * si tras sanear queda vacío, se descarta (`null`). NUNCA se renderiza el output del LLM como texto
 * arbitrario en la UI.
 */
export function agentCategoryDisplay(category: string | null | undefined): string | null {
  if (category == null) return null;
  const key = category.trim().toLowerCase();
  if (key === "") return null;
  const known = CATEGORY_DISPLAY[key];
  if (known) return known;
  return sanitizeRawCategory(key);
}
