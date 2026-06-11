// 044 FR-002 / SC-002 — tests del render de incidentes AGRUPADOS en CardsView:
// cabeceras/badges, grupo critical expandido en primer arranque, colapso persiste, badge de
// emergencia en grupo colapsado con critical, grupo vacío no aparece, skeleton/error/reintentar.
import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, fireEvent, within } from "@testing-library/react";
import { clearMocks } from "@tauri-apps/api/mocks";
import { CardsView } from "../../Shell";
import { setLocale } from "../../lib/i18n";
import { INCIDENT_GROUPS_COLLAPSED_KEY } from "../../lib/incidents";
import type { Card } from "../../types";

function card(p: Partial<Card>): Card {
  return {
    id: p.id ?? Math.random().toString(36).slice(2),
    created_at: p.created_at ?? "2026-06-01 10:00:00",
    project: p.project ?? "alpha",
    source: p.source ?? "monitor",
    title: p.title ?? "Algo pasó",
    severity: p.severity ?? "warning",
    status: p.status ?? "open",
    cause: p.cause,
    snooze_until: p.snooze_until,
    read_at: p.read_at,
    dismissed_at: p.dismissed_at,
    last_activity_at: p.last_activity_at,
    reopened: p.reopened,
  };
}

const noop = () => {};

describe("044 FR-002 — incidentes agrupados", () => {
  beforeEach(() => {
    clearMocks();
    try { localStorage.clear(); } catch { /* ignore */ }
    setLocale("es");
  });

  it("renderiza grupos con cabecera, conteo accionable y badge de severidad", () => {
    const cards = [
      card({ id: "a1", project: "alpha", severity: "critical", title: "alpha crit" }),
      card({ id: "b1", project: "beta", severity: "warning", title: "beta warn" }),
    ];
    render(<CardsView cards={cards} onDecide={noop} onGoToSource={noop} />);
    // dos grupos (alpha, beta): la cabecera de cada uno es un toggle con aria-label única.
    expect(screen.getByRole("button", { name: /Mostrar u ocultar el grupo alpha/ })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Mostrar u ocultar el grupo beta/ })).toBeInTheDocument();
    // conteo accionable por grupo (i18n: "{count} accionables").
    expect(screen.getAllByText(/accionables/).length).toBeGreaterThanOrEqual(2);
  });

  it("primer arranque: grupo con critical EXPANDIDO (cards visibles), grupo sin critical COLAPSADO", () => {
    const cards = [
      card({ id: "a1", project: "alpha", severity: "critical", title: "alpha crit card" }),
      card({ id: "b1", project: "beta", severity: "warning", title: "beta warn card" }),
    ];
    render(<CardsView cards={cards} onDecide={noop} onGoToSource={noop} />);
    // el grupo critical (alpha) arranca expandido → su card es visible.
    expect(screen.getByText("alpha crit card")).toBeInTheDocument();
    // el grupo sin critical (beta) arranca colapsado → su card NO está en el DOM.
    expect(screen.queryByText("beta warn card")).toBeNull();
  });

  it("badge de emergencia visible en la cabecera de un grupo COLAPSADO que contiene critical", () => {
    // Forzamos un escenario donde un grupo con critical arranque colapsado: lo persistimos colapsado.
    // Las claves persistidas están namespaceadas por groupBy (default "project") → "project:alpha".
    localStorage.setItem(INCIDENT_GROUPS_COLLAPSED_KEY, JSON.stringify({ "project:alpha": true }));
    const cards = [
      card({ id: "a1", project: "alpha", severity: "critical", title: "alpha crit card" }),
    ];
    render(<CardsView cards={cards} onDecide={noop} onGoToSource={noop} />);
    // colapsado → la card NO se renderiza…
    expect(screen.queryByText("alpha crit card")).toBeNull();
    // …pero el badge de emergencia SÍ está en la cabecera.
    expect(screen.getByText("Emergencia")).toBeInTheDocument();
  });

  it("toggle de un grupo persiste en localStorage (colapsar el critical lo guarda)", () => {
    const cards = [card({ id: "a1", project: "alpha", severity: "critical", title: "alpha crit card" })];
    render(<CardsView cards={cards} onDecide={noop} onGoToSource={noop} />);
    // arranca expandido (critical). Click en el toggle → colapsa.
    expect(screen.getByText("alpha crit card")).toBeInTheDocument();
    const toggle = screen.getByRole("button", { name: /Mostrar u ocultar el grupo alpha/ });
    fireEvent.click(toggle);
    expect(screen.queryByText("alpha crit card")).toBeNull(); // colapsó
    // persistió el colapso (clave namespaceada por groupBy default "project").
    const saved = JSON.parse(localStorage.getItem(INCIDENT_GROUPS_COLLAPSED_KEY) ?? "{}");
    expect(saved["project:alpha"]).toBe(true);
  });

  it("grupo vacío no aparece en el DOM (sólo se renderizan grupos con cards)", () => {
    const cards = [card({ id: "a1", project: "alpha", severity: "critical" })];
    render(<CardsView cards={cards} onDecide={noop} onGoToSource={noop} />);
    // sólo hay un grupo (alpha); no aparece ninguna cabecera de grupo "beta"/"gamma" espuria.
    expect(screen.getByRole("button", { name: /Mostrar u ocultar el grupo alpha/ })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Mostrar u ocultar el grupo beta/ })).toBeNull();
  });

  it("skeleton mientras loading y NO hay cards todavía", () => {
    const { container } = render(
      <CardsView cards={[]} onDecide={noop} onGoToSource={noop} loading error={null} />,
    );
    // 3 cards skeleton (aria-busy en el contenedor).
    expect(container.querySelector('[aria-busy="true"]')).not.toBeNull();
    expect(container.querySelectorAll(".skeleton-card").length).toBe(3);
  });

  it("error + Reintentar cuando el fetch falló y no hay datos; onRetry se dispara", () => {
    const onRetry = vi.fn();
    render(
      <CardsView cards={[]} onDecide={noop} onGoToSource={noop} loading={false} error="boom: list_cards" onRetry={onRetry} />,
    );
    expect(screen.getByRole("alert")).toHaveTextContent(/No se pudieron cargar los incidentes/);
    expect(screen.getByText(/boom: list_cards/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Reintentar" }));
    expect(onRetry).toHaveBeenCalledOnce();
  });

  it("con datos previos, un error de refresh muestra banner inline SIN vaciar la vista", () => {
    const cards = [card({ id: "a1", project: "alpha", severity: "critical", title: "alpha crit card" })];
    render(<CardsView cards={cards} onDecide={noop} onGoToSource={noop} error="refresh failed" onRetry={noop} />);
    // la card sigue visible (no se vació)…
    expect(screen.getByText("alpha crit card")).toBeInTheDocument();
    // …y hay un aviso inline de error.
    expect(screen.getByRole("alert")).toBeInTheDocument();
  });

  it("'ver más' aparece cuando el grupo expandido supera las 5 cards y suma cards al click", () => {
    // 7 critical en un grupo → arranca expandido, muestra 5, "ver 2 más".
    const cards = Array.from({ length: 7 }, (_, i) =>
      card({ id: `c${i}`, project: "alpha", severity: "critical", title: `crit ${i}` }),
    );
    render(<CardsView cards={cards} onDecide={noop} onGoToSource={noop} />);
    const group = screen.getByRole("button", { name: /Mostrar u ocultar el grupo alpha/ }).closest("section")!;
    // sólo 5 cards visibles inicialmente.
    expect(within(group).getAllByText(/^crit \d$/).length).toBe(5);
    const showMore = within(group).getByRole("button", { name: /Ver 2 más/ });
    fireEvent.click(showMore);
    // ahora las 7.
    expect(within(group).getAllByText(/^crit \d$/).length).toBe(7);
  });

  // ── audit-3 fixes ───────────────────────────────────────────────────────────────────────────────

  it("audit-3: el cap de 200 es GLOBAL al DOM (no por grupo) — 3 grupos expandidos no montan >200", () => {
    // 3 grupos critical (todos arrancan expandidos) de 150 cards c/u = 450 cards lógicas. Pero el
    // budget de DOM es 200: en el primer arranque cada grupo muestra 5 → 15 montadas (< 200). Lo que
    // verificamos es que NUNCA, ni pidiendo "ver más", el total renderizado supere 200.
    const cards: Card[] = [];
    for (const p of ["g1", "g2", "g3"]) {
      for (let i = 0; i < 150; i++) cards.push(card({ id: `${p}-${i}`, project: p, severity: "critical", title: `${p} card ${i}` }));
    }
    const { container } = render(<CardsView cards={cards} onDecide={noop} onGoToSource={noop} />);
    // pedir "ver más" muchas veces en todos los grupos para intentar exceder el cap.
    for (let round = 0; round < 6; round++) {
      const moreButtons = container.querySelectorAll<HTMLButtonElement>(".incidents-show-more");
      moreButtons.forEach((b) => fireEvent.click(b));
    }
    const mounted = container.querySelectorAll(".card-item").length;
    expect(mounted).toBeLessThanOrEqual(200);
    // y debe haber LLEGADO al tope (no quedó corto por un bug de presupuesto).
    expect(mounted).toBe(200);
  });

  it("audit-3: togglear un grupo visible NO borra de localStorage la preferencia de un grupo ausente", () => {
    // Persistimos la preferencia de un grupo que NO está en pantalla (zzz no existe en las cards).
    localStorage.setItem(INCIDENT_GROUPS_COLLAPSED_KEY, JSON.stringify({ "project:zzz": true }));
    const cards = [card({ id: "a1", project: "alpha", severity: "critical", title: "alpha crit card" })];
    render(<CardsView cards={cards} onDecide={noop} onGoToSource={noop} />);
    // togglear el grupo visible (alpha) → persiste alpha colapsado.
    fireEvent.click(screen.getByRole("button", { name: /Mostrar u ocultar el grupo alpha/ }));
    const saved = JSON.parse(localStorage.getItem(INCIDENT_GROUPS_COLLAPSED_KEY) ?? "{}");
    expect(saved["project:alpha"]).toBe(true);
    // y la preferencia del grupo ausente SIGUE persistida (no se perdió).
    expect(saved["project:zzz"]).toBe(true);
  });

  it("044 FR-003: con la card en vuelo (isDeciding), sus botones de decisión quedan deshabilitados", () => {
    const cards = [card({ id: "a1", project: "alpha", severity: "critical", title: "alpha crit card" })];
    render(
      <CardsView cards={cards} onDecide={noop} onGoToSource={noop} isDeciding={(id) => id === "a1"} cardErrors={{ a1: "boom" }} />,
    );
    // Aprobar/Rechazar/Descartar deshabilitados; "Ver detalle" (no es decisión) sigue habilitado.
    expect((screen.getByRole("button", { name: "Aprobar" }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: "Rechazar" }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: "Descartar" }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: "Ver detalle" }) as HTMLButtonElement).disabled).toBe(false);
    // el error inline de la card está visible (la card NO desapareció).
    expect(screen.getByText("boom")).toBeInTheDocument();
    expect(screen.getByText("alpha crit card")).toBeInTheDocument();
  });

  it("044 FR-003: el SnoozeMenu abierto se CIERRA al pasar la card a 'en vuelo' (no decide doble)", () => {
    const onDecide = vi.fn();
    const cards = [card({ id: "a1", project: "alpha", severity: "critical", title: "alpha crit card" })];
    const { rerender } = render(
      <CardsView cards={cards} onDecide={onDecide} onGoToSource={noop} isDeciding={() => false} cardErrors={{}} />,
    );
    // abrir el menú de snooze.
    fireEvent.click(screen.getByRole("button", { name: /Elegir cuánto posponer/ }));
    expect(screen.getByRole("menu")).toBeInTheDocument();
    // la card pasa a "en vuelo" → el menú debe cerrarse (sus opciones desaparecen).
    rerender(<CardsView cards={cards} onDecide={onDecide} onGoToSource={noop} isDeciding={() => true} cardErrors={{}} />);
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("audit-3: las claves de colapso se namespacean por groupBy (proyecto 'critical' ≠ severidad 'critical')", () => {
    // Un proyecto llamado "critical" no debe compartir estado de colapso con el grupo de severidad
    // "critical" cuando se cambia el agrupamiento. Persistimos colapsado SÓLO el namespace de severidad.
    localStorage.setItem(INCIDENT_GROUPS_COLLAPSED_KEY, JSON.stringify({ "severity:critical": true }));
    const cards = [card({ id: "x1", project: "critical", severity: "critical", title: "proj critical card" })];
    // groupBy default = "project" → la clave es "project:critical", NO afectada por la persistida.
    render(<CardsView cards={cards} onDecide={noop} onGoToSource={noop} />);
    // el grupo de PROYECTO "critical" arranca expandido (contiene un critical) → su card es visible
    // (no fue colapsada por el valor persistido de severidad).
    expect(screen.getByText("proj critical card")).toBeInTheDocument();
  });
});
