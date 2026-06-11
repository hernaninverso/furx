// 047 FR-002 — PaneCardStrip: cabecera contextual de ~28px. Verifica el modo/tokens/estado, la
// transición a "awaiting" cuando la cola de atención marca needs_input, y que "Aprobar" es una
// acción HUMANA explícita (onApprove se invoca SÓLO al clic, nunca solo). El estado de atención sale
// de `attention_list`; lo mockeamos vía mockIPC.
import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { mockIPC, clearMocks } from "@tauri-apps/api/mocks";
import { PaneCardStrip } from "../PaneCardStrip";
import { __resetAttentionForTest } from "../../hooks/useAttention";

function mockAttention(entries: { pane_id: string; priority: "needs_input" | "has_result" }[]) {
  mockIPC((cmd) => {
    if (cmd === "attention_list") {
      return entries.map((e, i) => ({ seq: i, pane_id: e.pane_id, priority: e.priority, attended: false }));
    }
    return [];
  });
}

describe("PaneCardStrip (047 FR-002)", () => {
  beforeEach(() => {
    clearMocks();
    __resetAttentionForTest();
  });

  it("muestra modo, tokens y estado idle cuando no hay proceso vivo", async () => {
    mockAttention([]);
    render(
      <PaneCardStrip paneId="p1" modeLabel="Claude Code" modeColor="#FF5C35" tokens="12.4k"
        bornAt={0} hasLiveProcess={false} onApprove={() => {}} />,
    );
    expect(screen.getByText("Claude Code")).toBeInTheDocument();
    expect(screen.getByText(/12\.4k tok/)).toBeInTheDocument();
    expect(screen.getByText(/en reposo/)).toBeInTheDocument();
    // Sin awaiting → no hay botón Aprobar.
    expect(screen.queryByRole("button", { name: /aprobar/i })).toBeNull();
  });

  it("estado 'activo' cuando el proceso está vivo y no reclama atención", async () => {
    mockAttention([]);
    render(
      <PaneCardStrip paneId="p1" modeLabel="Codex" modeColor="#e0a548" tokens={null}
        bornAt={Date.now()} hasLiveProcess onApprove={() => {}} />,
    );
    await waitFor(() => expect(screen.getByText(/activo/)).toBeInTheDocument());
    expect(screen.queryByRole("button", { name: /aprobar/i })).toBeNull();
  });

  it("cuando el pane reclama decisión (needs_input) muestra el overlay Aprobar; el clic es la acción humana", async () => {
    mockAttention([{ pane_id: "p1", priority: "needs_input" }]);
    const onApprove = vi.fn();
    render(
      <PaneCardStrip paneId="p1" modeLabel="Claude" modeColor="#FF5C35" tokens={null}
        bornAt={Date.now()} hasLiveProcess onApprove={onApprove} />,
    );
    const btn = await screen.findByRole("button", { name: /aprobar/i });
    expect(screen.getByText(/espera tu decisión/)).toBeInTheDocument();
    // FOCO HUMANO: onApprove NO se dispara sin clic.
    expect(onApprove).not.toHaveBeenCalled();
    await userEvent.click(btn);
    expect(onApprove).toHaveBeenCalledTimes(1);
  });

  it("el needs_input de OTRO pane no afecta a este", async () => {
    mockAttention([{ pane_id: "other", priority: "needs_input" }]);
    render(
      <PaneCardStrip paneId="p1" modeLabel="Claude" modeColor="#FF5C35" tokens={null}
        bornAt={Date.now()} hasLiveProcess onApprove={() => {}} />,
    );
    await waitFor(() => expect(screen.getByText(/activo/)).toBeInTheDocument());
    expect(screen.queryByRole("button", { name: /aprobar/i })).toBeNull();
  });
});
