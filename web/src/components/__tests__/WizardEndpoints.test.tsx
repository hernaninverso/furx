// 042 FR-002 / SC-002 — tests del paso "endpoints" del wizard (L2: RTL + mockIPC).
import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, waitFor, fireEvent } from "@testing-library/react";
import { mockIPC, clearMocks } from "@tauri-apps/api/mocks";
import { Wizard } from "../../Wizard";

// Avanza el wizard hasta el paso "endpoints" (welcome → privacy → aceptar Apache-2.0 → endpoints).
async function gotoEndpoints() {
  fireEvent.click(screen.getByText("Continue")); // welcome → privacy
  // privacy: aceptar la licencia Apache-2.0 habilita "Continue".
  fireEvent.click(screen.getByLabelText(/I accept the Apache-2.0 license/));
  fireEvent.click(screen.getByText("Continue")); // privacy → endpoints
  await waitFor(() => expect(screen.getByText("Your inference engine")).toBeInTheDocument());
}

describe("042 Wizard — paso endpoints", () => {
  beforeEach(() => clearMocks());

  it("'Test' pinta el resultado del health-check (verde/rojo) desde el backend", async () => {
    mockIPC((cmd, args) => {
      if (cmd === "setup_health_check") {
        // El usuario dejó los campos vacíos → el front prueba los defaults localhost.
        expect((args as { aieUrl: string }).aieUrl).toBe("http://localhost:8250");
        return { aie: { reachable: true, latency_ms: 12, error: null },
                 ollama: { reachable: false, latency_ms: null, error: "timeout" } };
      }
      return undefined;
    });
    render(<Wizard onDone={() => {}} onClose={() => {}} />);
    await gotoEndpoints();
    fireEvent.click(screen.getByText("Test"));
    await waitFor(() => expect(screen.getByText(/responding \(12ms\)/)).toBeInTheDocument());
    expect(screen.getByText(/not responding \(timeout\)/)).toBeInTheDocument();
  });

  it("'Save & continue' con una URL llama wizard_save_endpoints con esa URL y avanza", async () => {
    const saved: Record<string, unknown> = {};
    mockIPC((cmd, args) => {
      if (cmd === "wizard_save_endpoints") { Object.assign(saved, args); return null; }
      return undefined;
    });
    render(<Wizard onDone={() => {}} onClose={() => {}} />);
    await gotoEndpoints();
    fireEvent.change(screen.getByLabelText("AI Engine"), { target: { value: "http://my-host:8250" } });
    fireEvent.click(screen.getByText("Save & continue"));
    await waitFor(() => expect(saved.aieUrl).toBe("http://my-host:8250"));
    // avanzó a "connect"
    await waitFor(() => expect(screen.getByText("Connect your first provider")).toBeInTheDocument());
  });

  it("'Skip' NO llama wizard_save_endpoints (deja defaults) y avanza", async () => {
    const saveSpy = vi.fn();
    mockIPC((cmd) => {
      if (cmd === "wizard_save_endpoints") { saveSpy(); return null; }
      return undefined;
    });
    render(<Wizard onDone={() => {}} onClose={() => {}} />);
    await gotoEndpoints();
    fireEvent.click(screen.getByText("Skip"));
    await waitFor(() => expect(screen.getByText("Connect your first provider")).toBeInTheDocument());
    expect(saveSpy).not.toHaveBeenCalled();
  });

  it("'Save & continue' con ambos campos vacíos equivale a saltear (no llama save)", async () => {
    const saveSpy = vi.fn();
    mockIPC((cmd) => {
      if (cmd === "wizard_save_endpoints") { saveSpy(); return null; }
      return undefined;
    });
    render(<Wizard onDone={() => {}} onClose={() => {}} />);
    await gotoEndpoints();
    fireEvent.click(screen.getByText("Save & continue"));
    await waitFor(() => expect(screen.getByText("Connect your first provider")).toBeInTheDocument());
    expect(saveSpy).not.toHaveBeenCalled();
  });

  it("error del backend en 'Guardar' se SURFACEA (no se traga) y NO avanza", async () => {
    mockIPC((cmd) => {
      if (cmd === "wizard_save_endpoints") throw new Error("URL inválida: bla");
      return undefined;
    });
    render(<Wizard onDone={() => {}} onClose={() => {}} />);
    await gotoEndpoints();
    fireEvent.change(screen.getByLabelText("AI Engine"), { target: { value: "http://x:1" } });
    fireEvent.click(screen.getByText("Save & continue"));
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent(/URL inválida/));
    // sigue en el paso endpoints (no avanzó)
    expect(screen.getByText("Your inference engine")).toBeInTheDocument();
  });
});
