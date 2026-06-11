// 026-preference-loop F1/US3 — inspección + reset del prior local explicable.
//
// Gobierno total y explicable (FR-030/FR-031): el usuario ve QUÉ aprendió el prior de sus elecciones
// previas (features con su peso/dirección y cuántas muestras lo respaldan) y puede RESETEARLO (vuelve
// a cold-start, auditado). 100% local/determinista: ningún modelo, ninguna API key. SIEMPRE advisory:
// este panel NO altera el ranking ni elige por el usuario — sólo muestra y resetea lo aprendido.
import { useCallback, useEffect, useState } from "react";
import { invoke } from "../lib/invoke";
import type { PriorView } from "../types";
import { prettyFeature } from "./BestOfNCompare";

export function PreferencePriorPanel({
  repoPath, onClose, onToast,
}: {
  repoPath: string | null;
  onClose: () => void;
  onToast: (kind: "success" | "error" | "info", msg: string) => void;
}) {
  const [view, setView] = useState<PriorView | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    if (!repoPath) { setLoading(false); return; }
    setLoading(true);
    try {
      const v = await invoke<PriorView>("preference_prior_inspect", { repoPath, taskType: null });
      setView(v);
    } catch (e) {
      onToast("error", `No se pudo inspeccionar el prior: ${String(e)}`);
    } finally {
      setLoading(false);
    }
  }, [repoPath, onToast]);

  useEffect(() => { load(); }, [load]);

  // Reset del prior de ESTE contexto (repo + task_type del view). Auditado backend; vuelve a cold-start.
  const reset = async () => {
    if (!repoPath) return;
    if (!window.confirm("Resetear lo aprendido de tus elecciones en este repo? Vuelve a empezar de cero (tus registros de decisiones NO se borran).")) return;
    setBusy(true);
    try {
      const n = await invoke<number>("preference_prior_reset", { repoPath, taskType: view?.task_type ?? null });
      onToast("success", `Prior reseteado (${n} fila(s) borradas). Vuelve a aprender desde cero.`);
      await load();
    } catch (e) {
      onToast("error", String(e));
    } finally {
      setBusy(false);
    }
  };

  const lbl: React.CSSProperties = { fontFamily: "var(--mono)", fontSize: 11, letterSpacing: ".06em", textTransform: "uppercase", color: "var(--ink-dim, #6b6358)" };
  const btn = (bg?: string): React.CSSProperties => ({ cursor: "pointer", padding: "5px 11px", fontSize: 13, borderRadius: 6, border: bg ? "none" : "1px solid var(--line, rgba(0,0,0,.15))", background: bg ?? "var(--bg, #faf7f0)", color: bg ? "#fff" : "var(--ink, #1c1814)", fontFamily: "var(--body)", ...(bg ? { fontWeight: 600 } : {}) });

  return (
    <div role="dialog" aria-label="Tus preferencias aprendidas" onClick={onClose}
      style={{ position: "fixed", inset: 0, background: "rgba(0,0,0,.5)", zIndex: 450, display: "flex", alignItems: "center", justifyContent: "center" }}>
      <div onClick={(e) => e.stopPropagation()}
        style={{ width: "min(680px,94vw)", maxHeight: "88vh", overflowY: "auto", padding: 20,
                 background: "var(--bg, #f3efe6)", color: "var(--ink, #1c1814)", border: "1px solid var(--line, rgba(0,0,0,.18))", borderRadius: 10, boxShadow: "0 20px 60px -20px rgba(0,0,0,.5)" }}>
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 8 }}>
          <div style={{ fontFamily: "var(--display, serif)", fontSize: 19, fontWeight: 600 }}>Tus preferencias · prior local</div>
          <button onClick={onClose} style={btn()}>×</button>
        </div>
        <p style={{ ...lbl, marginTop: 0 }}>local · explicable · advisory · sin red · reseteable</p>

        {loading && <div style={{ fontSize: 13, color: "var(--ink-dim, #6b6358)" }}>Cargando…</div>}

        {!loading && !repoPath && (
          <div style={{ fontSize: 13, color: "var(--ink-dim, #6b6358)" }}>Sin repo de contexto para inspeccionar.</div>
        )}

        {!loading && view && (
          <>
            <div style={{ display: "flex", gap: 14, flexWrap: "wrap", marginBottom: 12, fontSize: 13 }}>
              <span><strong>{view.sample_count}</strong> decisión(es) registrada(s)</span>
              <span>·</span>
              <span style={{ color: view.is_warm ? "var(--accent)" : "#b8862a" }}>
                {view.is_warm ? "✓ aprendiendo activo (caliente)" : "○ aún aprendiendo (cold-start, ≥15 para influir)"}
              </span>
            </div>

            {view.features.length === 0 && (
              <div style={{ fontSize: 13, color: "var(--ink-dim, #6b6358)" }}>
                Todavía no hay preferencias aprendidas en este repo. Compará y elegí variantes para que el prior empiece a aprender.
              </div>
            )}

            {view.features.length > 0 && (
              <table style={{ width: "100%", borderCollapse: "collapse", fontSize: 13 }}>
                <thead>
                  <tr style={{ textAlign: "left" }}>
                    <th style={{ ...lbl, padding: "4px 6px" }}>feature</th>
                    <th style={{ ...lbl, padding: "4px 6px" }}>preferencia</th>
                    <th style={{ ...lbl, padding: "4px 6px", textAlign: "right" }}>peso</th>
                    <th style={{ ...lbl, padding: "4px 6px", textAlign: "right" }}>evidencia</th>
                  </tr>
                </thead>
                <tbody>
                  {[...view.features].sort((a, b) => Math.abs(b.weight) - Math.abs(a.weight)).map((f) => {
                    const color = f.direction === "neutro" ? "var(--ink-dim, #6b6358)" : "var(--accent)";
                    return (
                      <tr key={f.feature_key} style={{ borderTop: "1px solid var(--line, rgba(0,0,0,.1))" }}>
                        <td style={{ padding: "5px 6px" }}>{prettyFeature(f.feature_key)}</td>
                        <td style={{ padding: "5px 6px", color }}>{f.direction}</td>
                        <td style={{ padding: "5px 6px", textAlign: "right", fontFamily: "var(--mono)" }}>{f.weight.toFixed(2)}</td>
                        <td style={{ padding: "5px 6px", textAlign: "right", fontFamily: "var(--mono)", color: "var(--ink-dim, #6b6358)" }}>
                          α{f.alpha.toFixed(1)}/β{f.beta.toFixed(1)}
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            )}

            <div style={{ display: "flex", gap: 8, marginTop: 16, justifyContent: "flex-end" }}>
              <button style={btn()} disabled={busy} onClick={load}>Refrescar</button>
              <button style={btn("var(--clay, #b8543a)")} disabled={busy || view.sample_count === 0} onClick={reset}>
                Resetear lo aprendido
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
