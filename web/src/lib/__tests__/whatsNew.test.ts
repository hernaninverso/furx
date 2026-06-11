// 016 US3 (T033 + T076) — tests de What's New: parser semver REAL + fresh/upgrade/current. SC-003.
// `node --experimental-strip-types`.
import { parseSemver, compareSemver, compareVersionStrings, resolveWhatsNew } from "../whatsNew.ts";
import type { ReleaseNote } from "../../data/releaseNotes.ts";

let pass = 0, fail = 0;
function ok(cond: boolean, name: string) { if (cond) pass++; else { fail++; console.log(`FAIL ${name}`); } }

// ── Parser semver real (no lexicográfico) ──────────────────────────────────────────────────────
ok(compareVersionStrings("1.10.0", "1.9.0") > 0, "1.10.0 > 1.9.0 (NO lexicográfico)");
ok(compareVersionStrings("0.2.0", "0.10.0") < 0, "0.2.0 < 0.10.0");
ok(compareVersionStrings("1.0.0", "1.0.0") === 0, "iguales = 0");
ok(compareVersionStrings("v1.2.3", "1.2.3") === 0, "tolera prefijo v");
// Pre-release: 1.2.0-beta.1 < 1.2.0 (estable). SemVer §11.
ok(compareVersionStrings("1.2.0-beta.1", "1.2.0") < 0, "pre-release < estable");
ok(compareVersionStrings("1.2.0-beta.1", "1.2.0-beta.2") < 0, "beta.1 < beta.2");
ok(compareVersionStrings("1.2.0-alpha", "1.2.0-beta") < 0, "alpha < beta (alfanumérico)");
ok(compareVersionStrings("1.2.0+build1", "1.2.0+build2") === 0, "build metadata ignorada");
// no parseable → tratado como 0.0.0 (fail-soft, no muestra todo).
ok(parseSemver("garbage") === null, "no parseable → null");
ok(compareVersionStrings("garbage", "0.0.1") < 0, "basura tratada como 0.0.0");
ok(parseSemver("1.2.3-rc.1") !== null, "parsea pre-release");

// ── resolveWhatsNew: fresh / upgrade / current ─────────────────────────────────────────────────
const notes: ReleaseNote[] = [
  { version: "0.1.0", title: "a", description: "", date: "2026-01-01" },
  { version: "0.2.0", title: "b", description: "", date: "2026-02-01" },
  { version: "0.3.0", title: "c", description: "", date: "2026-03-01" },
];

// fresh (sin lastSeen) → NO spamea historial.
const fresh = resolveWhatsNew("0.3.0", null, notes);
ok(fresh.kind === "fresh" && fresh.entries.length === 0, "fresh → sin entradas (no spamea)");

// upgrade: lastSeen 0.1.0, actual 0.3.0 → muestra 0.2.0 y 0.3.0 (más nueva primero).
const up = resolveWhatsNew("0.3.0", "0.1.0", notes);
ok(up.kind === "upgrade", "upgrade kind");
ok(up.entries.length === 2, "upgrade muestra las 2 nuevas");
ok(up.entries[0].version === "0.3.0", "orden: más nueva primero");
ok(!up.entries.some((e) => e.version === "0.1.0"), "no incluye la versión ya vista");

// current: lastSeen == actual → nada.
const cur = resolveWhatsNew("0.3.0", "0.3.0", notes);
ok(cur.kind === "current" && cur.entries.length === 0, "current → sin entradas");

// downgrade defensivo: lastSeen > actual → current (no muestra).
const down = resolveWhatsNew("0.2.0", "0.3.0", notes);
ok(down.kind === "current", "downgrade → current (sin spamear)");

console.log(`whatsNew: ${pass} passed, ${fail} failed`);
process.exit(fail === 0 ? 0 : 1);
