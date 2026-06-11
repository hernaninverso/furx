// 047 FR-004 — AuditDrawer agrupado por sesión + trazabilidad card→audit. Verifica: agrupa por
// pane_id (fallback actor), las cabeceras de grupo colapsan/expanden, y al pasar highlightCardId el
// evento de esa card se resalta (clase audit-row-hit) y su grupo arranca expandido.
import { describe, it, expect } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { AuditDrawer } from "../AuditDrawer";
import type { AuditEvent } from "../../types";

const EVENTS: AuditEvent[] = [
  { id: "e1", at: "2026-06-04 10:00:00", kind: "pty.spawn", actor: "claude", pane_id: "paneAAA", card_id: null },
  { id: "e2", at: "2026-06-04 10:01:00", kind: "card.decided", actor: "human", pane_id: "paneAAA", card_id: "cardZZZ" },
  { id: "e3", at: "2026-06-04 10:02:00", kind: "guardrail.block", actor: "system", pane_id: "paneBBB", card_id: null },
  { id: "e4", at: "2026-06-04 10:03:00", kind: "boot", actor: "system", pane_id: null, card_id: null },
];

function noop() {}

describe("AuditDrawer (047 FR-004)", () => {
  it("agrupa los eventos por sesión (pane_id, fallback actor)", () => {
    render(<AuditDrawer events={EVENTS} filter="" onFilter={noop} onClose={noop} />);
    // 3 grupos: paneAAA, paneBBB, actor:system (el evento sin pane_id).
    const heads = screen.getAllByRole("button", { expanded: true });
    // los heads de grupo tienen aria-expanded; el botón de cerrar no.
    const groupHeads = heads.filter((h) => h.className.includes("audit-group-head"));
    expect(groupHeads.length).toBe(3);
    expect(screen.getByText(/Panel paneAAA/)).toBeInTheDocument();
    expect(screen.getByText(/Panel paneBBB/)).toBeInTheDocument();
  });

  it("colapsa y expande un grupo al clic en su cabecera", async () => {
    const user = userEvent.setup();
    render(<AuditDrawer events={EVENTS} filter="" onFilter={noop} onClose={noop} />);
    const head = screen.getByRole("button", { name: /Panel paneAAA/ });
    expect(head).toHaveAttribute("aria-expanded", "true");
    // Su evento es visible.
    expect(screen.getByText("pty.spawn")).toBeInTheDocument();
    await user.click(head);
    expect(head).toHaveAttribute("aria-expanded", "false");
    // Colapsado → su evento desaparece.
    expect(screen.queryByText("pty.spawn")).toBeNull();
  });

  it("resalta el evento de la card (highlightCardId) y expande su grupo", () => {
    const { container } = render(
      <AuditDrawer events={EVENTS} filter="" onFilter={noop} onClose={noop} highlightCardId="cardZZZ" />,
    );
    const hit = container.querySelector(".audit-row-hit");
    expect(hit).not.toBeNull();
    // El evento resaltado es el de cardZZZ (card.decided).
    expect(within(hit as HTMLElement).getByText("card.decided")).toBeInTheDocument();
  });

  it("filtra por kind/actor sin romper el agrupado", () => {
    render(<AuditDrawer events={EVENTS} filter="guardrail" onFilter={noop} onClose={noop} />);
    expect(screen.getByText("guardrail.block")).toBeInTheDocument();
    expect(screen.queryByText("pty.spawn")).toBeNull();
  });
});
