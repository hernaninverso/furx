// 019 F1 — superficie de diff/review UNIFICADA (la "pieza que faltaba" del informe): approve/reject
// por HUNK, cross-variante, con detección de conflictos y apply gobernado. Complementa a
// BestOfNCompare (que elige UNA variante entera): acá se cherry-pickean hunks de variantes distintas.
// Invoca review_open/review_hunk_decide/review_conflicts/review_apply (SSOT en Rust). Estética V3.
import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "../lib/invoke"; // 015 T015: invoke con flujo de aprobación universal (gate)

type HunkState = "pending" | "approved" | "rejected";
interface Hunk { id: string; file: string; header: string; state: HunkState }
interface VariantReview { task_id: string; change_set: { hunks: Hunk[] } }
interface GroupReview { group_id: string; revision: number; variants: VariantReview[] }
interface Conflict { file: string; hunk_a: string; hunk_b: string }

const lbl: React.CSSProperties = { fontFamily: "var(--mono)", fontSize: 11, letterSpacing: ".06em", textTransform: "uppercase", color: "var(--ink-dim, #6b6358)" };
const btn = (bg?: string): React.CSSProperties => ({ cursor: "pointer", padding: "4px 10px", fontSize: 12, borderRadius: 6, border: bg ? "none" : "1px solid var(--line, rgba(0,0,0,.15))", background: bg ?? "var(--bg, #faf7f0)", color: bg ? "#fff" : "var(--ink, #1c1814)", fontFamily: "var(--body)", fontWeight: bg ? 600 : 400 });

export function DiffReview({
  groupId, onClose, onToast,
}: {
  groupId: string;
  onClose: () => void;
  onToast: (kind: "success" | "error" | "info", msg: string) => void;
}) {
  const [review, setReview] = useState<GroupReview | null>(null);
  const [conflicts, setConflicts] = useState<Conflict[]>([]);
  const [busy, setBusy] = useState(false);

  const refreshConflicts = useCallback(async () => {
    try { setConflicts(await invoke<Conflict[]>("review_conflicts", { groupId })); }
    catch { /* no-op: conflicts es informativo */ }
  }, [groupId]);

  const open = useCallback(async () => {
    setBusy(true);
    try {
      const r = await invoke<GroupReview>("review_open", { groupId });
      setReview(r);
      await refreshConflicts();
    } catch (e) {
      onToast("error", `No se pudo abrir la review: ${String(e)}`);
    } finally { setBusy(false); }
  }, [groupId, onToast, refreshConflicts]);

  useEffect(() => { open(); }, [open]);

  // Decide un hunk; el backend versiona (rechaza si la revisión cambió) y devuelve la nueva.
  const decide = async (hunk: Hunk, decision: HunkState) => {
    if (!review) return;
    setBusy(true);
    try {
      const newRev = await invoke<number>("review_hunk_decide", {
        groupId, hunkId: hunk.id, decision, expectedRevision: review.revision,
      });
      // optimista local + revisión nueva.
      setReview((prev) => prev && ({
        ...prev,
        revision: newRev,
        variants: prev.variants.map((v) => ({
          ...v,
          change_set: { hunks: v.change_set.hunks.map((h) => h.id === hunk.id ? { ...h, state: decision } : h) },
        })),
      }));
      await refreshConflicts();
    } catch (e) {
      // típico: revisión stale (otra ventana decidió) → recargar.
      onToast("error", `Decisión rechazada (¿la review cambió?): ${String(e)}`);
      await open();
    } finally { setBusy(false); }
  };

  const apply = async () => {
    if (!review) return;
    // guard defensivo (audit codex): además del botón deshabilitado, no aplicar sin aprobados ni con conflictos.
    if (review.variants.every((v) => v.change_set.hunks.every((h) => h.state !== "approved"))) {
      onToast("info", "Aprobá al menos un hunk."); return;
    }
    if (conflicts.length > 0) { onToast("error", "Resolvé los conflictos antes de aplicar."); return; }
    setBusy(true);
    try {
      const res = await invoke<{ applied: boolean; approved_hunks?: number; reason?: string }>(
        "review_apply", { groupId, expectedRevision: review.revision });
      if (res.applied) {
        onToast("success", `Aplicados ${res.approved_hunks ?? 0} hunk(s) al working copy.`);
        onClose(); // apply es terminal: cerrar evita re-aplicar lo ya escrito (nit audit codex).
      } else {
        onToast("info", `Nada aplicado: ${res.reason ?? "sin hunks aprobados"}.`);
      }
    } catch (e) {
      onToast("error", `Apply falló: ${String(e)}`);
    } finally { setBusy(false); }
  };

  const approvedCount = useMemo(
    () => review?.variants.reduce((n, v) => n + v.change_set.hunks.filter((h) => h.state === "approved").length, 0) ?? 0,
    [review],
  );
  const conflictIds = useMemo(() => new Set(conflicts.flatMap((c) => [c.hunk_a, c.hunk_b])), [conflicts]);

  const stateColor = (s: HunkState) =>
    s === "approved" ? "var(--accent)" : s === "rejected" ? "var(--clay, #b8543a)" : "var(--ink-dim, #6b6358)";

  return (
    <div role="dialog" aria-label="Review por hunk (best-of-N)" onClick={onClose}
      style={{ position: "fixed", inset: 0, background: "rgba(0,0,0,.5)", zIndex: 430, display: "flex", alignItems: "center", justifyContent: "center" }}>
      <div onClick={(e) => e.stopPropagation()}
        style={{ width: "min(1200px,96vw)", maxHeight: "92vh", overflowY: "auto", padding: 22,
                 background: "var(--bg, #f3efe6)", color: "var(--ink, #1c1814)", border: "1px solid var(--line, rgba(0,0,0,.18))", borderRadius: 10, boxShadow: "0 20px 60px -20px rgba(0,0,0,.5)" }}>
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 8 }}>
          <div>
            <div style={{ fontFamily: "var(--display, serif)", fontSize: 21, fontWeight: 600 }}>Review por hunk · unificada</div>
            <div style={{ ...lbl, marginTop: 2 }}>
              {approvedCount} aprobado(s) · rev {review?.revision ?? "—"}
              {conflicts.length > 0 && <span style={{ color: "var(--clay, #b8543a)", marginLeft: 8 }}>⚠ {conflicts.length} conflicto(s)</span>}
            </div>
          </div>
          <button onClick={onClose} style={btn()}>×</button>
        </div>
        <p style={{ fontSize: 13, color: "var(--ink-dim, #6b6358)", marginTop: 0 }}>
          Aprobá o rechazá cada hunk de cualquier variante. Los conflictos (hunks aprobados que se
          pisan) se resuelven a mano — no hay merge automático. Aplicar escribe sólo lo aprobado al
          working copy (con tu confirmación).
        </p>

        {!review && <div style={{ ...lbl }}>{busy ? "Abriendo review…" : "Sin review."}</div>}

        {review?.variants.map((v) => (
          <div key={v.task_id} style={{ marginBottom: 16, border: "1px solid var(--line, rgba(0,0,0,.14))", borderRadius: 8, overflow: "hidden" }}>
            <div style={{ ...lbl, padding: "6px 10px", background: "var(--card, #fbf9f4)", borderBottom: "1px solid var(--line)" }}>
              variante {v.task_id.slice(0, 8)} · {v.change_set.hunks.length} hunk(s)
            </div>
            {v.change_set.hunks.length === 0 && <div style={{ padding: 10, ...lbl }}>sin cambios</div>}
            {v.change_set.hunks.map((h) => {
              const inConflict = conflictIds.has(h.id);
              return (
                <div key={h.id} style={{ display: "flex", alignItems: "center", gap: 10, padding: "7px 10px", borderBottom: "1px solid var(--line, rgba(0,0,0,.07))", background: inConflict ? "rgba(184,84,58,.06)" : undefined }}>
                  <span style={{ flex: "0 0 auto", width: 8, height: 8, borderRadius: 8, background: stateColor(h.state) }} title={h.state} />
                  <div style={{ flex: 1, minWidth: 0 }}>
                    <div style={{ fontFamily: "var(--mono)", fontSize: 12, whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>{h.file}</div>
                    <div style={{ ...lbl, textTransform: "none" }}>{h.header}{inConflict && <span style={{ color: "var(--clay, #b8543a)" }}> · conflicto</span>}</div>
                  </div>
                  <button style={btn(h.state === "approved" ? "var(--accent)" : undefined)} disabled={busy} onClick={() => decide(h, h.state === "approved" ? "pending" : "approved")}>✓</button>
                  <button style={btn(h.state === "rejected" ? "var(--clay, #b8543a)" : undefined)} disabled={busy} onClick={() => decide(h, h.state === "rejected" ? "pending" : "rejected")}>✕</button>
                </div>
              );
            })}
          </div>
        ))}

        {review && (
          <div style={{ display: "flex", gap: 8, marginTop: 12, justifyContent: "flex-end", alignItems: "center" }}>
            <button style={btn()} disabled={busy} onClick={open}>Refrescar</button>
            <button style={btn("var(--accent)")} disabled={busy || approvedCount === 0 || conflicts.length > 0} onClick={apply}
              title={conflicts.length > 0 ? "Resolvé los conflictos primero" : approvedCount === 0 ? "Aprobá al menos un hunk" : "Aplicar los hunks aprobados"}>
              Aplicar {approvedCount > 0 ? `(${approvedCount})` : ""}
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
