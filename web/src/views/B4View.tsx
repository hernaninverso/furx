// B4 consolidated view: Snippets + HTTP + Notes + Time + Templates + Themes + Bisect + GH
import { useEffect, useState } from "react";
import { invoke } from "../lib/invoke"; // 015 T015: invoke con flujo de aprobación universal
import { Button } from "../components/Button";

interface Snippet { id: string; title: string; body: string; tags: string; source: string; created_at: string; }
interface QuickNote { id: string; body: string; created_at: string; }
interface PaneTime { pane_id: string; events: number; active_minutes: number; }

export function B4View() {
  const [tab, setTab] = useState<"snippets"|"notes"|"http"|"time"|"templates"|"themes"|"bisect">("snippets");
  return (
    <div className="page">
      <div className="page-header">
        <div className="page-title">Tools (B4)</div>
        <div className="page-sub">{tab}</div>
      </div>
      <div style={{ display: "flex", gap: 6, marginBottom: 14, flexWrap: "wrap" }}>
        {["snippets","notes","http","time","templates","themes","bisect"].map((t) => (
          <button key={t} className={`drawer-toggle${tab === t ? " on" : ""}`} onClick={() => setTab(t as typeof tab)}>{t}</button>
        ))}
      </div>
      {tab === "snippets" && <SnippetsTab />}
      {tab === "notes" && <NotesTab />}
      {tab === "http" && <HttpTab />}
      {tab === "time" && <TimeTab />}
      {tab === "templates" && <TemplatesTab />}
      {tab === "themes" && <ThemesTab />}
      {tab === "bisect" && <BisectTab />}
    </div>
  );
}

function SnippetsTab() {
  const [items, setItems] = useState<Snippet[]>([]);
  const [q, setQ] = useState(""); const [title, setTitle] = useState(""); const [body, setBody] = useState(""); const [tags, setTags] = useState("");
  const refresh = async () => setItems(await invoke<Snippet[]>("snippets_list", { q }).catch(() => []));
  useEffect(() => { refresh(); /* eslint-disable-next-line */ }, [q]);
  const save = async () => { if (!title.trim() || !body.trim()) return; await invoke("snippets_save", { title, body, tags }); setTitle(""); setBody(""); setTags(""); refresh(); };
  return (
    <div>
      <div className="form-row"><label>Search</label><div className="form-input"><input value={q} onChange={(e) => setQ(e.target.value)} placeholder="filter…" /></div></div>
      <div className="form-row"><label>New snippet</label><div className="form-input"><input value={title} onChange={(e) => setTitle(e.target.value)} placeholder="title" /></div></div>
      <textarea value={body} onChange={(e) => setBody(e.target.value)} rows={3} placeholder="body…" style={{ width: "100%", background: "var(--bg2)", border: "1px solid var(--line)", padding: 8, fontFamily: "var(--mono)", fontSize: 12, color: "var(--text)" }} />
      <div className="form-input" style={{ marginTop: 6 }}>
        <input value={tags} onChange={(e) => setTags(e.target.value)} placeholder="tags (space-separated)" />
        <Button variant="primary" onClick={save}>Save</Button>
      </div>
      <div className="card-list" style={{ marginTop: 14 }}>
        {items.map((s) => (
          <article key={s.id} className="card-item">
            <div className="card-row"><strong>{s.title}</strong><span className="muted" style={{ marginLeft: "auto", fontSize: 11 }}>{s.tags}</span></div>
            <pre style={{ background: "var(--bg2)", padding: 8, marginTop: 4, fontSize: 11, whiteSpace: "pre-wrap" }}>{s.body}</pre>
            <div className="card-actions">
              <button onClick={() => navigator.clipboard.writeText(s.body)}>Copy</button>
              <Button variant="danger" onClick={() => invoke("snippets_delete", { id: s.id }).then(refresh)}>Delete</Button>
            </div>
          </article>
        ))}
      </div>
    </div>
  );
}

function NotesTab() {
  const [items, setItems] = useState<QuickNote[]>([]); const [body, setBody] = useState("");
  const refresh = async () => setItems(await invoke<QuickNote[]>("quick_notes_list").catch(() => []));
  useEffect(() => { refresh(); }, []);
  const add = async () => { if (!body.trim()) return; await invoke("quick_notes_add", { body }); setBody(""); refresh(); };
  return (
    <div>
      <textarea value={body} onChange={(e) => setBody(e.target.value)} rows={3} placeholder="quick note…" style={{ width: "100%", background: "var(--bg2)", border: "1px solid var(--line)", padding: 8, fontFamily: "var(--mono)", fontSize: 12, color: "var(--text)" }} />
      <Button variant="primary" onClick={add} disabled={!body.trim()}>Add</Button>
      <div className="audit-list" style={{ marginTop: 14 }}>
        {items.map((n) => (
          <div key={n.id} className="audit-row" style={{ display: "grid", gridTemplateColumns: "120px 1fr 60px" }}>
            <span className="at">{n.created_at}</span>
            <span>{n.body}</span>
            <Button variant="ghost" onClick={() => invoke("quick_notes_delete", { id: n.id }).then(refresh)}>×</Button>
          </div>
        ))}
      </div>
    </div>
  );
}

function HttpTab() {
  const [method, setMethod] = useState("GET"); const [url, setUrl] = useState("http://localhost:8080/");
  const [headers, setHeaders] = useState(""); const [body, setBody] = useState("");
  const [resp, setResp] = useState<{ status: number; body: string; elapsed_ms: number; bytes: number } | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const send = async () => {
    setErr(null);
    const hdrs: Record<string, string> = {};
    for (const line of headers.split("\n")) { const [k, ...rest] = line.split(":"); if (k && rest.length) hdrs[k.trim()] = rest.join(":").trim(); }
    try { setResp(await invoke("http_send", { req: { method, url, headers: hdrs, body: body || null, allow_external: false } })); }
    catch (e) { setErr(String(e)); }
  };
  return (
    <div>
      <div className="form-input">
        <select value={method} onChange={(e) => setMethod(e.target.value)} style={{ background: "var(--bg2)", border: "1px solid var(--line)", color: "var(--text)" }}>
          <option>GET</option><option>POST</option><option>PUT</option><option>DELETE</option><option>PATCH</option>
        </select>
        <input value={url} onChange={(e) => setUrl(e.target.value)} />
        <Button variant="primary" onClick={send}>Send</Button>
      </div>
      <textarea value={headers} onChange={(e) => setHeaders(e.target.value)} rows={2} placeholder="headers (Key: value, one per line)" style={{ width: "100%", marginTop: 8, background: "var(--bg2)", border: "1px solid var(--line)", padding: 8, fontFamily: "var(--mono)", fontSize: 11, color: "var(--text)" }} />
      <textarea value={body} onChange={(e) => setBody(e.target.value)} rows={3} placeholder="body…" style={{ width: "100%", marginTop: 8, background: "var(--bg2)", border: "1px solid var(--line)", padding: 8, fontFamily: "var(--mono)", fontSize: 11, color: "var(--text)" }} />
      {err && <div className="card-block info" style={{ borderLeftColor: "var(--red)", marginTop: 10 }}>{err}</div>}
      {resp && (
        <div style={{ marginTop: 12 }}>
          <strong style={{ color: resp.status < 400 ? "var(--green)" : "var(--red)" }}>{resp.status}</strong> · {resp.elapsed_ms}ms · {resp.bytes}B
          <pre style={{ background: "var(--bg2)", padding: 10, marginTop: 6, fontSize: 11, maxHeight: 360, overflow: "auto", whiteSpace: "pre-wrap" }}>{resp.body}</pre>
        </div>
      )}
    </div>
  );
}

function TimeTab() {
  const [data, setData] = useState<PaneTime[]>([]);
  useEffect(() => { invoke<PaneTime[]>("time_weekly").then(setData).catch(() => setData([])); }, []);
  const totalMin = data.reduce((a, b) => a + b.active_minutes, 0);
  return (
    <div>
      <div className="muted" style={{ marginBottom: 10 }}>Total ~{Math.round(totalMin/60)}h activity últimos 7 días</div>
      <table style={{ width: "100%", fontSize: 12 }}>
        <thead><tr><th>pane</th><th>events</th><th>~minutes</th></tr></thead>
        <tbody>
          {data.map((t) => (
            <tr key={t.pane_id}>
              <td style={{ fontFamily: "var(--mono)" }}>{t.pane_id}</td>
              <td>{t.events}</td>
              <td>{t.active_minutes}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function TemplatesTab() {
  const [items, setItems] = useState<{ name: string; mode: string; cwd: string | null; env_keys: string[]; initial_prompt: string | null }[]>([]);
  const [name, setName] = useState(""); const [mode, setMode] = useState("zsh"); const [cwd, setCwd] = useState(""); const [prompt, setPrompt] = useState("");
  const refresh = async () => setItems(await invoke("pane_template_list").catch(() => []) as typeof items);
  useEffect(() => { refresh(); }, []);
  const save = async () => { if (!name.trim()) return; await invoke("pane_template_save", { template: { name, mode, cwd: cwd || null, env_keys: [], initial_prompt: prompt || null } }); setName(""); refresh(); };
  return (
    <div>
      <div className="form-row"><label>Name</label><div className="form-input"><input value={name} onChange={(e) => setName(e.target.value)} /></div></div>
      <div className="form-row"><label>Mode</label><div className="form-input"><select value={mode} onChange={(e) => setMode(e.target.value)} style={{ background: "var(--bg2)", border: "1px solid var(--line)", color: "var(--text)", padding: "6px 10px" }}>
        <option>zsh</option><option>claude</option><option>codex</option><option>gemini</option><option>aider</option></select></div></div>
      <div className="form-row"><label>cwd</label><div className="form-input"><input value={cwd} onChange={(e) => setCwd(e.target.value)} placeholder="~/projects/my-repo" /></div></div>
      <div className="form-row"><label>Initial prompt</label><div className="form-input"><input value={prompt} onChange={(e) => setPrompt(e.target.value)} /></div></div>
      <Button variant="primary" onClick={save}>Save template</Button>
      <div className="mon-grid" style={{ marginTop: 14 }}>
        {items.map((t) => (
          <div key={t.name} className="mon">
            <div className="mon-head"><strong>{t.name}</strong><span className="mon-addr muted">{t.mode}</span></div>
            <div style={{ fontSize: 11, fontFamily: "var(--mono)", marginTop: 6 }}>cwd: <code>{t.cwd ?? "—"}</code></div>
            {t.initial_prompt && <div style={{ fontSize: 11, marginTop: 4 }}>{t.initial_prompt}</div>}
            <Button variant="ghost" onClick={() => invoke("pane_template_delete", { name: t.name }).then(refresh)} style={{ marginTop: 8 }}>Delete</Button>
          </div>
        ))}
      </div>
    </div>
  );
}

function ThemesTab() {
  const [items, setItems] = useState<{ project: string; accent_hex: string; label: string | null }[]>([]);
  const [project, setProject] = useState(""); const [hex, setHex] = useState("#bf3f18"); const [label, setLabel] = useState("");
  const refresh = async () => setItems(await invoke("theme_list").catch(() => []) as typeof items);
  useEffect(() => { refresh(); }, []);
  const save = async () => { if (!project.trim()) return; await invoke("theme_set", { project, accentHex: hex, label: label || null }); setProject(""); refresh(); };
  return (
    <div>
      <div className="form-input">
        <input value={project} onChange={(e) => setProject(e.target.value)} placeholder="project name" />
        <input value={hex} onChange={(e) => setHex(e.target.value)} placeholder="#aabbcc" style={{ maxWidth: 100 }} />
        <input value={label} onChange={(e) => setLabel(e.target.value)} placeholder="label" />
        <Button variant="primary" onClick={save}>Save</Button>
      </div>
      <div className="mon-grid" style={{ marginTop: 14 }}>
        {items.map((t) => (
          <div key={t.project} className="mon" style={{ borderLeft: `4px solid ${t.accent_hex}` }}>
            <div className="mon-head"><strong>{t.project}</strong><code style={{ marginLeft: "auto" }}>{t.accent_hex}</code></div>
            {t.label && <div style={{ fontSize: 11, marginTop: 4 }}>{t.label}</div>}
          </div>
        ))}
      </div>
    </div>
  );
}

function BisectTab() {
  // Codex MED v2: NO auto-fill — bisect on $HOME would explode. User must
  // enter the repo path explicitly.
  const [repo, setRepo] = useState(""); const [good, setGood] = useState("master"); const [bad, setBad] = useState("HEAD"); const [cmd, setCmd] = useState("cargo test");
  const [r, setR] = useState<{ status: string; result_sha: string | null; output: string } | null>(null);
  const [busy, setBusy] = useState(false);
  const run = async () => { setBusy(true); try { setR(await invoke("bisect_run", { repoPath: repo, good, bad, testCmd: cmd })); } catch (e) { console.error(e); } finally { setBusy(false); } };
  return (
    <div>
      <div className="card-block info"><strong>⚠</strong> bisect modifica el repo. Asegurate de tener WIP guardado.</div>
      <div className="form-row"><label>Repo</label><div className="form-input"><input value={repo} onChange={(e) => setRepo(e.target.value)} placeholder="~/projects/my-repo" /></div></div>
      <div className="form-row"><label>Good ref</label><div className="form-input"><input value={good} onChange={(e) => setGood(e.target.value)} /></div></div>
      <div className="form-row"><label>Bad ref</label><div className="form-input"><input value={bad} onChange={(e) => setBad(e.target.value)} /></div></div>
      <div className="form-row"><label>Test cmd</label><div className="form-input"><input value={cmd} onChange={(e) => setCmd(e.target.value)} /><Button variant="primary" onClick={run} disabled={busy || !repo.trim()}>{busy ? "..." : "Run"}</Button></div></div>
      {r && (
        <div style={{ marginTop: 12 }}>
          <strong style={{ color: r.status === "done" ? "var(--green)" : "var(--red)" }}>{r.status}</strong>
          {r.result_sha && <code style={{ marginLeft: 10 }}>{r.result_sha}</code>}
          <pre style={{ background: "var(--bg2)", padding: 10, marginTop: 6, fontSize: 11, maxHeight: 320, overflow: "auto", whiteSpace: "pre-wrap" }}>{r.output}</pre>
        </div>
      )}
    </div>
  );
}
