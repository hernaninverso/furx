import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Project, Card } from "../types";
import { Button } from "../components/Button";

// 053 — tipos cloud projects.
interface CloudProject {
  id: string;
  name: string;
  cloud_traces_enabled: boolean | null;
}

// BLOQUE F · F13 — best-effort "X time ago" formatter for git's short timestamp
// strings. Returns the original string when it can't be parsed.
function timeAgo(raw: string | null | undefined): string | null {
  if (!raw) return null;
  // git's `--format=%ar` gives strings like "3 days ago"; if so, just return.
  if (/ ago$/.test(raw)) return raw;
  const t = Date.parse(raw);
  if (!Number.isFinite(t)) return raw;
  const diff = Date.now() - t;
  if (diff < 60_000) return "just now";
  const mins = Math.floor(diff / 60_000);
  if (mins < 60) return `${mins}m ago`;
  const hrs = Math.floor(mins / 60);
  if (hrs < 48) return `${hrs}h ago`;
  const days = Math.floor(hrs / 24);
  if (days < 30) return `${days}d ago`;
  const mo = Math.floor(days / 30);
  if (mo < 12) return `${mo}mo ago`;
  return `${Math.floor(mo / 12)}y ago`;
}

export function SaasView() {
  const [projects, setProjects] = useState<Project[]>([]);
  const [cards, setCards] = useState<Card[]>([]);
  const [scanning, setScanning] = useState(false);
  const load = async () => {
    const [list, openCards] = await Promise.all([
      invoke<Project[]>("projects_list").catch(() => []),
      invoke<Card[]>("list_cards").catch(() => []),
    ]);
    setProjects(list);
    setCards(openCards);
  };
  const rescan = async () => {
    setScanning(true);
    try { await invoke<number>("projects_scan"); await load(); }
    finally { setScanning(false); }
  };
  useEffect(() => { void load(); }, []);

  // BLOQUE F · F13 — count open cards per project so the dashboard can flag
  // repos that have unresolved work.
  const cardsByProject = useMemo(() => {
    const m: Record<string, number> = {};
    for (const c of cards) {
      if (c.status !== "open") continue;
      m[c.project] = (m[c.project] ?? 0) + 1;
    }
    return m;
  }, [cards]);

  return (
    <div className="page">
      <div className="page-header">
        <div className="page-title">SaaS · my repos</div>
        <div className="page-sub">{projects.length} repos · {Object.values(cardsByProject).reduce((a,b) => a+b, 0)} open cards · click pasa cwd al pane focado (⌘K también)</div>
      </div>
      <div style={{ marginBottom: 12 }}>
        <Button variant="ghost" onClick={rescan} disabled={scanning}>{scanning ? "scaneando…" : "rescan ~/"}</Button>
      </div>
      {projects.length === 0
        ? <div className="empty"><span className="glyph" /><div className="head">Sin proyectos</div><div className="body muted">Apretá rescan para indexar ~/.</div></div>
        : <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(280px, 1fr))", gap: 12 }}>
            {projects.map((p) => (
              <SaasCard
                key={p.path}
                project={p}
                openCards={cardsByProject[p.name] ?? 0}
              />
            ))}
          </div>}

      {/* 053 — gestión de proyectos cloud */}
      <CloudProjectsSection />
    </div>
  );
}

function SaasCard({ project, openCards }: { project: Project; openCards: number }) {
  const ago = timeAgo(project.last_commit);
  return (
    <article className="mon" style={{ padding: "12px 14px" }}>
      <div style={{ display: "flex", alignItems: "center", gap: 8, flexWrap: "wrap" }}>
        <strong style={{ flex: 1, minWidth: 0 }}>{project.name}</strong>
        {openCards > 0 && (
          <span className="sev-tag sev-warning" title={`${openCards} open card${openCards === 1 ? "" : "s"} in this repo`}>
            ◇ {openCards}
          </span>
        )}
        {project.dirty
          ? <span className="sev-tag sev-warning">dirty</span>
          : <span className="sev-tag sev-info">clean</span>}
      </div>
      <div className="muted" style={{ fontSize: 11, fontFamily: "var(--mono)", margin: "4px 0", wordBreak: "break-all" }}>{project.path}</div>
      <div style={{ fontSize: 12 }}>
        <div>branch: <code>{project.branch ?? "—"}</code></div>
        {project.last_commit && (
          <div className="muted" style={{ fontSize: 11, marginTop: 4 }}>
            {ago && ago !== project.last_commit ? <><span title={project.last_commit}>{ago}</span></> : project.last_commit}
          </div>
        )}
      </div>
    </article>
  );
}

// 053 — Gestión de proyectos cloud (cloud_active_user / cloud_list_projects /
//        cloud_create_project / cloud_set_project_traces_enabled).
function CloudProjectsSection() {
  const [activeUser, setActiveUser] = useState<string | null>(null);
  const [projects, setProjects] = useState<CloudProject[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [newName, setNewName] = useState("");
  const [creating, setCreating] = useState(false);
  const [toggling, setToggling] = useState<string | null>(null);

  const load = async () => {
    setLoading(true);
    setError(null);
    try {
      const [user, list] = await Promise.all([
        invoke<string | null>("cloud_active_user").catch(() => null),
        invoke<CloudProject[]>("cloud_list_projects").catch(() => []),
      ]);
      setActiveUser(user ?? null);
      setProjects(list);
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => { void load(); }, []);

  const handleCreate = async () => {
    const name = newName.trim();
    if (!name) return;
    setCreating(true);
    setError(null);
    try {
      await invoke<CloudProject>("cloud_create_project", { name, cloudTracesEnabled: true });
      setNewName("");
      await load();
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setCreating(false);
    }
  };

  const handleToggleTraces = async (projectId: string, enabled: boolean) => {
    setToggling(projectId);
    setError(null);
    try {
      await invoke("cloud_set_project_traces_enabled", { projectId, enabled });
      await load();
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setToggling(null);
    }
  };

  return (
    <div className="mon" style={{ padding: "14px 16px", marginTop: 20 }}>
      <div className="mon-head">
        <strong>Proyectos Cloud</strong>
        {activeUser && (
          <span className="mon-addr muted" style={{ fontSize: 11 }}>{activeUser}</span>
        )}
      </div>

      {loading && <div className="muted" style={{ fontSize: 12, marginTop: 8 }}>Cargando…</div>}
      {error && <div style={{ color: "var(--red, #c0392b)", fontSize: 12, marginTop: 6 }}>{error}</div>}

      {!loading && !activeUser && (
        <div className="muted" style={{ fontSize: 12, marginTop: 8 }}>
          Sin sesión cloud. Iniciá sesión en Ajustes → Cloud para ver y gestionar proyectos.
        </div>
      )}

      {!loading && projects.length > 0 && (
        <div style={{ display: "flex", flexDirection: "column", gap: 6, marginTop: 10 }}>
          {projects.map((p) => {
            const tracesOn = p.cloud_traces_enabled === true || p.cloud_traces_enabled === null;
            return (
              <div
                key={p.id}
                style={{
                  display: "flex",
                  alignItems: "center",
                  gap: 10,
                  padding: "6px 10px",
                  background: "var(--bg2)",
                  borderRadius: 6,
                  fontSize: 12,
                  fontFamily: "var(--mono)",
                }}
              >
                <span style={{ flex: 1, minWidth: 0, overflow: "hidden", textOverflow: "ellipsis" }}>
                  {p.name}
                </span>
                <span className={`sev-tag ${tracesOn ? "sev-info" : ""}`} style={{ fontSize: 10 }}>
                  traces {tracesOn ? "on" : "off"}
                </span>
                <button
                  className="btn btn-secondary"
                  style={{ fontSize: 10, padding: "2px 7px" }}
                  disabled={toggling === p.id}
                  onClick={() => handleToggleTraces(p.id, !tracesOn)}
                >
                  {toggling === p.id ? "…" : tracesOn ? "Desactivar" : "Activar"}
                </button>
              </div>
            );
          })}
        </div>
      )}

      {!loading && projects.length === 0 && activeUser && (
        <div className="muted" style={{ fontSize: 12, marginTop: 8 }}>Sin proyectos cloud.</div>
      )}

      {/* Formulario de creación */}
      {activeUser && (
        <div style={{ display: "flex", gap: 8, marginTop: 12 }}>
          <input
            value={newName}
            onChange={(e) => setNewName(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && !creating && handleCreate()}
            placeholder="Nombre del nuevo proyecto"
            aria-label="Nombre del nuevo proyecto cloud"
            style={{
              flex: 1,
              padding: "5px 8px",
              borderRadius: 5,
              border: "1px solid var(--border)",
              background: "var(--bg)",
              color: "var(--text)",
              fontSize: 12,
              fontFamily: "var(--mono)",
            }}
          />
          <Button variant="ghost" onClick={handleCreate} disabled={creating || !newName.trim()}>
            {creating ? "Creando…" : "Crear"}
          </Button>
        </div>
      )}
    </div>
  );
}
