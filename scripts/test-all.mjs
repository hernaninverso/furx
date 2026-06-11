// scripts/test-all.mjs — 015 T023 · runner consolidado de las suites TS del front (FR-013).
// Sin vitest/jest: descubre `web/src/**/__tests__/*.test.ts` y corre cada uno con el type-stripping
// nativo de Node (>=23.6). Agrega exit codes. `npm test`.
import { readdirSync, statSync } from "node:fs";
import { join } from "node:path";
import { execFileSync } from "node:child_process";

const ROOT = "web/src";
function findTests(dir) {
  const out = [];
  for (const e of readdirSync(dir)) {
    const p = join(dir, e);
    if (statSync(p).isDirectory()) out.push(...findTests(p));
    else if (e.endsWith(".test.ts")) out.push(p);
  }
  return out;
}

const tests = findTests(ROOT).sort();
let failed = 0;
for (const t of tests) {
  try {
    process.stdout.write(execFileSync("node", ["--experimental-strip-types", t], { encoding: "utf8", maxBuffer: 16 * 1024 * 1024 }));
  } catch (e) {
    failed++;
    process.stdout.write((e.stdout || "") + (e.stderr || ""));
    console.log(`FAILED: ${t}`);
  }
}
console.log(`\n=== ${tests.length} suites, ${failed} failed ===`);
process.exit(failed === 0 ? 0 : 1);
