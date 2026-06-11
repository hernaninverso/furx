// scripts/test-pwa.mjs — runner de las suites del companion PWA (`mobile-companion/pwa/*.test.mjs`).
// Son scripts node planos (assert + process.exit), no type-stripped. `npm run test:pwa`.
import { readdirSync } from "node:fs";
import { join } from "node:path";
import { execFileSync } from "node:child_process";

const DIR = "mobile-companion/pwa";
const tests = readdirSync(DIR)
  .filter((e) => e.endsWith(".test.mjs"))
  .sort();

let failed = 0;
for (const t of tests) {
  const p = join(DIR, t);
  try {
    process.stdout.write(execFileSync("node", [p], { encoding: "utf8", maxBuffer: 16 * 1024 * 1024 }));
  } catch (e) {
    failed++;
    process.stdout.write((e.stdout || "") + (e.stderr || ""));
    console.log(`FAILED: ${p}`);
  }
}
console.log(`\n=== pwa: ${tests.length} suites, ${failed} failed ===`);
process.exit(failed === 0 ? 0 : 1);
