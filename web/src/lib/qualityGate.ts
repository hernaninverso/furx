// 024-quality-gate F1 — helper PURO de presentación de la evidencia objetiva por variante.
// Sin DOM/React: lógica testeable (node:assert). El componente `BestOfNCompare` consume estas
// funciones para pintar el badge y el detalle.
//
// INVARIANTE (espejo del contrato del motor Rust): "no disponible" ≠ 0. Un `LinterResult` con
// `status != "ok"` NUNCA debe renderizarse como "0 issues"; se muestra "no disponible".

import type { LinterResult, LinterStatus, VariantEvidence } from "../types.ts";

export type EvidenceBadgeKind = "issues" | "clean" | "unavailable" | "partial";

export interface EvidenceBadge {
  kind: EvidenceBadgeKind;
  errors: number;
  warnings: number;
  /** Texto corto para el badge (ej "2 err · 3 warn", "limpio", "no disponible"). */
  label: string;
  /** Herramientas que no se pudieron medir (para el sufijo "+N no disponible"). */
  unavailableTools: string[];
}

/** Etiqueta legible por estado de un linter (UI: gris para no-ok). */
export function statusLabel(status: LinterStatus): string {
  switch (status) {
    case "ok":
      return "ok";
    case "unavailable":
      return "no disponible";
    case "timeout":
      return "tiempo agotado";
    case "unparsable":
      return "salida no interpretable";
  }
}

/** ¿Este resultado se midió de verdad? Sólo `ok` cuenta como medición. */
export function isMeasured(r: LinterResult): boolean {
  return r.status === "ok";
}

/**
 * Texto a mostrar para UN linter. CONTRATO: si `status != "ok"` devuelve la etiqueta de
 * "no disponible" — NUNCA "0". Si `ok`, devuelve el conteo (que puede ser un 0 REAL = limpio).
 */
export function toolCellText(r: LinterResult): string {
  if (r.status !== "ok") {
    const why = r.reason ? `${statusLabel(r.status)}: ${r.reason}` : statusLabel(r.status);
    return why;
  }
  if (r.errors === 0 && r.warnings === 0) return "limpio";
  const parts: string[] = [];
  if (r.errors > 0) parts.push(`${r.errors} err`);
  if (r.warnings > 0) parts.push(`${r.warnings} warn`);
  return parts.join(" · ");
}

/**
 * Resume la evidencia de UNA variante en un badge. Distingue 4 casos:
 *  - `unavailable`: NADA se midió (`any_measured === false`) → "no disponible" (NUNCA "0").
 *  - `issues`: hay errores y/o warnings medidos.
 *  - `clean`: se midió y dio 0/0.
 *  - `partial`: hubo medición pero también herramientas no disponibles (transparencia).
 */
export function evidenceBadge(ev: VariantEvidence | undefined | null): EvidenceBadge | null {
  if (!ev) return null;
  const unavailableTools = ev.unavailable_tools ?? [];
  if (!ev.any_measured) {
    return {
      kind: "unavailable",
      errors: 0,
      warnings: 0,
      label: "no disponible",
      unavailableTools,
    };
  }
  const e = ev.total_errors;
  const w = ev.total_warnings;
  const someUnavailable = unavailableTools.length > 0;
  if (e === 0 && w === 0) {
    return {
      kind: someUnavailable ? "partial" : "clean",
      errors: 0,
      warnings: 0,
      label: "limpio",
      unavailableTools,
    };
  }
  const parts: string[] = [];
  if (e > 0) parts.push(`${e} ${e === 1 ? "error" : "errores"}`);
  if (w > 0) parts.push(`${w} ${w === 1 ? "warning" : "warnings"}`);
  return {
    kind: someUnavailable ? "partial" : "issues",
    errors: e,
    warnings: w,
    label: parts.join(" · "),
    unavailableTools,
  };
}

/** Empareja las evidencias (lista) a un mapa por task_id para indexar desde las cards. */
export function evidenceByTask(list: VariantEvidence[] | undefined | null): Map<string, VariantEvidence> {
  const m = new Map<string, VariantEvidence>();
  for (const ev of list ?? []) m.set(ev.task_id, ev);
  return m;
}
