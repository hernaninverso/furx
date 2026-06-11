// 008 parallel-orchestration — "Orchestration Board": crear un batch de tareas, lanzarlas
// (cada una en su worktree con su agente), seguir su estado y revisar/mergear. Estética V3.
// El montaje en pane + entrega del objetivo lo maneja el Shell (onLaunch); el merge reusa
// MergeReview (onReview). Council 2026-05-29: completion = mark-ready explícito.
// 053 — cablear huérfanos: worktrees, agentes ACP, pipeline_cancel, dag_parse, list_panes.
import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "../lib/invoke"; // 015 T015: invoke con flujo de aprobación universal
import type { AgentProfile, OrchTask, OrchLogEntry, EtaEstimate } from "../types";
import { BestOfNCompare } from "./BestOfNCompare";
import { AgentTimeline } from "./AgentTimeline"; // 035 — timeline EN VIVO por agente (Golpe 2)
import { agentCategoryDisplay } from "../lib/metaSuggest"; // 020 US3 — sugerencia de categoría de agente (advisory)

// 053 — tipos locales para los comandos huérfanos cableados.
interface Worktree {
  repo_path: string;
  branch: string;
  worktree_path: string;
  created: boolean;
}

interface AcpAgentDef {
  id: string;
  name: string;
  bin: string;
  args: string[];
  env_extra: Record<string, string>;
  enabled: boolean;
  is_default: boolean;
}

interface DagNode {
  id: string;
  title: string;
  status: string;
  deps: string[];
}

interface DagResult {
  nodes: DagNode[];
  source: string;
  cycle: string[] | null;
}

interface PaneInfo {
  id: string;
  layout_pos: number;
  mode: string;
  cwd: string | null;
  title: string | null;
  state: string;
}

interface PaneStateReport {
  state: string;
  idle_seconds: number;
}

// 019 F3 (T030) — formatea segundos a un "~Nm Ss" legible para el ETA.
function fmtEta(secs: number): string {
  const s = Math.max(0, Math.round(secs));
  if (s < 60) return `~${s}s`;
  const m = Math.floor(s / 60);
  const r = s % 60;
  return r ? `~${m}m ${r}s` : `~${m}m`;
}

type Row = { title: string; objective: string; agent_profile_id: string };

const STATE_META: Record<string, { label: string; color: string }> = {
  pending: { label: "Pendiente", color: "var(--ink-dim, #6b6358)" },
  running: { label: "Corriendo", color: "var(--accent)" },
  awaiting_review: { label: "Para revisar", color: "var(--warn, #9a6011)" },
  done: { label: "Hecha", color: "var(--ok, #3a6b3f)" },
  failed: { label: "Falló", color: "var(--clay, #b8543a)" },
  canceled: { label: "Cancelada", color: "var(--ink-dim, #6b6358)" },
};
const ORDER = ["running", "awaiting_review", "pending", "failed", "done", "canceled"];

export function OrchestrationBoard({
  open, onClose, agents, onLaunch, onReview, onToast,
}: {
  open: boolean;
  onClose: () => void;
  agents: AgentProfile[];
  onLaunch: (taskId: string) => Promise<void>;
  onReview: (task: OrchTask) => void;
  onToast: (kind: "success" | "error" | "info", msg: string) => void;
}) {
  const [tasks, setTasks] = useState<OrchTask[]>([]);
  const [repoPath, setRepoPath] = useState("");
  const [batchTitle, setBatchTitle] = useState("");
  const [rows, setRows] = useState<Row[]>([{ title: "", objective: "", agent_profile_id: "" }]);
  const [busy, setBusy] = useState(false);
  const [diff, setDiff] = useState<{ id: string; text: string } | null>(null);
  // 014 — log-history abierto por tarea (detalle de la card).
  const [logHist, setLogHist] = useState<{ id: string; entries: OrchLogEntry[] } | null>(null);
  // 035 — timeline EN VIVO abierto por tarea (id de la tarea desplegada, o null).
  const [timelineFor, setTimelineFor] = useState<string | null>(null);
  // 014 — best-of-N: grupo abierto en la vista de comparación N-way.
  const [compareGroup, setCompareGroup] = useState<string | null>(null);
  // 014 — best-of-N: número de variantes a lanzar (<=4) para el objetivo de arriba.
  const [bestOfN, setBestOfN] = useState(0); // 0 = batch normal; 2-4 = best-of-N
  // 020 US3 — categoría sugerida por el AIE por fila (advisory). Keyed por índice de fila.
  const [agentHints, setAgentHints] = useState<Record<number, string>>({});
  const [dismissedHints, setDismissedHints] = useState<Record<number, boolean>>({});
  // 019 F3 (T030) — live-logs: tail vivo del scrollback abierto por tarea (no persistido).
  const [liveTail, setLiveTail] = useState<{ id: string; lines: string[] } | null>(null);
  // 019 F3 (T030) — ETA por batch (proyección de tiempo restante).
  const [etaByBatch, setEtaByBatch] = useState<Record<string, EtaEstimate | null>>({});
  // 038 F1.5 (FR-009) — runs `waiting_on_human` (run_id -> minutos esperando review).
  const [waitingByRun, setWaitingByRun] = useState<Record<string, number>>({});

  // 053 — estado de las secciones huérfanas cableadas.
  const [worktrees, setWorktrees] = useState<Worktree[]>([]);
  const [wtBranch, setWtBranch] = useState("");
  const [wtOpen, setWtOpen] = useState(false);
  const [acpAgents, setAcpAgents] = useState<AcpAgentDef[]>([]);
  const [acpOpen, setAcpOpen] = useState(false);
  const [acpNewName, setAcpNewName] = useState("");
  const [acpNewBin, setAcpNewBin] = useState("");
  const [dagResults, setDagResults] = useState<DagResult[]>([]);
  const [dagOpen, setDagOpen] = useState(false);
  const [dagBusy, setDagBusy] = useState(false);
  const [panes, setPanes] = useState<PaneInfo[]>([]);
  const [panesOpen, setPanesOpen] = useState(false);
  const [paneDetails, setPaneDetails] = useState<Record<string, PaneStateReport | null>>({});
  // pipeline YAML runner (panel inline)
  const [pipelineYaml, setPipelineYaml] = useState("");
  const [pipelineYamlOpen, setPipelineYamlOpen] = useState(false);
  const [pipelineYamlBusy, setPipelineYamlBusy] = useState(false);

  const reload = useCallback(() => {
    invoke<OrchTask[]>("orchestration_list", { batchId: null }).then(setTasks).catch(() => setTasks([]));
  }, []);

  // 053 — cargar worktrees para el repo actual cuando abre la sección.
  const reloadWorktrees = useCallback(() => {
    if (!repoPath.trim()) return;
    invoke<Worktree[]>("worktree_list", { repoPath: repoPath.trim() })
      .then(setWorktrees)
      .catch(() => setWorktrees([]));
  }, [repoPath]);

  // 053 — cargar agentes ACP.
  const reloadAcpAgents = useCallback(() => {
    invoke<AcpAgentDef[]>("acp_agents_list")
      .then(setAcpAgents)
      .catch(() => setAcpAgents([]));
  }, []);

  // 053 — cargar panes activos.
  const reloadPanes = useCallback(() => {
    invoke<PaneInfo[]>("list_panes")
      .then(setPanes)
      .catch(() => setPanes([]));
  }, []);

  // 038 — mapa id->título para mostrar las DEPS por TÍTULO en vez de uuids.
  const taskTitleById = useMemo(() => {
    const m: Record<string, string> = {};
    for (const t of tasks) m[t.id] = t.title;
    return m;
  }, [tasks]);

  useEffect(() => { if (open) { reload(); setDiff(null); } }, [open, reload]);
  // 012-pty-done-detection — refrescar mientras está abierto.
  useEffect(() => {
    if (!open) return;
    const id = setInterval(reload, 3000);
    return () => clearInterval(id);
  }, [open, reload]);

  // 038 F1.5 (FR-009) — refrescar los runs `waiting_on_human`.
  useEffect(() => {
    if (!open) return;
    const tick = () => {
      invoke<{ run_id: string; waiting_minutes: number }[]>("pipeline_waiting_runs")
        .then((rows) => {
          const m: Record<string, number> = {};
          for (const r of rows) m[r.run_id] = r.waiting_minutes;
          setWaitingByRun(m);
        })
        .catch(() => setWaitingByRun({}));
    };
    tick();
    const id = setInterval(tick, 3000);
    return () => clearInterval(id);
  }, [open]);

  // 019 F3 (T030) — ETA por batch.
  useEffect(() => {
    if (!open) return;
    const tick = async () => {
      const batchIds = Array.from(new Set(tasks.filter((t) => t.state === "running").map((t) => t.batch_id)));
      const next: Record<string, EtaEstimate | null> = {};
      for (const bid of batchIds) {
        try { next[bid] = await invoke<EtaEstimate | null>("orchestration_eta", { batchId: bid }); }
        catch { next[bid] = null; }
      }
      setEtaByBatch(next);
    };
    tick();
    const id = setInterval(tick, 5000);
    return () => clearInterval(id);
  }, [open, tasks]);

  // 020 US3 — sugerir la categoría de agente por fila (con debounce 600ms).
  const objectivesKey = rows.map((r) => r.objective.trim()).join(" ");
  useEffect(() => {
    if (!open) return;
    let off = false;
    const handle = setTimeout(async () => {
      const next: Record<number, string> = {};
      await Promise.all(rows.map(async (r, i) => {
        const obj = r.objective.trim();
        if (!obj) return;
        try {
          const cat = await invoke<string | null>("meta_suggest_agent", { objective: obj });
          const disp = agentCategoryDisplay(cat);
          if (disp) next[i] = disp;
        } catch { /* advisory: no rompe la UI */ }
      }));
      if (!off) { setAgentHints(next); setDismissedHints({}); }
    }, 600);
    return () => { off = true; clearTimeout(handle); };
  }, [open, objectivesKey]); // eslint-disable-line react-hooks/exhaustive-deps

  // 019 F3 (T030) — live-logs: refrescar el tail abierto cada 1.5s.
  useEffect(() => {
    if (!open || !liveTail) return;
    const t = tasks.find((x) => x.id === liveTail.id);
    if (!t || t.state !== "running") return;
    const id = setInterval(async () => {
      try {
        const lines = await invoke<string[]>("orchestration_tail_log", { taskId: liveTail.id });
        setLiveTail((cur) => (cur && cur.id === liveTail.id ? { id: cur.id, lines } : cur));
      } catch { /* la tarea pudo salir; el siguiente reload lo refleja */ }
    }, 1500);
    return () => clearInterval(id);
  }, [open, liveTail, tasks]);

  // 053 — recargar worktrees cuando cambia el repoPath y la sección está abierta.
  useEffect(() => {
    if (open && wtOpen) reloadWorktrees();
  }, [open, wtOpen, reloadWorktrees]);

  // 053 — recargar agentes ACP al abrir la sección.
  useEffect(() => {
    if (open && acpOpen) reloadAcpAgents();
  }, [open, acpOpen, reloadAcpAgents]);

  // 053 — obtener el estado detallado de un pane individual (pane_state).
  const fetchPaneDetail = async (paneId: string) => {
    try {
      const detail = await invoke<PaneStateReport | null>("pane_state", { paneId });
      setPaneDetails((d) => ({ ...d, [paneId]: detail }));
    } catch { /* best-effort */ }
  };

  // 053 — recargar panes al abrir la sección (y cada 3s mientras está abierta).
  useEffect(() => {
    if (!open || !panesOpen) return;
    reloadPanes();
    const id = setInterval(reloadPanes, 3000);
    return () => clearInterval(id);
  }, [open, panesOpen, reloadPanes]);

  const grouped = useMemo(() => {
    const g: Record<string, OrchTask[]> = {};
    for (const t of tasks) (g[t.state] ??= []).push(t);
    return g;
  }, [tasks]);

  if (!open) return null;

  const createBatch = async () => {
    const clean = rows.map((r) => ({ title: r.title.trim(), objective: r.objective.trim(), agent_profile_id: r.agent_profile_id || null }))
      .filter((r) => r.title);
    if (!repoPath.trim()) { onToast("error", "Indicá el repo (path absoluto)."); return; }
    if (clean.length === 0) { onToast("error", "Agregá al menos una tarea con título."); return; }
    setBusy(true);
    try {
      if (bestOfN >= 2) {
        // 014 FR-001 best-of-N: la PRIMERA tarea define el objetivo; se lanza como N variantes.
        const base = clean[0];
        const agts: (string | null)[] = [];
        for (let i = 0; i < bestOfN; i++) agts.push(clean[i]?.agent_profile_id ?? base.agent_profile_id ?? null);
        await invoke("orchestration_create_best_of_n", {
          title: batchTitle.trim() || base.title, repoPath: repoPath.trim(),
          baseBranch: null, baseCommit: null,
          objective: base.objective || base.title,
          agents: agts,
        });
        onToast("success", `Best-of-${bestOfN} creado: ${bestOfN} variantes del objetivo.`);
      } else {
        await invoke("orchestration_create_batch", {
          title: batchTitle.trim() || "batch", repoPath: repoPath.trim(),
          baseBranch: null, baseCommit: null,
          tasks: clean.map((r) => ({ title: r.title, objective: r.objective, agent_profile_id: r.agent_profile_id, mode: r.agent_profile_id ? null : "zsh" })),
        });
        onToast("success", `Batch creado con ${clean.length} tarea(s).`);
      }
      setRows([{ title: "", objective: "", agent_profile_id: "" }]); setBatchTitle(""); setBestOfN(0);
      reload();
    } catch (e) { onToast("error", `No se pudo crear el batch: ${String(e)}`); }
    finally { setBusy(false); }
  };

  // 014 FR-003 — abrir/cerrar el log-history persistido de una tarea (detalle).
  const toggleLog = async (t: OrchTask) => {
    if (logHist?.id === t.id) { setLogHist(null); return; }
    try {
      await invoke("orchestration_capture_log", { taskId: t.id }).catch(() => {});
      const entries = await invoke<OrchLogEntry[]>("orchestration_log_history", { taskId: t.id, limit: 30 });
      setLogHist({ id: t.id, entries });
    } catch (e) { onToast("error", String(e)); }
  };

  // 014 FR-002 — pairing-sync.
  const pairingSync = async (t: OrchTask) => {
    if (!window.confirm(`Traer "${t.branch}" a tu working copy local? Si hay cambios sin commitear se guardan en un stash (no se pierden).`)) return;
    try {
      const r = await invoke<{ message: string; stashed: boolean }>("orchestration_pairing_sync", { taskId: t.id, confirm: true });
      onToast(r.stashed ? "info" : "success", r.message);
    } catch (e) { onToast("error", `Pairing-sync falló: ${String(e)}`); }
  };

  // 014 FR-004 — GC de worktrees terminales.
  const cleanupWorktrees = async () => {
    if (!repoPath.trim()) { onToast("error", "Indicá el repo para limpiar sus worktrees."); return; }
    if (!window.confirm("Limpiar los worktrees de tareas terminadas (done/failed/canceled) de este repo? Las ramas/commits quedan; sólo se borra el FS del worktree.")) return;
    try {
      const r = await invoke<{ disabled: boolean; reason?: string; removed: string[] }>("orchestration_cleanup_worktrees", { repoPath: repoPath.trim(), confirm: true });
      if (r.disabled) onToast("info", r.reason ?? "Cleanup desactivado por escape-hatch.");
      else onToast("success", `${r.removed.length} worktree(s) limpiado(s).`);
    } catch (e) { onToast("error", `Cleanup falló: ${String(e)}`); }
  };

  const doLaunch = async (t: OrchTask) => {
    setBusy(true);
    try { await onLaunch(t.id); onToast("success", `Tarea "${t.title}" lanzada en un pane.`); reload(); }
    catch (e) { onToast("error", `No se pudo lanzar: ${String(e)}`); }
    finally { setBusy(false); }
  };
  const markReady = async (t: OrchTask) => {
    try { await invoke("orchestration_mark_ready", { taskId: t.id }); onToast("success", "Marcada para revisar."); reload(); }
    catch (e) { onToast("error", String(e)); }
  };
  const cancel = async (t: OrchTask) => {
    try { await invoke("orchestration_cancel", { taskId: t.id }); onToast("info", "Tarea cancelada."); reload(); }
    catch (e) { onToast("error", String(e)); }
  };
  const showDiff = async (t: OrchTask) => {
    try { const text = await invoke<string>("orchestration_collect", { taskId: t.id }); setDiff({ id: t.id, text }); }
    catch (e) { onToast("error", String(e)); }
  };
  // 019 F3 (T030) — pausar/reanudar un attempt.
  const togglePause = async (t: OrchTask) => {
    try {
      if (t.paused_at) {
        await invoke("orchestration_resume_task", { taskId: t.id });
        onToast("success", "Attempt reanudado.");
      } else {
        await invoke("orchestration_pause_task", { taskId: t.id });
        onToast("info", "Attempt pausado (el proceso quedó congelado, no se mató).");
      }
      reload();
    } catch (e) { onToast("error", String(e)); }
  };

  // 019 F3 (T030) — abrir/cerrar el tail VIVO del scrollback de una tarea.
  const toggleLiveTail = async (t: OrchTask) => {
    if (liveTail?.id === t.id) { setLiveTail(null); return; }
    try {
      const lines = await invoke<string[]>("orchestration_tail_log", { taskId: t.id });
      setLiveTail({ id: t.id, lines });
    } catch (e) { onToast("error", String(e)); }
  };

  // 012-pty-done-detection — toggle del auto-confirm OPT-IN por tarea.
  const toggleAutoConfirm = async (t: OrchTask) => {
    const next = !(t.auto_confirm);
    try {
      await invoke("orchestration_set_auto_confirm", { taskId: t.id, enabled: next });
      onToast("info", next ? "Auto-confirm ON para esta tarea." : "Auto-confirm OFF.");
      reload();
    } catch (e) { onToast("error", String(e)); }
  };

  // 053 — cancelar un pipeline run completo (huérfano pipeline_cancel).
  const cancelPipelineRun = async (runId: string) => {
    if (!window.confirm(`Cancelar el pipeline run ${runId}? Se cancelarán todas las tareas pending/running.`)) return;
    try {
      await invoke("pipeline_cancel", { runId });
      onToast("info", "Pipeline cancelado.");
      reload();
    } catch (e) { onToast("error", `No se pudo cancelar el pipeline: ${String(e)}`); }
  };

  // 053 — asegurar/crear un worktree.
  const ensureWorktree = async () => {
    if (!repoPath.trim()) { onToast("error", "Indicá el repo primero."); return; }
    if (!wtBranch.trim()) { onToast("error", "Indicá el nombre de branch."); return; }
    try {
      const wt = await invoke<Worktree>("worktree_ensure", { repoPath: repoPath.trim(), branch: wtBranch.trim() });
      onToast("success", `Worktree ${wt.created ? "creado" : "reutilizado"}: ${wt.worktree_path}`);
      setWtBranch("");
      reloadWorktrees();
    } catch (e) { onToast("error", `No se pudo crear el worktree: ${String(e)}`); }
  };

  // 053 — abrir terminal en un worktree (pty_spawn_in_worktree).
  const spawnTerminalInWorktree = async (wt: Worktree) => {
    try {
      const paneId = `wt-${Date.now()}`;
      await invoke<string>("pty_spawn_in_worktree", {
        paneId,
        repoPath: wt.repo_path,
        branch: wt.branch,
        mode: "zsh",
        rows: 24,
        cols: 80,
      });
      onToast("success", `Terminal abierta en ${wt.branch}`);
    } catch (e) { onToast("error", `No se pudo abrir terminal: ${String(e)}`); }
  };

  // 053 — eliminar un agente ACP.
  const deleteAcpAgent = async (agent: AcpAgentDef) => {
    if (agent.is_default) { onToast("error", "No se puede eliminar el agente default."); return; }
    if (!window.confirm(`Eliminar agente ACP "${agent.name}" (${agent.bin})?`)) return;
    try {
      await invoke("acp_agents_delete", { id: agent.id });
      onToast("info", `Agente "${agent.name}" eliminado.`);
      reloadAcpAgents();
    } catch (e) { onToast("error", `No se pudo eliminar: ${String(e)}`); }
  };

  // 053 — registrar/actualizar agente ACP (huérfano acp_agents_upsert).
  const upsertAcpAgent = async () => {
    if (!acpNewName.trim()) { onToast("error", "Nombre del agente es obligatorio."); return; }
    if (!acpNewBin.trim()) { onToast("error", "Binario del agente es obligatorio."); return; }
    // 053 fix — derivar un id (slug) válido del nombre. El backend exige [A-Za-z0-9_-]{1,48} y
    // reserva "default" (audit-3 Codex): truncamos a 48 y rechazamos el id reservado.
    const id = acpNewName.trim().toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 48).replace(/-+$/g, "");
    if (!id) { onToast("error", "El nombre debe tener al menos un carácter alfanumérico."); return; }
    if (id === "default") { onToast("error", `"${id}" es un id reservado — elegí otro nombre.`); return; }
    try {
      await invoke("acp_agents_upsert", {
        def: { id, name: acpNewName.trim(), bin: acpNewBin.trim(), args: [], env_extra: {}, enabled: true, is_default: false },
      });
      onToast("success", `Agente "${acpNewName}" registrado.`);
      setAcpNewName(""); setAcpNewBin("");
      reloadAcpAgents();
    } catch (e) { onToast("error", `No se pudo registrar: ${String(e)}`); }
  };

  // 053 — parsear DAG del repo.
  const parseDag = async () => {
    if (!repoPath.trim()) { onToast("error", "Indicá el repo primero."); return; }
    setDagBusy(true);
    try {
      const dags = await invoke<DagResult[]>("dag_parse", { repoPath: repoPath.trim() });
      setDagResults(dags);
      setDagOpen(true);
    } catch (e) { onToast("error", `DAG: ${String(e)}`); }
    finally { setDagBusy(false); }
  };

  // 053 — lanzar pipeline desde YAML.
  const runPipelineYaml = async () => {
    if (!pipelineYaml.trim()) { onToast("error", "Pegá el YAML del pipeline."); return; }
    if (!repoPath.trim()) { onToast("error", "Indicá el repo primero."); return; }
    setPipelineYamlBusy(true);
    try {
      const r = await invoke<{ run_id: string; batch_id: string }>("pipeline_run_yaml", {
        yaml: pipelineYaml.trim(),
        repoPath: repoPath.trim(),
        baseBranch: null,
        baseCommit: null,
      });
      onToast("success", `Pipeline creado: run_id=${r.run_id}`);
      setPipelineYaml("");
      setPipelineYamlOpen(false);
      reload();
    } catch (e) { onToast("error", `Pipeline YAML: ${String(e)}`); }
    finally { setPipelineYamlBusy(false); }
  };

  const lbl: React.CSSProperties = { fontFamily: "var(--mono)", fontSize: 11, letterSpacing: ".06em", textTransform: "uppercase", color: "var(--ink-dim, #6b6358)", display: "block", margin: "10px 0 3px" };
  const inp: React.CSSProperties = { width: "100%", background: "var(--bg, #faf7f0)", color: "var(--ink, #1c1814)", border: "1px solid var(--line, rgba(0,0,0,.15))", borderRadius: 6, padding: "7px 9px", fontFamily: "var(--body)", fontSize: 14 };
  const btn = (bg?: string): React.CSSProperties => ({ ...inp, width: "auto", cursor: "pointer", padding: "5px 11px", fontSize: 13, ...(bg ? { background: bg, color: "#fff", border: "none", fontWeight: 600 } : {}) });

  const agentName = (id?: string | null) => agents.find((a) => a.id === id)?.name;

  // 053 — color de estado de pane.
  const paneStateColor = (s: string) => s === "busy" ? "var(--accent)" : s === "idle" ? "var(--ok, #3a6b3f)" : "var(--ink-dim, #6b6358)";

  // 053 — sección colapsable reutilizable.
  const SectionHeader = ({ label, open: isOpen, onToggle, badge }: { label: string; open: boolean; onToggle: () => void; badge?: number }) => (
    <div onClick={onToggle} style={{ display: "flex", alignItems: "center", gap: 8, cursor: "pointer", userSelect: "none",
      fontFamily: "var(--display, serif)", fontSize: 16, fontWeight: 600, padding: "8px 0", borderTop: "1px solid var(--line, rgba(0,0,0,.1))", marginTop: 14 }}>
      <span style={{ fontFamily: "var(--mono)", fontSize: 12, color: "var(--ink-dim, #6b6358)" }}>{isOpen ? "▾" : "▸"}</span>
      {label}
      {badge !== undefined && badge > 0 && (
        <span style={{ fontFamily: "var(--mono)", fontSize: 10, background: "var(--accent)", color: "#fff", borderRadius: 10, padding: "1px 7px" }}>{badge}</span>
      )}
    </div>
  );

  return (
    <div role="dialog" aria-label="Orquestación de agentes" onClick={onClose}
      style={{ position: "fixed", inset: 0, background: "rgba(0,0,0,.45)", zIndex: 400, display: "flex", alignItems: "center", justifyContent: "center" }}>
      <div onClick={(e) => e.stopPropagation()}
        style={{ width: "min(960px,95vw)", maxHeight: "90vh", overflowY: "auto", padding: 22,
                 background: "var(--bg, #f3efe6)", color: "var(--ink, #1c1814)", border: "1px solid var(--line, rgba(0,0,0,.18))", borderRadius: 10, boxShadow: "0 20px 60px -20px rgba(0,0,0,.5)" }}>
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 6 }}>
          <div style={{ fontFamily: "var(--display, serif)", fontSize: 22, fontWeight: 600 }}>Orquestación</div>
          <button onClick={onClose} style={btn()}>×</button>
        </div>
        <p style={{ fontSize: 13, color: "var(--ink-dim, #6b6358)", marginTop: 0 }}>
          Lanzá N tareas en paralelo, cada una en su worktree aislado con su agente. El merge es siempre con tu confirmación.
        </p>

        {/* Crear batch */}
        <div style={{ border: "1px solid var(--line, rgba(0,0,0,.12))", borderRadius: 8, padding: 14, marginBottom: 18 }}>
          <div style={{ fontFamily: "var(--display, serif)", fontSize: 17, fontWeight: 600 }}>Nuevo batch</div>
          <div style={{ display: "flex", gap: 10 }}>
            <div style={{ flex: 2 }}><label style={lbl}>Repo (path absoluto)</label>
              <input style={inp} value={repoPath} onChange={(e) => setRepoPath(e.target.value)} placeholder="/path/to/your/repo" /></div>
            <div style={{ flex: 1 }}><label style={lbl}>Título (opcional)</label>
              <input style={inp} value={batchTitle} onChange={(e) => setBatchTitle(e.target.value)} placeholder="refactor X" /></div>
          </div>
          <label style={lbl}>Tareas</label>
          {rows.map((r, i) => (
            <div key={i} style={{ marginBottom: 6 }}>
              <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
                <input style={{ ...inp, flex: 1 }} value={r.title} placeholder="título" onChange={(e) => setRows((rs) => rs.map((x, j) => j === i ? { ...x, title: e.target.value } : x))} />
                <input style={{ ...inp, flex: 2 }} value={r.objective} placeholder="objetivo / prompt para el agente" onChange={(e) => setRows((rs) => rs.map((x, j) => j === i ? { ...x, objective: e.target.value } : x))} />
                <select style={{ ...inp, width: 180 }} value={r.agent_profile_id} onChange={(e) => setRows((rs) => rs.map((x, j) => j === i ? { ...x, agent_profile_id: e.target.value } : x))}>
                  <option value="">— agente —</option>
                  {agents.map((a) => <option key={a.id} value={a.id}>{a.name}</option>)}
                </select>
                {rows.length > 1 && <button style={btn()} onClick={() => setRows((rs) => rs.filter((_, j) => j !== i))}>−</button>}
              </div>
              {/* 020 US3 — hint de categoría sugerida por el AIE (advisory, descartable). */}
              {agentHints[i] && !dismissedHints[i] && (
                <div style={{ display: "flex", alignItems: "center", gap: 6, marginTop: 3, marginLeft: 2, fontSize: 12, color: "var(--ink-dim, #6b6358)" }}>
                  <span aria-hidden>✨</span>
                  <span>AIE sugiere: <strong style={{ color: "var(--accent)" }}>{agentHints[i]}</strong></span>
                  <button onClick={() => setDismissedHints((d) => ({ ...d, [i]: true }))}
                    title="Descartar sugerencia" aria-label="Descartar sugerencia"
                    style={{ ...btn(), padding: "1px 7px", fontSize: 12, lineHeight: 1.2 }}>×</button>
                </div>
              )}
            </div>
          ))}
          {/* 014 FR-001 — best-of-N + controles del batch */}
          <div style={{ display: "flex", gap: 8, marginTop: 8, alignItems: "center", flexWrap: "wrap" }}>
            <button style={btn()} onClick={() => setRows((rs) => [...rs, { title: "", objective: "", agent_profile_id: "" }])}>+ tarea</button>
            <label style={{ ...lbl, margin: 0, display: "flex", alignItems: "center", gap: 6 }} title="Lanzá el primer objetivo como N variantes (worktrees/ramas separadas) y comparalas para elegir la mejor.">
              best-of-N
              <select style={{ ...inp, width: 130 }} value={bestOfN} onChange={(e) => setBestOfN(Number(e.target.value))}>
                <option value={0}>off (batch normal)</option>
                <option value={2}>2 variantes</option>
                <option value={3}>3 variantes</option>
                <option value={4}>4 variantes</option>
              </select>
            </label>
            <button style={btn("var(--accent)")} disabled={busy} onClick={createBatch}>
              {bestOfN >= 2 ? `Lanzar best-of-${bestOfN}` : "Crear batch"}
            </button>
            <button style={btn()} disabled={busy} onClick={cleanupWorktrees} title="GC de worktrees de tareas terminadas (respeta DISABLE_WORKTREE_CLEANUP).">Limpiar worktrees</button>
            {/* 053 — DAG del repo */}
            <button style={btn()} disabled={dagBusy} onClick={parseDag} title="Parsear el grafo de dependencias de tasks del repo (spec-kit .specify/*.md).">
              {dagBusy ? "Leyendo DAG…" : "DAG"}
            </button>
            {/* 053 — lanzar pipeline desde YAML */}
            <button style={btn()} onClick={() => setPipelineYamlOpen((v) => !v)} title="Lanzar un pipeline desde YAML (038 Goose-C).">
              {pipelineYamlOpen ? "Cerrar YAML" : "Pipeline YAML"}
            </button>
          </div>

          {/* 053 — DAG inline (colapsable). */}
          {dagOpen && dagResults.length > 0 && (
            <div style={{ marginTop: 12, border: "1px solid var(--line, rgba(0,0,0,.12))", borderRadius: 6, padding: 10 }}>
              <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 6 }}>
                <span style={{ fontFamily: "var(--mono)", fontSize: 11, letterSpacing: ".06em", textTransform: "uppercase", color: "var(--ink-dim, #6b6358)" }}>
                  DAG · {dagResults.reduce((s, d) => s + d.nodes.length, 0)} nodos en {dagResults.length} spec(s)
                </span>
                <button style={{ ...btn(), padding: "2px 8px", fontSize: 12 }} onClick={() => setDagOpen(false)}>×</button>
              </div>
              {dagResults.map((dag, di) => (
                <div key={di} style={{ marginBottom: 10 }}>
                  <div style={{ fontFamily: "var(--mono)", fontSize: 11, color: "var(--accent)", marginBottom: 4 }}>{dag.source}</div>
                  {dag.cycle && (
                    <div style={{ fontSize: 12, color: "var(--clay, #b8543a)", marginBottom: 4 }}>
                      ⚠ Ciclo detectado: {dag.cycle.join(" → ")}
                    </div>
                  )}
                  {dag.nodes.length === 0 ? (
                    <div style={{ fontSize: 12, color: "var(--ink-dim, #6b6358)" }}>Sin nodos.</div>
                  ) : (
                    <div style={{ display: "flex", flexDirection: "column", gap: 3 }}>
                      {dag.nodes.map((n) => (
                        <div key={n.id} style={{ display: "flex", alignItems: "center", gap: 8, fontSize: 12, background: "var(--card, #fbf9f4)", borderRadius: 4, padding: "3px 8px" }}>
                          <span style={{ fontFamily: "var(--mono)", fontSize: 10, color: n.status === "done" ? "var(--ok, #3a6b3f)" : n.status === "pending" ? "var(--warn, #9a6011)" : "var(--ink-dim, #6b6358)", minWidth: 52 }}>{n.status}</span>
                          <span style={{ fontWeight: 500 }}>{n.title || n.id}</span>
                          {n.deps.length > 0 && (
                            <span style={{ color: "var(--ink-dim, #6b6358)", fontFamily: "var(--mono)", fontSize: 10 }}>← {n.deps.join(", ")}</span>
                          )}
                        </div>
                      ))}
                    </div>
                  )}
                </div>
              ))}
            </div>
          )}
          {dagOpen && dagResults.length === 0 && (
            <div style={{ marginTop: 8, fontSize: 13, color: "var(--ink-dim, #6b6358)" }}>No se encontró ningún DAG en el repo (¿existe .specify/?)</div>
          )}

          {/* 053 — Pipeline YAML inline. */}
          {pipelineYamlOpen && (
            <div style={{ marginTop: 10, border: "1px solid var(--line, rgba(0,0,0,.12))", borderRadius: 6, padding: 10 }}>
              <label style={lbl}>YAML del pipeline</label>
              <textarea
                style={{ ...inp, fontFamily: "var(--mono)", fontSize: 12, minHeight: 120, resize: "vertical" }}
                value={pipelineYaml}
                onChange={(e) => setPipelineYaml(e.target.value)}
                placeholder={"name: mi-pipeline\nrepo: /path/to/your/repo\ntasks:\n  - id: t1\n    title: Tarea 1\n    objective: Hacer X\n  - id: t2\n    title: Tarea 2\n    depends_on: [t1]"}
              />
              <div style={{ marginTop: 6, display: "flex", gap: 8 }}>
                <button style={btn("var(--accent)")} disabled={pipelineYamlBusy} onClick={runPipelineYaml}>
                  {pipelineYamlBusy ? "Creando…" : "Crear pipeline"}
                </button>
                <button style={btn()} onClick={() => { setPipelineYamlOpen(false); setPipelineYaml(""); }}>Cancelar</button>
              </div>
            </div>
          )}
        </div>

        {/* Tablero por estado */}
        {tasks.length === 0 ? (
          <p style={{ color: "var(--ink-dim, #6b6358)", fontSize: 14 }}>No hay tareas todavía. Creá un batch arriba.</p>
        ) : ORDER.filter((s) => grouped[s]?.length).map((s) => (
          <div key={s} style={{ marginBottom: 14 }}>
            <div style={{ fontFamily: "var(--mono)", fontSize: 11, letterSpacing: ".08em", textTransform: "uppercase", color: STATE_META[s].color, marginBottom: 6 }}>
              {STATE_META[s].label} · {grouped[s].length}
              {/* 019 F3 (T030) — ETA agregado de los batches con tareas corriendo. */}
              {s === "running" && (() => {
                const etas = Array.from(new Set(grouped[s].map((t) => t.batch_id)))
                  .map((bid) => etaByBatch[bid]).filter((e): e is EtaEstimate => !!e);
                if (etas.length === 0) return null;
                const maxEta = Math.max(...etas.map((e) => e.eta_secs));
                return <span style={{ marginLeft: 8, textTransform: "none", letterSpacing: 0, color: "var(--accent)" }} title="Estimación basada en la duración de los attempts ya terminados.">ETA {fmtEta(maxEta)}</span>;
              })()}
            </div>
            {grouped[s].map((t) => (
              <div key={t.id} style={{ border: "1px solid var(--line, rgba(0,0,0,.12))", borderLeft: `3px solid ${STATE_META[t.state].color}`, borderRadius: 6, padding: "9px 12px", marginBottom: 6 }}>
                <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", gap: 10 }}>
                  <div style={{ minWidth: 0 }}>
                    <div style={{ fontSize: 14, fontWeight: 500, display: "flex", alignItems: "center", gap: 6 }}>
                      {t.title}
                      {t.group_id ? (
                        <span title="Variante de un objetivo best-of-N"
                          style={{ fontFamily: "var(--mono)", fontSize: 10, letterSpacing: ".05em", textTransform: "uppercase", color: "var(--accent)", border: "1px solid var(--accent)", borderRadius: 4, padding: "1px 6px" }}>
                          best-of-N · v{(t.variant_index ?? 0) + 1}
                        </span>
                      ) : null}
                      {/* 038 F1.5 (FR-009) — advisory "esperando tu review". */}
                      {t.state === "awaiting_review" && t.pipeline_run_id && t.pipeline_run_id in waitingByRun ? (
                        <span title="El pipeline está esperando que revises/apruebes esta etapa para avanzar a la siguiente. No está colgado."
                          style={{ fontFamily: "var(--mono)", fontSize: 10, letterSpacing: ".05em", textTransform: "uppercase", color: "#fff", background: "var(--warn, #9a6011)", borderRadius: 4, padding: "1px 6px" }}>
                          ⏳ esperando tu review{waitingByRun[t.pipeline_run_id] > 0 ? ` hace ${waitingByRun[t.pipeline_run_id]}m` : ""}
                        </span>
                      ) : null}
                      {t.state === "running" && t.paused_at ? (
                        <span title="Attempt pausado (SIGSTOP): el proceso está congelado, no muerto. Reanudá para continuar."
                          style={{ fontFamily: "var(--mono)", fontSize: 10, letterSpacing: ".05em", textTransform: "uppercase", color: "#fff", background: "var(--ink-dim, #6b6358)", borderRadius: 4, padding: "1px 6px" }}>
                          ⏸ pausado
                        </span>
                      ) : null}
                      {t.state === "running" && t.needs_input ? (
                        <span title="El agente está esperando tu confirmación a un prompt de permiso"
                          style={{ fontFamily: "var(--mono)", fontSize: 10, letterSpacing: ".05em", textTransform: "uppercase", color: "#fff", background: "var(--warn, #9a6011)", borderRadius: 4, padding: "1px 6px" }}>
                          ⚠ necesita input
                        </span>
                      ) : null}
                      {t.state === "running" && t.auto_confirm ? (
                        <span title="Auto-confirm ON: Furx auto-presiona Enter ante trust prompts conocidos (con tope/min)"
                          style={{ fontFamily: "var(--mono)", fontSize: 10, letterSpacing: ".05em", textTransform: "uppercase", color: "var(--accent)", border: "1px solid var(--accent)", borderRadius: 4, padding: "1px 6px" }}>
                          auto-confirm
                        </span>
                      ) : null}
                    </div>
                    <div style={{ fontFamily: "var(--mono)", fontSize: 11, color: "var(--ink-dim, #6b6358)", whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>
                      {agentName(t.agent_profile_id) ?? t.mode ?? "?"} · {t.branch}
                    </div>
                  </div>
                  <div style={{ display: "flex", gap: 6, flexShrink: 0, alignItems: "center", flexWrap: "wrap" }}>
                    {/* 038 F1.5 — tarea de pipeline BLOQUEADA por deps. */}
                    {t.state === "pending" && (t.dag_blocked ? (
                      <span title="Esta etapa del pipeline espera a que terminen (done) sus dependencias."
                        style={{ fontSize: 12, color: "var(--ink-dim, #6b6358)", fontStyle: "italic", maxWidth: 260, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                        esperando: {(t.depends_on ?? []).map((d) => taskTitleById[d] ?? d).join(", ") || "dependencias"}
                      </span>
                    ) : (
                      <button style={btn("var(--accent)")} disabled={busy} onClick={() => doLaunch(t)}>Lanzar</button>
                    ))}
                    {t.state === "running" && (
                      <button style={btn()} title="Auto-confirm es opt-in y sólo confirma trust prompts conocidos, con tope por minuto."
                        onClick={() => toggleAutoConfirm(t)}>
                        {t.auto_confirm ? "Auto-confirm: ON" : "Auto-confirm: OFF"}
                      </button>
                    )}
                    {t.state === "running" && (
                      <button style={btn()} title="Pausar (SIGSTOP) o reanudar (SIGCONT) el proceso. NO lo mata — queda congelado y se reanuda intacto." onClick={() => togglePause(t)}>
                        {t.paused_at ? "Reanudar" : "Pausar"}
                      </button>
                    )}
                    {t.state === "running" && <button style={btn()} title="Tail en vivo del output del agente (se actualiza cada 1.5s)" onClick={() => toggleLiveTail(t)}>{liveTail?.id === t.id ? "Ocultar live" : "Live-log"}</button>}
                    {t.state === "running" && <button style={btn()} onClick={() => markReady(t)}>Listo p/review</button>}
                    {t.group_id && <button style={btn()} title="Comparar las N variantes de este objetivo lado a lado" onClick={() => setCompareGroup(t.group_id!)}>Comparar variantes</button>}
                    {(t.state === "running" || t.state === "awaiting_review" || t.state === "done") && <button style={btn()} onClick={() => showDiff(t)}>Ver diff</button>}
                    {(t.state === "running" || t.state === "awaiting_review" || t.state === "done" || t.state === "failed") && <button style={btn()} title="Historial de comandos/output del agente (no sólo el diff)" onClick={() => toggleLog(t)}>{logHist?.id === t.id ? "Ocultar log" : "Log-history"}</button>}
                    {(t.state === "running" || t.state === "pending" || t.state === "awaiting_review" || t.state === "done" || t.state === "failed") && <button style={btn()} title="Línea de tiempo EN VIVO del agente (lo que está haciendo AHORA, con modo cine)" onClick={() => setTimelineFor((cur) => cur === t.id ? null : t.id)}>{timelineFor === t.id ? "Ocultar timeline" : "Timeline"}</button>}
                    {t.state === "awaiting_review" && <button style={btn()} title="Traer este branch a tu working copy local (con stash-guard, no pisa cambios)" onClick={() => pairingSync(t)}>Traer a local</button>}
                    {t.state === "awaiting_review" && <button style={btn("var(--accent)")} onClick={() => onReview(t)}>Revisar/merge</button>}
                    {(t.state === "running" || t.state === "pending") && <button style={{ ...btn(), color: "var(--clay, #b8543a)" }} onClick={() => cancel(t)}>Cancelar</button>}
                    {/* 053 — Cancelar pipeline run completo (si la tarea forma parte de un pipeline). */}
                    {t.pipeline_run_id && (t.state === "running" || t.state === "pending") && (
                      <button
                        style={{ ...btn(), color: "var(--clay, #b8543a)", border: "1px solid var(--clay, #b8543a)" }}
                        title="Cancelar todo el pipeline run (todas las tareas pending/running)"
                        onClick={() => cancelPipelineRun(t.pipeline_run_id!)}
                      >
                        Cancelar pipeline
                      </button>
                    )}
                  </div>
                </div>
                {t.objective && <div style={{ fontSize: 12, color: "#3a342c", marginTop: 4 }}>{t.objective}</div>}
                {diff?.id === t.id && (
                  <pre style={{ background: "var(--card, #fbf9f4)", border: "1px solid var(--line)", borderRadius: 6, padding: 10, fontSize: 12, fontFamily: "var(--mono)", overflowX: "auto", marginTop: 6 }}>{diff.text}</pre>
                )}
                {liveTail?.id === t.id && (
                  <div style={{ marginTop: 6 }}>
                    <div style={{ fontFamily: "var(--mono)", fontSize: 10, letterSpacing: ".06em", textTransform: "uppercase", color: "var(--accent)", marginBottom: 4 }}>
                      Live-log · en vivo {t.paused_at ? "(pausado)" : ""}
                    </div>
                    {liveTail.lines.length === 0 ? (
                      <div style={{ fontSize: 12, color: "var(--ink-dim, #6b6358)" }}>Sin output todavía.</div>
                    ) : (
                      <pre style={{ background: "var(--card, #fbf9f4)", border: "1px solid var(--line)", borderRadius: 6, padding: 8, fontSize: 11, fontFamily: "var(--mono)", overflowX: "auto", margin: 0, maxHeight: 240 }}>{liveTail.lines.join("\n")}</pre>
                    )}
                  </div>
                )}
                {/* 035 — timeline EN VIVO inline (Golpe 2). */}
                {timelineFor === t.id && <AgentTimeline task={t} />}
                {logHist?.id === t.id && (
                  <div style={{ marginTop: 6 }}>
                    <div style={{ fontFamily: "var(--mono)", fontSize: 10, letterSpacing: ".06em", textTransform: "uppercase", color: "var(--ink-dim, #6b6358)", marginBottom: 4 }}>
                      Log-history · {logHist.entries.length} snapshot(s)
                    </div>
                    {logHist.entries.length === 0 ? (
                      <div style={{ fontSize: 12, color: "var(--ink-dim, #6b6358)" }}>Todavía no hay historial para esta tarea.</div>
                    ) : (
                      <div style={{ maxHeight: 260, overflowY: "auto", display: "flex", flexDirection: "column", gap: 6 }}>
                        {logHist.entries.map((e) => (
                          <div key={e.id}>
                            <div style={{ fontFamily: "var(--mono)", fontSize: 10, color: "var(--ink-dim, #6b6358)" }}>{e.captured_at} · {e.source}</div>
                            <pre style={{ background: "var(--card, #fbf9f4)", border: "1px solid var(--line)", borderRadius: 6, padding: 8, fontSize: 11, fontFamily: "var(--mono)", overflowX: "auto", margin: "2px 0 0", maxHeight: 160 }}>{e.content}</pre>
                          </div>
                        ))}
                      </div>
                    )}
                  </div>
                )}
              </div>
            ))}
          </div>
        ))}

        {/* ── 053 — Sección Worktrees ───────────────────────────────────────────── */}
        <SectionHeader
          label="Worktrees"
          open={wtOpen}
          onToggle={() => { setWtOpen((v) => !v); if (!wtOpen) reloadWorktrees(); }}
          badge={worktrees.length || undefined}
        />
        {wtOpen && (
          <div style={{ marginBottom: 10 }}>
            {/* Crear worktree */}
            <div style={{ display: "flex", gap: 8, marginBottom: 10, alignItems: "flex-end" }}>
              <div style={{ flex: 1 }}>
                <label style={lbl}>Branch (nuevo o existente)</label>
                <input style={inp} value={wtBranch} onChange={(e) => setWtBranch(e.target.value)} placeholder="feature/mi-cambio" />
              </div>
              <button style={btn("var(--accent)")} onClick={ensureWorktree} disabled={!repoPath.trim() || !wtBranch.trim()}>
                Asegurar worktree
              </button>
              <button style={btn()} onClick={reloadWorktrees} title="Recargar lista de worktrees">↺</button>
            </div>
            {/* Lista de worktrees */}
            {worktrees.length === 0 ? (
              <div style={{ fontSize: 13, color: "var(--ink-dim, #6b6358)" }}>
                {repoPath.trim() ? "No hay worktrees activos en este repo." : "Indicá el repo para ver sus worktrees."}
              </div>
            ) : (
              <div style={{ display: "flex", flexDirection: "column", gap: 5 }}>
                {worktrees.map((wt, i) => (
                  <div key={i} style={{ border: "1px solid var(--line, rgba(0,0,0,.12))", borderRadius: 6, padding: "7px 10px", display: "flex", justifyContent: "space-between", alignItems: "center", gap: 10 }}>
                    <div style={{ minWidth: 0 }}>
                      <div style={{ fontSize: 13, fontWeight: 500, fontFamily: "var(--mono)" }}>{wt.branch}</div>
                      <div style={{ fontSize: 11, color: "var(--ink-dim, #6b6358)", whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>{wt.worktree_path}</div>
                    </div>
                    <div style={{ display: "flex", gap: 6, flexShrink: 0 }}>
                      <button
                        style={btn()}
                        title={`Abrir una terminal zsh en el worktree de "${wt.branch}"`}
                        onClick={() => spawnTerminalInWorktree(wt)}
                      >
                        Terminal
                      </button>
                    </div>
                  </div>
                ))}
              </div>
            )}
          </div>
        )}

        {/* ── 053 — Sección Agentes ACP ──────────────────────────────────────────── */}
        <SectionHeader
          label="Agentes ACP"
          open={acpOpen}
          onToggle={() => { setAcpOpen((v) => !v); if (!acpOpen) reloadAcpAgents(); }}
          badge={acpAgents.length || undefined}
        />
        {acpOpen && (
          <div style={{ marginBottom: 10 }}>
            {acpAgents.length === 0 ? (
              <div style={{ fontSize: 13, color: "var(--ink-dim, #6b6358)" }}>No hay agentes ACP registrados.</div>
            ) : (
              <div style={{ display: "flex", flexDirection: "column", gap: 5 }}>
                {acpAgents.map((a) => (
                  <div key={a.id} style={{ border: "1px solid var(--line, rgba(0,0,0,.12))", borderLeft: `3px solid ${a.enabled ? "var(--ok, #3a6b3f)" : "var(--ink-dim, #6b6358)"}`, borderRadius: 6, padding: "7px 10px", display: "flex", justifyContent: "space-between", alignItems: "center", gap: 10 }}>
                    <div style={{ minWidth: 0 }}>
                      <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
                        <span style={{ fontSize: 13, fontWeight: 500 }}>{a.name}</span>
                        {a.is_default && (
                          <span style={{ fontFamily: "var(--mono)", fontSize: 10, color: "var(--accent)", border: "1px solid var(--accent)", borderRadius: 4, padding: "1px 5px", textTransform: "uppercase", letterSpacing: ".05em" }}>default</span>
                        )}
                        {!a.enabled && (
                          <span style={{ fontFamily: "var(--mono)", fontSize: 10, color: "var(--ink-dim, #6b6358)", border: "1px solid var(--line)", borderRadius: 4, padding: "1px 5px", textTransform: "uppercase", letterSpacing: ".05em" }}>deshabilitado</span>
                        )}
                      </div>
                      <div style={{ fontFamily: "var(--mono)", fontSize: 11, color: "var(--ink-dim, #6b6358)", marginTop: 1 }}>
                        {a.bin}{a.args.length > 0 ? " " + a.args.join(" ") : ""}
                      </div>
                    </div>
                    <div style={{ display: "flex", gap: 6, flexShrink: 0 }}>
                      {!a.is_default && (
                        <button
                          style={{ ...btn(), color: "var(--clay, #b8543a)" }}
                          title={`Eliminar agente ACP "${a.name}"`}
                          onClick={() => deleteAcpAgent(a)}
                        >
                          Eliminar
                        </button>
                      )}
                    </div>
                  </div>
                ))}
              </div>
            )}
            {/* 053 — upsert form para registrar un nuevo agente ACP. */}
            <div style={{ marginTop: 10, border: "1px dashed var(--line, rgba(0,0,0,.15))", borderRadius: 6, padding: 10 }}>
              <div style={{ fontFamily: "var(--mono)", fontSize: 11, letterSpacing: ".06em", textTransform: "uppercase", color: "var(--ink-dim, #6b6358)", marginBottom: 6 }}>Agregar agente ACP</div>
              <div style={{ display: "flex", gap: 8, alignItems: "flex-end" }}>
                <div style={{ flex: 1 }}>
                  <label style={lbl}>Nombre</label>
                  <input style={inp} value={acpNewName} onChange={(e) => setAcpNewName(e.target.value)} placeholder="mi-agente" />
                </div>
                <div style={{ flex: 2 }}>
                  <label style={lbl}>Binario (PATH)</label>
                  <input style={inp} value={acpNewBin} onChange={(e) => setAcpNewBin(e.target.value)} placeholder="my-acp-agent (nombre en PATH, no ruta absoluta)" />
                </div>
                <button style={btn("var(--accent)")} onClick={upsertAcpAgent} disabled={!acpNewName.trim() || !acpNewBin.trim()}>
                  Registrar
                </button>
              </div>
              <div style={{ fontSize: 11, color: "var(--ink-dim, #6b6358)", marginTop: 5 }}>
                Pasa por aprobación de gating (acp_agents_upsert tiene requires_confirmation).
              </div>
            </div>
          </div>
        )}

        {/* ── 053 — Sección Panes activos ────────────────────────────────────────── */}
        <SectionHeader
          label="Panes activos"
          open={panesOpen}
          onToggle={() => { setPanesOpen((v) => !v); if (!panesOpen) reloadPanes(); }}
          badge={panes.length || undefined}
        />
        {panesOpen && (
          <div style={{ marginBottom: 10 }}>
            <div style={{ display: "flex", justifyContent: "flex-end", marginBottom: 6 }}>
              <button style={btn()} onClick={reloadPanes} title="Recargar lista de panes">↺ Actualizar</button>
            </div>
            {panes.length === 0 ? (
              <div style={{ fontSize: 13, color: "var(--ink-dim, #6b6358)" }}>No hay panes activos.</div>
            ) : (
              <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
                {panes.map((p) => (
                  <div key={p.id}>
                    <div style={{ border: "1px solid var(--line, rgba(0,0,0,.12))", borderLeft: `3px solid ${paneStateColor(p.state)}`, borderRadius: 6, padding: "6px 10px", display: "flex", alignItems: "center", gap: 10 }}>
                      <span style={{ fontFamily: "var(--mono)", fontSize: 10, textTransform: "uppercase", letterSpacing: ".05em", color: paneStateColor(p.state), minWidth: 40 }}>{p.state}</span>
                      <span style={{ fontFamily: "var(--mono)", fontSize: 11, color: "var(--accent)", minWidth: 60, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{p.mode}</span>
                      <span style={{ fontSize: 12, flex: 1, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", color: "var(--ink-dim, #6b6358)" }}>{p.title ?? p.cwd ?? p.id}</span>
                      <span style={{ fontFamily: "var(--mono)", fontSize: 10, color: "var(--ink-dim, #6b6358)", flexShrink: 0 }}>pos {p.layout_pos}</span>
                      <button
                        style={{ ...btn(), padding: "2px 8px", fontSize: 11 }}
                        title="Obtener estado detallado del pane (pane_state)"
                        onClick={() => fetchPaneDetail(p.id)}
                      >Estado</button>
                    </div>
                    {paneDetails[p.id] !== undefined && (
                      <div style={{ fontFamily: "var(--mono)", fontSize: 11, color: "var(--ink-dim, #6b6358)", paddingLeft: 10, marginTop: 2 }}>
                        {paneDetails[p.id] === null
                          ? "pane no encontrado"
                          : `state: ${paneDetails[p.id]!.state}  ·  idle: ${paneDetails[p.id]!.idle_seconds}s`}
                      </div>
                    )}
                  </div>
                ))}
              </div>
            )}
          </div>
        )}
      </div>

      {/* 014 FR-001 — comparación N-way de un grupo best-of-N (modal aparte). */}
      {compareGroup && (
        <BestOfNCompare
          groupId={compareGroup}
          onClose={() => { setCompareGroup(null); reload(); }}
          onReview={onReview}
          onToast={onToast}
        />
      )}
    </div>
  );
}
