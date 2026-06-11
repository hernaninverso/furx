// web/src/views/QueueView.tsx — 058 · "Cola" rediseñada (design system Atelier Terminal).
// Diseño del consejo (codex 0.96 + gemini 0.97): tabla EDITORIAL expandible (no cards por job) — mejor
// para escanear con 8 workers / poll 3s. Estado = dot de color (pending dim · running coral pulso · done
// verde · error terracota · canceled gris). Form de encolar en card COLAPSABLE bajo el header. Stat tiles
// (en curso / fallidos). Empty state diseñado. La fila expande para ver output/error (y cancelar pending).
import { useRef, useState } from "react";
import { ListChecks, ChevronRight } from "lucide-react";
import { invoke } from "../lib/invoke";
import { usePolling } from "../hooks/usePolling";

interface BgJob {
  id: string; kind: string; args_json: string; status: string;
  created_at: string; started_at: string | null; finished_at: string | null;
  output: string | null; error: string | null;
}

const BG_KINDS = ["distill", "auto-eval", "council3", "noop", "otro"] as const;
const STATUS_CLASS = (s: string) =>
  s === "running" || s === "done" || s === "error" || s === "canceled" || s === "pending" ? s : "pending";

function ageOf(j: BgJob): string {
  if (j.status === "running" && j.started_at) {
    const sec = Math.max(0, Math.round((Date.now() - new Date(j.started_at).getTime()) / 1000));
    return `${sec}s en curso`;
  }
  if (j.finished_at && j.started_at) {
    return `${Math.round((new Date(j.finished_at).getTime() - new Date(j.started_at).getTime()) / 1000)}s`;
  }
  try {
    const sec = Math.max(0, Math.round((Date.now() - new Date(j.created_at).getTime()) / 1000));
    if (sec < 60) return `hace ${sec}s`;
    if (sec < 3600) return `hace ${Math.round(sec / 60)}m`;
    return `hace ${Math.round(sec / 3600)}h`;
  } catch {
    return "—";
  }
}

function argsSummary(j: BgJob): string {
  try {
    const o = JSON.parse(j.args_json);
    if (o && typeof o === "object" && !Array.isArray(o)) {
      const keys = Object.keys(o);
      if (keys.length === 0) return "";
      // 058 (ultrareview fix): valores objeto/array → JSON.stringify (antes `String(v)` daba
      // "[object Object]" / "1,2" — sumario ilegible). Primitivos siguen con String().
      return keys.map((k) => {
        const v = (o as Record<string, unknown>)[k];
        const s = v !== null && typeof v === "object" ? JSON.stringify(v) : String(v);
        return `${k}=${s.slice(0, 24)}`;
      }).join(" · ");
    }
  } catch {
    /* raw */
  }
  return String(j.args_json ?? "").slice(0, 60);
}

export function QueueView() {
  const [jobs, setJobs] = useState<BgJob[]>([]);
  const [expanded, setExpanded] = useState<string | null>(null);
  // 058 (audit): guard de generación — el poll de 3s + refreshes manuales pueden resolver fuera de
  // orden y restaurar jobs obsoletos. Sólo el refresh más reciente escribe.
  const refreshSeq = useRef(0);
  const refresh = async () => {
    const my = ++refreshSeq.current;
    try {
      const r = await invoke<BgJob[]>("bg_list", { limit: 50 });
      if (my !== refreshSeq.current) return;
      setJobs(Array.isArray(r) ? r : []);
    } catch {
      /* sin datos — poll silencioso */
    }
  };
  // Poll cada 3s; `refresh` mantiene su propio `refreshSeq` porque también lo invocan acciones manuales
  // (cancel/enqueue) que pueden resolver fuera de orden respecto del poll — eso lo serializa el seq, no
  // el `inFlight` de usePolling (que sólo cubre los ticks del poll). usePolling aporta el timer + teardown.
  usePolling(refresh, { intervalMs: 3000 });
  // 058 (ultrareview fix): el cancel ya NO traga el error en silencio — si bg_cancel falla (job ya
  // tomado por un worker, error de backend), el usuario ve el motivo en vez de un "no pasó nada".
  // (audit fix) el error se asocia al JOB: si bg_cancel resuelve tarde y el user ya cambió de fila,
  // NO aparece bajo la fila equivocada (sólo se muestra en el detalle del job al que pertenece).
  const [cancelErr, setCancelErr] = useState<{ id: string; msg: string } | null>(null);
  const cancel = async (id: string) => {
    setCancelErr(null);
    try {
      await invoke("bg_cancel", { id });
    } catch (e) {
      setCancelErr({ id, msg: `no se pudo cancelar: ${String(e)}` });
    }
    refresh();
  };

  const [formOpen, setFormOpen] = useState(false);
  const [eKind, setEKind] = useState<string>(BG_KINDS[0]);
  const [eArgs, setEArgs] = useState("{}");
  const [enqMsg, setEnqMsg] = useState<string | null>(null);
  const [enqErr, setEnqErr] = useState<string | null>(null);
  const enqueue = async () => {
    setEnqMsg(null);
    setEnqErr(null);
    let parsed: unknown;
    try {
      parsed = JSON.parse(eArgs);
    } catch (e) {
      setEnqErr(`args JSON inválido: ${String(e)}`);
      return;
    }
    try {
      const id = await invoke<string>("bg_enqueue", { kind: eKind, args: parsed });
      setEnqMsg(`encolado: ${id}`);
      refresh();
    } catch (e) {
      setEnqErr(String(e));
    }
  };

  const running = jobs.filter((j) => j.status === "running").length;
  const failed = jobs.filter((j) => j.status === "error").length;

  return (
    <div className="activity-view">
      <div className="view-head">
        <h1>Cola</h1>
        <span className="fresh">{jobs.length} jobs · 8 workers · poll 3s</span>
      </div>

      {jobs.length > 0 && (
        <div className="q-stats">
          <div className="vtile"><div className={`vn ${running > 0 ? "" : "ok"}`}>{running}</div><div className="vl">En curso</div></div>
          <div className="vtile"><div className={`vn ${failed > 0 ? "" : "ok"}`}>{failed}</div><div className="vl">Fallidos</div></div>
        </div>
      )}

      {/* Encolar job — card colapsable. */}
      <div className={`q-form ${formOpen ? "open" : ""}`}>
        <div className="q-form-head" onClick={() => setFormOpen((o) => !o)}>
          Encolar un job
          <span className="chev"><ChevronRight /></span>
        </div>
        {formOpen && (
          <>
            <div className="q-form-body">
              <select className="q-select" value={eKind} onChange={(e) => setEKind(e.target.value)}>
                {BG_KINDS.map((k) => <option key={k} value={k}>{k}</option>)}
              </select>
              <input className="q-input mono" placeholder='args JSON (ej. {"session":"abc"})' value={eArgs} onChange={(e) => setEArgs(e.target.value)} />
              <button className="btn-coral" onClick={enqueue}>Encolar</button>
            </div>
            {enqMsg && <div className="q-msg ok">{enqMsg}</div>}
            {enqErr && <div className="q-msg err">{enqErr}</div>}
          </>
        )}
      </div>

      {jobs.length === 0 ? (
        <div className="empty-state">
          <div className="empty-glyph"><ListChecks /></div>
          <h3>La cola está despejada</h3>
          <p>Acá aparecen los jobs en segundo plano (distill, auto-eval, council3…). Encolá uno arriba o se llenan solos cuando un agente dispara una tarea.</p>
        </div>
      ) : (
        <div className="q-table">
          {jobs.map((j) => {
            const cls = STATUS_CLASS(j.status);
            const open = expanded === j.id;
            const args = argsSummary(j);
            return (
              <div key={j.id}>
                <button type="button" className={`q-row ${cls}`} onClick={() => { setExpanded(open ? null : j.id); setCancelErr(null); }}>
                  <span className="st"><span className="dot" />{j.status}</span>
                  <span className="kind">{j.kind}{args && <span className="args">  {args}</span>}</span>
                  <span className="age">{ageOf(j)}</span>
                  <span className="chev" style={{ color: "var(--faint)", transform: open ? "rotate(90deg)" : "none", transition: "transform .15s", display: "inline-flex" }}><ChevronRight size={15} /></span>
                </button>
                {open && (
                  <div className="q-detail">
                    {j.error && <pre className="derr">{j.error}</pre>}
                    <pre>{j.output || (j.status === "pending" ? "Esperando un worker…" : j.status === "running" ? "Ejecutando…" : "Sin output.")}</pre>
                    {j.status === "pending" && <button className="cancel" onClick={(e) => { e.stopPropagation(); cancel(j.id); }}>Cancelar job</button>}
                    {cancelErr?.id === j.id && <div className="q-msg err">{cancelErr.msg}</div>}
                  </div>
                )}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
