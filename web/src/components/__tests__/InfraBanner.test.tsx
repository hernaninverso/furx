// 042 FR-003 / SC-003 — tests del banner "inferencia no configurada".
// TODO(i18n): las aserciones usan los strings EN (default en jsdom). Si el default de
// locale cambia, resolver las keys vía translate() en vez de hardcodear texto.
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { InfraBanner } from "../InfraBanner";

describe("042 InfraBanner", () => {
  it("visible=false → no renderiza nada", () => {
    const { container } = render(
      <InfraBanner visible={false} onOpenSettings={() => {}} onDismiss={() => {}} />,
    );
    expect(container).toBeEmptyDOMElement();
  });

  it("visible=true → muestra el aviso con link a Ajustes (no bloqueante)", () => {
    render(<InfraBanner visible onOpenSettings={() => {}} onDismiss={() => {}} />);
    expect(screen.getByText(/Inference not configured/)).toBeInTheDocument();
    expect(screen.getByText(/Settings → Services/)).toBeInTheDocument();
  });

  it("click en el link → onOpenSettings (navega a Servicios)", () => {
    const onOpen = vi.fn();
    render(<InfraBanner visible onOpenSettings={onOpen} onDismiss={() => {}} />);
    fireEvent.click(screen.getByText(/Settings → Services/));
    expect(onOpen).toHaveBeenCalledOnce();
  });

  it("link accesible por teclado (Enter) → onOpenSettings", () => {
    const onOpen = vi.fn();
    render(<InfraBanner visible onOpenSettings={onOpen} onDismiss={() => {}} />);
    fireEvent.keyDown(screen.getByText(/Settings → Services/), { key: "Enter" });
    expect(onOpen).toHaveBeenCalledOnce();
  });

  it("click en cerrar → onDismiss", () => {
    const onDismiss = vi.fn();
    render(<InfraBanner visible onOpenSettings={() => {}} onDismiss={onDismiss} />);
    fireEvent.click(screen.getByLabelText("Dismiss notice"));
    expect(onDismiss).toHaveBeenCalledOnce();
  });
});
