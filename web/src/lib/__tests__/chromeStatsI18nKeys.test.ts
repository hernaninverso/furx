// 022 US13 · L1 (refuerzo) — paridad RUNTIME de las keys i18n que consume la chrome de stats.
// `node --experimental-strip-types`.
//
// Por qué: stats.ts recibe un translator INYECTADO y castea las keys con `as LocaleKey`/string
// literal → `tsc -b` NO garantiza que esas keys existan en el catálogo (la parity de build-time sólo
// cubre `en` vs `es` para keys YA declaradas, no que el CÓDIGO referencie keys reales). Si alguien
// renombra `chrome.stats.monitorsValue` en es.ts pero no en el código, producción cae al fallback ES
// silencioso. Este test ejerce el translator REAL de ambos locales contra TODAS las keys que la
// chrome de stats/freshness usa, y falla si una falta. Cierra el hueco del patrón "translator inyectado".
import { buildSidebarStats, freshnessLabel } from "../stats.ts";
import { es } from "../../locales/es.ts";
import { en } from "../../locales/en.ts";

let pass = 0, fail = 0;
function ok(cond: boolean, name: string) { if (cond) pass++; else { fail++; console.log(`FAIL ${name}`); } }

// Keys de chrome.stats.* que el código de stats consume (stats footer + freshness).
const CHROME_STAT_KEYS = [
  "chrome.stats.incidents", "chrome.stats.panes", "chrome.stats.monitors",
  "chrome.stats.incidentsAria", "chrome.stats.panesAria",
  "chrome.stats.monitorsDownAria", "chrome.stats.monitorsUpAria", "chrome.stats.monitorsValue",
  "chrome.stats.freshNow", "chrome.stats.freshSecs", "chrome.stats.freshMins", "chrome.stats.freshHrs",
];

const esKeys = new Set(Object.keys(es));
const enKeys = new Set(Object.keys(en));
for (const k of CHROME_STAT_KEYS) {
  ok(esKeys.has(k), `key de chrome stats en es.ts: ${k}`);
  ok(enKeys.has(k), `key de chrome stats en en.ts: ${k}`);
}

// Translator real por locale: resuelve la key contra el catálogo con interpolación {x}.
function makeT(cat: Record<string, string>) {
  return (key: string, p?: Record<string, string | number>) => {
    const raw = cat[key];
    if (raw === undefined) return `«MISS:${key}»`; // marcador para que el assert lo cace
    return raw.replace(/\{(\w+)\}/g, (_, n) => String(p?.[n] ?? `{${n}}`));
  };
}

for (const [name, cat] of [["es", es], ["en", en]] as const) {
  const t = makeT(cat as unknown as Record<string, string>);
  // Ejercitar buildSidebarStats con el translator real → ninguna etiqueta debe contener «MISS:».
  const stats = buildSidebarStats({ openIncidents: 3, panes: 1, monitorsUp: 2, monitorsTotal: 5 }, t as never);
  for (const s of stats) {
    ok(!s.label.includes("«MISS:"), `[${name}] stat ${s.id} label resuelve (sin MISS)`);
    ok(!s.ariaLabel.includes("«MISS:"), `[${name}] stat ${s.id} aria resuelve (sin MISS)`);
    ok(!s.value.includes("«MISS:"), `[${name}] stat ${s.id} value resuelve (sin MISS)`);
  }
  // Freshness en sus 4 ramas.
  const now = 10_000_000;
  for (const stamp of [now - 500, now - 3000, now - 120_000, now - 7_200_000]) {
    const out = freshnessLabel(stamp, now, t as never);
    ok(!out.includes("«MISS:"), `[${name}] freshness(${now - stamp}ms) resuelve (sin MISS): "${out}"`);
  }
}

console.log(`chromeStatsI18nKeys: ${pass} passed, ${fail} failed`);
process.exit(fail === 0 ? 0 : 1);
