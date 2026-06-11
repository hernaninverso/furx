// 047 FR-008 — el flag `orchestrationSSE` (refresco por eventos) debe estar DEFAULT OFF: sin opt-in,
// el comportamiento es el polling de siempre (cero regresión). Lo verificamos a nivel del registry de
// flags y de getFlag (sin valor en localStorage → default).
import { describe, it, expect, beforeEach } from "vitest";
import { FLAGS, getFlag } from "../../lib/flags";

describe("orchestrationSSE flag (047 FR-008)", () => {
  beforeEach(() => {
    try { localStorage.clear(); } catch { /* jsdom */ }
  });

  it("existe en el registry y está implementado, marcado beta", () => {
    expect(FLAGS.orchestrationSSE).toBeDefined();
    expect(FLAGS.orchestrationSSE.impl).toBe(true);
    expect(FLAGS.orchestrationSSE.beta).toBe(true);
  });

  it("default OFF — sin opt-in, el polling de siempre (cero regresión)", () => {
    expect(FLAGS.orchestrationSSE.default).toBe(false);
    expect(getFlag("orchestrationSSE")).toBe(false);
  });
});
