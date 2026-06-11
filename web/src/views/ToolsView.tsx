// FASE 1 — ToolsView: Skills marketplace and management.
// Council UX V4: empty/loading/error states, responsive, keyboard nav.
// Shows installed skills with enable/disable toggle, run button, history.

import { useCallback, useEffect, useState } from "react";
import { invoke } from "../lib/invoke"; // 015 T015: invoke con flujo de aprobación universal
import { SkillPane } from "../components/SkillPane";

interface SkillSummary {
  id: string; name: string; version: string;
  description?: string; category: string;
  enabled: boolean; installed_at: string;
}

interface RunHistory {
  id: string; skill_name: string;
  input?: string; output?: string;
  model_used?: string; tokens_used: number;
  latency_ms: number; status: string;
  error?: string;
  started_at: string; finished_at?: string;
}

type ViewState = "loading" | "empty" | "ready" | "error";

export function ToolsView() {
  const [skills, setSkills] = useState<SkillSummary[]>([]);
  const [viewState, setViewState] = useState<ViewState>("loading");
  const [error, setError] = useState<string | null>(null);
  const [runningSkill, setRunningSkill] = useState<string | null>(null);
  const [runInput, setRunInput] = useState("");
  const [history, setHistory] = useState<RunHistory[]>([]);
  const [historySkill, setHistorySkill] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  // 050 FR-005 — CRL: revocar una signing-key (mata spans vivos firmados por ella).
  const [revokeKeyHex, setRevokeKeyHex] = useState("");
  const [revokeMsg, setRevokeMsg] = useState<string | null>(null);
  const [revoking, setRevoking] = useState(false);

  const loadSkills = useCallback(async () => {
    setViewState("loading");
    try {
      const list = await invoke<SkillSummary[]>("skill_list");
      setSkills(list);
      setViewState(list.length === 0 ? "empty" : "ready");
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e));
      setViewState("error");
    }
  }, []);

  const refreshFromDisk = useCallback(async () => {
    setRefreshing(true);
    try {
      const count = await invoke<number>("skill_refresh");
      await loadSkills();
      if (count > 0) {
        setError(null);
      }
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e));
    }
    setRefreshing(false);
  }, [loadSkills]);

  useEffect(() => { loadSkills(); }, [loadSkills]);

  // 050 FR-005 — revoca una signing-key vía CRL activa: bloquea cargas futuras Y corta los spans
  // vivos firmados por ella. DESTRUCTIVO: el invoke pasa por el gate universal de confirmación.
  const revokeKey = async () => {
    const key = revokeKeyHex.trim().toLowerCase();
    if (!/^[0-9a-f]{64}$/.test(key)) {
      setRevokeMsg("La key debe ser 64 caracteres hexadecimales (SHA-256 del pubkey).");
      return;
    }
    setRevoking(true);
    setRevokeMsg(null);
    try {
      const r = await invoke<{ persisted: boolean; signaled_spans: number }>("crl_revoke_key", { keyHex: key });
      setRevokeMsg(`Key revocada. ${r.signaled_spans} ejecución(es) en curso señalizada(s) para abortar.`);
      setRevokeKeyHex("");
    } catch (e: unknown) {
      setRevokeMsg(e instanceof Error ? e.message : String(e));
    } finally {
      setRevoking(false);
    }
  };

  const toggleEnabled = async (name: string, enabled: boolean) => {
    try {
      await invoke("skill_set_enabled", { name, enabled });
      setSkills((prev) => prev.map((s) => s.name === name ? { ...s, enabled } : s));
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const deleteSkill = async (name: string) => {
    try {
      await invoke("skill_delete", { name });
      setSkills((prev) => prev.filter((s) => s.name !== name));
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const loadHistory = async (name: string) => {
    setHistorySkill(name);
    try {
      const h = await invoke<RunHistory[]>("skill_history", { name, limit: 10 });
      setHistory(h);
    } catch {
      setHistory([]);
    }
  };

  const runSkill = async (name: string, input: string) => {
    if (!input.trim()) return;
    setRunningSkill(name);
    try {
      await invoke("skill_run", { name, input });
      setRunInput("");
      setTimeout(() => loadHistory(name), 500);
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e));
    }
    setRunningSkill(null);
  };

  // Render
  if (viewState === "loading") {
    return (
      <div className="tools-view" style={{ padding: "2rem", textAlign: "center" }}>
        <div className="skeleton" style={{ height: 24, width: 200, margin: "0 auto 1rem", background: "var(--surface2)", borderRadius: 4 }} />
        <div className="skeleton" style={{ height: 16, width: 300, margin: "0 auto", background: "var(--surface2)", borderRadius: 4 }} />
        <p style={{ color: "var(--text2)", marginTop: "1rem" }}>Loading skills...</p>
      </div>
    );
  }

  if (viewState === "error") {
    return (
      <div className="tools-view" style={{ padding: "2rem", textAlign: "center" }}>
        <div style={{ color: "var(--red)", fontSize: "1.2rem", marginBottom: "1rem" }}>⚠ Error loading skills</div>
        <p style={{ color: "var(--text2)", marginBottom: "1rem" }}>{error}</p>
        <button className="btn btn-primary" onClick={loadSkills}>Retry</button>
      </div>
    );
  }

  if (viewState === "empty") {
    return (
      <div className="tools-view" style={{ padding: "2rem", textAlign: "center" }}>
        <div style={{ fontSize: "3rem", marginBottom: "1rem", opacity: 0.3 }}>🧩</div>
        <h3 style={{ marginBottom: ".5rem" }}>No skills installed</h3>
        <p style={{ color: "var(--text2)", marginBottom: "1.5rem", maxWidth: 400, margin: "0 auto 1.5rem" }}>
          Skills are reusable AI workflows that run in Furx panes.
          Install skills by placing <code>skill.yaml</code> or <code>SKILL.md</code> files in
          <code> ~/.furx/skills/</code> or <code>~/.claude/skills/</code>.
        </p>
        <button className="btn btn-primary" onClick={refreshFromDisk} disabled={refreshing}>
          {refreshing ? "Scanning..." : "Scan for skills"}
        </button>
      </div>
    );
  }

  return (
    <div className="tools-view" style={{ padding: "1.5rem", display: "flex", flexDirection: "column", gap: "1rem" }}>
      {/* Header */}
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
        <h2 style={{ margin: 0 }}>🧩 Skills</h2>
        <div style={{ display: "flex", gap: ".5rem" }}>
          <button className="btn btn-secondary" onClick={refreshFromDisk} disabled={refreshing}>
            {refreshing ? "⟳" : "↻"} Refresh
          </button>
        </div>
      </div>

      {/* Error banner */}
      {error && (
        <div style={{ color: "var(--red)", fontSize: ".85rem", padding: ".5rem", background: "rgba(248,81,73,.1)", borderRadius: 6 }}>
          {error}
          <button style={{ marginLeft: ".5rem", cursor: "pointer", background: "none", border: "none", color: "var(--red)" }} onClick={() => setError(null)}>✕</button>
        </div>
      )}

      {/* Skills list */}
      {skills.length === 0 ? (
        <p style={{ color: "var(--text2)", textAlign: "center", padding: "2rem" }}>
          No skills found. Click Refresh to scan <code>~/.furx/skills/</code> and <code>~/.claude/skills/</code>.
        </p>
      ) : (
        <div style={{ display: "grid", gap: ".5rem" }}>
          {skills.map((s) => (
            <div key={s.id} style={{
              display: "flex", alignItems: "center", gap: ".75rem",
              padding: ".75rem", background: "var(--surface)", borderRadius: 8,
              border: "1px solid var(--border)",
            }}>
              {/* Toggle */}
              <button
                onClick={() => toggleEnabled(s.name, !s.enabled)}
                style={{
                  width: 36, height: 20, borderRadius: 10, border: "none", cursor: "pointer",
                  background: s.enabled ? "var(--green)" : "var(--border)",
                  position: "relative", transition: "background .2s",
                }}
                aria-label={`Toggle ${s.name}`}
              >
                <span style={{
                  position: "absolute", top: 2, width: 16, height: 16, borderRadius: "50%",
                  background: "#fff", transition: "left .2s",
                  left: s.enabled ? 18 : 2,
                }} />
              </button>

              {/* Info */}
              <div style={{ flex: 1, minWidth: 0 }}>
                <div style={{ fontWeight: 600, fontSize: ".9rem" }}>{s.name}</div>
                <div style={{ color: "var(--text2)", fontSize: ".8rem" }}>
                  {s.category} · v{s.version}
                  {s.description && ` · ${s.description}`}
                </div>
              </div>

              {/* Run button */}
              <button className="btn btn-primary" style={{ fontSize: ".8rem", padding: ".3rem .8rem" }}
                onClick={() => { setRunInput(""); setRunningSkill(s.name); }}
                disabled={!s.enabled}>
                ▶ Run
              </button>

              {/* History */}
              <button className="btn btn-secondary" style={{ fontSize: ".8rem", padding: ".3rem .8rem" }}
                onClick={() => loadHistory(s.name)}>
                📋
              </button>

              {/* Delete */}
              <button className="btn btn-secondary" style={{ fontSize: ".8rem", padding: ".3rem .8rem", color: "var(--red)" }}
                onClick={() => deleteSkill(s.name)}>
                ✕
              </button>
            </div>
          ))}
        </div>
      )}

      {/* Skill run modal */}
      {runningSkill && (
        <SkillPane
          skillName={runningSkill}
          onClose={() => setRunningSkill(null)}
          onRun={(input: string) => runSkill(runningSkill!, input)}
          input={runInput}
          onInputChange={setRunInput}
        />
      )}

      {/* History panel */}
      {historySkill && history.length > 0 && (
        <div style={{ background: "var(--surface)", borderRadius: 8, border: "1px solid var(--border)", padding: "1rem" }}>
          <div style={{ display: "flex", justifyContent: "space-between", marginBottom: ".5rem" }}>
            <h4 style={{ margin: 0 }}>📋 {historySkill} — Recent Runs</h4>
            <button className="btn btn-secondary" style={{ fontSize: ".75rem", padding: ".2rem .5rem" }}
              onClick={() => { setHistorySkill(null); setHistory([]); }}>
              ✕
            </button>
          </div>
          <div style={{ maxHeight: 200, overflowY: "auto" }}>
            {history.map((r) => (
              <div key={r.id} style={{
                display: "flex", justifyContent: "space-between", alignItems: "center",
                padding: ".4rem 0", borderBottom: "1px solid var(--border)", fontSize: ".8rem"
              }}>
                <span style={{
                  color: r.status === "success" ? "var(--green)" : r.status === "error" ? "var(--red)" : "var(--yellow)"
                }}>
                  {r.status === "success" ? "✓" : r.status === "error" ? "✗" : "⟳"} {r.status}
                </span>
                <span style={{ color: "var(--text2)" }}>
                  {r.model_used && `${r.model_used} · `}
                  {r.tokens_used > 0 && `${r.tokens_used} tok · `}
                  {r.latency_ms}ms
                </span>
              </div>
            ))}
          </div>
        </div>
      )}

      {/* 050 FR-005 — CRL: revocación activa de una signing-key. Bloquea cargas futuras Y corta los
          spans (ejecuciones) vivos firmados por esa key. Acción avanzada/destructiva. */}
      <details style={{ background: "var(--surface)", borderRadius: 8, border: "1px solid var(--border)", padding: ".75rem" }}>
        <summary style={{ cursor: "pointer", fontSize: ".85rem", color: "var(--text2)" }}>
          🛡️ Revocar signing-key (CRL)
        </summary>
        <p style={{ fontSize: ".78rem", color: "var(--text2)", margin: ".5rem 0" }}>
          Revoca la clave de firma de un skill (SHA-256 hex de 64 caracteres del pubkey). Bloquea las
          cargas futuras firmadas por ella <strong>y</strong> aborta cualquier ejecución en curso
          firmada por esa clave. No se puede deshacer.
        </p>
        <div style={{ display: "flex", gap: ".5rem", alignItems: "center", flexWrap: "wrap" }}>
          <input
            value={revokeKeyHex}
            onChange={(e) => setRevokeKeyHex(e.target.value)}
            placeholder="64 hex chars (SHA-256 del pubkey)"
            spellCheck={false}
            style={{ flex: 1, minWidth: 280, padding: ".35rem", borderRadius: 6, border: "1px solid var(--border)", background: "var(--bg)", color: "var(--text)", fontSize: ".78rem", fontFamily: "var(--mono, monospace)" }}
            aria-label="Signing key a revocar (64 hex)"
          />
          <button
            className="btn"
            style={{ fontSize: ".78rem", color: "var(--red, #c0392b)", borderColor: "var(--red, #c0392b)" }}
            onClick={revokeKey}
            disabled={revoking || revokeKeyHex.trim().length === 0}
          >
            {revoking ? "Revocando…" : "Revocar"}
          </button>
        </div>
        {revokeMsg && (
          <div style={{ fontSize: ".78rem", color: "var(--text2)", marginTop: ".5rem" }}>{revokeMsg}</div>
        )}
      </details>
    </div>
  );
}
