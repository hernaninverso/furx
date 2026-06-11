// web/src/views/GithubView.tsx — 015 T030 · UI mínima para el huérfano `gh_panel`.
//
// Backend: gh_list_prs(repo_path)->Vec<GhItem>, gh_list_issues(repo_path)->Vec<GhItem>.
// GhItem = {number, title, state, author, updated_at, url, kind}. El usuario indica el repo local.

import { useState } from "react";
import { invoke } from "../lib/invoke";

interface GhItem {
  number: number;
  title: string;
  state: string;
  author: string | null;
  updated_at: string | null;
  url: string | null;
  kind: string; // "pr" | "issue"
}

export function GithubView() {
  const [repo, setRepo] = useState<string>("");
  const [prs, setPrs] = useState<GhItem[]>([]);
  const [issues, setIssues] = useState<GhItem[]>([]);
  const [loading, setLoading] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);

  const load = async () => {
    const repoPath = repo.trim();
    if (!repoPath) {
      // audit LOW (codex): limpiar resultados viejos al pedir con ruta vacía (no dejar stale).
      setPrs([]); setIssues([]);
      setMsg("indicá la ruta local del repo");
      return;
    }
    setLoading(true); setMsg(null);
    try {
      const [p, i] = await Promise.all([
        invoke<GhItem[]>("gh_list_prs", { repoPath }),
        invoke<GhItem[]>("gh_list_issues", { repoPath }),
      ]);
      setPrs(p); setIssues(i);
    } catch (e) {
      setMsg(`error: ${String(e)} — ¿es un repo git con remote de GitHub y \`gh\` autenticado?`);
    } finally {
      setLoading(false);
    }
  };

  const list = (items: GhItem[], title: string) => (
    <div>
      <h4 style={{ margin: "8px 0" }}>{title} ({items.length})</h4>
      {items.length === 0 ? (
        <div className="muted">—</div>
      ) : (
        items.map((it) => (
          <div key={`${it.kind}-${it.number}`} className="row-meta" style={{ display: "flex", gap: 8, alignItems: "baseline" }}>
            <span className="muted">#{it.number}</span>
            {it.url ? <a href={it.url} target="_blank" rel="noopener noreferrer">{it.title}</a> : <span>{it.title}</span>}
            <span className="muted">{it.state}{it.author ? ` · ${it.author}` : ""}</span>
          </div>
        ))
      )}
    </div>
  );

  return (
    <div className="page github-view">
      <div className="page-header">
        <div className="page-title">GitHub</div>
        <div className="page-sub">PRs e issues del repo local (vía `gh`).</div>
      </div>
      {msg && <div className="toast-inline">{msg}</div>}
      <div style={{ display: "flex", gap: 8, marginBottom: 12 }}>
        <input
          type="text"
          placeholder="~/ruta/al/repo"
          value={repo}
          onChange={(e) => setRepo(e.target.value)}
          onKeyDown={(e) => { if (e.key === "Enter") void load(); }}
          style={{ flex: 1 }}
        />
        <button className="fxc-btn" onClick={() => void load()} disabled={loading}>
          {loading ? "Cargando…" : "Cargar"}
        </button>
      </div>
      <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 16 }}>
        {list(prs, "Pull requests")}
        {list(issues, "Issues")}
      </div>
    </div>
  );
}
