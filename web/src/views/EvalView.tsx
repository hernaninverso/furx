// 2.6 — Eval harness UI.
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button } from "../components/Button";

interface EvalTask { name: string; kind: string; path: string; }
interface EvalRun { task: string; status: string; stdout: string; stderr: string; elapsed_ms: number; }

export function EvalView() {
  const [tasks, setTasks] = useState<EvalTask[]>([]);
  const [running, setRunning] = useState<string | null>(null);
  const [results, setResults] = useState<Record<string, EvalRun>>({});
  useEffect(() => { invoke<EvalTask[]>("eval_list_tasks").then(setTasks).catch((e) => { console.warn("eval_list_tasks failed", e); }); }, []);
  const run = async (task: EvalTask) => {
    setRunning(task.name);
    try { const r = await invoke<EvalRun>("eval_run_task", { task }); setResults((p) => ({ ...p, [task.name]: r })); }
    catch (e) { console.error(e); }
    finally { setRunning(null); }
  };
  return (
    <div className="page">
      <div className="page-header">
        <div className="page-title">Eval harness</div>
        <div className="page-sub">{tasks.length} tasks en ~/eval/ · Inspect AI + promptfoo wrappers</div>
      </div>
      {tasks.length === 0
        ? <div className="empty"><div className="head">Sin tasks</div><div className="body muted">Creá <code>~/eval/*.yaml</code> (promptfoo) o <code>~/eval/*.py</code> (Inspect AI).</div></div>
        : <div className="mon-grid" style={{ marginTop: 14 }}>
            {tasks.map((t) => {
              const r = results[t.name];
              return (
                <div key={t.name} className={`mon ${r?.status === "ok" ? "up" : r?.status === "fail" ? "down" : ""}`}>
                  <div className="mon-head">
                    <span className="mon-label">{t.name}</span>
                    <span className="mon-addr muted">{t.kind}</span>
                  </div>
                  <div className="mon-body" style={{ fontSize: 12 }}>
                    {r ? <>{r.status} · {r.elapsed_ms}ms</> : <span className="muted">{running === t.name ? "running…" : "not run"}</span>}
                    <Button variant="ghost" style={{ marginLeft: 10 }} onClick={() => run(t)} disabled={running !== null}>Run</Button>
                  </div>
                  {r && <pre style={{ background: "var(--bg2)", padding: 8, marginTop: 6, fontSize: 10, maxHeight: 120, overflow: "auto", whiteSpace: "pre-wrap" }}>{r.stdout.slice(-1200) || r.stderr.slice(-1200)}</pre>}
                </div>
              );
            })}
          </div>}
    </div>
  );
}
