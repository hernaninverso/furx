// web/src/views/CrashLogView.tsx — 015 T030 · UI mínima para el huérfano `crash_log`.
//
// Backend: crash_log_list (Vec<CrashSummary>), crash_log_read(filename)->String, crash_log_clear()
// (Destructive → gateado; el invoke envuelto dispara la aprobación). Tabla de crashes + ver + limpiar.

import { useEffect, useState } from "react";
import { invoke } from "../lib/invoke";

interface CrashSummary {
  filename: string;
  iso_ts: string;
  bytes: number;
}

export function CrashLogView() {
  const [items, setItems] = useState<CrashSummary[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [content, setContent] = useState<string>("");
  const [msg, setMsg] = useState<string | null>(null);

  const refresh = async () => {
    try {
      setItems(await invoke<CrashSummary[]>("crash_log_list"));
    } catch (e) {
      setMsg(String(e));
    }
  };
  useEffect(() => { void refresh(); }, []);

  const open = async (filename: string) => {
    setSelected(filename);
    setContent("");
    try {
      setContent(await invoke<string>("crash_log_read", { filename }));
    } catch (e) {
      setContent(`error: ${String(e)}`);
    }
  };

  const clear = async () => {
    setMsg(null);
    try {
      const n = await invoke<number>("crash_log_clear");
      setMsg(`${n} crash logs borrados`);
      setSelected(null); setContent("");
      await refresh();
    } catch (e) {
      setMsg(String(e)); // si la aprobación se rechaza, cae acá
    }
  };

  return (
    <div className="page crashlog-view">
      <div className="page-header">
        <div className="page-title">Crash logs</div>
        <div className="page-sub">Reportes de crashes locales (UI + backend). {items.length} archivo(s).</div>
      </div>
      {msg && <div className="toast-inline">{msg}</div>}
      <div style={{ display: "flex", gap: 8, marginBottom: 10 }}>
        <button className="fxc-btn" onClick={() => void refresh()}>Refrescar</button>
        <button
          className="fxc-btn fxc-btn--danger"
          disabled={!selected}
          title="Borrar el crash log seleccionado"
          onClick={async () => {
            if (!selected) return;
            setMsg(null);
            try {
              await invoke<void>("crash_log_delete", { filename: selected });
              setMsg(`borrado: ${selected}`);
              setSelected(null); setContent("");
              await refresh();
            } catch (e) { setMsg(String(e)); }
          }}
        >
          Borrar log
        </button>
        <button className="fxc-btn fxc-btn--danger" onClick={() => void clear()} disabled={items.length === 0}>
          Limpiar todos
        </button>
      </div>
      {items.length === 0 ? (
        <div className="empty"><div className="head">Sin crashes</div><div className="body muted">Nada que mostrar — buena señal.</div></div>
      ) : (
        <div style={{ display: "grid", gridTemplateColumns: "minmax(260px, 1fr) 2fr", gap: 12 }}>
          <div className="crashlog-list">
            {items.map((it) => (
              <button
                key={it.filename}
                type="button"
                className={`nav-btn ${selected === it.filename ? "active" : ""}`}
                onClick={() => void open(it.filename)}
                style={{ width: "100%", justifyContent: "space-between" }}
              >
                <span>{it.iso_ts}</span>
                <span className="muted">{(it.bytes / 1024).toFixed(1)} KB</span>
              </button>
            ))}
          </div>
          <div className="crashlog-content">
            {selected ? (
              <pre style={{ whiteSpace: "pre-wrap", maxHeight: "70vh", overflow: "auto", fontSize: 12 }}>{content}</pre>
            ) : (
              <div className="muted">Elegí un crash para ver su contenido.</div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
