// 022 P0b (audit 3-frontera HIGH 2) — la palette universal NO debe listar comandos dev-only
// (p.ej. `seed_demo_cards`) en builds de PRODUCCIÓN. Defensa en capas: el backend además los
// rechaza en release. Acá probamos la lógica PURA de filtrado (`usablePaletteCommands`) que el
// CommandPalette015 consume con `import.meta.env.DEV` como `devVisible`.
// `node --experimental-strip-types`.
import {
  usablePaletteCommands,
  isDevOnly,
  type CommandDef,
} from "../commandRegistry.ts";

let pass = 0, fail = 0;
function ok(cond: boolean, name: string) { if (cond) pass++; else { fail++; console.log(`FAIL ${name}`); } }

function mk(id: string, extra: Record<string, unknown> = {}): CommandDef {
  return {
    id, label: id, description: "", category: "test",
    scope: "view", risk: "safe", visibility: "palette",
    shortcut: null, requires_confirmation: false, reversible: true, deeplink: null, extra,
  };
}

const list: CommandDef[] = [
  mk("list_cards"),
  mk("seed_demo_cards", { dev_only: true }),
  mk("decide_card"),
];

// 1) isDevOnly detecta el marcador del backend.
ok(isDevOnly(mk("x", { dev_only: true })) === true, "isDevOnly true con extra.dev_only");
ok(isDevOnly(mk("x")) === false, "isDevOnly false sin marcador");
ok(isDevOnly(mk("x", { dev_only: false })) === false, "isDevOnly false con dev_only=false");
ok(isDevOnly(mk("x", { dev_only: "yes" as unknown })) === false, "isDevOnly estricto (=== true)");

// 2) En PRODUCCIÓN (devVisible=false) seed_demo_cards NO aparece.
const prod = usablePaletteCommands(list, false);
ok(!prod.some((c) => c.id === "seed_demo_cards"), "PROD: seed_demo_cards oculto de la palette");
ok(prod.some((c) => c.id === "list_cards"), "PROD: comandos normales siguen");
ok(prod.length === 2, "PROD: sólo se filtró el dev-only");

// 3) En DESARROLLO (devVisible=true) seed_demo_cards SÍ aparece.
const dev = usablePaletteCommands(list, true);
ok(dev.some((c) => c.id === "seed_demo_cards"), "DEV: seed_demo_cards visible");
ok(dev.length === 3, "DEV: nada se filtra");

// 4) Sin dev-only en la lista, prod === lista completa.
const noDev = [mk("a"), mk("b")];
ok(usablePaletteCommands(noDev, false).length === 2, "PROD sin dev-only: pasa todo");

console.log(`commandRegistryDevOnly: ${pass} passed, ${fail} failed`);
if (fail > 0) process.exit(1);
