// 042 FR-001 / SC-001 — tests de la lógica PURA del gate del wizard en el boot.
// Runner: vitest (`npm run test:components`).
import { describe, it, expect, beforeEach } from "vitest";
import {
  decideBoot, decideBootTimeout, type Settled,
  safeLocalSet, firstRunCompletedLocal, markFirstRunCompletedLocal, FIRST_RUN_LOCAL_FLAG,
} from "../boot";

const ok = <T,>(value: T): Settled<T> => ({ status: "fulfilled", value });
const fail = <T,>(): Settled<T> => ({ status: "rejected" });

describe("042 boot — gate del wizard (decideBoot)", () => {
  it("SC-001: settings_get RECHAZA → app NO crashea, settingsState=error y va al wizard", () => {
    const d = decideBoot(fail<unknown>(), ok(true), false);
    expect(d.settingsState).toBe("error");
    expect(d.needsWizard).toBe(true);
  });

  it("tmux falla SOLO (first_run OK) → loaded, sin wizard espurio, tmuxAvailable=false", () => {
    const d = decideBoot(ok(true), fail<boolean>(), false);
    expect(d.settingsState).toBe("loaded");
    expect(d.needsWizard).toBe(false); // first_run=true → NO wizard
    expect(d.tmuxAvailable).toBe(false);
  });

  it("first_run completado en DB (true) → no wizard", () => {
    expect(decideBoot(ok(true), ok(true), false).needsWizard).toBe(false);
  });

  it("first_run NO completado (no true) → wizard", () => {
    expect(decideBoot(ok(null), ok(false), false).needsWizard).toBe(true);
    expect(decideBoot(ok(false), ok(true), false).needsWizard).toBe(true);
  });

  it("FR-005 fallsafe: first_run falla PERO el flag local está → NO re-abre el wizard (anti-loop)", () => {
    const d = decideBoot(fail<unknown>(), ok(true), /*local*/ true);
    expect(d.needsWizard).toBe(false);
    expect(d.settingsState).toBe("error");
  });

  it("FR-005 fallsafe: first_run dice no-completado PERO el flag local está → no wizard", () => {
    expect(decideBoot(ok(null), ok(true), /*local*/ true).needsWizard).toBe(false);
  });

  it("tmuxAvailable refleja el valor cuando resuelve OK", () => {
    expect(decideBoot(ok(true), ok(true), false).tmuxAvailable).toBe(true);
    expect(decideBoot(ok(true), ok(false), false).tmuxAvailable).toBe(false);
  });
});

describe("042 boot — timeout DURO 8s (decideBootTimeout)", () => {
  it("timeout sin flag local → error + wizard", () => {
    expect(decideBootTimeout(false)).toEqual({ settingsState: "error", needsWizard: true });
  });
  it("timeout CON flag local → error pero sin wizard (anti-loop)", () => {
    expect(decideBootTimeout(true)).toEqual({ settingsState: "error", needsWizard: false });
  });
});

describe("042 boot — fallsafe local (FR-005)", () => {
  beforeEach(() => { try { localStorage.clear(); } catch { /* ignore */ } });

  it("safeLocalSet escribe y verifica; firstRunCompletedLocal lo refleja", () => {
    expect(firstRunCompletedLocal()).toBe(false);
    expect(safeLocalSet(FIRST_RUN_LOCAL_FLAG, "true")).toBe(true);
    expect(firstRunCompletedLocal()).toBe(true);
  });

  it("markFirstRunCompletedLocal marca el flag y devuelve true", () => {
    expect(markFirstRunCompletedLocal()).toBe(true);
    expect(localStorage.getItem(FIRST_RUN_LOCAL_FLAG)).toBe("true");
  });

  it("firstRunCompletedLocal es false con cualquier valor != 'true'", () => {
    safeLocalSet(FIRST_RUN_LOCAL_FLAG, "false");
    expect(firstRunCompletedLocal()).toBe(false);
  });
});
