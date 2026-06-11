// 1.10 / W1 — Cross-LLM disagreement modal.
// Manda mismo prompt a panes Claude/Codex/Gemini/Aider via broadcast,
// captura responses con timeout 60s, llama analyze para consensus score.

import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { PaneCfg } from "../types";
import { Modal } from "./Modal";
import { Button } from "./Button";

interface PaneSummary { pane_id: string; mode: string; avg_sim: number; chars: number; first_line: string; }
interface PairwiseSim { a: string; b: string; jaccard: number; }
interface DisagreementReport { consensus_score: number; pairwise: PairwiseSim[]; outliers: string[]; by_pane_summary: PaneSummary[]; }

interface Props { panes: PaneCfg[]; onClose: () => void; onCapture: () => Promise<Record<string, string>>; }

export function DisagreementModal({ panes, onClose, onCapture }: Props) {
  const [prompt, setPrompt] = useState("");
  const [step, setStep] = useState<"compose" | "broadcasting" | "waiting" | "done">("compose");
  const [report, setReport] = useState<DisagreementReport | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const timer = useRef<number | null>(null);
  useEffect(() => {
    return () => { if (timer.current) window.clearTimeout(timer.current); };
  }, []);
  const llmPanes = panes.filter((p) => p.mode !== "zsh");
  const run = async () => {
    if (!prompt.trim() || llmPanes.length < 2) return;
    setStep("broadcasting"); setErr(null);
    const cid = `disagree-${Date.now()}`;
    for (const p of llmPanes) {
      await invoke("pty_write", { paneId: p.id, data: prompt + "\n", actionId: `${cid}-${p.id}`, correlationId: cid }).catch(console.error);
    }
    setStep("waiting");
    // wait 60s for responses to accumulate in pane buffers
    timer.current = window.setTimeout(async () => {
      try {
        const buffers = await onCapture();
        const responses = llmPanes.map((p) => ({
          pane_id: p.id, mode: p.mode,
          // Tail of buffer, skip the echoed prompt (best-effort): use last 4KB.
          text: (buffers[p.id] ?? "").slice(-4096),
        }));
        const r = await invoke<DisagreementReport>("disagreement_analyze", { responses });
        setReport(r); setStep("done");
      } catch (e) { setErr(String(e)); setStep("done"); }
    }, 60000);
  };
  return (
    <Modal
      title="Cross-LLM disagreement"
      subtitle={`W1 · mismo prompt a ${llmPanes.length} panes LLM · 60s wait + jaccard consensus`}
      maxWidth={760}
      onClose={onClose}
      onSubmit={run}
      canSubmit={step === "compose" && !!prompt.trim() && llmPanes.length >= 2}
    >
          {step === "compose" && (
            <>
              <textarea
                value={prompt} onChange={(e) => setPrompt(e.target.value)}
                placeholder="Prompt común para todos los LLMs…" rows={5} autoFocus
                style={{ width: "100%", background: "var(--bg2)", border: "1px solid var(--line)", borderRadius: 6, padding: 10, fontFamily: "var(--mono)", fontSize: 12, color: "var(--text)", outline: "none" }}
              />
              {llmPanes.length < 2 && <div className="card-block info" style={{ borderLeftColor: "var(--amber)" }}>Necesitás ≥2 panes LLM (claude/codex/gemini/aider).</div>}
              <div className="wizard-actions">
                <button onClick={onClose}>Cancelar</button>
                <Button variant="primary" disabled={!prompt.trim() || llmPanes.length < 2} onClick={run}>Broadcast + measure</Button>
              </div>
            </>
          )}
          {(step === "broadcasting" || step === "waiting") && (
            <>
              <div className="muted">{step === "broadcasting" ? "Enviando…" : "Esperando responses (60s)…"}</div>
              <div className="card-block info" style={{ marginTop: 10 }}>El analizador captura los últimos 4KB del buffer de cada pane cuando termine el timer.</div>
            </>
          )}
          {step === "done" && report && (
            <>
              <div style={{ display: "flex", gap: 12, alignItems: "baseline", marginBottom: 12 }}>
                <strong style={{ fontSize: 20, color: report.consensus_score > 0.7 ? "var(--green)" : report.consensus_score > 0.4 ? "var(--amber)" : "var(--red)" }}>
                  Consenso {(report.consensus_score * 100).toFixed(0)}%
                </strong>
                {report.outliers.length > 0 && <span className="sev-tag sev-warning">outliers: {report.outliers.join(", ")}</span>}
              </div>
              <div className="mon-grid">
                {report.by_pane_summary.map((p) => (
                  <div key={p.pane_id} className={`mon ${report.outliers.includes(p.pane_id) ? "down" : "up"}`}>
                    <div className="mon-head">
                      <span className="mon-label">{p.mode}</span>
                      <span className="mon-addr muted">{p.pane_id}</span>
                    </div>
                    <div style={{ fontSize: 12 }}>
                      avg_sim <strong>{(p.avg_sim * 100).toFixed(0)}%</strong> · {p.chars} chars
                      <div className="muted" style={{ fontSize: 11, marginTop: 4 }}>{p.first_line}</div>
                    </div>
                  </div>
                ))}
              </div>
            </>
          )}
          {err && <div className="card-block info" style={{ borderLeftColor: "var(--red)", marginTop: 12 }}>{err}</div>}
    </Modal>
  );
}
