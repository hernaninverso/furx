// 022 US13 · L2 (componentes, RTL + @tauri-apps/api/mocks) — patrón de referencia para testear un
// componente que LEE del backend Rust vía `invoke`, sin levantar Tauri.
// Runner: Vitest + jsdom (`npm run test:components`).
//
// `mockIPC` (oficial de Tauri) intercepta TODAS las llamadas `invoke(cmd, args)` y deja que el test
// responda con datos sintéticos. Así un componente que hace `invoke("plugins_list")` se puede montar
// y verificar que pinta el VALOR MOCKEADO (no un placeholder). Este test usa un componente mínimo
// representativo (no acopla a una vista pesada) para fijar el patrón; los tests de vistas reales
// (PluginsView/IncidentInbox) lo reusan. Ver docs/testing.md.
import { describe, it, expect } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { mockIPC } from "@tauri-apps/api/mocks";

// ── Componente representativo: lee una lista del backend y la muestra. ──
interface PluginRow { id: string; name: string; version: string; enabled: boolean }

function PluginListMini() {
  const [rows, setRows] = useState<PluginRow[] | null>(null);
  const [err, setErr] = useState<string | null>(null);
  useEffect(() => {
    invoke<PluginRow[]>("plugins_list")
      .then(setRows)
      .catch((e) => setErr(String(e)));
  }, []);
  if (err) return <div role="alert">error: {err}</div>;
  if (rows === null) return <div>cargando…</div>;
  if (rows.length === 0) return <div>Sin plugins instalados</div>;
  return (
    <ul aria-label="plugins">
      {rows.map((p) => (
        <li key={p.id}>
          {p.name} v{p.version} — {p.enabled ? "activo" : "inactivo"}
        </li>
      ))}
    </ul>
  );
}

describe("componente que lee del backend (L2, mockIPC)", () => {
  it("muestra el valor mockeado del backend (no el placeholder)", async () => {
    mockIPC((cmd) => {
      if (cmd === "plugins_list") {
        return [
          { id: "codanna", name: "Codanna", version: "1.2.0", enabled: true },
          { id: "word-count", name: "Word Count", version: "0.4.1", enabled: false },
        ] satisfies PluginRow[];
      }
      throw new Error(`comando no mockeado: ${cmd}`);
    });

    render(<PluginListMini />);

    // El valor del backend mockeado aparece en pantalla.
    await waitFor(() => expect(screen.getByText(/Codanna v1\.2\.0 — activo/)).toBeInTheDocument());
    expect(screen.getByText(/Word Count v0\.4\.1 — inactivo/)).toBeInTheDocument();
    // Y NO se quedó en el placeholder "Sin plugins instalados".
    expect(screen.queryByText("Sin plugins instalados")).toBeNull();
  });

  it("backend vacío → muestra el estado vacío real (no un mock falso)", async () => {
    mockIPC((cmd) => (cmd === "plugins_list" ? [] : undefined));
    render(<PluginListMini />);
    await waitFor(() => expect(screen.getByText("Sin plugins instalados")).toBeInTheDocument());
  });

  it("error del backend → estado de error accionable (fail-closed)", async () => {
    mockIPC((cmd) => {
      if (cmd === "plugins_list") throw new Error("manifest corrupto");
      return undefined;
    });
    render(<PluginListMini />);
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent(/manifest corrupto/));
  });
});
