// 042 FR-004 — tests del paso 4 "tu primer pane" + el contrato onDone (WizardResult).
import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, waitFor, fireEvent } from "@testing-library/react";
import { mockIPC, clearMocks } from "@tauri-apps/api/mocks";
import { Wizard, type WizardResult } from "../../Wizard";
import { firstRunCompletedLocal } from "../../lib/boot";

// welcome → privacy(aceptar Apache-2.0) → endpoints(saltear) → connect → firstpane.
async function gotoFirstPane(skipConnect = true) {
  fireEvent.click(screen.getByText("Continue")); // welcome → privacy
  fireEvent.click(screen.getByLabelText(/I accept the Apache-2.0 license/));
  fireEvent.click(screen.getByText("Continue")); // privacy → endpoints
  await waitFor(() => expect(screen.getByText("Your inference engine")).toBeInTheDocument());
  fireEvent.click(screen.getByText("Skip")); // endpoints → connect
  await waitFor(() => expect(screen.getByText("Connect your first provider")).toBeInTheDocument());
  if (skipConnect) fireEvent.click(screen.getByText("Skip for now")); // connect → firstpane
  else fireEvent.click(screen.getByText("Open Furx Connect"));
  await waitFor(() => expect(screen.getByText("Your first pane")).toBeInTheDocument());
}

describe("042 Wizard — paso 4 'tu primer pane'", () => {
  beforeEach(() => {
    clearMocks();
    try { localStorage.clear(); } catch { /* ignore */ }
    mockIPC((cmd) => (cmd === "settings_set" ? null : undefined));
  });

  it("FR-005 (audit cross-fase) — un finish() exitoso ESPEJA el flag local (anti-loop futuro)", async () => {
    mockIPC((cmd) => (cmd === "settings_set" ? null : undefined));
    expect(firstRunCompletedLocal()).toBe(false);
    render(<Wizard onDone={vi.fn()} onClose={vi.fn()} />);
    await gotoFirstPane();
    fireEvent.click(screen.getByRole("button", { name: "Finish" }));
    // tras el éxito, el flag local quedó marcado: un boot futuro con settings_get caído NO re-abre.
    await waitFor(() => expect(firstRunCompletedLocal()).toBe(true));
  });

  it("'Open a terminal (zsh)' → onDone con firstPaneMode='zsh' y marca first_run_completed", async () => {
    const sets: string[] = [];
    let result: WizardResult | null = null;
    mockIPC((cmd, args) => {
      if (cmd === "settings_set") { sets.push((args as { key: string }).key); return null; }
      return undefined;
    });
    render(<Wizard onDone={(r) => { result = r; }} onClose={() => {}} />);
    await gotoFirstPane();
    fireEvent.click(screen.getByText("Open a terminal (zsh)"));
    await waitFor(() => expect(result).not.toBeNull());
    expect(result!.firstPaneMode).toBe("zsh");
    expect(result!.openConnect).toBe(false);
    expect(sets).toContain("app.first_run_completed");
  });

  it("'Finish' sin elegir → onDone con firstPaneMode=null (cierra el wizard igual)", async () => {
    let result: WizardResult | null = null;
    mockIPC((cmd) => (cmd === "settings_set" ? null : undefined));
    render(<Wizard onDone={(r) => { result = r; }} onClose={() => {}} />);
    await gotoFirstPane();
    fireEvent.click(screen.getByRole("button", { name: "Finish" }));
    await waitFor(() => expect(result).not.toBeNull());
    expect(result!.firstPaneMode).toBeNull();
  });

  it("'Open Furx Connect' en connect → al finalizar openConnect=true", async () => {
    let result: WizardResult | null = null;
    mockIPC((cmd) => (cmd === "settings_set" ? null : undefined));
    render(<Wizard onDone={(r) => { result = r; }} onClose={() => {}} />);
    await gotoFirstPane(/*skipConnect*/ false);
    fireEvent.click(screen.getByRole("button", { name: "Finish" }));
    await waitFor(() => expect(result).not.toBeNull());
    expect(result!.openConnect).toBe(true);
  });

  it("el paso 4 es el ÚLTIMO (5 de 5)", async () => {
    mockIPC((cmd) => (cmd === "settings_set" ? null : undefined));
    render(<Wizard onDone={vi.fn()} onClose={vi.fn()} />);
    await gotoFirstPane();
    expect(screen.getByText("5 of 5")).toBeInTheDocument();
  });
});
