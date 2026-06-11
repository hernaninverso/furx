// ⌘P search · ⌘⇧K project switcher · ⌘K actions palette.
// Council 5/5 consenso 2026-05-26: opción B — ⌘K abre con "actions" default,
// Tab cicla actions → search → project → actions. Legacy ⌘⇧K para project.
// Pattern inspired by ~/eleion-workspace WorkspaceSwitcher.tsx + QuickOpen.tsx.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { ActionEntry, ActionGroup } from "./actions";
import { loadRecent, pushRecent } from "./actions";

export type PaletteMode = "search" | "project" | "actions";

const MODE_ORDER: PaletteMode[] = ["actions", "search", "project"];

interface SearchHit {
  source: string;
  path: string;
  line: number | null;
  snippet: string;
  score: number;
}
interface SearchResult { query: string; hits: SearchHit[]; elapsed_ms: number; }
interface Project {
  path: string; name: string;
  branch: string | null; last_commit: string | null; last_commit_at: string | null;
  dirty: boolean; scanned_at: string;
}

interface Props {
  mode: PaletteMode;
  onClose: () => void;
  /** Setea cwd del pane focado a path. */
  onPickProject: (path: string) => void;
  /** Abre archivo en algún visor / inserta path en pane. */
  onPickHit: (hit: SearchHit) => void;
  /** Ejecuta /spec <args> en el pane focado. F20. */
  onRunSpec: (args: string) => void;
  /** cwd del pane focado para search rooting. */
  focusedCwd?: string;
  /** Lista de acciones para modo "actions". */
  actions: ActionEntry[];
}

export function CommandPalette({ mode: initialMode, onClose, onPickProject, onPickHit, onRunSpec, focusedCwd, actions }: Props) {
  const [mode, setMode] = useState<PaletteMode>(initialMode);
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<SearchHit[]>([]);
  const [projects, setProjects] = useState<Project[]>([]);
  const [focus, setFocus] = useState(0);
  const [loading, setLoading] = useState(false);
  const debounce = useRef<number | null>(null);
  const listRef = useRef<HTMLDivElement>(null);

  // Codex HIGH-1: sync internal mode when Shell changes paletteMode (e.g. user presses
  // ⌘P then ⌘K without closing — old impl kept the stale mode).
  useEffect(() => { setMode(initialMode); setQuery(""); setFocus(0); }, [initialMode]);

  // Initial load.
  useEffect(() => {
    if (mode === "project") {
      invoke<Project[]>("projects_list").then(setProjects).catch(() => setProjects([]));
      // Trigger background rescan (cached), don't await.
      invoke<number>("projects_scan").catch((e) => { console.warn("projects_scan background refresh failed", e); });
    }
  }, [mode]);

  // Search mode — debounced query.
  // Codex LOW-1: guard against late results after unmount/mode change with a request seq.
  const searchSeq = useRef(0);
  useEffect(() => {
    if (mode !== "search") return;
    if (!query.trim()) { setHits([]); return; }
    if (debounce.current) window.clearTimeout(debounce.current);
    setLoading(true);
    const seq = ++searchSeq.current;
    debounce.current = window.setTimeout(() => {
      invoke<SearchResult>("search_run", { query, cwd: focusedCwd ?? null })
        .then((r) => {
          if (seq !== searchSeq.current) return; // stale
          setHits(r.hits); setLoading(false); setFocus(0);
        })
        .catch(() => {
          if (seq !== searchSeq.current) return;
          setHits([]); setLoading(false);
        });
    }, 200);
    return () => { if (debounce.current) window.clearTimeout(debounce.current); };
  }, [query, mode, focusedCwd]);

  // Project mode — local fuzzy on projects.
  const filteredProjects = mode === "project"
    ? fuzzyFilter(projects, query, (p) => `${p.name} ${p.path} ${p.branch ?? ""}`)
    : [];

  // Actions mode — filter by available() + fuzzy by label + hint + group.
  // EDGE_3: history surface on empty query.
  const validActionIds = useMemo(() => new Set(actions.map((a) => a.id)), [actions]);
  const recentIds = useMemo(() => loadRecent(validActionIds), [validActionIds]);
  const filteredActions = useMemo(() => {
    if (mode !== "actions") return [] as ActionEntry[];
    const visible = actions.filter((a) => a.available?.() !== false);
    const q = query.trim();
    if (!q) {
      // Empty query: recent first (preserving order), then the rest in registry order.
      const recentSet = new Set(recentIds);
      const recent = recentIds
        .map((id) => visible.find((a) => a.id === id))
        .filter((a): a is ActionEntry => !!a);
      const rest = visible.filter((a) => !recentSet.has(a.id));
      return [...recent, ...rest];
    }
    return fuzzyFilter(visible, q, (a) => `${a.label} ${a.hint ?? ""} ${a.group}`);
  }, [mode, actions, query, recentIds]);

  // F20 spec-kit inline — query "/spec <args>" surfaces a single actionable row.
  const specMatch = mode === "search" ? query.trim().match(/^\/spec(?:\s+(.+))?$/) : null;
  const items: SearchHit[] | Project[] | ActionEntry[] = mode === "search"
    ? hits
    : mode === "project"
      ? filteredProjects
      : filteredActions;

  // Auto-scroll focused row into view (EDGE: 50+ acciones + arrow nav).
  useEffect(() => {
    const list = listRef.current;
    if (!list) return;
    const row = list.querySelector<HTMLElement>(`[data-row-idx="${focus}"]`);
    if (row) row.scrollIntoView({ block: "nearest" });
  }, [focus, mode]);

  const onPick = useCallback((idx: number) => {
    // /spec inline overrides only in search mode (council EDGE_8).
    if (specMatch) {
      const args = (specMatch[1] ?? "").trim();
      onRunSpec(args);
      onClose();
      return;
    }
    if (mode === "search") {
      const h = hits[idx]; if (h) { onPickHit(h); onClose(); }
    } else if (mode === "project") {
      const p = filteredProjects[idx]; if (p) { onPickProject(p.path); onClose(); }
    } else {
      const a = filteredActions[idx];
      if (!a) return;
      // EDGE_5: cerrar palette ANTES de correr la acción para evitar modal stacking.
      onClose();
      pushRecent(a.id);
      a.run();
    }
  }, [mode, hits, filteredProjects, filteredActions, onPickHit, onPickProject, onClose, specMatch, onRunSpec]);

  // Keyboard.
  useEffect(() => {
    const fn = (e: KeyboardEvent) => {
      // EDGE_1: cuando el palette está montado, sus inputs son target — está bien
      // que arrows/Enter/Tab/Esc se manejen. NO interfiere con xterm porque el palette
      // monta su propio input y captura focus via autoFocus.
      if (e.key === "Escape") { e.preventDefault(); onClose(); return; }
      if (e.key === "ArrowDown") { e.preventDefault(); setFocus((i) => Math.min(items.length - 1, i + 1)); return; }
      if (e.key === "ArrowUp") { e.preventDefault(); setFocus((i) => Math.max(0, i - 1)); return; }
      if (e.key === "Enter") { e.preventDefault(); onPick(focus); return; }
      if (e.key === "Tab") {
        e.preventDefault();
        const i = MODE_ORDER.indexOf(mode);
        const next = MODE_ORDER[(i + (e.shiftKey ? MODE_ORDER.length - 1 : 1)) % MODE_ORDER.length];
        setMode(next); setFocus(0); setQuery("");
        return;
      }
    };
    window.addEventListener("keydown", fn);
    return () => window.removeEventListener("keydown", fn);
  }, [items, focus, onPick, onClose, mode]);

  const placeholder = mode === "search"
    ? "buscar en code · memories · git…"
    : mode === "project"
      ? "saltar a proyecto…"
      : "ejecutar acción…";

  const statusCount = mode === "search"
    ? (loading ? "buscando…" : hits.length > 0 ? `${hits.length} hits` : "")
    : mode === "project"
      ? `${filteredProjects.length}/${projects.length}`
      : `${filteredActions.length} acciones`;

  const liveAnnounce = mode === "actions" && !loading ? `${filteredActions.length} acciones disponibles` : "";

  return (
    <div className="palette-backdrop" onClick={onClose}>
      <div className="palette" onClick={(e) => e.stopPropagation()} role="dialog" aria-modal="true" aria-label="Command palette">
        <input
          className="palette-input"
          autoFocus
          placeholder={placeholder}
          value={query}
          onChange={(e) => { setQuery(e.target.value); setFocus(0); }}
          role="combobox"
          aria-expanded="true"
          aria-controls="palette-listbox"
          aria-autocomplete="list"
          aria-activedescendant={(items.length > 0 || specMatch) ? `palette-opt-${focus}` : undefined}
        />
        <div className="palette-mode" role="tablist" aria-label="Palette mode">
          {/* Codex LOW-3: <button> en lugar de <span role=tab> para que sea keyboard-focusable */}
          <button type="button" className={`seg ${mode === "actions" ? "active" : ""}`} role="tab" aria-selected={mode === "actions"} onClick={() => { setMode("actions"); setFocus(0); setQuery(""); }}>⌘K actions</button>
          <button type="button" className={`seg ${mode === "search" ? "active" : ""}`} role="tab" aria-selected={mode === "search"} onClick={() => { setMode("search"); setFocus(0); setQuery(""); }}>⌘P search</button>
          <button type="button" className={`seg ${mode === "project" ? "active" : ""}`} role="tab" aria-selected={mode === "project"} onClick={() => { setMode("project"); setFocus(0); setQuery(""); }}>⌘⇧K projects</button>
          {statusCount && <span style={{ marginLeft: "auto" }}>{statusCount}</span>}
        </div>
        <div
          ref={listRef}
          className="palette-list"
          role="listbox"
          id="palette-listbox"
          aria-label={mode === "actions" ? "Acciones" : mode === "search" ? "Resultados de búsqueda" : "Proyectos"}
        >
          {specMatch
            ? (
              <div className="palette-row focused" id="palette-opt-0" role="option" aria-selected="true" data-row-idx={0} onClick={() => onPick(0)}>
                <span className="tag">spec</span>
                <div>
                  <div className="name">Run · <code>specify {specMatch[1] ?? ""}</code></div>
                  <div className="sub">Ejecutará en el pane focado vía pty_write. Enter para confirmar.</div>
                </div>
              </div>
            )
            : items.length === 0
            ? <div className="palette-empty">{
                mode === "search"
                  ? (query.trim() ? "sin resultados" : "tipeá para buscar · probá /spec <args>")
                  : mode === "project"
                    ? "sin proyectos · scan corriendo…"
                    : "sin acciones (filtro vacío)"
              }</div>
            : (items as Array<SearchHit | Project | ActionEntry>).map((it, i) => {
                const rowId = `palette-opt-${i}`;
                const rowProps = {
                  id: rowId,
                  role: "option" as const,
                  "aria-selected": i === focus,
                  "data-row-idx": i,
                  className: `palette-row ${i === focus ? "focused" : ""}`,
                  onClick: () => onPick(i),
                };
                if (mode === "search") {
                  const h = it as SearchHit;
                  return (
                    <div key={`${i}::${h.path}::${h.line}`} {...rowProps}>
                      <span className="tag">{h.source}</span>
                      <div>
                        <div className="snippet">{h.snippet}</div>
                        <div className="sub">{h.path}{h.line ? `:${h.line}` : ""}</div>
                      </div>
                    </div>
                  );
                }
                if (mode === "project") {
                  const p = it as Project;
                  return (
                    <div key={p.path} {...rowProps}>
                      <span className="tag">{p.branch ?? "—"}{p.dirty ? "*" : ""}</span>
                      <div>
                        <div className="name">{p.name}</div>
                        <div className="sub">{p.path} {p.last_commit ? `· ${p.last_commit}` : ""}</div>
                      </div>
                    </div>
                  );
                }
                // actions mode
                const a = it as ActionEntry;
                const gated = a.proGated === true; // visual hint; el run() ya rutea al ProGate cuando aplica.
                return (
                  <div key={a.id} {...rowProps} className={`${rowProps.className} ${gated ? "gated" : ""}`} title={gated ? "Pro feature" : undefined}>
                    <span className={`tag tag-${groupClass(a.group)}`}>{a.group}</span>
                    <div>
                      <div className="name">{a.label}{gated && <span className="pro-pill" aria-label="Pro feature"> PRO</span>}</div>
                      {a.hint && <div className="sub">{a.hint}</div>}
                    </div>
                    {a.shortcut && <kbd className="palette-kbd">{a.shortcut}</kbd>}
                  </div>
                );
              })}
        </div>
        <div className="palette-foot">
          <span><kbd>↑↓</kbd> nav</span>
          <span><kbd>Enter</kbd> pick</span>
          <span><kbd>Tab</kbd> swap mode</span>
          <span><kbd>Esc</kbd> close</span>
        </div>
        {/* Live region para SR (NVDA/VoiceOver) — EDGE_10 */}
        <div role="status" aria-live="polite" style={{ position: "absolute", left: "-10000px", width: 1, height: 1, overflow: "hidden" }}>
          {liveAnnounce}
        </div>
      </div>
    </div>
  );
}

function groupClass(g: ActionGroup): string {
  switch (g) {
    case "Pane": return "pane";
    case "Modal": return "modal";
    case "View": return "view";
    case "System": return "system";
  }
}

/** Simple subsequence-fuzzy: items containing all chars of query in order get higher score. */
function fuzzyFilter<T>(items: T[], query: string, key: (t: T) => string): T[] {
  const q = query.trim().toLowerCase();
  if (!q) return items.slice(0, 200);
  const scored = items
    .map((it) => {
      const hay = key(it).toLowerCase();
      // Substring → score 2, subsequence → score 1, none → -1
      if (hay.includes(q)) return { it, score: 2 - hay.indexOf(q) / hay.length };
      // subsequence
      let i = 0;
      for (const c of hay) {
        if (c === q[i]) i++;
        if (i === q.length) break;
      }
      return { it, score: i === q.length ? 1 : -1 };
    })
    .filter((x) => x.score >= 0)
    .sort((a, b) => b.score - a.score)
    .slice(0, 200);
  return scored.map((x) => x.it);
}
