// 035-ai-visibility-evidence F1 — núcleo PURO de la visibilidad verificable de la IA.
// Sin DOM/React: lógica testeable (node:assert). Deriva, de datos que YA EXISTEN
// (VariantEvidence, OrchTask, OrchLogEntry, PaneUsage/SessionUsage de Claude), tres cosas:
//   1) un VEREDICTO por dimensión (evidencia / costo) con 3 estados de MEDICIÓN,
//   2) un veredicto GLOBAL que NUNCA es "verde" si alguna dimensión no se midió,
//   3) un TIMELINE scrubbable reconstruido del log-history + transiciones de estado.
//
// INVARIANTE CENTRAL (espejo del contrato de qualityGate.ts y del motor Rust):
//   "no medido" ≠ 0 y ≠ verde. Sin dato → estado `unmeasured` (gris, borde punteado).
//   Una dimensión no medida JAMÁS se agrega a un score "bien": contamina el global a `partial`.
//   El producto SEÑALA lo que sabe y lo que NO; no tranquiliza con un check verde falso.

import type { LinterResult, OrchLogEntry, OrchTask, VariantEvidence } from "../types.ts";
import { evidenceBadge } from "./qualityGate.ts";

// ── 1. Estados de MEDICIÓN (los 3 estados visuales del badge) ───────────────────────────────

/**
 * El estado de medición de UNA dimensión:
 *  - `measured_good`  → medido y bien (cian/verde sólido, ✓).
 *  - `measured_bad`   → medido y mal (ámbar/rojo sólido; salta primero en el orden).
 *  - `unmeasured`     → no medido (gris apagado + borde punteado, ⊘). NUNCA color de alarma ni verde.
 */
export type MeasurementState = "measured_good" | "measured_bad" | "unmeasured";

/** Orden de severidad para "lo malo salta primero": bad < good < unmeasured nunca pisa al resto. */
const STATE_RANK: Record<MeasurementState, number> = {
  measured_bad: 0,
  measured_good: 1,
  unmeasured: 2,
};

/** El badge de UNA dimensión: estado + etiqueta + por qué (tooltip) + hint de cómo medirlo. */
export interface MeasurementBadge {
  /** Clave de la dimensión (ej "evidence", "tokens"). */
  dimension: string;
  state: MeasurementState;
  /** Texto corto del badge (ej "0 errores", "no medido", "12.3k tokens"). */
  label: string;
  /**
   * El POR QUÉ (tooltip). Para `unmeasured`: explica por qué no se midió.
   * Para los medidos: contexto breve (de qué herramienta/origen sale el dato).
   */
  reason: string;
  /** Sólo para `unmeasured`: un hint accionable de CÓMO medirlo. null si no aplica. */
  measureHint: string | null;
  /**
   * COBERTURA PARCIAL (finding codex#2): el estado es `measured_good`/`measured_bad` PERO la
   * medición fue INCOMPLETA (alguna herramienta no se pudo correr). Un badge así NUNCA debe
   * producir un veredicto global "todo medido y limpio": `globalVerdict` lo degrada a `partial`.
   * `false`/`undefined` = cobertura total para esta dimensión.
   */
  partialCoverage?: boolean;
}

// ── 2. Dimensión EVIDENCIA (errores/warnings por herramienta) ───────────────────────────────

/**
 * Deriva el badge de la dimensión "evidencia" (lint/typecheck/tests) de UNA variante.
 * Reusa `evidenceBadge` (qualityGate.ts) — la fuente de verdad del contrato "no disponible ≠ 0".
 *  - `unavailable` (any_measured=false) → `unmeasured` + hint de instalar/correr el quality-gate.
 *  - `issues` (errores/warnings reales) → `measured_bad`.
 *  - `clean`/`partial` (0/0 medido)     → `measured_good` (un 0 REAL).
 * `partial` (medido pero con algún tool no disponible) se reporta como `measured_good` PERO
 * con `reason` que nombra los tools faltantes — la "parcialidad" la propaga el veredicto global.
 */
export function evidenceDimension(ev: VariantEvidence | undefined | null): MeasurementBadge {
  const b = evidenceBadge(ev);
  if (!b || b.kind === "unavailable") {
    const tools = b?.unavailableTools ?? [];
    return {
      dimension: "evidence",
      state: "unmeasured",
      label: "no medido",
      reason: tools.length > 0
        ? `no se corrieron linters en esta variante (${tools.join(", ")} no disponible)`
        : "no se corrieron linters/tests en esta variante",
      measureHint: "activá «qualitygate.enabled» en Ajustes y corré el quality-gate",
    };
  }
  // BAD si hay errores o warnings REALES — independiente de `kind` (codex#1): `evidenceBadge`
  // devuelve `kind:"partial"` cuando hay issues Y tools no disponibles, así que NO alcanza con
  // chequear `kind === "issues"`; hay que mirar los conteos.
  const someUnavailable = hasUnavailable(b.unavailableTools);
  if (b.errors > 0 || b.warnings > 0) {
    return {
      dimension: "evidence",
      state: "measured_bad",
      label: b.label,
      reason: someUnavailable
        ? `medido por los linters del repo · ${b.unavailableTools.length} herramienta(s) sin medir`
        : "medido por los linters del repo",
      measureHint: null,
      partialCoverage: someUnavailable,
    };
  }
  // 0/0 REAL medido → measured_good. Si la cobertura fue PARCIAL (algún tool sin correr), se marca
  // `partialCoverage` para que el veredicto global NO diga "todo medido y limpio" (codex#2).
  return {
    dimension: "evidence",
    state: "measured_good",
    label: "limpio",
    reason: someUnavailable
      ? `0 issues medidos · ${b.unavailableTools.length} herramienta(s) sin medir`
      : "0 issues medidos por los linters del repo",
    measureHint: null,
    partialCoverage: someUnavailable,
  };
}

function hasUnavailable(tools: string[] | undefined): boolean {
  return (tools?.length ?? 0) > 0;
}

// ── 3. Dimensión COSTO en tokens ────────────────────────────────────────────────────────────

/** Uso de tokens medido de un agente (espejo mínimo de PaneUsage/SessionUsage de Claude). */
export interface TokenUsageLike {
  input_tokens: number;
  output_tokens: number;
  model?: string | null;
}

/**
 * Deriva el badge de la dimensión "costo" de un agente.
 *  - Si NO hay uso medido (Codex/Gemini/Aider hoy, o Claude sin sesión) → `unmeasured`.
 *  - Si HAY uso medido (Claude) → `measured_good` con el total de tokens. NO inventamos USD:
 *    v1 reporta tokens; el USD multi-agente es follow-up (G2).
 * `cliKind` se usa SOLO para el reason/hint (por qué este agente no expone uso) — NUNCA cambia
 * el estado: un agente sin dato es `unmeasured`, medido o no es lo único que decide el color.
 */
export function tokenCostDimension(
  usage: TokenUsageLike | null | undefined,
  cliKind?: string | null,
): MeasurementBadge {
  // Fail-closed (codex#3): sólo es "medido" si AMBOS conteos son finitos y el total es > 0. Un
  // NaN/negativo/no-finito NO es una medición válida → `unmeasured` (nunca un verde con label basura).
  const validUsage =
    !!usage &&
    Number.isFinite(usage.input_tokens) &&
    Number.isFinite(usage.output_tokens) &&
    usage.input_tokens >= 0 &&
    usage.output_tokens >= 0 &&
    usage.input_tokens + usage.output_tokens > 0;
  if (!validUsage) {
    const kind = (cliKind ?? "").toLowerCase();
    // Claude expone uso; los demás CLIs todavía no (G2). El hint refleja el caso real.
    const claudeButEmpty = kind === "claude" || kind === "" || kind === "zsh";
    return {
      dimension: "tokens",
      state: "unmeasured",
      label: "no medido",
      reason: claudeButEmpty
        ? "todavía no hay sesión con uso de tokens para este agente"
        : `${kind} no expone uso de tokens (sólo Claude lo registra hoy)`,
      measureHint: claudeButEmpty
        ? "corré al menos un turno con este agente para registrar uso"
        : "el costo multi-agente (Codex/Gemini/Aider) llega en una próxima versión",
    };
  }
  // validUsage ya garantizó `usage` no-nulo y conteos finitos/positivos (el `!` es seguro acá).
  const u = usage!;
  const total = u.input_tokens + u.output_tokens;
  return {
    dimension: "tokens",
    state: "measured_good",
    label: `${fmtTokens(total)} tokens`,
    reason: u.model
      ? `medido del registro de uso de Claude (${u.model})`
      : "medido del registro de uso de Claude",
    measureHint: null,
  };
}

/** Formateo compacto de un conteo de tokens (espejo de types.ts::formatTok, sin importar React). */
export function fmtTokens(n: number): string {
  if (n < 1000) return String(n);
  if (n < 1_000_000) return `${(n / 1000).toFixed(1)}k`;
  return `${(n / 1_000_000).toFixed(2)}M`;
}

// ── 4. VEREDICTO GLOBAL (nunca verde si falta una dimensión) ────────────────────────────────

/**
 * El veredicto agregado de una variante/agente:
 *  - `measured_issues` → al menos una dimensión medida dio MAL (errores/warnings). Manda (salta primero).
 *  - `partial`         → todo lo medido está bien PERO al menos una dimensión NO se midió.
 *  - `measured_ok`     → TODAS las dimensiones se midieron y están bien.
 *  - `unmeasured`      → NINGUNA dimensión se midió.
 */
export type GlobalVerdictKind = "measured_issues" | "measured_ok" | "partial" | "unmeasured";

export interface GlobalVerdict {
  kind: GlobalVerdictKind;
  /** Texto legible del veredicto ("parcialmente medido", "todo medido y limpio", …). */
  label: string;
  /** Las dimensiones que NO se midieron (para el detalle "qué falta medir"). */
  unmeasuredDimensions: string[];
}

/**
 * Agrega las dimensiones en el veredicto global. REGLA NON-NEGOTIABLE: una dimensión `unmeasured`
 * JAMÁS produce `measured_ok` — degrada a `partial` (o, si nada se midió, `unmeasured`). Un
 * `measured_bad` siempre manda (la señal de problema no se diluye con lo no medido).
 */
export function globalVerdict(dims: MeasurementBadge[]): GlobalVerdict {
  const unmeasured = dims.filter((d) => d.state === "unmeasured").map((d) => d.dimension);
  const anyBad = dims.some((d) => d.state === "measured_bad");
  const anyGood = dims.some((d) => d.state === "measured_good");
  // Cobertura PARCIAL en alguna dimensión MEDIDA (codex#2): aunque su estado sea good/bad, la
  // medición fue incompleta → el global NO puede ser "todo medido y limpio".
  const anyPartialCoverage = dims.some((d) => d.partialCoverage);

  if (anyBad) {
    return { kind: "measured_issues", label: "se detectaron problemas", unmeasuredDimensions: unmeasured };
  }
  if (unmeasured.length === 0 && !anyPartialCoverage && anyGood) {
    return { kind: "measured_ok", label: "todo medido y limpio", unmeasuredDimensions: [] };
  }
  if (anyGood && (unmeasured.length > 0 || anyPartialCoverage)) {
    return { kind: "partial", label: "parcialmente medido", unmeasuredDimensions: unmeasured };
  }
  // ni bad ni good → nada se midió (todas unmeasured, o lista vacía).
  return { kind: "unmeasured", label: "no medido", unmeasuredDimensions: unmeasured };
}

/** Ordena badges para mostrar "lo malo primero, lo no-medido al final". Estable. */
export function orderBadges(dims: MeasurementBadge[]): MeasurementBadge[] {
  return [...dims].sort((a, b) => STATE_RANK[a.state] - STATE_RANK[b.state]);
}

// ── 5. TIMELINE scrubbable (Golpe 2 cinematográfico) ────────────────────────────────────────

/** El tipo de paso del timeline — derivado del `source` del snapshot o de una transición de estado. */
export type TimelineStepKind = "spawned" | "working" | "ready" | "manual" | "state" | "terminal" | "live";

/** Un paso del timeline de un agente: ordenado por tiempo, scrubbable por índice. */
export interface TimelineStep {
  /** Índice 0-based del paso en el orden cronológico (el scrub se hace sobre esto). */
  index: number;
  /** ISO del momento (captured_at del snapshot, o updated_at del task para transiciones). */
  at: string;
  kind: TimelineStepKind;
  /** Etiqueta legible del paso (ej "Snapshot del poller", "Marcada para revisar"). */
  label: string;
  /** El contenido asociado (el snapshot del log) para mostrar al hacer scrub. "" si no hay. */
  content: string;
  /** El `source` original del snapshot (poller|mark_ready|manual) si vino de un log. */
  source: string | null;
  /**
   * LIVE (finding council ALTA/MEDIA): un paso PROVISIONAL observado por el eventBus en vivo
   * (transición de estado), NO un dato persistido. Los logs son canónicos: un paso provisional
   * NUNCA contamina `globalVerdict`/evidencia/costo (FR-012) y es superseded por un snapshot real
   * al mismo estado terminal (FR-004b). `undefined`/`false` = paso canónico (de `buildTimeline`).
   */
  provisional?: boolean;
}

/** Mapea el `source` de un OrchLogEntry a un kind + etiqueta del timeline. */
function stepFromSource(source: string): { kind: TimelineStepKind; label: string } {
  switch (source) {
    case "poller":
      return { kind: "working", label: "Snapshot del agente trabajando" };
    case "mark_ready":
      return { kind: "ready", label: "Marcada para revisar" };
    case "manual":
      return { kind: "manual", label: "Snapshot manual" };
    default:
      return { kind: "working", label: `Snapshot (${source})` };
  }
}

/** Etiqueta del estado terminal/transición para el primer/último paso sintético. */
function labelForState(state: OrchTask["state"]): string {
  switch (state) {
    case "pending": return "Pendiente";
    case "running": return "Corriendo";
    case "awaiting_review": return "Para revisar";
    case "done": return "Hecha";
    case "failed": return "Falló";
    case "canceled": return "Cancelada";
    default: return state;
  }
}

/**
 * Reconstruye el timeline scrubbable de UNA tarea/agente a partir de:
 *  - el `OrchTask` (para el paso inicial "spawned" y el terminal si aplica),
 *  - sus `OrchLogEntry[]` snapshots (cada uno = un paso, ordenados por captured_at ASC).
 * NO inventa eventos: cada paso corresponde a un dato real (un snapshot o una transición observada).
 * Ordena ASC por tiempo y re-indexa. El primer paso sintético "spawned" usa created_at del task.
 * Si el estado es terminal (done/failed/canceled) agrega un paso final con updated_at.
 */
export function buildTimeline(task: OrchTask, logs: OrchLogEntry[]): TimelineStep[] {
  const raw: Omit<TimelineStep, "index">[] = [];

  // Paso 0 sintético: el agente arrancó (created_at siempre existe).
  raw.push({
    at: task.created_at,
    kind: "spawned",
    label: `Agente lanzado (${task.cli_kind ?? task.mode ?? "?"})`,
    content: "",
    source: null,
  });

  // Un paso por cada snapshot del log-history (dato real persistido).
  for (const e of logs) {
    const { kind, label } = stepFromSource(e.source);
    raw.push({
      at: e.captured_at,
      kind,
      label,
      content: e.content ?? "",
      source: e.source,
    });
  }

  // Paso terminal sintético si la tarea ya terminó (transición observada por el estado actual).
  if (task.state === "done" || task.state === "failed" || task.state === "canceled") {
    raw.push({
      at: task.updated_at,
      kind: "terminal",
      label: labelForState(task.state),
      content: task.result_summary ?? "",
      source: null,
    });
  }

  // Orden cronológico ESTABLE (ASC). Empate de timestamp: conserva el orden de inserción
  // (spawned < snapshots en el orden que vinieron < terminal), que ya es el cronológico correcto.
  const sorted = stableSortByTime(raw);
  return sorted.map((s, index) => ({ ...s, index }));
}

/**
 * Sort estable ASC por el campo `at` (ISO). Empates conservan el orden original.
 *
 * Fail-closed con timestamps NO parseables (finding codex): un timestamp roto NO debe saltar al
 * frente. Se ancla al ÚLTIMO timestamp válido visto en el orden de inserción (carry-forward) → el
 * paso roto queda DONDE estaba (justo después de su predecesor válido), nunca reordenado a ciegas.
 * El índice de inserción rompe empates (estabilidad).
 */
function stableSortByTime<T extends { at: string }>(arr: T[]): T[] {
  // Pre-pass: clave numérica por elemento, llevando hacia adelante el último epoch válido.
  let lastValid = Number.NEGATIVE_INFINITY; // antes del primer válido → los rotos iniciales van al frente, en su orden
  const keyed = arr.map((v, i) => {
    const t = Date.parse(normalizeIso(v.at));
    if (!Number.isNaN(t)) lastValid = t;
    return { v, i, key: Number.isNaN(t) ? lastValid : t };
  });
  return keyed
    .sort((a, b) => (a.key !== b.key ? a.key - b.key : a.i - b.i))
    .map((x) => x.v);
}

/** Normaliza un timestamp del backend ("YYYY-MM-DD HH:MM:SS" sin TZ) a ISO con Z (UTC). */
function normalizeIso(s: string): string {
  if (!s) return s;
  if (s.includes("T")) return s;
  return s.replace(" ", "T") + "Z";
}

/**
 * Clampa un índice de scrub al rango válido del timeline. Devuelve 0 para timeline vacío.
 * Garantiza que la UI nunca indexe fuera de rango al hacer scrub.
 */
export function clampScrub(index: number, len: number): number {
  if (len <= 0) return 0;
  if (!Number.isFinite(index)) return 0; // fail-closed: NaN/±Inf → índice válido (0), nunca NaN
  if (index < 0) return 0;
  if (index >= len) return len - 1;
  return Math.floor(index);
}

// ── 5b. TIMELINE EN VIVO (Golpe 2 primario: el agente trabajando AHORA) ──────────────────────
// Los logs son CANÓNICOS (finding council ALTA). Los pasos vivos son PROVISIONALES: registran una
// transición de estado OBSERVADA por el eventBus, sin fabricar contenido. La UI suscribe SÓLO el
// timeline ABIERTO (una suscripción filtrada por task id) y appendea con `mergeLiveSteps`.

/** Estados terminales: una transición a uno de estos = paso terminal canónico (no se duplica). */
const TERMINAL_STATES = new Set<OrchTask["state"]>(["done", "failed", "canceled"]);

/**
 * Deriva un paso VIVO de una transición de estado observada por el eventBus.
 *  - Sólo emite ante una transición REAL (`prev !== next`); estados iguales → `null` (dedup).
 *  - El paso es PROVISIONAL (`provisional: true`) y NO fabrica contenido (`content: ""`): sólo
 *    registra QUE el agente pasó a `next` y CUÁNDO. El snapshot/terminal canónico lo supersede.
 * `at` es el momento observado (ISO o "YYYY-MM-DD HH:MM:SS"); `cliKind` sólo enriquece la etiqueta.
 */
export function liveStepFromTransition(
  prevState: OrchTask["state"] | null | undefined,
  nextState: OrchTask["state"],
  at: string,
  cliKind?: string | null,
): TimelineStep | null {
  if (prevState === nextState) return null; // no hubo transición real → no se agrega paso
  const kind: TimelineStepKind = TERMINAL_STATES.has(nextState) ? "terminal" : "live";
  const suffix = cliKind ? ` (${cliKind})` : "";
  return {
    index: -1, // lo re-asigna mergeLiveSteps al re-indexar
    at,
    kind,
    label: `→ ${labelForState(nextState)}${suffix}`,
    content: "",
    source: null,
    provisional: true,
  };
}

/**
 * Mergea pasos VIVOS (provisionales) sobre el timeline CANÓNICO de `buildTimeline`. Reglas
 * (finding council ALTA — prioridad canónica, sin reconciliation-ID de backend):
 *  - Un paso vivo se DESCARTA si el canónico ya representa ese estado terminal (un `terminal`
 *    canónico domina al provisional del mismo estado) — evita el doble conteo.
 *  - Re-ordena ASC ESTABLE por tiempo (cubre eventos out-of-order; el eventBus ya es monotónico).
 *  - Acota a `cap` pasos (crecimiento acotado, FR-006): conserva los `cap` más recientes.
 *  - Re-indexa 0..n. No muta las entradas de entrada.
 */
export function mergeLiveSteps(
  canonical: TimelineStep[],
  live: TimelineStep[],
  cap = 50,
): TimelineStep[] {
  // ¿Qué estados terminales ya cubre el timeline canónico? (label sintético de labelForState).
  const canonicalTerminalLabels = new Set(
    canonical.filter((s) => s.kind === "terminal").map((s) => s.label),
  );
  const keptLive = live.filter((s) => {
    // Un paso vivo terminal cuyo estado ya está en el canónico = duplicado → descartar.
    if (s.kind === "terminal") {
      const stateLabel = s.label.replace(/^→\s*/, "").replace(/\s*\(.*\)$/, "");
      // El canónico usa labelForState directo (sin "→"); comparamos por ese label limpio.
      if (canonicalTerminalLabels.has(stateLabel)) return false;
    }
    return true;
  });
  const all = stableSortByTime([...canonical, ...keptLive]);
  const capped = all.length > cap ? all.slice(all.length - cap) : all;
  return capped.map((s, index) => ({ ...s, index }));
}

// ── 6. Helper de presentación: by_tool → filas con su estado de medición ─────────────────────

/** Una fila del scorecard de evidencia: una herramienta + su estado de medición + texto. */
export interface ToolRow {
  tool: string;
  state: MeasurementState;
}

/** Mapea un LinterResult a su estado de medición (para colorear cada fila del scorecard). */
export function toolRowState(r: LinterResult): MeasurementState {
  if (r.status !== "ok") return "unmeasured";
  if (r.errors > 0) return "measured_bad";
  if (r.warnings > 0) return "measured_bad";
  return "measured_good";
}
