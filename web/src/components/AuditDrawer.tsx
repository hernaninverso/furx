// 047 FR-004 — Audit storytelling: el AuditDrawer agrupa los eventos POR SESIÓN (en vez de una lista
// plana), con cabeceras colapsables (sesión · conteo · último evento). La "sesión" se deriva del
// pane_id del evento (fallback al actor cuando no hay pane). Además, cuando se llega desde una card de
// incidente (highlightCardId), el grupo que contiene ese evento se abre y el evento se resalta —
// trazabilidad card→audit. Sin "honesto"; tokens V3 (clases existentes audit-*).
import { useEffect, useMemo, useRef, useState } from "react";
import { AuditEvent, fmtTime } from "../types";

interface Props {
  events: AuditEvent[];
  filter: string;
  onFilter: (v: string) => void;
  onClose: () => void;
  /** 047 FR-004 — card desde la que se abrió el drawer: su evento se resalta + su grupo se expande. */
  highlightCardId?: string | null;
}

interface SessionGroup {
  key: string;
  label: string;
  events: AuditEvent[];
  /** at del evento más reciente del grupo (para ordenar grupos por recencia). */
  latestAt: string;
}

/** Clave de sesión de un evento: pane_id si existe, sino el actor (fallback honesto y estable). */
function sessionKey(e: AuditEvent): { key: string; label: string } {
  if (e.pane_id) return { key: `pane:${e.pane_id}`, label: `Panel ${e.pane_id.slice(0, 8)}` };
  return { key: `actor:${e.actor}`, label: e.actor || "—" };
}

export function AuditDrawer({ events, filter, onFilter, onClose, highlightCardId }: Props) {
  const f = filter.trim().toLowerCase();
  const filtered = useMemo(
    () =>
      f
        ? events.filter((e) => e.kind.toLowerCase().includes(f) || e.actor.toLowerCase().includes(f))
        : events,
    [events, f],
  );

  // Agrupar por sesión, preservando orden de recencia (los `events` ya vienen DESC por `at`).
  const groups = useMemo<SessionGroup[]>(() => {
    const byKey = new Map<string, SessionGroup>();
    for (const e of filtered) {
      const { key, label } = sessionKey(e);
      const g = byKey.get(key);
      if (g) {
        g.events.push(e);
        if (e.at > g.latestAt) g.latestAt = e.at;
      } else {
        byKey.set(key, { key, label, events: [e], latestAt: e.at });
      }
    }
    return Array.from(byKey.values()).sort((a, b) => (a.latestAt < b.latestAt ? 1 : -1));
  }, [filtered]);

  // El grupo que contiene el evento de la card resaltada (para auto-expandirlo).
  const highlightGroupKey = useMemo(() => {
    if (!highlightCardId) return null;
    const hit = filtered.find((e) => e.card_id === highlightCardId);
    return hit ? sessionKey(hit).key : null;
  }, [filtered, highlightCardId]);

  // Estado de colapso por grupo. Por defecto: todos expandidos (la "historia" se lee de un vistazo);
  // un toggle por cabecera. El grupo resaltado siempre arranca abierto.
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  useEffect(() => {
    // Si llega un highlight, garantizamos que su grupo esté expandido.
    if (highlightGroupKey) {
      setCollapsed((prev) => {
        if (!prev.has(highlightGroupKey)) return prev;
        const next = new Set(prev);
        next.delete(highlightGroupKey);
        return next;
      });
    }
  }, [highlightGroupKey]);

  const toggle = (key: string) =>
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });

  // Scroll al evento resaltado cuando aparece.
  const highlightRef = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    const el = highlightRef.current;
    // scrollIntoView no existe en jsdom (tests) ni en todos los entornos — guard defensivo.
    if (highlightCardId && el && typeof el.scrollIntoView === "function") {
      el.scrollIntoView({ block: "center", behavior: "smooth" });
    }
  }, [highlightCardId, groups]);

  return (
    <aside className="audit-drawer" role="dialog" aria-label="Audit drawer">
      <div className="audit-drawer-head">
        <h3>Audit · por sesión</h3>
        <input
          placeholder="filtrar kind/actor…"
          value={filter}
          onChange={(e) => onFilter(e.target.value)}
          autoFocus
          aria-label="Filter audit events"
        />
        <button onClick={onClose} title="Cerrar" aria-label="Close audit drawer">×</button>
      </div>
      <div className="audit-stream">
        {groups.length === 0 ? (
          <div className="muted" style={{ padding: 12, textAlign: "center" }}>sin eventos coincidentes</div>
        ) : (
          groups.map((g) => {
            const isCollapsed = collapsed.has(g.key);
            return (
              <div key={g.key} className="audit-group">
                <button
                  type="button"
                  className="audit-group-head"
                  aria-expanded={!isCollapsed}
                  onClick={() => toggle(g.key)}
                  title={`${g.events.length} evento(s) · ${g.label}`}
                >
                  <span className="audit-group-caret" aria-hidden="true">{isCollapsed ? "▸" : "▾"}</span>
                  <span className="audit-group-name">{g.label}</span>
                  <span className="audit-group-count">{g.events.length}</span>
                  <span className="audit-group-time muted">{fmtTime(g.latestAt)}</span>
                </button>
                {!isCollapsed && (
                  <div className="audit-group-body">
                    {g.events.map((e) => {
                      const hit = !!highlightCardId && e.card_id === highlightCardId;
                      return (
                        <div
                          key={e.id}
                          ref={hit ? highlightRef : undefined}
                          className={`row k-${rowClass(e.kind)} ${hit ? "audit-row-hit" : ""}`}
                        >
                          <span className="ts">{fmtTime(e.at)}</span>
                          <div>
                            <div className="kind">{e.kind}</div>
                            <div className="actor">{e.actor}{e.card_id ? ` · card ${e.card_id.slice(0, 8)}` : ""}</div>
                          </div>
                        </div>
                      );
                    })}
                  </div>
                )}
              </div>
            );
          })
        )}
      </div>
    </aside>
  );
}

function rowClass(kind: string): string {
  if (kind.startsWith("guardrail")) return "guard";
  if (kind.includes("error") || kind.includes("denied")) return "error";
  return "";
}
