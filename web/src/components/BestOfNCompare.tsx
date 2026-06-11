// 014-orchestration-ux FR-001 — comparación N-way de las variantes de un grupo best-of-N.
// Muestra los N diffs lado a lado (reusa orchestration_compare_group, que reusa worktree_merge_review),
// permite ELEGIR una para mergear y DESCARTAR el resto (con confirmación — constitución VI). Estética V3.
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "../lib/invoke"; // 015 T015: invoke con flujo de aprobación universal
import type { OrchTask, OrchTaskGroup, OrchVariantDiff, VariantEvidence, RankingExplanation, PaneUsage } from "../types";
// 035-ai-visibility-evidence — veredicto verificable por variante (evidencia + costo) con badge 3-estados.
import { evidenceDimension, tokenCostDimension, globalVerdict } from "../lib/aiVisibility";
import { MeasurementBadge, GlobalVerdictBadge } from "./MeasurementBadge";
// 026-preference-loop — inspección/reset del prior local explicable (US3).
import { PreferencePriorPanel } from "./PreferencePriorPanel";
import { DiffReview } from "./DiffReview"; // 019 F1 — review hunk-level unificada (cherry-pick cross-variante)
// 020 meta-orchestrator US2 — ranking advisory del AIE (parseo/validación puro, testeado).
import { parseRankingSuggestion, variantLabel, variantsContentKey, type RankingSuggestion } from "../lib/metaSuggest";
// 024-quality-gate F1 — evidencia objetiva por variante (helper puro de presentación, testeado).
import { evidenceBadge, evidenceByTask, toolCellText, statusLabel } from "../lib/qualityGate";

export function BestOfNCompare({
  groupId, onClose, onReview, onToast,
}: {
  groupId: string;
  onClose: () => void;
  onReview: (task: OrchTask) => void;
  onToast: (kind: "success" | "error" | "info", msg: string) => void;
}) {
  const [group, setGroup] = useState<OrchTaskGroup | null>(null);
  const [variants, setVariants] = useState<OrchVariantDiff[]>([]);
  const [busy, setBusy] = useState(false);
  // 040 FR-008 — guard SÍNCRONO contra doble-click: `setBusy(true)` schedula un render (async),
  // así dos clicks muy rápidos pueden pasar AMBOS el `disabled={busy}` antes del re-render. Un
  // `useRef` se lee/escribe síncrono y cierra esa ventana. Aplica a choose() y discardOthers().
  const busyRef = useRef(false);
  const [hunkReview, setHunkReview] = useState(false); // 019 F1 — abre la review por hunk
  // 020 US2 — sugerencia de ranking del AIE (advisory). null = OFF/sin diffs/AIE caído → no se muestra.
  // Guardamos junto la clave de contenido a la que corresponde la sugerencia: si el estado actual
  // (diffs/count) difiere, NO la mostramos (codex/deepseek HIGH 1: la vieja se descarta).
  const [ranking, setRanking] = useState<{ key: string; sugg: RankingSuggestion } | null>(null);
  // 026 F1 (US2) — ranking ENRIQUECIDO con el prior local explicable (advisory). null = sin prior
  // aplicado (inject OFF / cold-start / sin features). Trae la EXPLICACIÓN por variante (FR-023).
  const [explained, setExplained] = useState<{ key: string; expl: RankingExplanation } | null>(null);
  const [priorPanel, setPriorPanel] = useState(false); // 026 US3 — inspector del prior
  // 024-quality-gate F1 — evidencia objetiva por variante (advisory). null/[] = nunca corrida o gate OFF.
  const [evidence, setEvidence] = useState<VariantEvidence[]>([]);
  const [qgBusy, setQgBusy] = useState(false);
  const [qgDetail, setQgDetail] = useState<VariantEvidence | null>(null); // detalle clickeable (read-only).
  const evByTask = useMemo(() => evidenceByTask(evidence), [evidence]);
  // 035 — uso de tokens medido (Claude) por repo_path de variante. null/ausente = "no medido" (badge gris).
  const [usageByTask, setUsageByTask] = useState<Record<string, PaneUsage | null>>({});

  // Clave estable derivada del CONTENIDO de las variantes (no sólo del count). Un refresh de diffs
  // con el mismo count cambia esta clave → re-dispara el efecto que pide la sugerencia.
  const diffsKey = variantsContentKey(variants);

  const reload = useCallback(async () => {
    try {
      const [g, v] = await Promise.all([
        invoke<OrchTaskGroup | null>("orchestration_get_group", { groupId }),
        invoke<OrchVariantDiff[]>("orchestration_compare_group", { groupId }),
      ]);
      setGroup(g); setVariants(v);
    } catch (e) { onToast("error", `No se pudo cargar la comparación: ${String(e)}`); }
  }, [groupId, onToast]);

  useEffect(() => { reload(); }, [reload]);

  // 024 F1 — al abrir, traer la evidencia ya calculada (si la hubo) SIN re-ejecutar (read-only).
  useEffect(() => {
    let off = false;
    invoke<VariantEvidence[]>("quality_gate_get", { groupId })
      .then((e) => { if (!off) setEvidence(e ?? []); })
      .catch(() => { /* advisory: sin evidencia previa, se ignora */ });
    return () => { off = true; };
  }, [groupId]);

  // 035 — uso de tokens (Claude) por variante: read-only, sin red, sin comando nuevo
  // (`claude_usage_for_cwd` ya existe). Ausente/null → la dimensión costo cae a "no medido" (badge gris).
  useEffect(() => {
    let off = false;
    (async () => {
      const next: Record<string, PaneUsage | null> = {};
      await Promise.all(variants.map(async (v) => {
        try { next[v.task_id] = await invoke<PaneUsage | null>("claude_usage_for_cwd", { cwd: v.repo_path }); }
        catch { next[v.task_id] = null; }
      }));
      if (!off) setUsageByTask(next);
    })();
    return () => { off = true; };
  }, [variants]);

  // 024 F1 — correr el quality-gate ON-DEMAND (gateado por `qualitygate.enabled`, default OFF).
  // El comando ejecuta los linters del repo sobre cada variante (sandbox + timeout). Advisory.
  const runQualityGate = async () => {
    setQgBusy(true);
    try {
      const ev = await invoke<VariantEvidence[]>("quality_gate_run", { groupId });
      setEvidence(ev ?? []);
      const measured = (ev ?? []).filter((e) => e.any_measured).length;
      onToast("success", measured > 0
        ? `Quality-gate: ${measured}/${(ev ?? []).length} variante(s) medida(s).`
        : "Quality-gate: ningún linter detectable midió (revisá que estén instalados).");
    } catch (e) {
      onToast("error", String(e));
    } finally {
      setQgBusy(false);
    }
  };

  // 020 US2 — cargar el ranking sugerido NO-bloqueante: la UI ya renderizó las variantes; esto
  // sólo agrega un hint cuando el AIE responde. El comando es read-only/advisory (gate por el
  // setting `orchestration.use_aie_for_meta`, default OFF ⇒ None). Cualquier error se traga.
  //
  // codex/deepseek HIGH 1: la dep es `diffsKey` (contenido de las variantes), NO `variants.length`.
  // Así un refresh de diffs con el mismo count re-pide la sugerencia y nunca queda stale. Cualquier
  // respuesta que llegue para una clave ya cambiada se descarta (flag `off` + se valida la clave
  // contra la actual al guardar/mostrar). La sugerencia se valida contra el count ACTUAL.
  useEffect(() => {
    if (variants.length === 0) { setRanking(null); return; }
    let off = false;
    const reqKey = diffsKey;
    const reqCount = variants.length;
    invoke<number[] | null>("meta_suggest_variant_ranking", { groupId })
      .then((r) => {
        if (off) return; // llegó una respuesta vieja (groupId/diffs ya cambiaron) → descartar
        const sugg = parseRankingSuggestion(r, reqCount);
        setRanking(sugg ? { key: reqKey, sugg } : null);
      })
      .catch(() => { if (!off) setRanking(null); /* advisory: no rompe la UI */ });
    return () => { off = true; }; // cleanup: no setState tras unmount ni con respuesta stale
  }, [groupId, diffsKey, variants.length]);

  // 026 F1 (US2) — ranking enriquecido con el prior local (advisory). NO-bloqueante: agrega la
  // EXPLICACIÓN de por qué se sugiere cada variante cuando el prior aporta (inject ON + caliente).
  // Si inject OFF / cold-start / sin base → el comando degrada y `inject_disabled`/`still_learning`
  // lo marcan → no mostramos factores (cero ruido). Cualquier error se traga (advisory).
  useEffect(() => {
    if (variants.length === 0) { setExplained(null); return; }
    let off = false;
    const reqKey = diffsKey;
    invoke<RankingExplanation | null>("meta_suggest_variant_ranking_explained", { groupId })
      .then((expl) => {
        if (off) return;
        // sólo guardamos si el prior REALMENTE se inyectó (hay al menos un factor) — sino no aporta.
        const contributed = !!expl && !expl.inject_disabled && !expl.still_learning
          && expl.variants.some((v) => v.factors.length > 0);
        setExplained(contributed ? { key: reqKey, expl: expl! } : null);
      })
      .catch(() => { if (!off) setExplained(null); });
    return () => { off = true; };
  }, [groupId, diffsKey, variants.length]);

  const choose = async (v: OrchVariantDiff) => {
    if (busyRef.current) return; // 040 FR-008 — cierra la ventana entre click y render
    busyRef.current = true;
    setBusy(true);
    try {
      await invoke("orchestration_choose_variant", { groupId, taskId: v.task_id });
      onToast("success", `Variante "${v.title}" elegida para mergear.`);
      await reload();
    } catch (e) { onToast("error", String(e)); }
    finally { busyRef.current = false; setBusy(false); }
  };

  // Descartar las NO-elegidas, una por una con confirmación (constitución VI: no destructivo silencioso).
  const discardOthers = async () => {
    if (busyRef.current) return; // 040 FR-008 — guard síncrono (idéntico a choose())
    if (!group?.chosen_task_id) { onToast("error", "Elegí una variante antes de descartar el resto."); return; }
    const others = variants.filter((v) => v.task_id !== group.chosen_task_id
      && !["done", "failed", "canceled"].includes(v.state));
    if (others.length === 0) { onToast("info", "No hay variantes que descartar."); return; }
    if (!window.confirm(`Descartar ${others.length} variante(s) no elegida(s)? Se cancelan (su worktree no se borra acá).`)) return;
    // Tomamos el guard sólo cuando va a haber trabajo async (tras pasar las validaciones/confirm),
    // así un early-return de validación NO deja el ref trabado.
    busyRef.current = true;
    setBusy(true);
    try {
      let done = 0;
      for (const v of others) {
        try { if (await invoke<boolean>("orchestration_discard_variant", { groupId, taskId: v.task_id })) done++; }
        catch (e) { onToast("error", `No se pudo descartar "${v.title}": ${String(e)}`); }
      }
      onToast("success", `${done} variante(s) descartada(s).`);
      await reload();
    } finally { busyRef.current = false; setBusy(false); }
  };

  const chosen = group?.chosen_task_id;
  // Sólo usamos la sugerencia si corresponde al estado/diffs ACTUAL (HIGH 1: la stale se descarta).
  // Además re-validamos contra el count actual: nunca indexamos variants[bestIndex] fuera de rango.
  const liveRanking: RankingSuggestion | null =
    ranking && ranking.key === diffsKey
      && ranking.sugg.bestIndex >= 0 && ranking.sugg.bestIndex < variants.length
      ? ranking.sugg
      : null;
  // 026 F1 — explicación del prior por task_id (sólo si se inyectó para el estado/diffs ACTUAL).
  const liveExplained: RankingExplanation | null =
    explained && explained.key === diffsKey ? explained.expl : null;
  const explByTask = useMemo(() => {
    const m = new Map<string, RankingExplanation["variants"][number]>();
    if (liveExplained) for (const v of liveExplained.variants) m.set(v.task_id, v);
    return m;
  }, [liveExplained]);
  const lbl: React.CSSProperties = { fontFamily: "var(--mono)", fontSize: 11, letterSpacing: ".06em", textTransform: "uppercase", color: "var(--ink-dim, #6b6358)" };
  const btn = (bg?: string): React.CSSProperties => ({ cursor: "pointer", padding: "5px 11px", fontSize: 13, borderRadius: 6, border: "1px solid var(--line, rgba(0,0,0,.15))", background: bg ?? "var(--bg, #faf7f0)", color: bg ? "#fff" : "var(--ink, #1c1814)", fontFamily: "var(--body)", ...(bg ? { border: "none", fontWeight: 600 } : {}) });

  return (
    <div role="dialog" aria-label="Comparar variantes (best-of-N)" onClick={onClose}
      style={{ position: "fixed", inset: 0, background: "rgba(0,0,0,.5)", zIndex: 420, display: "flex", alignItems: "center", justifyContent: "center" }}>
      <div onClick={(e) => e.stopPropagation()}
        style={{ width: "min(1100px,96vw)", maxHeight: "92vh", overflowY: "auto", padding: 22,
                 background: "var(--bg, #f3efe6)", color: "var(--ink, #1c1814)", border: "1px solid var(--line, rgba(0,0,0,.18))", borderRadius: 10, boxShadow: "0 20px 60px -20px rgba(0,0,0,.5)" }}>
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 8 }}>
          <div>
            <div style={{ fontFamily: "var(--display, serif)", fontSize: 21, fontWeight: 600 }}>Comparar variantes · best-of-{group?.n ?? variants.length}</div>
            {group?.objective && <div style={{ ...lbl, marginTop: 2 }}>{group.objective}</div>}
          </div>
          <button onClick={onClose} style={btn()}>×</button>
        </div>
        <p style={{ fontSize: 13, color: "var(--ink-dim, #6b6358)", marginTop: 0 }}>
          Los N diffs lado a lado. Elegí 1 para mergear; el resto se descarta con tu confirmación. El merge es siempre con tu OK.
        </p>

        {/* 020 US2 — sugerencia de ranking del AIE (advisory; el picker manual sigue mandando). */}
        {liveRanking && (
          <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 12, padding: "7px 11px",
                        background: "var(--accent-glow)", border: "1px solid var(--accent)", borderRadius: 8, fontSize: 13 }}>
            <span aria-hidden>✨</span>
            <span>AIE sugiere: <strong>{liveRanking.summary}</strong></span>
            {liveExplained && <span style={{ ...lbl }}>+ tus elecciones previas</span>}
            <span style={{ ...lbl, marginLeft: "auto" }}>sugerencia · vos elegís</span>
          </div>
        )}

        <div style={{ display: "grid", gridTemplateColumns: `repeat(${Math.min(variants.length || 1, 3)}, 1fr)`, gap: 12 }}>
          {variants.map((v, i) => {
            const isChosen = v.task_id === chosen;
            const isSuggested = liveRanking?.bestIndex === i; // posición en el orden de variantes (variant_index ASC)
            return (
              <div key={v.task_id}
                style={{ border: `2px solid ${isChosen ? "var(--accent)" : "var(--line, rgba(0,0,0,.14))"}`, borderRadius: 8, padding: 12, display: "flex", flexDirection: "column", gap: 8, background: isChosen ? "rgba(255,92,53,.05)" : "var(--card, #fbf9f4)" }}>
                <div style={{ display: "flex", justifyContent: "space-between", alignItems: "baseline", gap: 6 }}>
                  <div style={{ fontWeight: 600, fontSize: 14 }}>
                    {variantLabel(v.variant_index ?? i)}
                    {isChosen && <span style={{ ...lbl, color: "var(--accent)", marginLeft: 6 }}>✓ elegida</span>}
                    {isSuggested && !isChosen && <span style={{ ...lbl, color: "var(--accent)", marginLeft: 6 }} title="Sugerida por el AIE (advisory)">✨ sugerida</span>}
                  </div>
                  <span style={{ ...lbl }}>{v.state}</span>
                </div>
                <div style={{ ...lbl, whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }} title={v.branch}>{v.branch}</div>
                <pre style={{ background: "var(--bg, #faf7f0)", border: "1px solid var(--line)", borderRadius: 6, padding: 8, fontSize: 11, fontFamily: "var(--mono)", overflowX: "auto", margin: 0, maxHeight: 220 }}>{v.diff_stat || "(sin cambios)"}</pre>
                {v.risky_paths.length > 0 && (
                  <div style={{ fontSize: 11, color: "var(--clay, #b8543a)" }}>⚠ {v.risky_paths.length} path(s) riesgoso(s): {v.risky_paths.slice(0, 3).join(", ")}</div>
                )}
                {/* 024 F1 — evidencia objetiva (advisory): errores rojo, warnings ámbar, "no disponible" gris. */}
                {(() => {
                  const badge = evidenceBadge(evByTask.get(v.task_id));
                  if (!badge) return null;
                  const color =
                    badge.kind === "issues" || badge.errors > 0 ? "var(--clay, #b8543a)"
                    : badge.kind === "unavailable" ? "var(--ink-dim, #6b6358)"
                    : badge.warnings > 0 ? "#b8862a"
                    : "var(--accent)";
                  return (
                    <button
                      type="button"
                      onClick={() => setQgDetail(evByTask.get(v.task_id) ?? null)}
                      title="Ver detalle de la evidencia (por herramienta) — advisory, no altera la review"
                      style={{ display: "flex", alignItems: "center", gap: 6, padding: "4px 8px", borderRadius: 6,
                               border: "1px solid var(--line, rgba(0,0,0,.14))", background: "var(--bg, #faf7f0)",
                               color, cursor: "pointer", fontSize: 12, fontFamily: "var(--mono)", textAlign: "left" }}>
                      <span aria-hidden>{badge.kind === "unavailable" ? "○" : badge.errors > 0 ? "●" : badge.warnings > 0 ? "◐" : "✓"}</span>
                      <span>{badge.label}</span>
                      {badge.kind === "partial" && badge.unavailableTools.length > 0 && (
                        <span style={{ color: "var(--ink-dim, #6b6358)" }}>· +{badge.unavailableTools.length} n/d</span>
                      )}
                    </button>
                  );
                })()}
                {/* 035 — VEREDICTO VERIFICABLE por variante (Golpe 1 evidencia + Golpe 3 tokens), con
                    badge de 3 estados. El veredicto global dice "parcialmente medido" si falta una
                    dimensión — NUNCA un verde tranquilizador falso. Los pasos vivos del timeline NO
                    entran acá (ortogonalidad FR-012): sólo evidencia (linters) y costo (tokens). */}
                {(() => {
                  const evDim = evidenceDimension(evByTask.get(v.task_id));
                  const costDim = tokenCostDimension(usageByTask[v.task_id]);
                  const verdict = globalVerdict([evDim, costDim]);
                  return (
                    <div style={{ display: "flex", flexDirection: "column", gap: 6, borderTop: "1px dashed var(--line, rgba(0,0,0,.14))", paddingTop: 8 }}>
                      <GlobalVerdictBadge kind={verdict.kind} label={verdict.label} />
                      <div style={{ display: "flex", flexWrap: "wrap", gap: 6 }}>
                        <MeasurementBadge badge={evDim} prefix="evidencia" compact />
                        <MeasurementBadge badge={costDim} prefix="tokens" compact />
                      </div>
                    </div>
                  );
                })()}
                {/* 026 F1 — POR QUÉ el prior favorece (o no) esta variante. Explicable, NUNCA opaco:
                    lista los factores aprendidos (feature + dirección) que contribuyen. Sólo aparece
                    cuando el prior se inyectó (inject ON + caliente) — default OFF ⇒ no se ve nada. */}
                {(() => {
                  const ex = explByTask.get(v.task_id);
                  if (!ex || ex.factors.length === 0) return null;
                  const top = [...ex.factors].sort((a, b) => Math.abs(b.contribution) - Math.abs(a.contribution)).slice(0, 3);
                  return (
                    <div style={{ fontSize: 11, fontFamily: "var(--mono)", color: "var(--ink-dim, #6b6358)", lineHeight: 1.45 }}
                         title="Sugerencia por tus elecciones previas en este repo (advisory)">
                      <span style={{ color: "var(--accent)" }}>✨ tus preferencias:</span>{" "}
                      {top.map((f) => `${prettyFeature(f.feature_key)} (${f.direction})`).join(" · ")}
                    </div>
                  );
                })()}
                <div style={{ display: "flex", gap: 6, marginTop: "auto" }}>
                  <button style={btn(isChosen ? undefined : "var(--accent)")} disabled={busy} onClick={() => choose(v)}>
                    {isChosen ? "Elegida" : "Elegir esta"}
                  </button>
                  {v.state === "awaiting_review" && (
                    <button style={btn()} disabled={busy} onClick={() => onReview({ id: v.task_id, repo_path: v.repo_path, branch: v.branch } as OrchTask)}>Revisar/merge</button>
                  )}
                </div>
              </div>
            );
          })}
        </div>

        {variants.length > 0 && (
          <div style={{ display: "flex", gap: 8, marginTop: 16, justifyContent: "flex-end", flexWrap: "wrap" }}>
            {/* 024 F1 — correr los linters del repo sobre las variantes (gate `qualitygate.enabled`, OFF). */}
            <button style={btn()} disabled={busy || qgBusy} onClick={runQualityGate}
              title="Corre los linters/typecheck de tu repo sobre cada variante (sandbox, sin red). Advisory. Activá «qualitygate.enabled» en Ajustes.">
              {qgBusy ? "Corriendo quality-gate…" : "Correr quality-gate"}
            </button>
            <button style={btn()} disabled={busy} onClick={reload}>Refrescar diffs</button>
            {/* 026 US3 — inspeccionar/resetear lo que el prior aprendió de tus elecciones (gobierno). */}
            <button style={btn()} disabled={busy} onClick={() => setPriorPanel(true)}
              title="Ver qué features preferís según tus elecciones previas y resetearlo. Local, advisory.">Tus preferencias…</button>
            {/* 019 F1 — en vez de elegir UNA variante entera, cherry-pickear hunks de varias. */}
            <button style={btn()} disabled={busy} onClick={() => setHunkReview(true)}>Review por hunk…</button>
            <button style={btn("var(--clay, #b8543a)")} disabled={busy || !chosen} onClick={discardOthers}>Descartar no-elegidas</button>
          </div>
        )}
      </div>
      {hunkReview && <DiffReview groupId={groupId} onClose={() => setHunkReview(false)} onToast={onToast} />}
      {qgDetail && <QualityGateDetail evidence={qgDetail} onClose={() => setQgDetail(null)} lbl={lbl} btn={btn} />}
      {priorPanel && (
        <PreferencePriorPanel
          repoPath={variants.find((v) => v.repo_path)?.repo_path ?? null}
          onClose={() => setPriorPanel(false)}
          onToast={onToast}
        />
      )}
    </div>
  );
}

// 026 F1 — nombre legible de un feature_key del prior (espejo de variant_features.rs).
export function prettyFeature(key: string): string {
  switch (key) {
    case "diff_added": return "líneas agregadas";
    case "diff_removed": return "líneas eliminadas";
    case "diff_total": return "tamaño del cambio";
    case "files_touched": return "archivos tocados";
    case "risky_paths": return "rutas sensibles";
    case "qg_errors": return "errores de lint";
    case "qg_warnings": return "warnings de lint";
    default: return key;
  }
}

// 024-quality-gate F1 — detalle clickeable de la evidencia de UNA variante: por herramienta
// (by_tool) + las primeras N issues. READ-ONLY (no toca ningún estado de la review). Advisory.
function QualityGateDetail({
  evidence, onClose, lbl, btn,
}: {
  evidence: VariantEvidence;
  onClose: () => void;
  lbl: React.CSSProperties;
  btn: (bg?: string) => React.CSSProperties;
}) {
  return (
    <div role="dialog" aria-label="Detalle de evidencia (quality-gate)" onClick={onClose}
      style={{ position: "fixed", inset: 0, background: "rgba(0,0,0,.5)", zIndex: 440, display: "flex", alignItems: "center", justifyContent: "center" }}>
      <div onClick={(e) => e.stopPropagation()}
        style={{ width: "min(720px,94vw)", maxHeight: "88vh", overflowY: "auto", padding: 20,
                 background: "var(--bg, #f3efe6)", color: "var(--ink, #1c1814)", border: "1px solid var(--line, rgba(0,0,0,.18))", borderRadius: 10, boxShadow: "0 20px 60px -20px rgba(0,0,0,.5)" }}>
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 8 }}>
          <div style={{ fontFamily: "var(--display, serif)", fontSize: 18, fontWeight: 600 }}>Evidencia objetiva · por herramienta</div>
          <button onClick={onClose} style={btn()}>×</button>
        </div>
        <p style={{ ...lbl, marginTop: 0 }}>advisory · no altera la review · todo local (sin red)</p>
        {evidence.by_tool.length === 0 && (
          <div style={{ fontSize: 13, color: "var(--ink-dim, #6b6358)" }}>Sin linters detectados para esta variante.</div>
        )}
        {evidence.by_tool.map((r) => (
          <div key={r.tool} style={{ border: "1px solid var(--line, rgba(0,0,0,.14))", borderRadius: 8, padding: 10, marginBottom: 10 }}>
            <div style={{ display: "flex", justifyContent: "space-between", alignItems: "baseline", gap: 8 }}>
              <strong style={{ fontFamily: "var(--mono)" }}>{r.tool}</strong>
              <span style={{ ...lbl, color: r.status === "ok" ? (r.errors > 0 ? "var(--clay, #b8543a)" : r.warnings > 0 ? "#b8862a" : "var(--accent)") : "var(--ink-dim, #6b6358)" }}>
                {r.status === "ok" ? toolCellText(r) : statusLabel(r.status)}
              </span>
            </div>
            {r.status !== "ok" && r.reason && (
              <div style={{ fontSize: 12, color: "var(--ink-dim, #6b6358)", marginTop: 4 }}>{r.reason}</div>
            )}
            {r.status === "ok" && (r.issues?.length ?? 0) > 0 && (
              <ul style={{ margin: "8px 0 0", paddingLeft: 18, fontSize: 12, fontFamily: "var(--mono)" }}>
                {(r.issues ?? []).slice(0, 20).map((iss, k) => (
                  <li key={k} style={{ marginBottom: 2 }}>
                    <span style={{ color: iss.severity === "error" ? "var(--clay, #b8543a)" : "#b8862a" }}>{iss.severity}</span>
                    {" · "}{iss.file}:{iss.line}{iss.rule ? ` · ${iss.rule}` : ""} — {iss.message}
                  </li>
                ))}
              </ul>
            )}
          </div>
        ))}
        {evidence.unavailable_tools.length > 0 && (
          <div style={{ ...lbl, marginTop: 4 }}>no disponible: {evidence.unavailable_tools.join(", ")}</div>
        )}
      </div>
    </div>
  );
}
