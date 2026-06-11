// 022 US9 (FR-014) — tests del design-system <Button>.
// ──────────────────────────────────────────────────────────────────────────
// Invariantes:
//  1) La escala de variantes/sizes es CERRADA (lista explícita).
//  2) buttonClasses() solo emite clases `fx-button*` + el escape `className`.
//  3) El CSS del componente usa SOLO tokens: 0 literales de color (hex/rgb/hsl).
//  4) Inventario: cuántos `<button … className="ghost|primary|danger|mini…">`
//     ad-hoc quedan (className en CUALQUIER posición de atributo)
//     ad-hoc quedan en la chrome migrable (views/components/wizard/Settings).
// `node --experimental-strip-types`. Lo corre scripts/test-all.mjs.
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import {
  BUTTON_VARIANTS,
  BUTTON_SIZES,
  LEGACY_BUTTON_MAP,
  buttonClasses,
} from "../buttonVariants.ts";

let pass = 0, fail = 0;
function ok(cond: boolean, name: string) { if (cond) pass++; else { fail++; console.log(`FAIL ${name}`); } }

const here = dirname(fileURLToPath(import.meta.url));
const srcRoot = join(here, "..", ".."); // web/src
const repoWeb = join(srcRoot, "..");    // web

// ── 1) Escala CERRADA ─────────────────────────────────────────────────────
ok(BUTTON_VARIANTS.length === 5, "5 variantes");
ok(["primary", "secondary", "danger", "ghost", "success"].every((v) => (BUTTON_VARIANTS as readonly string[]).includes(v)), "variantes esperadas");
ok(BUTTON_SIZES.length === 3, "3 sizes");
ok(["sm", "md", "lg"].every((s) => (BUTTON_SIZES as readonly string[]).includes(s)), "sizes esperados");

// ── 2) buttonClasses: mapeo determinista + escala cerrada ─────────────────
ok(buttonClasses({ variant: "primary", size: "md" }) === "fx-button fx-button--primary fx-button--md", "primary/md");
ok(buttonClasses({ variant: "danger", size: "sm" }) === "fx-button fx-button--danger fx-button--sm", "danger/sm");
ok(buttonClasses() === "fx-button fx-button--secondary fx-button--md", "defaults secondary/md");
ok(buttonClasses({ loading: true }).includes("fx-button--loading"), "loading clase");
// variante/size inválidos → fallback seguro (escala cerrada, nunca inventa clase).
ok(buttonClasses({ variant: "fucsia" as never }).includes("fx-button--secondary"), "variante inválida → secondary");
ok(buttonClasses({ size: "huge" as never }).includes("fx-button--md"), "size inválido → md");
// escape className se anexa al final, no reemplaza las clases canónicas.
{
  const c = buttonClasses({ variant: "ghost", className: "u-mt-2" });
  ok(c.startsWith("fx-button fx-button--ghost") && c.endsWith("u-mt-2"), "className escape anexado");
}
// solo emite clases del design-system + escape (cero clases ad-hoc de color).
ok(buttonClasses({ variant: "primary" }).split(" ").every((c) => c.startsWith("fx-button")), "solo clases fx-button");

// ── 3) LEGACY_BUTTON_MAP cubre las clases ad-hoc del repo ─────────────────
for (const k of ["primary", "ghost", "danger", "mini", "mini primary", "mini danger"]) {
  ok(LEGACY_BUTTON_MAP[k] !== undefined, `legacy map cubre "${k}"`);
}
ok(LEGACY_BUTTON_MAP["mini"].size === "sm", "mini → size sm");
ok(LEGACY_BUTTON_MAP["mini primary"].variant === "primary" && LEGACY_BUTTON_MAP["mini primary"].size === "sm", "mini primary → primary/sm");

// ── 4) El CSS del componente usa SOLO tokens (0 color hex/rgb/hsl) ────────
{
  const css = readFileSync(join(srcRoot, "styles", "buttonComponent.css"), "utf8");
  // strip comentarios /* … */ para no falsear con notas.
  const code = css.replace(/\/\*[\s\S]*?\*\//g, "");
  const hex = code.match(/#[0-9a-fA-F]{3,8}\b/g) || [];
  const rgb = code.match(/\brgba?\s*\(/g) || [];
  const hsl = code.match(/\bhsla?\s*\(/g) || [];
  ok(hex.length === 0, `CSS sin hex de color (encontrados: ${hex.join(",")})`);
  ok(rgb.length === 0, `CSS sin rgb() de color (encontrados: ${rgb.length})`);
  ok(hsl.length === 0, `CSS sin hsl() de color (encontrados: ${hsl.length})`);
  // y SÍ usa tokens var(--…).
  ok(/var\(--color-/.test(code), "CSS usa tokens --color-*");
  // un único radio por size: cada size declara border-radius una vez.
  ok((code.match(/\.fx-button--(sm|md|lg)\s*\{[^}]*border-radius/g) || []).length === 3, "un radio por size");
}

// ── 5) Inventario: botones ad-hoc restantes en la chrome migrable ─────────
// (Shell.tsx queda EXCLUIDO: es zona de otra unidad — stats/nav/rail.)
function walk(dir: string, acc: string[]) {
  for (const e of readdirSync(dir)) {
    if (e === "__tests__" || e === "node_modules") continue;
    const p = join(dir, e);
    if (statSync(p).isDirectory()) walk(p, acc);
    else if (p.endsWith(".tsx") && !p.endsWith("Shell.tsx") && !p.endsWith("Button.tsx")) acc.push(p);
  }
}
{
  const files: string[] = [];
  walk(srcRoot, files);
  // Detecta `<button>` con className legacy del design-system (ghost|primary|
  // danger|secondary|success|mini) en CUALQUIER posición de atributo — no solo
  // como primer atributo. `[^>]*?` consume atributos previos (onClick/disabled/
  // type…) sin cruzar el cierre `>` del tag. El valor de className debe EMPEZAR
  // por una clase legacy (eventualmente `mini primary`, `mini danger`), nunca a
  // mitad de otra clase (`legal-link`, `update-banner-btn`, `btn-primary`…).
  const re = /<button\b[^>]*?\sclassName="(ghost|primary|danger|secondary|success|mini)(\s[^"]*)?"/g;
  let total = 0;
  const offenders: string[] = [];
  for (const f of files) {
    const txt = readFileSync(f, "utf8");
    const n = (txt.match(re) || []).length;
    if (n > 0) { total += n; offenders.push(`${f.replace(repoWeb + "/", "")}: ${n}`); }
  }
  // Tras la migración deben quedar 0 en views/components/wizard/Settings.
  ok(total === 0, `0 botones ad-hoc fuera de Shell.tsx (quedan ${total}: ${offenders.join(" | ")})`);
}

console.log(`buttonVariants: ${pass} passed, ${fail} failed`);
process.exit(fail === 0 ? 0 : 1);
