// 042 FR-005 / SC-003 — tests del cierre robusto del wizard: X, errores surfaceados, fallsafe anti-loop.
import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, waitFor, fireEvent } from "@testing-library/react";
import { mockIPC, clearMocks } from "@tauri-apps/api/mocks";
import { Wizard, type WizardResult } from "../../Wizard";
import { FIRST_RUN_LOCAL_FLAG, firstRunCompletedLocal } from "../../lib/boot";

async function gotoFirstPane() {
  fireEvent.click(screen.getByText("Continue"));
  fireEvent.click(screen.getByLabelText(/I accept the Apache-2.0 license/));
  fireEvent.click(screen.getByText("Continue"));
  await waitFor(() => expect(screen.getByText("Your inference engine")).toBeInTheDocument());
  fireEvent.click(screen.getByText("Skip"));
  await waitFor(() => expect(screen.getByText("Connect your first provider")).toBeInTheDocument());
  fireEvent.click(screen.getByText("Skip for now"));
  await waitFor(() => expect(screen.getByText("Your first pane")).toBeInTheDocument());
}

describe("042 Wizard — cierre robusto (FR-005)", () => {
  beforeEach(() => {
    clearMocks();
    try { localStorage.clear(); } catch { /* ignore */ }
  });

  it("X cierra el wizard (onClose) sin completar cuando no hubo error", () => {
    const onClose = vi.fn();
    mockIPC(() => undefined);
    render(<Wizard onDone={vi.fn()} onClose={onClose} />);
    fireEvent.click(screen.getByLabelText("Close"));
    expect(onClose).toHaveBeenCalledOnce();
    // sin error previo, NO marca el flag local (el wizard re-aparece al próximo arranque).
    expect(firstRunCompletedLocal()).toBe(false);
  });

  it("error de finish() se SURFACEA (no console.error tragado) y el modal NO cierra", async () => {
    const onDone = vi.fn();
    mockIPC((cmd) => {
      if (cmd === "settings_set") throw new Error("database is locked");
      return undefined;
    });
    render(<Wizard onDone={onDone} onClose={vi.fn()} />);
    await gotoFirstPane();
    fireEvent.click(screen.getByRole("button", { name: "Finish" }));
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent(/database is locked/));
    expect(onDone).not.toHaveBeenCalled(); // no cerró
    expect(screen.getByText("Your first pane")).toBeInTheDocument(); // sigue abierto
  });

  it("tras 2 fallos aparece 'Finish anyway' → escribe fallsafe local + onDone (anti-loop)", async () => {
    let result: WizardResult | null = null;
    mockIPC((cmd) => {
      if (cmd === "settings_set") throw new Error("disk full");
      return undefined;
    });
    render(<Wizard onDone={(r) => { result = r; }} onClose={vi.fn()} />);
    await gotoFirstPane();
    fireEvent.click(screen.getByRole("button", { name: "Finish" })); // fallo 1
    await waitFor(() => expect(screen.getByRole("alert")).toBeInTheDocument());
    fireEvent.click(screen.getByRole("button", { name: "Finish" })); // fallo 2
    await waitFor(() => expect(screen.getByText("Finish anyway")).toBeInTheDocument());
    fireEvent.click(screen.getByText("Finish anyway"));
    await waitFor(() => expect(result).not.toBeNull());
    // el fallsafe local quedó marcado → el boot NO re-abrirá el wizard en bucle.
    expect(localStorage.getItem(FIRST_RUN_LOCAL_FLAG)).toBe("true");
  });

  it("X tras un finish() fallido marca el fallsafe local antes de cerrar (anti-loop)", async () => {
    const onClose = vi.fn();
    mockIPC((cmd) => {
      if (cmd === "settings_set") throw new Error("locked");
      return undefined;
    });
    render(<Wizard onDone={vi.fn()} onClose={onClose} />);
    await gotoFirstPane();
    fireEvent.click(screen.getByRole("button", { name: "Finish" })); // falla
    await waitFor(() => expect(screen.getByRole("alert")).toBeInTheDocument());
    fireEvent.click(screen.getByLabelText("Close"));
    expect(onClose).toHaveBeenCalledOnce();
    expect(firstRunCompletedLocal()).toBe(true); // marcó el fallsafe
  });

  it("X tras finish() fallido CUANDO localStorage tampoco persiste → NO cierra, muestra warning (audit codex HIGH)", async () => {
    const onClose = vi.fn();
    mockIPC((cmd) => {
      if (cmd === "settings_set") throw new Error("locked");
      return undefined;
    });
    // Simular localStorage no persistente (quota / modo privado): setItem no guarda.
    const orig = globalThis.localStorage;
    Object.defineProperty(globalThis, "localStorage", {
      value: { getItem: () => null, setItem: () => {}, removeItem: () => {}, clear: () => {}, key: () => null, length: 0 },
      configurable: true, writable: true,
    });
    try {
      render(<Wizard onDone={vi.fn()} onClose={onClose} />);
      await gotoFirstPane();
      fireEvent.click(screen.getByRole("button", { name: "Finish" })); // falla
      await waitFor(() => expect(screen.getByRole("alert")).toBeInTheDocument());
      fireEvent.click(screen.getByLabelText("Close"));
      // NO cerró a ciegas (cerrar re-abriría el wizard en cada arranque) — surfacea el warning.
      expect(onClose).not.toHaveBeenCalled();
      expect(screen.getByText(/We couldn't save progress locally/)).toBeInTheDocument();
    } finally {
      Object.defineProperty(globalThis, "localStorage", { value: orig, configurable: true, writable: true });
    }
  });
});
