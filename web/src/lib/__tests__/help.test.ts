// 016 US2 (T025 + T075) — tests del índice de Help (PURO sobre CommandDef[]). Verifica SC-002.
// `node --experimental-strip-types`.
import { buildHelpIndex, searchHelp, groupByDomain, helpEntryScore, __resetHelpMemo } from "../help.ts";
import type { CommandDef } from "../commandRegistry.ts";
import { NAV_GROUPS } from "../navGroups.ts";

let pass = 0, fail = 0;
function ok(cond: boolean, name: string) { if (cond) pass++; else { fail++; console.log(`FAIL ${name}`); } }

function cmd(over: Partial<CommandDef>): CommandDef {
  return {
    id: "x", label: "X", description: "", category: "work", scope: "app", risk: "safe",
    visibility: "palette", shortcut: null, requires_confirmation: false, reversible: true,
    deeplink: null, extra: {}, ...over,
  };
}

const cmds: CommandDef[] = [
  cmd({ id: "do_work", label: "Hacer trabajo", description: "Ejecuta el trabajo", category: "work" }),
  cmd({ id: "open_search", label: "Buscar", description: "Abre la búsqueda", category: "intelligence", deeplink: "furx://search" }),
  cmd({ id: "danger_cmd", label: "Borrar todo", description: "Acción destructiva", category: "work", risk: "destructive", requires_confirmation: true }),
  cmd({ id: "internal_cmd", label: "Interno", description: "no debe verse", category: "work", visibility: "internal" }),
  cmd({ id: "hidden_cmd", label: "Oculto", description: "no debe verse", category: "work", visibility: "hidden" }),
  cmd({ id: "no_desc", label: "Sin descripción", description: "", category: "work" }),
  cmd({ id: "kw_cmd", label: "Comando con keywords", description: "", category: "work", extra: { keywords: ["zebra", "atajo"] } }),
];

__resetHelpMemo();
const index = buildHelpIndex(cmds);

// SC-002: 100% de comandos con description aparecen; 0 entradas vacías; respeta visibility.
ok(index.some((e) => e.id === "do_work"), "incluye do_work (palette)");
ok(!index.some((e) => e.id === "internal_cmd"), "excluye internal");
ok(!index.some((e) => e.id === "hidden_cmd"), "excluye hidden");
ok(index.every((e) => e.label.length > 0), "0 entradas con label vacío");
// comando sin description → entrada presente (label + categoría), no vacía.
ok(index.some((e) => e.id === "no_desc"), "comando sin description igual aparece (FR edge)");

// Entradas de navegación (los 6 dominios de navGroups) presentes.
const navCount = NAV_GROUPS.reduce((n, g) => n + g.items.length, 0);
ok(index.filter((e) => e.id.startsWith("nav.")).length === navCount, "entradas nav = cobertura navGroups");

// commandId vs deeplink: comando sin deeplink → commandId (gate del kernel); con deeplink → navega.
ok(index.find((e) => e.id === "do_work")?.commandId === "do_work", "do_work se ejecuta vía commandId (gate)");
ok(index.find((e) => e.id === "open_search")?.deeplink === "furx://search", "open_search navega vía deeplink");
ok(index.find((e) => e.id === "danger_cmd")?.risk === "destructive", "riesgo preservado para el gate");

// Memoización por identidad (T075): mismo array → misma referencia de índice.
ok(buildHelpIndex(cmds) === index, "buildHelpIndex memoiza por identidad de cmds");

// Búsqueda fuzzy: keywords indexadas (T075).
const kwResults = searchHelp(index, "zebra");
ok(kwResults.some((e) => e.id === "kw_cmd"), "fuzzy encuentra por keyword (zebra)");
// label match.
ok(searchHelp(index, "buscar").some((e) => e.id === "open_search"), "fuzzy por label");
// query vacío → todas.
ok(searchHelp(index, "").length === index.length, "query vacío devuelve todas");

// Agrupación por dominio.
const grouped = groupByDomain(searchHelp(index, ""));
ok(grouped.length >= 2, "agrupa por dominio (>=2 grupos)");
ok(grouped.every((g) => g.entries.length > 0), "sin grupos vacíos");

// helpEntryScore: label gana sobre keyword/domain.
const e1 = index.find((e) => e.id === "do_work")!;
ok((helpEntryScore("trabajo", e1) ?? -1) > 0, "score>0 para match de label");
ok(helpEntryScore("zzzzzz", e1) === null, "score null para no-match");

console.log(`help: ${pass} passed, ${fail} failed`);
process.exit(fail === 0 ? 0 : 1);
