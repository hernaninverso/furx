// 044 FR-003 / SC-003 — tests del guard anti doble-respuesta (useDecideGuard):
// botón deshabilitado durante el invoke; re-habilitado a 15s aunque no responda; respuesta tardía
// post-timeout NO muta; doble-clic post-timeout no deja el botón habilitado durante el 2º invoke;
// invoke que falla muestra error inline y la card NO desaparece.
import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { render, screen, fireEvent, act } from "@testing-library/react";
import { useDecideGuard } from "../../lib/decideGuardHook";

// Harness: un botón por card que dispara `run` con una acción controlable (deferred). Refleja
// `isDeciding` (disabled) y el error inline, igual que CardItem.
function Harness({ makeAction }: { makeAction: (cardId: string) => () => Promise<void> }) {
  const guard = useDecideGuard();
  const cardId = "card-1";
  return (
    <div>
      <button
        data-testid="approve"
        disabled={guard.isDeciding(cardId)}
        onClick={() => guard.run(cardId, makeAction(cardId))}
      >
        Aprobar
      </button>
      {guard.cardErrors[cardId] && <div role="alert" data-testid="err">{guard.cardErrors[cardId]}</div>}
    </div>
  );
}

describe("044 FR-003 — guard anti doble-respuesta", () => {
  beforeEach(() => { vi.useFakeTimers(); });
  afterEach(() => { vi.runOnlyPendingTimers(); vi.useRealTimers(); });

  it("botón deshabilitado DURANTE el invoke; se re-habilita al resolver con éxito", async () => {
    let resolve!: () => void;
    const action = () => new Promise<void>((r) => { resolve = r; });
    render(<Harness makeAction={() => action} />);
    const btn = screen.getByTestId("approve") as HTMLButtonElement;
    expect(btn.disabled).toBe(false);
    fireEvent.click(btn);
    expect(btn.disabled).toBe(true); // en vuelo
    await act(async () => { resolve(); });
    expect(btn.disabled).toBe(false); // re-habilitado tras éxito
    expect(screen.queryByTestId("err")).toBeNull();
  });

  it("a los 15s SIN respuesta, el botón se re-habilita con un error temporal", async () => {
    // acción que nunca resuelve.
    const action = () => new Promise<void>(() => {});
    render(<Harness makeAction={() => action} />);
    const btn = screen.getByTestId("approve") as HTMLButtonElement;
    fireEvent.click(btn);
    expect(btn.disabled).toBe(true);
    await act(async () => { vi.advanceTimersByTime(15000); });
    expect(btn.disabled).toBe(false); // re-habilitado por timeout
    expect(screen.getByTestId("err")).toHaveTextContent(/intentá de nuevo/);
  });

  it("respuesta TARDÍA (post-timeout) NO muta estado si ya hubo otro invoke", async () => {
    // 1er invoke nunca resuelve → timeout a 15s → re-habilita. Luego 2do invoke en vuelo. Después
    // resuelve TARDE el 1ro: NO debe re-habilitar el botón (que está en vuelo por el 2do).
    let resolve1!: () => void;
    let n = 0;
    const make = () => {
      n += 1;
      if (n === 1) return () => new Promise<void>((r) => { resolve1 = r; });
      return () => new Promise<void>(() => {}); // 2do: nunca resuelve
    };
    render(<Harness makeAction={make} />);
    const btn = screen.getByTestId("approve") as HTMLButtonElement;
    fireEvent.click(btn); // invoke 1
    await act(async () => { vi.advanceTimersByTime(15000); }); // timeout → re-habilita
    expect(btn.disabled).toBe(false);
    fireEvent.click(btn); // invoke 2 (en vuelo)
    expect(btn.disabled).toBe(true);
    // ahora resuelve TARDE el invoke 1 → NO debe tocar el estado (seq viejo).
    await act(async () => { resolve1(); });
    expect(btn.disabled).toBe(true); // sigue en vuelo por el invoke 2 (la respuesta vieja se descartó)
  });

  it("doble-clic DURANTE el invoke no arranca un 2º invoke (el botón sigue deshabilitado, 1 sola acción)", async () => {
    let resolve!: () => void;
    let calls = 0;
    const action = () => { calls += 1; return new Promise<void>((r) => { resolve = r; }); };
    render(<Harness makeAction={() => action} />);
    const btn = screen.getByTestId("approve") as HTMLButtonElement;
    fireEvent.click(btn);
    // 2º clic mientras está en vuelo (aunque disabled, forzamos el handler) → NO debe disparar otra acción.
    fireEvent.click(btn);
    expect(calls).toBe(1); // una sola acción ejecutada
    await act(async () => { resolve(); });
    expect(btn.disabled).toBe(false);
  });

  it("invoke que FALLA muestra error inline (la card no desaparece) y re-habilita el botón", async () => {
    const action = () => Promise.reject(new Error("backend boom"));
    render(<Harness makeAction={() => action} />);
    const btn = screen.getByTestId("approve") as HTMLButtonElement;
    await act(async () => { fireEvent.click(btn); });
    expect(screen.getByTestId("err")).toHaveTextContent(/backend boom/);
    expect(btn.disabled).toBe(false); // re-habilitado tras el error
    expect(screen.getByTestId("approve")).toBeInTheDocument(); // la card/botón sigue presente
  });

  // ── audit-3 fixes ───────────────────────────────────────────────────────────────────────────────

  it("audit-3: dos cards distintas son INDEPENDIENTES (decidir B no bloquea ni libera a A)", async () => {
    // Harness con dos cards.
    function Two() {
      const guard = useDecideGuard();
      const mk = (id: string) => () => new Promise<void>((r) => { (resolvers[id] = r); });
      return (
        <div>
          <button data-testid="A" disabled={guard.isDeciding("A")} onClick={() => guard.run("A", mk("A"))}>A</button>
          <button data-testid="B" disabled={guard.isDeciding("B")} onClick={() => guard.run("B", mk("B"))}>B</button>
        </div>
      );
    }
    const resolvers: Record<string, () => void> = {};
    render(<Two />);
    const a = screen.getByTestId("A") as HTMLButtonElement;
    const b = screen.getByTestId("B") as HTMLButtonElement;
    fireEvent.click(a); // A en vuelo
    expect(a.disabled).toBe(true);
    expect(b.disabled).toBe(false); // B NO se bloqueó por A
    fireEvent.click(b); // B en vuelo
    expect(a.disabled).toBe(true);
    expect(b.disabled).toBe(true);
    // resolver B → A SIGUE en vuelo (no se liberó por la resolución de B).
    await act(async () => { resolvers.B(); });
    expect(a.disabled).toBe(true);
    expect(b.disabled).toBe(false);
    // resolver A → A se libera.
    await act(async () => { resolvers.A(); });
    expect(a.disabled).toBe(false);
  });

  it("audit-3: onApplied (refreshAll) corre SÓLO si la resolución sigue vigente (tardía post-timeout NO refresca)", async () => {
    let resolve1!: () => void;
    let n = 0;
    let applied = 0;
    function H() {
      const guard = useDecideGuard();
      const mk = () => {
        n += 1;
        if (n === 1) return () => new Promise<void>((r) => { resolve1 = r; });
        return () => new Promise<void>(() => {});
      };
      return <button data-testid="x" disabled={guard.isDeciding("c")} onClick={() => guard.run("c", mk(), () => { applied += 1; })}>x</button>;
    }
    render(<H />);
    const btn = screen.getByTestId("x") as HTMLButtonElement;
    fireEvent.click(btn); // invoke 1 (nunca resuelve aún)
    await act(async () => { vi.advanceTimersByTime(15000); }); // timeout → consume seq, re-habilita
    fireEvent.click(btn); // invoke 2 (en vuelo)
    // ahora resuelve TARDE el invoke 1 → NO debe llamar onApplied (seq consumido por el timeout).
    await act(async () => { resolve1(); });
    expect(applied).toBe(0); // refreshAll NO corrió para la respuesta tardía
  });

  it("audit-3: tras el timeout, una resolución TARDÍA del mismo invoke no produce doble-efecto", async () => {
    // invoke que resuelve recién DESPUÉS del timeout. Esperamos: error mostrado (por timeout) y NO un
    // segundo efecto de éxito que limpie el error (seq consumido por el timeout).
    let resolve!: () => void;
    let applied = 0;
    function H() {
      const guard = useDecideGuard();
      return (
        <div>
          <button data-testid="x" disabled={guard.isDeciding("c")} onClick={() => guard.run("c", () => new Promise<void>((r) => { resolve = r; }), () => { applied += 1; })}>x</button>
          {guard.cardErrors.c && <div data-testid="err">{guard.cardErrors.c}</div>}
        </div>
      );
    }
    render(<H />);
    fireEvent.click(screen.getByTestId("x"));
    await act(async () => { vi.advanceTimersByTime(15000); }); // timeout → error
    expect(screen.getByTestId("err")).toBeInTheDocument();
    // ahora resuelve TARDE → no debe limpiar el error ni aplicar onApplied (doble-efecto).
    await act(async () => { resolve(); });
    expect(screen.getByTestId("err")).toBeInTheDocument(); // el error PERSISTE
    expect(applied).toBe(0); // no hubo efecto de éxito tardío
  });
});
