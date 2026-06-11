// 022 US13 · L2/L3 — setup global de Vitest para los tests de componente/a11y.
// Carga los matchers de jest-dom (`toBeInTheDocument`, etc.) y los de vitest-axe
// (`toHaveNoViolations`). Limpia el DOM y los mocks de Tauri entre tests.
import "@testing-library/jest-dom/vitest";
import * as axeMatchers from "vitest-axe/matchers";
import { expect, afterEach } from "vitest";
import { cleanup } from "@testing-library/react";
import { clearMocks } from "@tauri-apps/api/mocks";

// Matchers de a11y (axe) — `expect(results).toHaveNoViolations()`.
expect.extend(axeMatchers);

// 042 FR-005 — el jsdom de este runner NO trae un localStorage funcional (setItem no persiste →
// getItem devuelve null). Eso rompería cualquier test del fallsafe local del wizard. Instalamos un
// localStorage en memoria realista (API estándar) para que los tests ejerciten el camino real.
{
  const store = new Map<string, string>();
  const mem: Storage = {
    get length() { return store.size; },
    clear() { store.clear(); },
    getItem(k: string) { return store.has(k) ? store.get(k)! : null; },
    key(i: number) { return Array.from(store.keys())[i] ?? null; },
    removeItem(k: string) { store.delete(k); },
    setItem(k: string, v: string) { store.set(k, String(v)); },
  };
  Object.defineProperty(globalThis, "localStorage", { value: mem, configurable: true, writable: true });
}

afterEach(() => {
  // Desmonta el árbol React montado por RTL.
  cleanup();
  // Resetea cualquier `mockIPC`/`mockWindows` registrado por un test (aislamiento).
  try {
    clearMocks();
  } catch {
    // clearMocks sólo aplica si hubo mocks; ignorar si no.
  }
});
