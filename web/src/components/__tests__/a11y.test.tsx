// 022 US13 · L3 (a11y, axe) — baseline de accesibilidad sobre componentes clave de la chrome.
// Runner: Vitest + jsdom + vitest-axe (`npm run test:a11y`, también incluido en `test:components`).
//
// axe-core corre sobre el DOM renderizado y falla ante violaciones WCAG (contraste se omite en jsdom
// porque no computa estilos, pero sí caza: botones sin nombre accesible, roles inválidos, dialogs sin
// label, listas mal formadas, etc.). Es un BASELINE obligatorio: si un componente introduce una
// violación a11y, este test la bloquea antes de mergear.
import { describe, it, expect } from "vitest";
import { render } from "@testing-library/react";
import { axe } from "vitest-axe";
import { Button } from "../Button.tsx";

describe("a11y baseline (L3, axe)", () => {
  it("<Button> con label no tiene violaciones a11y", async () => {
    const { container } = render(<Button variant="primary">Confirmar acción</Button>);
    const results = await axe(container);
    expect(results).toHaveNoViolations();
  });

  it("<Button> icon-only DEBE tener aria-label (sin él, axe falla)", async () => {
    // Con aria-label → accesible.
    const { container } = render(<Button variant="ghost" aria-label="Cerrar panel">✕</Button>);
    const results = await axe(container);
    expect(results).toHaveNoViolations();
  });

  it("un dialog modal bien formado (role=dialog + aria-label) no tiene violaciones", async () => {
    const { container } = render(
      <div role="dialog" aria-label="Confirmar borrado" aria-modal="true">
        <h2 id="t">Confirmar borrado</h2>
        <p>Esta acción es irreversible.</p>
        <Button variant="danger">Borrar</Button>
        <Button variant="secondary">Cancelar</Button>
      </div>,
    );
    const results = await axe(container);
    expect(results).toHaveNoViolations();
  });

  it("DETECTA una violación: botón sin nombre accesible (guard del baseline)", async () => {
    // Botón nativo sin texto ni aria-label → axe DEBE reportar `button-name`.
    // Esto prueba que el gate axe está realmente activo (no un no-op que siempre pasa).
    const { container } = render(<button type="button" />);
    const results = await axe(container);
    const violationIds = (results.violations ?? []).map((v: { id: string }) => v.id);
    expect(violationIds).toContain("button-name");
  });
});
