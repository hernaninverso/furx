// web/src/views/SearchView.tsx — 058 · "Buscar" rediseñada (design system Atelier Terminal).
// Diseño del consejo (codex 0.96 + gemini 0.97): query como HERO (barra grande, primera y prominente),
// el path del proyecto como selector SUBORDINADO debajo. Hits = lista PLANA (preserva el ranking —
// codex), cada uno con file_path en mono, snippet como contenido, score en Fraunces + barra fina de 2-3px
// (sin badges ni medidores circulares). Estados: sin-indexar / sin-resultados / buscando.
import { useRef, useState } from "react";
import { Search } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";

interface SearchHit { file_path: string; chunk_id: number; snippet: string; score: number; }

export function SearchView() {
  const [project, setProject] = useState("");
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<SearchHit[]>([]);
  const [busy, setBusy] = useState(false);
  const [searched, setSearched] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [indexCount, setIndexCount] = useState<number | null>(null);

  // 058 (ultrareview fix): guard de generación para reindex (faltaba — search SÍ lo tenía). Si el
  // proyecto cambia mientras indexa, el resultado viejo NO debe pisar indexCount/err del nuevo repo.
  const indexSeq = useRef(0);
  const reindex = async () => {
    if (!project.trim()) return;
    const my = ++indexSeq.current;
    setBusy(true);
    setErr(null);
    try {
      const n = await invoke<number>("embeddings_index", { projectPath: project });
      if (my !== indexSeq.current) return; // el proyecto cambió mientras indexaba
      setIndexCount(n);
    } catch (e) {
      if (my !== indexSeq.current) return;
      setErr(String(e));
    } finally {
      if (my === indexSeq.current) setBusy(false);
    }
  };
  // 058 (audit): guard de generación — dos búsquedas en vuelo pueden resolver fuera de orden y una
  // vieja pisar la nueva. Sólo la búsqueda más reciente escribe resultados/estado.
  const searchSeq = useRef(0);
  const search = async () => {
    if (!query.trim() || !project.trim()) return;
    const my = ++searchSeq.current;
    setBusy(true);
    setErr(null);
    setSearched(true);
    try {
      const r = await invoke<SearchHit[]>("embeddings_search", { projectPath: project, query, topK: 15 });
      if (my !== searchSeq.current) return; // una búsqueda más nueva ya ganó
      setHits(Array.isArray(r) ? r : []);
    } catch (e) {
      if (my !== searchSeq.current) return;
      setErr(String(e));
      setHits([]);
    } finally {
      if (my === searchSeq.current) setBusy(false);
    }
  };

  return (
    <div className="activity-view">
      <div className="view-head">
        <h1>Buscar</h1>
        <span className="fresh">{indexCount !== null ? `${indexCount} chunks indexados` : "sin indexar"} · nomic-embed-text · 768d</span>
      </div>

      {/* Query como hero. */}
      <div className="search-hero">
        <div className="qbar">
          <Search />
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => { if (e.key === "Enter") search(); }}
            placeholder="Buscá en tu código por significado…"
            autoFocus
          />
          <button className="go" onClick={search} disabled={busy || !query.trim() || !project.trim()}>
            {busy ? "Buscando…" : "Buscar"}
          </button>
        </div>
        <div className="search-proj">
          <span>repo</span>
          <input value={project} onChange={(e) => { setProject(e.target.value); setHits([]); setSearched(false); setIndexCount(null); searchSeq.current++; indexSeq.current++; setBusy(false); }} placeholder="~/projects/mi-repo" />
          <button onClick={reindex} disabled={busy || !project.trim()}>{busy ? "…" : "Indexar"}</button>
        </div>
      </div>

      {err && <div className="activity-ok" style={{ borderColor: "var(--red)", color: "var(--red)" }}>error: {err}</div>}

      {/* 058 (ultrareview fix): el spinner sale SIEMPRE que `busy` (antes exigía hits.length===0, así que
          una RE-búsqueda con hits viejos no mostraba ni spinner ni la lista → quedaba en blanco). */}
      {busy && <div className="search-busy">Buscando coincidencias semánticas…</div>}

      {!busy && hits.length > 0 && (
        <div>
          {hits.map((h) => {
            const pct = Math.round((h.score ?? 0) * 100);
            return (
              <article key={`${h.file_path}:${h.chunk_id}`} className="hit">
                <div className="top">
                  <span className="path"><b>{h.file_path}</b>:{h.chunk_id}</span>
                  <span className="bar"><i style={{ width: `${Math.max(0, Math.min(100, pct))}%` }} /></span>
                  <span className="score">{pct}%</span>
                </div>
                <pre>{h.snippet}</pre>
              </article>
            );
          })}
        </div>
      )}

      {/* Estados vacíos. */}
      {!busy && searched && hits.length === 0 && !err && (
        <div className="empty-state">
          <div className="empty-glyph"><Search /></div>
          <h3>Sin coincidencias</h3>
          <p>Probá una consulta más simple o más conceptual. La búsqueda es semántica — describí lo que hace el código, no el nombre exacto.</p>
        </div>
      )}
      {!busy && !searched && (
        <div className="empty-state">
          <div className="empty-glyph"><Search /></div>
          <h3>{indexCount === null ? "Indexá un repo para empezar" : "Escribí qué estás buscando"}</h3>
          <p>{indexCount === null
            ? "Pegá la ruta de un repo arriba e Indexá. Después buscás por significado, no por texto exacto — recall semántico con embeddings locales (Ollama)."
            : "Describí el comportamiento o la idea; la búsqueda encuentra el código aunque no coincida la palabra exacta."}</p>
        </div>
      )}
    </div>
  );
}
