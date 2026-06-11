// 019 F2 — estilos compartidos del kit. Tokens V3 de `design/furx-theme.css` (dark+light vía CSS
// vars: el navegador resuelve según `:root`/`[data-theme]`). NUNCA colores hardcodeados como valor
// primario; los fallbacks existen sólo para SSR/tests sin la hoja cargada. Sin "honest/honesto".
import type { CSSProperties } from "react";

export const kitLbl: CSSProperties = {
  fontFamily: "var(--font-mono, monospace)",
  fontSize: 11,
  letterSpacing: ".06em",
  textTransform: "uppercase",
  color: "var(--ink-3, #635849)",
};

export const kitInput: CSSProperties = {
  width: "100%",
  background: "var(--bg-1, #faf7f0)",
  color: "var(--ink, #1c1814)",
  border: "1px solid var(--line, #d8d1bf)",
  borderRadius: "var(--radius, 3px)",
  padding: "7px 9px",
  fontFamily: "var(--font-sans, sans-serif)",
  fontSize: 14,
};

export function kitBtn(variant?: "accent" | "clay"): CSSProperties {
  const base: CSSProperties = {
    cursor: "pointer",
    padding: "5px 11px",
    fontSize: 13,
    borderRadius: "var(--radius, 3px)",
    border: "1px solid var(--line, #d8d1bf)",
    background: "var(--bg-1, #faf7f0)",
    color: "var(--ink, #1c1814)",
    fontFamily: "var(--font-sans, sans-serif)",
  };
  if (variant === "accent") return { ...base, background: "var(--accent)", color: "#fff", border: "none", fontWeight: 600 };
  if (variant === "clay") return { ...base, background: "var(--clay, #b8543a)", color: "#fff", border: "none", fontWeight: 600 };
  return base;
}

export const kitChip = (active: boolean): CSSProperties => ({
  cursor: "pointer",
  padding: "3px 9px",
  fontSize: 12,
  borderRadius: 999,
  fontFamily: "var(--font-sans, sans-serif)",
  border: `1px solid ${active ? "var(--accent)" : "var(--line, #d8d1bf)"}`,
  background: active ? "var(--accent-dim)" : "transparent",
  color: active ? "var(--accent-2)" : "var(--ink-2, #5a5246)",
});
