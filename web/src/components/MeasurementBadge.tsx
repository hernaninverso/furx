// 035-ai-visibility-evidence F2 — el badge VISUAL de UNA dimensión de medición (3 estados).
// Reusado por el scorecard de evidencia (Golpe 1) y el costo en tokens (Golpe 3).
//
// LOS 3 ESTADOS son visualmente DISTINTOS (diseño cerrado con el dueño):
//   - measured_good → cian/verde SÓLIDO (✓).
//   - measured_bad  → ámbar/rojo SÓLIDO (●) — salta primero en el orden.
//   - unmeasured    → gris apagado + BORDE PUNTEADO (⊘) — NUNCA color de alarma ni verde.
// El "no medido" comunica TRANSPARENCIA, no "falta de feature": tooltip con el POR QUÉ + un HINT
// de cómo medirlo. Tokens del tema (F-VI), dark+light.

import type { MeasurementBadge as Badge } from "../lib/aiVisibility";

/** Glyph + colores por estado. unmeasured: borde punteado, gris (jamás alarma/verde). */
function styleFor(state: Badge["state"]): { glyph: string; color: string; border: string; bg: string } {
  switch (state) {
    case "measured_bad":
      return {
        glyph: "●",
        color: "var(--clay, #b8543a)",
        border: "1px solid var(--clay, #b8543a)",
        bg: "color-mix(in srgb, var(--clay, #b8543a) 8%, transparent)",
      };
    case "measured_good":
      return {
        glyph: "✓",
        color: "var(--accent)",
        border: "1px solid var(--accent)",
        bg: "color-mix(in srgb, var(--accent) 8%, transparent)",
      };
    case "unmeasured":
    default:
      return {
        glyph: "⊘",
        color: "var(--ink-dim, #6b6358)",
        // BORDE PUNTEADO — el rasgo que distingue "no medido" de un warning sólido.
        border: "1px dashed var(--line, rgba(0,0,0,.3))",
        bg: "transparent",
      };
  }
}

/**
 * Tooltip nativo (title) con el POR QUÉ + el HINT de cómo medirlo. Para los medidos, sólo el reason.
 * El por qué SIEMPRE está visible al hover (transparencia: el producto SEÑALA lo que sabe y lo que no).
 */
function tooltip(b: Badge): string {
  if (b.state === "unmeasured" && b.measureHint) return `${b.reason}\n→ ${b.measureHint}`;
  return b.reason;
}

/**
 * Render de UN badge de medición. `compact` reduce padding/typografía para filas densas.
 * `prefix` opcional (ej "evidencia") rotula la dimensión antes del label.
 */
export function MeasurementBadge({ badge, prefix, compact }: { badge: Badge; prefix?: string; compact?: boolean }) {
  const s = styleFor(badge.state);
  return (
    <span
      title={tooltip(badge)}
      data-state={badge.state}
      style={{
        display: "inline-flex",
        alignItems: "center",
        gap: 5,
        padding: compact ? "2px 7px" : "4px 9px",
        borderRadius: 6,
        border: s.border,
        background: s.bg,
        color: s.color,
        fontFamily: "var(--mono)",
        fontSize: compact ? 11 : 12,
        lineHeight: 1.3,
        // El no-medido se ve "apagado" — no compite con las señales reales.
        opacity: badge.state === "unmeasured" ? 0.82 : 1,
        whiteSpace: "nowrap",
      }}
    >
      <span aria-hidden>{s.glyph}</span>
      {prefix && <span style={{ opacity: 0.7 }}>{prefix}</span>}
      <span>{badge.label}</span>
      {/* La cobertura PARCIAL de una dim MEDIDA: marca discreta (no rompe el verde, pero avisa). */}
      {badge.partialCoverage && (
        <span title="medición incompleta: alguna herramienta no se pudo correr" style={{ opacity: 0.7, fontStyle: "italic" }}>
          · parcial
        </span>
      )}
    </span>
  );
}

/**
 * El VEREDICTO GLOBAL de una variante/agente, con su tratamiento visual. NUNCA un verde tranquilizador
 * falso: "parcialmente medido" cuando falta una dimensión; "se detectaron problemas" cuando algo midió mal.
 */
export function GlobalVerdictBadge({
  kind, label,
}: {
  kind: "measured_issues" | "measured_ok" | "partial" | "unmeasured";
  label: string;
}) {
  const styles = {
    measured_issues: { color: "var(--clay, #b8543a)", border: "1px solid var(--clay, #b8543a)", glyph: "●", bg: "color-mix(in srgb, var(--clay, #b8543a) 10%, transparent)" },
    measured_ok: { color: "var(--accent)", border: "1px solid var(--accent)", glyph: "✓", bg: "color-mix(in srgb, var(--accent) 10%, transparent)" },
    partial: { color: "var(--warn, #9a6011)", border: "1px dashed var(--warn, #9a6011)", glyph: "◐", bg: "transparent" },
    unmeasured: { color: "var(--ink-dim, #6b6358)", border: "1px dashed var(--line, rgba(0,0,0,.3))", glyph: "⊘", bg: "transparent" },
  };
  // Fail-closed (audit deepseek 035#2): un `kind` inesperado (dato no tipado) degrada al estado SEGURO
  // `unmeasured` (gris punteado) — NUNCA un crash ni un verde falso. La unión TS ya lo cubre en compile.
  const map = styles[kind] ?? styles.unmeasured;
  return (
    <span
      data-verdict={kind}
      style={{
        display: "inline-flex",
        alignItems: "center",
        gap: 6,
        padding: "4px 10px",
        borderRadius: 6,
        border: map.border,
        background: map.bg,
        color: map.color,
        fontFamily: "var(--mono)",
        fontSize: 12,
        fontWeight: 600,
        letterSpacing: ".02em",
      }}
    >
      <span aria-hidden>{map.glyph}</span>
      <span>{label}</span>
    </span>
  );
}
