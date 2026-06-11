// 022 MED 2 — tests del DISPLAY de plugins (lib/pluginDisplay.ts).
// Invariante: el label sale del `name` SANITIZADO (nunca de description); la
// description se muestra como texto plano truncado, sin caracteres de control.
// `node --experimental-strip-types`. Lo corre scripts/test-all.mjs.
import { pluginLabel, pluginDescription } from "../pluginDisplay.ts";

let pass = 0, fail = 0;
function ok(cond: boolean, name: string) { if (cond) pass++; else { fail++; console.log(`FAIL ${name}`); } }

// 1) label limpio pasa tal cual.
ok(pluginLabel("codebase-memory") === "codebase-memory", "label limpio");
ok(pluginLabel("word_count-2") === "word_count-2", "label alfanum+guion+underscore");

// 2) caracteres maliciosos/HTML en el name se eliminan del label visible.
ok(pluginLabel("<script>alert(1)</script>") === "scriptalert1script", "label sin HTML");
ok(pluginLabel("evil name; rm -rf /") === "evilnamerm-rf", "label sin espacios/símbolos");
ok(!pluginLabel("<b>x</b>").includes("<"), "label sin angle brackets");

// 3) label truncado a 48 chars.
ok(pluginLabel("a".repeat(100)).length === 48, "label truncado a 48");

// 4) name vacío/garbage → fallback "plugin", nunca cadena vacía.
ok(pluginLabel("") === "plugin", "name vacío → 'plugin'");
ok(pluginLabel("@@@@") === "plugin", "name todo-símbolos → 'plugin'");

// 5) description: null/empty → null.
ok(pluginDescription(null) === null, "desc null");
ok(pluginDescription(undefined) === null, "desc undefined");
ok(pluginDescription("   ") === null, "desc solo-espacios → null");

// 6) description colapsa whitespace y remueve controles.
ok(pluginDescription("hola\n\tmundo") === "hola mundo", "desc colapsa whitespace");
ok(pluginDescription("a\x00b\x07c") === "a b c", "desc remueve caracteres de control");

// 7) description truncada a 240 chars (+ elipsis).
{
  const long = pluginDescription("x".repeat(500))!;
  ok(long.length === 241 && long.endsWith("…"), `desc truncada (len=${long.length})`);
}

// 8) markdown/HTML en description NO se interpreta — se conserva como texto literal
//    (React lo escapa; acá sólo verificamos que no rompe ni se vacía).
ok(pluginDescription("<img src=x onerror=alert(1)>") === "<img src=x onerror=alert(1)>", "desc HTML como texto literal");

console.log(`pluginDisplay: ${pass} passed, ${fail} failed`);
process.exit(fail === 0 ? 0 : 1);
