// 022 US13 · L2 (componentes, RTL) — <Button> design-system.
// Runner: Vitest + jsdom (`npm run test:components`). Verifica el contrato VISIBLE del componente:
// la variante/size se reflejan en las clases canónicas (fx-button*), el click dispara onClick, y los
// estados disabled/loading bloquean la interacción con la semántica de accesibilidad correcta.
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Button } from "../Button.tsx";

describe("<Button> (L2 RTL)", () => {
  it("renderiza el variant + size correcto en las clases canónicas", () => {
    render(<Button variant="danger" size="sm">Borrar</Button>);
    const btn = screen.getByRole("button", { name: "Borrar" });
    expect(btn).toBeInTheDocument();
    expect(btn.className).toContain("fx-button");
    expect(btn.className).toContain("fx-button--danger");
    expect(btn.className).toContain("fx-button--sm");
  });

  it("default = secondary md cuando no se pasa variant/size", () => {
    render(<Button>Aceptar</Button>);
    const btn = screen.getByRole("button", { name: "Aceptar" });
    expect(btn.className).toContain("fx-button--secondary");
    expect(btn.className).toContain("fx-button--md");
  });

  it("dispara onClick al clickear", async () => {
    const onClick = vi.fn();
    render(<Button onClick={onClick}>Guardar</Button>);
    await userEvent.click(screen.getByRole("button", { name: "Guardar" }));
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  it("disabled NO dispara onClick y marca aria-disabled", async () => {
    const onClick = vi.fn();
    render(<Button onClick={onClick} disabled>Imposible</Button>);
    const btn = screen.getByRole("button", { name: "Imposible" });
    expect(btn).toBeDisabled();
    expect(btn).toHaveAttribute("aria-disabled", "true");
    await userEvent.click(btn);
    expect(onClick).not.toHaveBeenCalled();
  });

  it("loading: aria-busy, deshabilitado y muestra spinner", () => {
    render(<Button loading>Procesando</Button>);
    const btn = screen.getByRole("button", { name: "Procesando" });
    expect(btn).toHaveAttribute("aria-busy", "true");
    expect(btn).toBeDisabled();
    expect(btn.className).toContain("fx-button--loading");
    expect(btn.querySelector(".fx-button__spinner")).not.toBeNull();
  });

  it("type por defecto es 'button' (evita submits accidentales)", () => {
    render(<Button>X</Button>);
    expect(screen.getByRole("button", { name: "X" })).toHaveAttribute("type", "button");
  });
});
