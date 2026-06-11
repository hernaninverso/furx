// FASE 2+3 — MemoryView: Cross-CLI Memory Hub browser + UMP + LaunchAgent + Hooks + Graph.
import { useCallback, useEffect, useState } from "react";
import { Brain } from "lucide-react"; // 057 — glyph del empty state
import { invoke } from "../lib/invoke"; // 015 T015: invoke con flujo de aprobación universal

interface MemoryEntry {
  id: string; source: string; source_id?: string;
  content: string; tags: string[];
  created_at: string;
  // 023 — procedencia/gobierno no-opaco.
  rationale?: string | null;
  kind?: string;
  cli_kind?: string | null;
  session_id?: string | null;
  project_key?: string;
}
interface MemoryStats {
  total_entries: number;
  by_source: { source: string; count: number }[];
  by_project?: { project_key: string; count: number }[];
  latest: MemoryEntry | null;
}
// 023 F1 — una propuesta de memoria de la bandeja de revisión.
interface MemoryProposal {
  id: string;
  project_key: string;
  source: string;
  source_id?: string | null;
  cli_kind?: string | null;
  session_id?: string | null;
  content: string;
  kind?: string | null;
  confidence_score?: number | null;
  status: string;
  rationale?: string | null;
  created_at: string;
  decided_at?: string | null;
}
interface AutocaptureSettings {
  autocapture: boolean;
  auto_accept: boolean;
  inject: boolean;
  max_candidates: number;
}
// 025 F1 — una lección procedural aprobada + su estado de activación (gobierno de inyección).
interface ActiveLessonDto {
  entry_id: string;
  project_key: string;
  content: string;
  created_at: string;
  active: boolean;
  // 050 FR-002 — feedback de utilidad (advisory; NO afecta `active`, decisión humana).
  useful_count: number;
  not_useful_count: number;
  last_vote: string; // "useful" | "not_useful" | ""
}
// 050 FR-002 — conteo devuelto por lesson_record_feedback (refleja el voto sin re-fetch).
interface LessonFeedback {
  useful_count: number;
  not_useful_count: number;
  last_vote: string;
}
// 025 F1 — vista dry-run de "Lecciones aprendidas": lista + el TEXTO LITERAL que se inyectaría.
interface LessonsActiveView {
  lessons: ActiveLessonDto[];
  injected_block: string | null;
  token_budget: number;
}
const KIND_OPTIONS = ["episodic", "procedural", "project_fact", "preference"] as const;
// BLOQUE J ext 2 (council TS must-fix): concrete shape for the F3.4 knowledge
// graph entity that the backend returns from `memory_graph_entities`.
interface GraphEntity {
  id: string;
  name: string;
  entity_type: string;
  metadata?: string;
  created_at: string;
}
interface MemoryCliHooks {
  claude_commands: number;
  claude_hooks: string;
  codex_commands: number;
}
type ViewState = "loading" | "empty" | "ready" | "error";
type Tab = "memories" | "proposals" | "lessons" | "ump" | "launchagent" | "hooks" | "graph" | "agent_recall" | "corpus";
// Spec 066 — Furx Memory (corpus-engine). Tipos = envelope CorpusResult del backend Rust.
type CorpusErr = "not_installed" | "timeout" | "locked" | "invalid_json" | "incompatible_version" | "output_too_large" | "bad_input";
interface CorpusResult<T> { available: boolean; data?: T; error_code?: CorpusErr; error_message?: string; }
interface CorpusStatus { schema_version: number; sessions: number; messages: number; tool_events: number; }
interface CorpusHit { uuid: string; session: string; project: string; timestamp: string; human: boolean; snippet: string; }
interface CorpusSearch { n: number; results: CorpusHit[]; }
interface CorpusDeadend { signature: string; count: number; example: string; }
interface CorpusDeadends { distinct_errors: number; recurrent: CorpusDeadend[]; }
interface CorpusDecision { type: string; timestamp: string; project: string; session: string; text: string; }
interface CorpusLedger { n: number; decisions: CorpusDecision[]; }

// 053 — tipos del recall de agente (ProjectMemory de Rust).
interface AgentRecallResult {
  project: string;
  recalled: string;
  source: string; // "mnemo" | "memento" | "fallback"
}

export function MemoryView() {
  const [entries, setEntries] = useState<MemoryEntry[]>([]);
  const [viewState, setViewState] = useState<ViewState>("loading");
  const [error, setError] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [stats, setStats] = useState<MemoryStats | null>(null);
  const [searching, setSearching] = useState(false);
  // 045 FR-003 — backend del último recall ("fts" baseline | "vector" re-rank) para el badge.
  const [searchBackend, setSearchBackend] = useState<string | null>(null);
  const [activeTab, setActiveTab] = useState<Tab>("memories");
  // Furx Memory (corpus-engine)
  const [corpusStatus, setCorpusStatus] = useState<CorpusResult<CorpusStatus> | null>(null);
  const [corpusQuery, setCorpusQuery] = useState("");
  const [corpusHits, setCorpusHits] = useState<CorpusResult<CorpusSearch> | null>(null);
  const [corpusDead, setCorpusDead] = useState<CorpusResult<CorpusDeadends> | null>(null);
  const [corpusBusy, setCorpusBusy] = useState(false);
  const [launchAgentRunning, setLaunchAgentRunning] = useState(false);
  const [hooksGenerated, setHooksGenerated] = useState<string | null>(null);
  const [graphEntities, setGraphEntities] = useState<GraphEntity[]>([]);
  // 023 F1 — bandeja de propuestas + settings de auto-captura.
  const [proposals, setProposals] = useState<MemoryProposal[]>([]);
  const [autoSettings, setAutoSettings] = useState<AutocaptureSettings | null>(null);
  // edición en la bandeja: id → { content, kind } draft.
  const [drafts, setDrafts] = useState<Record<string, { content: string; kind: string }>>({});
  // 025 F1 — sub-vista "Lecciones aprendidas": lecciones activas por proyecto + preview del bloque.
  const [lessonsView, setLessonsView] = useState<LessonsActiveView | null>(null);
  const [lessonsProject, setLessonsProject] = useState("__global__");
  // 025 (LOW del audit) — proyecto auto-detectado del spawn actual (lo que de verdad se inyecta),
  // para que el usuario no tenga que adivinar el project_key.
  const [autoProject, setAutoProject] = useState<string | null>(null);

  // 053 — estado del recall de agente.
  const [agentRecallProject, setAgentRecallProject] = useState("");
  const [agentRecallResult, setAgentRecallResult] = useState<AgentRecallResult | null>(null);
  const [agentRecallLoading, setAgentRecallLoading] = useState(false);
  const [agentRecallError, setAgentRecallError] = useState<string | null>(null);

  const loadStats = useCallback(async () => {
    try { const s = await invoke<MemoryStats>("memory_stats"); setStats(s); } catch { /* best-effort */ }
  }, []);
  const loadRecent = useCallback(async () => {
    setViewState("loading");
    try {
      const s = await invoke<MemoryEntry[]>("memory_recall", { query: "", limit: 20 });
      setEntries(s); setViewState(s.length === 0 ? "empty" : "ready"); await loadStats();
    } catch (e: unknown) { setError(e instanceof Error ? e.message : String(e)); setViewState("error"); }
  }, [loadStats]);
  useEffect(() => { loadRecent(); }, [loadRecent]);

  // 023 F1 — bandeja de propuestas.
  const loadProposals = useCallback(async () => {
    try {
      const [p, s] = await Promise.all([
        invoke<MemoryProposal[]>("memory_proposals_list"),
        invoke<AutocaptureSettings>("memory_autocapture_settings"),
      ]);
      setProposals(p);
      setAutoSettings(s);
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, []);
  useEffect(() => { if (activeTab === "proposals") loadProposals(); }, [activeTab, loadProposals]);

  // 025 F1 — cargar las lecciones activas + el preview del bloque inyectable (dry-run).
  const loadLessons = useCallback(async (overrideProject?: string) => {
    try {
      // Permitir un project explícito para evitar el stale load: si el caller acaba de setear el
      // project (p.ej. "usar <autoProject>"), el re-render aún no corrió, así que `lessonsProject`
      // tendría el valor VIEJO. Pasando el override, el preview corresponde al proyecto recién elegido.
      const pk = (overrideProject ?? lessonsProject) || "__global__";
      const v = await invoke<LessonsActiveView>("lessons_active_list", {
        projectKey: pk,
      });
      setLessonsView(v);
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, [lessonsProject]);
  useEffect(() => { if (activeTab === "lessons") loadLessons(); }, [activeTab, loadLessons]);
  // Furx Memory: al abrir la tab Sessions, cargar status + dead-ends (corpus-engine local).
  useEffect(() => {
    if (activeTab !== "corpus") return;
    invoke<CorpusResult<CorpusStatus>>("corpus_status").then(setCorpusStatus).catch(() => setCorpusStatus({ available: false, error_code: "not_installed" }));
    invoke<CorpusResult<CorpusDeadends>>("corpus_deadends", { top: 8 }).then(setCorpusDead).catch(() => setCorpusDead(null));
  }, [activeTab]);
  const runCorpusSearch = useCallback(async () => {
    const q = corpusQuery.trim();
    if (!q) return;
    setCorpusBusy(true);
    try { setCorpusHits(await invoke<CorpusResult<CorpusSearch>>("corpus_search", { query: q, limit: 20 })); }
    catch { setCorpusHits({ available: false, error_code: "invalid_json" }); }
    finally { setCorpusBusy(false); }
  }, [corpusQuery]);

  // 025 (LOW) — al entrar a la pestaña Lecciones, auto-detectar el project_key del spawn actual y
  // usarlo como default (en vez de pedirlo a mano). El usuario puede igual overridearlo en el input.
  useEffect(() => {
    if (activeTab !== "lessons") return;
    let cancelled = false;
    (async () => {
      try {
        const pk = await invoke<string>("lessons_current_project_key", {});
        if (cancelled) return;
        setAutoProject(pk);
        // sólo auto-setear si el usuario aún no tocó el input (sigue en el default global).
        setLessonsProject((cur) => (cur === "__global__" ? pk : cur));
      } catch { /* best-effort: queda __global__ */ }
    })();
    return () => { cancelled = true; };
  }, [activeTab]);

  const toggleLesson = useCallback(async (entryId: string, projectKey: string, active: boolean) => {
    try {
      await invoke("lesson_set_active", { entryId, projectKey, active });
      await loadLessons();
    } catch (e: unknown) { setError(e instanceof Error ? e.message : String(e)); }
  }, [loadLessons]);

  const deleteLesson = useCallback(async (entryId: string) => {
    if (!confirm("¿Borrar esta lección? Esta acción no se puede deshacer.")) return;
    try {
      await invoke("lesson_delete", { entryId });
      await loadLessons();
    } catch (e: unknown) { setError(e instanceof Error ? e.message : String(e)); }
  }, [loadLessons]);

  // 050 FR-002 — voto de utilidad sobre una lección ("¿fue útil?"). ADVISORY: NO la desactiva ni la
  // borra (eso es decisión humana con los botones de arriba) — solo registra el voto y refresca el
  // conteo de la lista. El backend devuelve el conteo actualizado; refrescamos la lista para verlo.
  const rateLesson = useCallback(async (entryId: string, projectKey: string, useful: boolean) => {
    try {
      await invoke<LessonFeedback>("lesson_record_feedback", { entryId, projectKey, useful });
      await loadLessons();
    } catch (e: unknown) { setError(e instanceof Error ? e.message : String(e)); }
  }, [loadLessons]);

  const decideProposal = useCallback(
    async (id: string, action: "accept" | "reject" | "edit") => {
      const draft = drafts[id];
      try {
        await invoke("memory_proposal_decide", {
          id,
          action,
          content: action !== "reject" ? draft?.content : undefined,
          kind: action !== "reject" ? draft?.kind : undefined,
        });
        setDrafts((d) => {
          const next = { ...d };
          delete next[id];
          return next;
        });
        await loadProposals();
      } catch (e: unknown) {
        setError(e instanceof Error ? e.message : String(e));
      }
    },
    [drafts, loadProposals],
  );

  const forgetProject = useCallback(async (projectKey: string) => {
    if (!confirm(`¿Borrar toda la memoria del proyecto "${projectKey}"? Esta acción no se puede deshacer.`)) return;
    try {
      await invoke<number>("memory_forget_project", { projectKey, includeShared: false });
      await loadRecent();
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, [loadRecent]);

  const handleSearch = async () => {
    if (!query.trim()) { await loadRecent(); setSearchBackend(null); return; }
    setSearching(true);
    try {
      // 045 FR-003 — pide re-rank vectorial (opt-in). Si el embedder está caído, el backend
      // devuelve "fts" (degradación limpia, nunca cuelga). Exponemos `backend` en el badge.
      const res = await invoke<{ entries: MemoryEntry[]; total: number; backend: string }>(
        "memory_recall_ranked", { query: query.trim(), limit: 30, rerank: true });
      setEntries(res.entries); setSearchBackend(res.backend);
      setViewState(res.entries.length === 0 ? "empty" : "ready");
    } catch (e: unknown) { setError(e instanceof Error ? e.message : String(e)); }
    setSearching(false);
  };
  const handleStore = async () => {
    const content = prompt("Enter memory content:");
    if (!content?.trim()) return;
    try { await invoke("memory_store", { content: content.trim(), source: "furx" }); await loadRecent(); }
    catch (e: unknown) { setError(e instanceof Error ? e.message : String(e)); }
  };

  const sourceColor = (s: string) => {
    const colors: Record<string, string> = { furx: "var(--accent)", claude: "var(--cyan)", codex: "var(--amber)", gemini: "var(--green)", aider: "var(--red)", manual: "var(--text2)", ump: "var(--purple)" };
    return colors[s] || "var(--text2)";
  };

  if (viewState === "loading" && activeTab === "memories") {
    return <div className="memory-view" style={{ padding: "2rem", textAlign: "center" }}><p style={{ color: "var(--text2)" }}>Loading memory hub...</p></div>;
  }
  if (viewState === "error") {
    return <div className="memory-view" style={{ padding: "2rem", textAlign: "center" }}><div style={{ color: "var(--red)", marginBottom: "1rem" }}>⚠ Error: {error}</div><button className="btn btn-primary" onClick={loadRecent}>Retry</button></div>;
  }

  return (
    <div className="memory-view" style={{ padding: "1.5rem", display: "flex", flexDirection: "column", gap: "1rem" }}>
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
        <h2 style={{ margin: 0 }}>🧠 Memory Hub</h2>
        <button className="btn btn-primary" onClick={handleStore}>+ Add Memory</button>
      </div>

      {/* Tabs */}
      <div style={{ display: "flex", gap: ".25rem", borderBottom: "1px solid var(--border)", flexWrap: "wrap" }}>
        {(["memories","corpus","proposals","lessons","ump","launchagent","hooks","graph","agent_recall"] as Tab[]).map((tab) => (
          <button key={tab} onClick={() => setActiveTab(tab)} style={{
            padding: ".4rem .8rem", fontSize: ".8rem", cursor: "pointer",
            background: activeTab === tab ? "var(--surface)" : "transparent",
            color: activeTab === tab ? "var(--accent)" : "var(--text2)",
            border: "none", borderBottom: activeTab === tab ? "2px solid var(--accent)" : "2px solid transparent",
            borderRadius: "4px 4px 0 0",
          }}>{tab === "memories" ? "Memories" : tab === "proposals" ? `Propuestas${proposals.length ? ` (${proposals.length})` : ""}` : tab === "corpus" ? "Sessions" : tab === "lessons" ? "Lecciones" : tab === "ump" ? "UMP" : tab === "launchagent" ? "LaunchAgent" : tab === "hooks" ? "CLI Hooks" : tab === "graph" ? "Graph" : "Recall agente"}</button>
        ))}
      </div>

      {/* Memories tab */}
      {activeTab === "memories" && (<>
        {stats && (
          <div style={{ display: "flex", gap: "1rem", flexWrap: "wrap", fontSize: ".85rem", color: "var(--text2)" }}>
            <span>{stats.total_entries} memories</span>
            {stats.by_source.map((s) => (
              <span key={s.source}><span style={{ color: sourceColor(s.source) }}>■</span> {s.source}: {s.count}</span>
            ))}
          </div>
        )}
        {/* 023 — borrar la memoria de un proyecto entero (acción destructiva, con confirmación). */}
        {stats?.by_project && stats.by_project.length > 0 && (
          <div style={{ display: "flex", gap: ".5rem", flexWrap: "wrap", alignItems: "center", fontSize: ".75rem" }}>
            <span style={{ color: "var(--text2)" }}>Borrar memoria por proyecto:</span>
            {stats.by_project
              .filter((p) => p.project_key !== "__shared__")
              .map((p) => (
                <button
                  key={p.project_key}
                  onClick={() => forgetProject(p.project_key)}
                  title={`Borrar las ${p.count} memorias de ${p.project_key}`}
                  style={{ fontSize: ".7rem", padding: ".15rem .5rem", borderRadius: 4, border: "1px solid var(--border)", background: "var(--surface2)", color: "var(--red)", cursor: "pointer" }}
                >
                  {p.project_key === "__global__" ? "global" : p.project_key.split("/").pop()} ({p.count}) ✕
                </button>
              ))}
          </div>
        )}
        <div style={{ display: "flex", gap: ".5rem" }}>
          <input value={query} onChange={(e) => setQuery(e.target.value)} onKeyDown={(e) => e.key === "Enter" && handleSearch()} placeholder="Search memories... (semantic FTS5)" style={{ flex: 1, padding: ".5rem", borderRadius: 6, border: "1px solid var(--border)", background: "var(--surface)", color: "var(--text)", fontSize: ".85rem" }} aria-label="Search memories" />
          <button className="btn btn-primary" onClick={handleSearch} disabled={searching}>{searching ? "..." : "Search"}</button>
        </div>
        {/* 045 FR-003 — indicador de calidad del recall: re-rank vectorial vs FTS baseline. */}
        {searchBackend && (
          <div style={{ marginTop: ".35rem" }}>
            <span
              className={`sev-tag ${searchBackend === "vector" ? "sev-info" : ""}`}
              title={searchBackend === "vector"
                ? "Resultados re-rankeados por similitud vectorial (embeddings)"
                : "Re-rank vectorial no disponible (embedder caído o sin embeddings) — orden FTS5"}
              style={{ fontSize: ".7rem" }}
            >
              {searchBackend === "vector" ? "re-rank vectorial" : "orden FTS5"}
            </span>
          </div>
        )}
        {viewState === "empty" && (
          <div className="empty-state">
            <div className="empty-glyph"><Brain /></div>
            <h3>Tu memoria está vacía</h3>
            <p>Furx captura lecciones, decisiones y contexto de tus sesiones para recuperarlos cuando los necesites. Apenas trabajes con un agente, esto se va a poblar solo.</p>
          </div>
        )}
        {entries.length > 0 && (
          <div>
            {entries.map((e) => (
              <div key={e.id} className="mem-card-2">
                <div className="meta">
                  {e.kind && <span className="kind">{e.kind}</span>}
                  <span style={{ fontSize: ".75rem", color: sourceColor(e.source), fontWeight: 600 }}>{e.source}</span>
                  {e.cli_kind && <span style={{ fontSize: ".7rem", color: "var(--faint)" }}>· {e.cli_kind}</span>}
                  <time>{new Date(e.created_at).toLocaleString()}</time>
                </div>
                <div className="txt" style={{ whiteSpace: "pre-wrap" }}>{e.content.length > 300 ? e.content.slice(0, 300) + "…" : e.content}</div>
                {e.rationale && (
                  <div style={{ fontSize: ".72rem", color: "var(--faint)", marginTop: ".4rem", fontStyle: "italic" }}>
                    Por qué: {e.rationale}
                  </div>
                )}
                {e.tags.length > 0 && (
                  <div style={{ display: "flex", gap: ".3rem", marginTop: ".55rem", flexWrap: "wrap" }}>
                    {e.tags.map((t, i) => (
                      <span key={i} style={{ fontSize: ".68rem", padding: ".12rem .45rem", borderRadius: 5, background: "var(--wash-2)", color: "var(--dim)", fontFamily: "var(--mono)" }}>{t}</span>
                    ))}
                  </div>
                )}
              </div>
            ))}
          </div>
        )}
      </>)}

      {/* 023 F1 — Bandeja de propuestas (revisión humana). NUNCA opaca. */}
      {activeTab === "proposals" && (<>
        <div style={{ padding: ".85rem", background: "var(--surface)", borderRadius: 8, border: "1px solid var(--border)", fontSize: ".82rem", color: "var(--text2)", lineHeight: 1.5 }}>
          <p style={{ margin: 0 }}>
            Cuando cerrás un pane de un CLI de agente, Furx destila la sesión en memorias candidatas y te las muestra acá.
            El texto pasa por el redactor de secretos antes de salir de tu Mac. Nada entra al Hub sin que vos lo aceptes.
          </p>
          {autoSettings && (
            <div style={{ marginTop: ".5rem", fontSize: ".75rem" }}>
              Auto-captura: <strong style={{ color: autoSettings.autocapture ? "var(--green)" : "var(--text2)" }}>{autoSettings.autocapture ? "activada" : "desactivada"}</strong>
              {" · "}Auto-aceptar: <strong>{autoSettings.auto_accept ? "sí" : "no"}</strong>
              {!autoSettings.autocapture && <span> — activala en Ajustes → Memoria para empezar a poblar el Hub.</span>}
            </div>
          )}
        </div>
        {proposals.length === 0 ? (
          <div style={{ textAlign: "center", padding: "2.5rem", color: "var(--text2)" }}>
            <div style={{ fontSize: "2.5rem", marginBottom: ".75rem", opacity: 0.3 }}>📥</div>
            <p>No hay memorias propuestas. Cerrá una sesión de CLI con la auto-captura activada para ver candidatas acá.</p>
          </div>
        ) : (
          <div style={{ display: "flex", flexDirection: "column", gap: ".6rem" }}>
            {proposals.map((p) => {
              const draft = drafts[p.id] ?? { content: p.content, kind: p.kind ?? "episodic" };
              return (
                <div key={p.id} style={{ padding: ".8rem", background: "var(--surface)", borderRadius: 8, border: "1px solid var(--border)" }}>
                  <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: ".4rem", flexWrap: "wrap", gap: ".4rem" }}>
                    <span style={{ display: "flex", gap: ".4rem", alignItems: "center", fontSize: ".72rem", color: "var(--text2)" }}>
                      <span style={{ color: sourceColor(p.source), fontWeight: 600 }}>{p.cli_kind ?? p.source}</span>
                      <span>· {p.project_key === "__global__" ? "global" : p.project_key.split("/").pop()}</span>
                      {p.source_id && <span>· {p.source_id}</span>}
                      {typeof p.confidence_score === "number" && <span>· confianza {(p.confidence_score * 100).toFixed(0)}%</span>}
                    </span>
                    <span style={{ fontSize: ".7rem", color: "var(--text2)" }}>{new Date(p.created_at).toLocaleString()}</span>
                  </div>
                  <textarea
                    value={draft.content}
                    onChange={(ev) => setDrafts((d) => ({ ...d, [p.id]: { ...draft, content: ev.target.value } }))}
                    aria-label="Contenido de la memoria propuesta"
                    rows={Math.min(6, Math.max(2, Math.ceil(draft.content.length / 80)))}
                    style={{ width: "100%", boxSizing: "border-box", padding: ".5rem", borderRadius: 6, border: "1px solid var(--border)", background: "var(--surface2)", color: "var(--text)", fontSize: ".85rem", lineHeight: 1.45, resize: "vertical" }}
                  />
                  {p.rationale && (
                    <div style={{ fontSize: ".72rem", color: "var(--text2)", marginTop: ".35rem", fontStyle: "italic" }}>Por qué: {p.rationale}</div>
                  )}
                  <div style={{ display: "flex", gap: ".5rem", marginTop: ".5rem", alignItems: "center", flexWrap: "wrap" }}>
                    <label style={{ fontSize: ".72rem", color: "var(--text2)", display: "flex", gap: ".3rem", alignItems: "center" }}>
                      Tipo:
                      <select
                        value={draft.kind}
                        onChange={(ev) => setDrafts((d) => ({ ...d, [p.id]: { ...draft, kind: ev.target.value } }))}
                        aria-label="Tipo de memoria"
                        style={{ fontSize: ".72rem", padding: ".15rem .3rem", borderRadius: 4, border: "1px solid var(--border)", background: "var(--surface2)", color: "var(--text)" }}
                      >
                        {KIND_OPTIONS.map((k) => <option key={k} value={k}>{k}</option>)}
                      </select>
                    </label>
                    <div style={{ flex: 1 }} />
                    <button
                      className="btn btn-primary"
                      style={{ fontSize: ".75rem" }}
                      onClick={() => decideProposal(p.id, draft.content !== p.content || draft.kind !== (p.kind ?? "episodic") ? "edit" : "accept")}
                    >Aceptar</button>
                    <button
                      className="btn btn-secondary"
                      style={{ fontSize: ".75rem" }}
                      onClick={() => decideProposal(p.id, "reject")}
                    >Descartar</button>
                  </div>
                </div>
              );
            })}
          </div>
        )}
      </>)}

      {/* 025 F1 — Lecciones aprendidas (gobierno de inyección: visible + reversible + dry-run) */}
      {activeTab === "lessons" && (<>
        <div style={{ padding: "1rem", background: "var(--surface)", borderRadius: 8, border: "1px solid var(--border)", marginBottom: "1rem" }}>
          <h4 style={{ marginTop: 0, marginBottom: ".5rem" }}>Lecciones aprendidas (procedurales)</h4>
          <p style={{ fontSize: ".8rem", color: "var(--text2)", marginTop: 0 }}>
            Lecciones aprobadas de patrones fallo→fix. Las activas se inyectan en el contexto del perfil (Claude)
            como un bloque delimitado, sin reemplazar tu system prompt. Acá ves exactamente qué se inyecta y podés
            activar, desactivar o borrar cada una. Por defecto está desactivado en Ajustes.
          </p>
          <div style={{ display: "flex", gap: ".5rem", alignItems: "center", marginTop: ".5rem" }}>
            <label style={{ fontSize: ".8rem", color: "var(--text2)" }}>Proyecto:</label>
            <input
              value={lessonsProject}
              onChange={(e) => setLessonsProject(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && loadLessons()}
              placeholder="__global__ o el project_key"
              style={{ flex: 1, padding: ".35rem", borderRadius: 6, border: "1px solid var(--border)", background: "var(--bg)", color: "var(--text)", fontSize: ".8rem" }}
              aria-label="Project key de las lecciones"
            />
            <button className="btn btn-secondary" style={{ fontSize: ".75rem" }} onClick={() => loadLessons()}>Cargar</button>
          </div>
          {autoProject && (
            <p style={{ fontSize: ".75rem", color: "var(--text2)", margin: ".35rem 0 0" }}>
              Proyecto del spawn actual (lo que se inyectaría):{" "}
              <code style={{ color: "var(--text)" }}>{autoProject === "__global__" ? "global" : autoProject}</code>
              {lessonsProject !== autoProject && (
                <button
                  className="btn btn-secondary"
                  style={{ fontSize: ".7rem", marginLeft: ".5rem" }}
                  onClick={() => { setLessonsProject(autoProject); loadLessons(autoProject); }}
                >Usar el actual</button>
              )}
            </p>
          )}
        </div>

        {/* Preview del bloque que se inyectaría (dry-run, byte a byte) */}
        <div style={{ marginBottom: "1rem" }}>
          <h5 style={{ margin: "0 0 .35rem" }}>Vista previa del bloque inyectado <span style={{ fontWeight: 400, color: "var(--text2)", fontSize: ".75rem" }}>(presupuesto: {lessonsView?.token_budget ?? 0} tokens)</span></h5>
          {lessonsView?.injected_block ? (
            <pre style={{ whiteSpace: "pre-wrap", fontSize: ".75rem", padding: ".75rem", background: "var(--bg)", border: "1px solid var(--border)", borderRadius: 6, color: "var(--text)", overflowX: "auto" }}>{lessonsView.injected_block}</pre>
          ) : (
            <p style={{ fontSize: ".8rem", color: "var(--text2)" }}>No hay lecciones activas para este proyecto: no se inyecta nada.</p>
          )}
        </div>

        {/* Lista de lecciones con toggle + borrar */}
        <div style={{ display: "flex", flexDirection: "column", gap: ".5rem" }}>
          {(!lessonsView || lessonsView.lessons.length === 0) ? (
            <p style={{ fontSize: ".85rem", color: "var(--text2)" }}>No hay lecciones aprobadas para este proyecto. Aprobá propuestas procedurales en la pestaña Propuestas.</p>
          ) : (
            lessonsView.lessons.map((l) => (
              <div key={l.entry_id} style={{ padding: ".75rem", background: "var(--surface)", borderRadius: 8, border: "1px solid var(--border)", opacity: l.active ? 1 : 0.55 }}>
                <div style={{ display: "flex", justifyContent: "space-between", alignItems: "flex-start", gap: ".5rem" }}>
                  <pre style={{ whiteSpace: "pre-wrap", fontSize: ".78rem", margin: 0, flex: 1, color: "var(--text)" }}>{l.content}</pre>
                  <div style={{ display: "flex", gap: ".35rem", flexShrink: 0 }}>
                    <button
                      className="btn btn-secondary"
                      style={{ fontSize: ".72rem" }}
                      onClick={() => toggleLesson(l.entry_id, l.project_key, !l.active)}
                      aria-label={l.active ? "Desactivar lección" : "Activar lección"}
                    >{l.active ? "Desactivar" : "Activar"}</button>
                    <button
                      className="btn btn-secondary"
                      style={{ fontSize: ".72rem", color: "var(--danger, #c0392b)" }}
                      onClick={() => deleteLesson(l.entry_id)}
                      aria-label="Borrar lección"
                    >Borrar</button>
                  </div>
                </div>
                <div style={{ fontSize: ".7rem", color: "var(--text2)", marginTop: ".35rem", display: "flex", alignItems: "center", gap: ".5rem", flexWrap: "wrap" }}>
                  <span>{l.active ? "Activa (se inyecta)" : "Inactiva (no se inyecta)"} · {l.created_at}</span>
                  {/* 050 FR-002 — feedback de utilidad (advisory): votar NO desactiva ni borra (decisión
                      humana con los botones de arriba); solo registra "¿fue útil?". */}
                  <span style={{ display: "inline-flex", alignItems: "center", gap: ".3rem", marginLeft: "auto" }}>
                    <span style={{ color: "var(--text2)" }}>¿Útil?</span>
                    <button
                      className="btn btn-secondary"
                      style={{ fontSize: ".7rem", padding: ".15rem .4rem", fontWeight: l.last_vote === "useful" ? 700 : 400 }}
                      onClick={() => rateLesson(l.entry_id, l.project_key, true)}
                      aria-pressed={l.last_vote === "useful"}
                      aria-label="Marcar la lección como útil"
                      title="Fue útil (advisory — no la activa ni desactiva)"
                    >👍 {l.useful_count}</button>
                    <button
                      className="btn btn-secondary"
                      style={{ fontSize: ".7rem", padding: ".15rem .4rem", fontWeight: l.last_vote === "not_useful" ? 700 : 400 }}
                      onClick={() => rateLesson(l.entry_id, l.project_key, false)}
                      aria-pressed={l.last_vote === "not_useful"}
                      aria-label="Marcar la lección como no útil"
                      title="No fue útil (advisory — no la activa ni desactiva)"
                    >👎 {l.not_useful_count}</button>
                  </span>
                </div>
              </div>
            ))
          )}
        </div>
      </>)}

      {/* UMP tab */}
      {activeTab === "ump" && (
        <div style={{ padding: "1rem", background: "var(--surface)", borderRadius: 8, border: "1px solid var(--border)" }}>
          <h4 style={{ marginBottom: ".75rem" }}>Universal Memory Protocol (UMP)</h4>
          <p style={{ color: "var(--text2)", fontSize: ".85rem", marginBottom: ".75rem" }}>JSON-RPC 2.0 at <code>localhost:43119/v1/ump</code>. AIMEAT-compatible. Methods: memory_store, memory_recall, memory_forget, memory_stats.</p>
          <div style={{ fontSize: ".8rem", color: "var(--text2)", padding: ".5rem", background: "var(--surface2)", borderRadius: 4 }}>
            <strong>OpenRPC:</strong> <code>GET /v1/ump/openrpc.json</code><br/>
            <strong>Store:</strong> <code>{'{"jsonrpc":"2.0","method":"memory_store","params":{"content":"...","source":"claude"},"id":1}'}</code><br/>
            <strong>Recall:</strong> <code>{'{"jsonrpc":"2.0","method":"memory_recall","params":{"query":"...","limit":10},"id":1}'}</code><br/>
            <strong>Compatible with:</strong> Claude Code, Codex CLI, Gemini CLI, Aider, any HTTP-capable agent.
          </div>
        </div>
      )}

      {/* LaunchAgent tab */}
      {activeTab === "launchagent" && (
        <div style={{ padding: "1rem", background: "var(--surface)", borderRadius: 8, border: "1px solid var(--border)" }}>
          <h4 style={{ marginBottom: ".75rem" }}>LaunchAgent — Background Service</h4>
          <p style={{ color: "var(--text2)", fontSize: ".85rem", marginBottom: ".75rem" }}>Run Memory Hub as system-level background service, available to ALL CLI tools at login, independent of Furx.</p>
          <div style={{ display: "flex", gap: ".5rem", marginBottom: ".75rem" }}>
            <button className="btn btn-primary" style={{ fontSize: ".8rem" }} onClick={async () => { await invoke("memory_launchagent_install"); setLaunchAgentRunning(await invoke<boolean>("memory_launchagent_status")); }}>Install</button>
            <button className="btn btn-secondary" style={{ fontSize: ".8rem" }} onClick={async () => { await invoke("memory_launchagent_uninstall"); setLaunchAgentRunning(false); }}>Uninstall</button>
            <button className="btn btn-secondary" style={{ fontSize: ".8rem" }} onClick={async () => setLaunchAgentRunning(await invoke<boolean>("memory_launchagent_status"))}>Status</button>
          </div>
          <div style={{ fontSize: ".85rem", color: launchAgentRunning ? "var(--green)" : "var(--text2)" }}>Status: {launchAgentRunning ? "● Hub running (port 43119)" : "○ Hub not responding"}</div>
          <div style={{ fontSize: ".75rem", color: "var(--text2)", marginTop: ".5rem" }}>El Hub corre dentro de Furx (in-process). El LaunchAgent solo hace falta para tenerlo disponible con Furx cerrado.</div>
          <div style={{ fontSize: ".75rem", color: "var(--text2)", marginTop: ".5rem" }}>Plist: <code>~/Library/LaunchAgents/cloud.furx.memory-daemon.plist</code></div>
        </div>
      )}

      {/* CLI Hooks tab */}
      {activeTab === "hooks" && (
        <div style={{ padding: "1rem", background: "var(--surface)", borderRadius: 8, border: "1px solid var(--border)" }}>
          <h4 style={{ marginBottom: ".75rem" }}>CLI Hooks Integration</h4>
          <p style={{ color: "var(--text2)", fontSize: ".85rem", marginBottom: ".75rem" }}>Generate slash commands + hooks for Claude Code and Codex CLI. Store/recall memories directly from any CLI via <code>/memory-store</code> or <code>/memory-recall</code>.</p>
          <button className="btn btn-primary" style={{ fontSize: ".8rem", marginBottom: ".75rem" }} onClick={async () => {
            try {
              const r = await invoke<MemoryCliHooks>("memory_generate_cli_hooks");
              setHooksGenerated(JSON.stringify(r, null, 2));
            } catch (e: unknown) { setError(e instanceof Error ? e.message : String(e)); }
          }}>Generate CLI Hooks</button>
          {hooksGenerated && <pre style={{ fontSize: ".75rem", padding: ".5rem", background: "var(--surface2)", borderRadius: 4, color: "var(--text2)", overflow: "auto" }}>{hooksGenerated}</pre>}
          <div style={{ fontSize: ".8rem", color: "var(--text2)", marginTop: ".5rem" }}>
            <strong>Claude Code:</strong> <code>~/.claude/commands/memory-*.md</code> + <code>~/.claude/hooks/memory-hooks.json</code><br/>
            <strong>Codex CLI:</strong> <code>~/.codex/commands/memory-*.md</code><br/>
            <strong>Usage:</strong> <code>/memory-store My important thought</code> or <code>/memory-recall project context</code>
          </div>
        </div>
      )}

      {/* Knowledge Graph tab */}
      {activeTab === "graph" && (
        <div style={{ padding: "1rem", background: "var(--surface)", borderRadius: 8, border: "1px solid var(--border)" }}>
          <h4 style={{ marginBottom: ".75rem" }}>Knowledge Graph — Cross-Source</h4>
          <p style={{ color: "var(--text2)", fontSize: ".85rem", marginBottom: ".75rem" }}>Entities and relations extracted from memories across Claude, Codex, Gemini, and Furx.</p>
          <button className="btn btn-primary" style={{ fontSize: ".8rem", marginBottom: ".75rem" }} onClick={async () => {
            try { setGraphEntities(await invoke<GraphEntity[]>("memory_graph_entities")); }
            catch (e: unknown) { setError(e instanceof Error ? e.message : String(e)); }
          }}>Load Entities</button>
          {graphEntities.length > 0 ? (
            <div style={{ display: "flex", flexDirection: "column", gap: ".25rem", maxHeight: 300, overflowY: "auto" }}>
              {graphEntities.map((e) => (
                <div key={e.id} style={{ padding: ".4rem .6rem", background: "var(--surface2)", borderRadius: 4, fontSize: ".8rem", display: "flex", justifyContent: "space-between" }}>
                  <span>{e.name}</span><span style={{ color: "var(--text2)" }}>{e.entity_type}</span>
                </div>
              ))}
            </div>
          ) : <p style={{ color: "var(--text2)", fontSize: ".85rem" }}>No entities yet. Store memories via UMP to auto-extract entities.</p>}
        </div>
      )}

      {/* 053 — Recall de agente (agent_memory_recall por project/path) */}
      {activeTab === "corpus" && (
        <div style={{ padding: ".5rem 0", display: "flex", flexDirection: "column", gap: "1rem" }}>
          {/* status banner */}
          {corpusStatus && !corpusStatus.available && corpusStatus.error_code === "not_installed" ? (
            <div style={{ padding: ".8rem", background: "var(--surface)", borderRadius: 6, fontSize: ".85rem", color: "var(--text2)" }}>
              corpus-engine no está instalado. Furx Memory analiza tus sesiones de Claude Code localmente.
              Instalalo en <code>~/corpus-engine</code> (o seteá <code>FURX_CORPUS_ENGINE_BIN</code>) y reabrí esta pestaña.
            </div>
          ) : corpusStatus?.data ? (
            <div style={{ fontSize: ".8rem", color: "var(--text2)" }}>
              {corpusStatus.data.sessions.toLocaleString()} sessions · {corpusStatus.data.messages.toLocaleString()} messages · local-first
            </div>
          ) : corpusStatus && corpusStatus.error_code ? (
            <div style={{ fontSize: ".8rem", color: "var(--warn, #c97)" }}>
              corpus-engine: {corpusStatus.error_code === "locked" ? "indexando, reintentá en un momento" : corpusStatus.error_code}
            </div>
          ) : null}

          {/* search */}
          <div style={{ display: "flex", gap: ".4rem" }}>
            <input value={corpusQuery} onChange={(e) => setCorpusQuery(e.target.value)}
              onKeyDown={(e) => { if (e.key === "Enter") runCorpusSearch(); }}
              placeholder="Search your past sessions…"
              style={{ flex: 1, padding: ".5rem", fontSize: ".85rem", background: "var(--surface)", border: "1px solid var(--border)", borderRadius: 6, color: "var(--text)" }} />
            <button className="btn btn-primary" style={{ fontSize: ".8rem" }} disabled={corpusBusy || !corpusQuery.trim()} onClick={runCorpusSearch}>{corpusBusy ? "…" : "Search"}</button>
          </div>
          {corpusHits?.data && (
            <div style={{ display: "flex", flexDirection: "column", gap: ".5rem" }}>
              {corpusHits.data.results.length === 0 ? (
                <div style={{ fontSize: ".8rem", color: "var(--text2)" }}>Sin resultados.</div>
              ) : corpusHits.data.results.map((h) => (
                <div key={h.uuid} style={{ padding: ".5rem .6rem", background: "var(--surface)", borderRadius: 6, fontSize: ".8rem" }}>
                  <div style={{ color: "var(--text2)", fontSize: ".72rem", marginBottom: ".25rem" }}>
                    {h.project} · {h.timestamp.slice(0, 10)} · {h.human ? "you" : "agent"}
                  </div>
                  <div style={{ fontFamily: "var(--mono)", color: "var(--text)" }}>{h.snippet}</div>
                </div>
              ))}
            </div>
          )}
          {corpusHits && !corpusHits.available && (
            <div style={{ fontSize: ".8rem", color: "var(--warn, #c97)" }}>Búsqueda no disponible: {corpusHits.error_code}</div>
          )}

          {/* dead-ends */}
          {corpusDead?.data && corpusDead.data.recurrent.length > 0 && (
            <div>
              <div style={{ fontSize: ".82rem", fontWeight: 600, marginBottom: ".4rem", color: "var(--text)" }}>Errores recurrentes — no repetir</div>
              <div style={{ display: "flex", flexDirection: "column", gap: ".3rem" }}>
                {corpusDead.data.recurrent.map((d, i) => (
                  <div key={i} style={{ display: "flex", gap: ".5rem", alignItems: "baseline", fontSize: ".78rem" }}>
                    <span style={{ color: "var(--accent)", fontVariantNumeric: "tabular-nums", minWidth: "3.5rem" }}>{d.count.toLocaleString()}×</span>
                    <span style={{ fontFamily: "var(--mono)", color: "var(--text2)" }}>{d.signature}</span>
                  </div>
                ))}
              </div>
            </div>
          )}
        </div>
      )}

      {activeTab === "agent_recall" && (
        <div style={{ padding: "1rem", background: "var(--surface)", borderRadius: 8, border: "1px solid var(--border)" }}>
          <h4 style={{ marginBottom: ".5rem" }}>Recall de agente</h4>
          <p style={{ color: "var(--text2)", fontSize: ".82rem", marginBottom: ".75rem", lineHeight: 1.5 }}>
            Recupera el contexto de memoria de un proyecto vía <code>mnemo recall</code> (o <code>memento ask</code> como fallback).
            Ingresá el nombre del proyecto o la ruta absoluta del repo.
          </p>
          <div style={{ display: "flex", gap: ".5rem", marginBottom: ".75rem" }}>
            <input
              value={agentRecallProject}
              onChange={(e) => setAgentRecallProject(e.target.value)}
              onKeyDown={(e) => {
                if (e.key !== "Enter" || agentRecallLoading) return;
                const project = agentRecallProject.trim();
                if (!project) return;
                setAgentRecallLoading(true);
                setAgentRecallError(null);
                setAgentRecallResult(null);
                invoke<AgentRecallResult>("agent_memory_recall", { project })
                  .then((r) => { setAgentRecallResult(r); })
                  .catch((err: unknown) => { setAgentRecallError(err instanceof Error ? err.message : String(err)); })
                  .finally(() => setAgentRecallLoading(false));
              }}
              placeholder="proyecto o /ruta/al/repo"
              aria-label="Proyecto para recall de agente"
              style={{
                flex: 1,
                padding: ".4rem .6rem",
                borderRadius: 6,
                border: "1px solid var(--border)",
                background: "var(--bg)",
                color: "var(--text)",
                fontSize: ".85rem",
                fontFamily: "var(--mono, monospace)",
              }}
            />
            <button
              className="btn btn-primary"
              style={{ fontSize: ".8rem" }}
              disabled={agentRecallLoading || !agentRecallProject.trim()}
              onClick={() => {
                const project = agentRecallProject.trim();
                if (!project || agentRecallLoading) return;
                setAgentRecallLoading(true);
                setAgentRecallError(null);
                setAgentRecallResult(null);
                invoke<AgentRecallResult>("agent_memory_recall", { project })
                  .then((r) => { setAgentRecallResult(r); })
                  .catch((err: unknown) => { setAgentRecallError(err instanceof Error ? err.message : String(err)); })
                  .finally(() => setAgentRecallLoading(false));
              }}
            >
              {agentRecallLoading ? "Buscando…" : "Recall"}
            </button>
          </div>

          {agentRecallError && (
            <div style={{ color: "var(--red, #c0392b)", fontSize: ".82rem", marginBottom: ".5rem" }}>
              {agentRecallError}
            </div>
          )}

          {agentRecallResult && (
            <div style={{ display: "flex", flexDirection: "column", gap: ".4rem" }}>
              <div style={{ display: "flex", gap: ".5rem", alignItems: "center", fontSize: ".75rem" }}>
                <span style={{ color: "var(--text2)" }}>Proyecto:</span>
                <code style={{ color: "var(--text)" }}>{agentRecallResult.project}</code>
                <span
                  className="sev-tag sev-info"
                  title="Fuente del recall"
                  style={{ fontSize: ".7rem" }}
                >
                  {agentRecallResult.source}
                </span>
              </div>
              {agentRecallResult.recalled ? (
                <pre
                  style={{
                    whiteSpace: "pre-wrap",
                    fontSize: ".78rem",
                    padding: ".75rem",
                    background: "var(--bg)",
                    border: "1px solid var(--border)",
                    borderRadius: 6,
                    color: "var(--text)",
                    overflowX: "auto",
                    maxHeight: 400,
                    overflowY: "auto",
                  }}
                >
                  {agentRecallResult.recalled}
                </pre>
              ) : (
                <p style={{ color: "var(--text2)", fontSize: ".82rem" }}>
                  Sin memorias para este proyecto (fuente: {agentRecallResult.source}).
                </p>
              )}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
