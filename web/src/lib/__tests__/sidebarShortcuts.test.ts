// 022 P0b · REFORMA 4 — tests de la derivación de shortcuts destacados del sidebar.
// `node --experimental-strip-types`. Cubre: deriva del registry (no literales), respeta el
// orden curado, omite ids ausentes o sin binding, y SIEMPRE refiere a acciones reales.
import { featuredSidebarShortcuts, FEATURED_SHORTCUT_IDS } from "../sidebarShortcuts.ts";
import type { ActionEntry } from "../../actions.ts";

let pass = 0, fail = 0;
function ok(cond: boolean, name: string) { if (cond) pass++; else { fail++; console.log(`FAIL ${name}`); } }

const noop = () => {};
function entry(id: string, shortcut: string | undefined, label: string): ActionEntry {
  return { id, label, group: "Pane", shortcut, run: noop };
}

// Registry sintético que cubre todos los featured + algunos extra y uno sin binding.
const registry: ActionEntry[] = [
  entry("pane.focus.1", "⌘1", "Focar pane 1"),
  entry("pane.add", "⌘N", "Nuevo pane"),
  entry("pane.close", "⌘W", "Cerrar pane focado"),
  entry("pane.cycle-mode", "⌘⇧M", "Cambiar modo del pane"),
  entry("modal.voice", "⌘⇧V", "Voice (preview)"),
  entry("modal.broadcast", "⌘B", "Broadcast"),
  entry("modal.council", "⌘J", "Council Mode"),
  entry("modal.smartpaste", undefined, "Smart Paste"), // sin binding → nunca destacado
  entry("system.snapshot", "⌘⇧S", "Snapshot manual"),  // existe pero no featured
];

const out = featuredSidebarShortcuts(registry);

// 1) Cada shortcut destacado EXISTE en el registry (cero literales inventados).
const ids = new Set(registry.map((a) => a.id));
ok(out.every((s) => ids.has(s.id)), "todo shortcut destacado existe en el registry");
ok(out.every((s) => typeof s.shortcut === "string" && s.shortcut.length > 0), "cada uno tiene binding del registry");

// 2) Respeta el orden curado de FEATURED_SHORTCUT_IDS.
const expected = FEATURED_SHORTCUT_IDS.filter((id) => ids.has(id));
ok(JSON.stringify(out.map((s) => s.id)) === JSON.stringify(expected), "respeta el orden curado");

// 3) El shortcut + label vienen del registry (no hardcode).
const focus = out.find((s) => s.id === "pane.focus.1");
ok(!!focus && focus.shortcut === "⌘1" && focus.label === "Focar pane 1", "shortcut+label derivados del registry");

// 4) Un id ausente del registry se OMITE (no inventa literal).
const partial = featuredSidebarShortcuts(registry, ["pane.add", "no.existe", "pane.close"]);
ok(partial.map((s) => s.id).join(",") === "pane.add,pane.close", "id ausente se omite, no se inventa");

// 5) Una acción sin `shortcut` NUNCA aparece, aunque esté en featuredIds.
const noBinding = featuredSidebarShortcuts(registry, ["modal.smartpaste", "pane.add"]);
ok(noBinding.map((s) => s.id).join(",") === "pane.add", "acción sin binding se omite");

// 6) Registry vacío → lista vacía (sin crash, sin literal).
ok(featuredSidebarShortcuts([]).length === 0, "registry vacío → 0 shortcuts");

// 7) Todos los FEATURED_SHORTCUT_IDS son ids reales del registry de producción (no typos).
//    (chequeo de cobertura: los ids curados deben matchear los del buildActions real).
const PROD_IDS = new Set([
  "pane.add", "pane.close", "pane.cycle-mode", "pane.focus.1",
  "modal.broadcast", "modal.council", "modal.voice",
]);
ok(FEATURED_SHORTCUT_IDS.every((id) => PROD_IDS.has(id)), "ids curados existen en el registry real");

console.log(`sidebarShortcuts: ${pass} passed, ${fail} failed`);
process.exit(fail === 0 ? 0 : 1);
