import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";

/**
 * spec 004 F2 — project-context pane. NOT a generic file explorer (council said no):
 * a read-only Git surface (branch / dirty / diff-stat) for a repo dir, plus a list of
 * file/glob paths the user marks as CONTEXT to feed the Council (the actual feed into a
 * council run is a follow-up; here we persist the selection + show the Git state).
 */
interface GitOverview { branch: string; dirty: number; clean: boolean; diff_stat: string; }

export default function ContextPane({
  repo, paths, onChange,
}: {
  repo?: string;
  paths?: string;
  onChange: (repo: string, paths: string) => void;
}) {
  const [repoInput, setRepoInput] = useState(repo ?? "");
  const [pathsInput, setPathsInput] = useState(paths ?? "");
  const [git, setGit] = useState<GitOverview | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  async function refreshGit() {
    if (!repoInput.trim()) return;
    setLoading(true); setErr(null);
    try {
      const g = await invoke<GitOverview>("git_overview", { repoPath: repoInput.trim() });
      setGit(g);
    } catch (e) {
      setGit(null);
      setErr(typeof e === "string" ? e : String(e));
    } finally { setLoading(false); }
  }

  const pathList = pathsInput.split("\n").map((l) => l.trim()).filter(Boolean);

  return (
    <div className="ctx-wrap">
      <div className="ctx-section">
        <label className="ctx-label">Repo</label>
        <div className="ctx-row">
          <input
            className="ctx-input" value={repoInput} spellCheck={false}
            placeholder="/Users/you/project"
            onChange={(e) => { setRepoInput(e.target.value); onChange(e.target.value, pathsInput); }}
            onBlur={refreshGit}
          />
          <button className="ctx-btn" onClick={refreshGit} disabled={loading || !repoInput.trim()}>
            {loading ? "…" : "git"}
          </button>
        </div>
        {err && <div className="ctx-err">{err}</div>}
        {git && (
          <div className="ctx-git">
            <span className="ctx-badge">{git.branch}</span>
            <span className={`ctx-dot ${git.clean ? "ok" : "dirty"}`} />
            <span className="ctx-dirty">{git.clean ? "clean" : `${git.dirty} changed`}</span>
            {git.diff_stat && <pre className="ctx-diff">{git.diff_stat}</pre>}
          </div>
        )}
      </div>
      <div className="ctx-section ctx-grow">
        <label className="ctx-label">Context files for the Council <span className="ctx-count">{pathList.length}</span></label>
        <textarea
          className="ctx-paths" spellCheck={false}
          placeholder={"src/lib/auth.ts\ndocs/spec.md\n(one path per line — fed to the Council as context)"}
          value={pathsInput}
          onChange={(e) => { setPathsInput(e.target.value); onChange(repoInput, e.target.value); }}
        />
      </div>
    </div>
  );
}
