// 040 FR-009 (P2) · L2 (componentes, RTL + @tauri-apps/api/mocks) — el ranking advisory del AIE
// en BestOfNCompare. Verifica que la sugerencia (badge "✨ sugerida") sólo se muestra cuando es un
// ranking VÁLIDO (permutación exacta), y CRÍTICO: que el ranking NUNCA auto-elige — el humano clickea
// la variante que quiere y `orchestration_choose_variant` recibe ESE taskId, no el sugerido
// (invariante de foco humano 030-034). Patrón de mock: backendMock.test.tsx. Ver docs/testing.md.
import { describe, it, expect, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import { mockIPC } from "@tauri-apps/api/mocks";
import { BestOfNCompare } from "../BestOfNCompare";
import type { OrchTaskGroup, OrchVariantDiff } from "../../types";

const GROUP_ID = "g1";

const group: OrchTaskGroup = {
  id: GROUP_ID,
  batch_id: "b1",
  objective: "arreglar el bug",
  n: 3,
  chosen_task_id: null,
  created_at: "2026-06-04T00:00:00Z",
  updated_at: "2026-06-04T00:00:00Z",
};

// 3 variantes (v0/v1/v2) en orden de array; variant_index ASC; task_ids distintos.
const variants: OrchVariantDiff[] = [0, 1, 2].map((i) => ({
  task_id: `task-${i}`,
  variant_index: i,
  title: `Variante ${i}`,
  repo_path: `/repo/v${i}`,
  branch: `furx/v${i}`,
  state: "awaiting_review",
  diff_stat: ` ${i + 1} files changed`,
  risky_paths: [],
}));

// Instala mockIPC: las lecturas base devuelven el grupo + variantes; las dimensiones (evidencia/
// uso/explicación) devuelven vacío; `meta_suggest_variant_ranking` devuelve `ranking`. `onChoose`
// (si se pasa) captura el payload del `orchestration_choose_variant`.
// Expone un contador `rankingCalls`: cuántas veces el componente PIDIÓ `meta_suggest_variant_ranking`.
// Sirve para los casos negativos (C3/C4): la sugerencia llega ASYNC, así que esperar a que el comando
// se haya invocado (y dar un flush de render) es la barrera determinista para afirmar "0 badges" sin
// que un badge tardío pase desapercibido (codex BLOCKER: no afirmar ausencia antes de que el efecto
// resuelva).
function installMocks(
  ranking: number[] | null,
  onChoose?: (args: Record<string, unknown>) => void,
): { rankingCalls: () => number } {
  let calls = 0;
  mockIPC((cmd, args) => {
    switch (cmd) {
      case "orchestration_get_group":
        return group;
      case "orchestration_compare_group":
        return variants;
      case "quality_gate_get":
        return []; // sin evidencia previa (advisory)
      case "claude_usage_for_cwd":
        return null; // costo "no medido" (badge gris)
      case "meta_suggest_variant_ranking":
        calls += 1;
        return ranking; // el ranking bajo prueba (puede ser null/válido/malformado)
      case "meta_suggest_variant_ranking_explained":
        return null; // sin prior inyectado → no se enriquece
      case "orchestration_choose_variant":
        onChoose?.((args ?? {}) as Record<string, unknown>);
        return null;
      default:
        // Cualquier otro comando no esperado en este flujo → falla ruidoso (no silencioso).
        throw new Error(`comando no mockeado: ${cmd}`);
    }
  });
  return { rankingCalls: () => calls };
}

// Barrera determinista para los casos NEGATIVOS: espera a que (1) el componente haya PEDIDO el ranking
// y (2) varios ciclos de microtask/render hayan corrido, de modo que si un ranking inválido FUERA a
// pintar un badge, ya lo habría hecho. Recién entonces es válido afirmar "0 badges". Como control de
// que la barrera funciona, primero confirmamos (en C2) que un ranking VÁLIDO SÍ pinta el badge — la
// misma maquinaria de efecto/estado; así el "0 badges" de C3/C4 no es un falso negativo por timing.
async function settleRankingEffect(rankingCalls: () => number) {
  await waitFor(() => expect(rankingCalls()).toBeGreaterThan(0));
  // Tras la resolución del comando, dejá que el `.then(setRanking(...))` y el re-render corran.
  for (let i = 0; i < 5; i++) {
    await waitFor(() => expect(true).toBe(true)); // flush de microtasks/timers de RTL
    await Promise.resolve();
  }
}

function renderComponent(extra?: Partial<React.ComponentProps<typeof BestOfNCompare>>) {
  return render(
    <BestOfNCompare
      groupId={GROUP_ID}
      onClose={() => {}}
      onReview={() => {}}
      onToast={() => {}}
      {...extra}
    />,
  );
}

// Espera a que las 3 variantes estén renderizadas (las cards traen el botón "Elegir esta").
async function waitForVariants() {
  await waitFor(() => expect(screen.getAllByText("Elegir esta")).toHaveLength(3));
}

describe("BestOfNCompare — ranking advisory (040 FR-009)", () => {
  beforeEach(() => {
    // jsdom no implementa matchMedia; algunos sub-componentes/estilos lo consultan.
    if (!window.matchMedia) {
      // @ts-expect-error — stub mínimo para jsdom
      window.matchMedia = () => ({ matches: false, addEventListener() {}, removeEventListener() {} });
    }
  });

  it("Caso 1 — ranking null (feature OFF) → orden natural, 0 badges", async () => {
    installMocks(null);
    renderComponent();
    await waitForVariants();
    // Sin sugerencia: ninguna badge "✨ sugerida" en el DOM.
    expect(screen.queryAllByText(/✨ sugerida/)).toHaveLength(0);
    // Y el orden renderizado es el natural (v1/v2/v3 en orden de array).
    const labels = screen.getAllByText(/^v[123]$/).map((el) => el.textContent);
    expect(labels).toEqual(["v1", "v2", "v3"]);
  });

  it("Caso 2 — ranking [2,0,1] → exactamente 1 badge, en la variante de bestIndex=2 (task-2)", async () => {
    installMocks([2, 0, 1]);
    renderComponent();
    await waitForVariants();
    // Aparece exactamente 1 badge (waitFor: el ranking llega async tras el primer render).
    await waitFor(() => expect(screen.queryAllByText(/✨ sugerida/)).toHaveLength(1));
    const badge = screen.getByText(/✨ sugerida/);
    // El badge es advisory (title explícito) — no un check verde.
    expect(badge).toHaveAttribute("title", expect.stringMatching(/advisory/i));

    // El badge vive en la MISMA card que la variante sugerida (bestIndex=2 = variant_index 2 = "v3"),
    // NO en v1/v2. Localizamos cada card por su botón "Elegir esta" (1 por card, orden de array) y
    // subimos al contenedor de la card; luego verificamos qué card contiene el badge.
    const cards = screen.getAllByText("Elegir esta").map((btn) => btn.closest("div[style]")!.parentElement!);
    // El botón está en un row <div> que es hijo directo de la card → parentElement = card. La card de
    // task-2 (índice 2 en orden de array) debe contener el badge; las de task-0/task-1 no.
    expect(cards[2]).toContainElement(badge);
    expect(cards[0]).not.toContainElement(badge);
    expect(cards[1]).not.toContainElement(badge);
    // Y la card sugerida muestra su etiqueta "v3" (variant_index 2).
    expect(cards[2].textContent).toMatch(/v3/);
  });

  it("Caso 3 — ranking [0,0,1] (índice repetido, malformado) → rechazado, 0 badges", async () => {
    const { rankingCalls } = installMocks([0, 0, 1]);
    renderComponent();
    await waitForVariants();
    // parseRankingSuggestion rechaza repetidos → liveRanking null → orden natural, cero badges.
    // Barrera DETERMINISTA (codex BLOCKER): esperamos a que el ranking se haya PEDIDO y a que el
    // efecto/render hayan corrido, así un badge tardío NO pasaría desapercibido al afirmar ausencia.
    await settleRankingEffect(rankingCalls);
    expect(screen.queryAllByText(/✨ sugerida/)).toHaveLength(0);
  });

  it("Caso 4 — ranking [0,1,5] (índice fuera de rango para N=3) → rechazado, 0 badges, sin crash", async () => {
    const { rankingCalls } = installMocks([0, 1, 5]);
    renderComponent();
    await waitForVariants();
    // Mismo barrera determinista que C3: fuera de rango → rechazado → cero badges, sin crash.
    await settleRankingEffect(rankingCalls);
    expect(screen.queryAllByText(/✨ sugerida/)).toHaveLength(0);
    expect(screen.getAllByText("Elegir esta")).toHaveLength(3); // las 3 cards siguen (no crash)
  });

  it("Caso 5 — INVARIANTE advisory: click en la variante NO sugerida → choose recibe el taskId CLICKEADO", async () => {
    const chooseCalls: Record<string, unknown>[] = [];
    installMocks([2, 0, 1], (args) => chooseCalls.push(args));
    renderComponent();
    await waitForVariants();
    // Esperamos que el badge de la sugerida (task-2) aparezca antes de clickear.
    await waitFor(() => expect(screen.queryAllByText(/✨ sugerida/)).toHaveLength(1));
    // ANTES de la acción humana: choose NO se llamó (no hay auto-selección por el ranking).
    expect(chooseCalls).toHaveLength(0);

    // El usuario IGNORA la sugerencia y clickea "Elegir esta" de la PRIMERA card (task-0, v1).
    // Las cards renderizan en orden de array → el botón [0] es el de task-0 (la NO sugerida).
    const buttons = screen.getAllByText("Elegir esta");
    buttons[0].click();

    // El ranking es advisory: NUNCA auto-elige. choose() recibe el taskId CLICKEADO (task-0),
    // NO el sugerido (task-2). Esto fija el invariante de foco humano (030-034).
    await waitFor(() => expect(chooseCalls).toHaveLength(1));
    expect(chooseCalls[0]).toMatchObject({ groupId: GROUP_ID, taskId: "task-0" });
    expect(chooseCalls[0].taskId).not.toBe("task-2");
  });
});
