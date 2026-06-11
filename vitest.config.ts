// 022 US13 · L2/L3 — runner Vitest para componentes (RTL + @tauri-apps/api/mocks) y a11y (axe).
// ──────────────────────────────────────────────────────────────────────────
// ADITIVO: NO reemplaza `scripts/test-all.mjs` (L1, node type-strip de `*.test.ts`). Este runner
// SÓLO levanta los tests de componente/a11y, que son `*.test.tsx` (JSX → necesitan jsdom + plugin
// react + transformación). Para que los dos runners NO colisionen:
//   - `include` matchea SÓLO `**/*.test.tsx` (los `*.test.ts` de L1 quedan fuera de Vitest).
//   - el runner L1 (test-all.mjs) descubre SÓLO `*.test.ts` (los `.tsx` quedan fuera de L1).
// Así `npm test` (L1) y `npm run test:components`/`test:a11y` (L2/L3) son ortogonales.
import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  test: {
    // SÓLO componentes/a11y (JSX). Los `*.test.ts` de L1 los corre scripts/test-all.mjs.
    include: ["web/src/**/*.test.tsx"],
    environment: "jsdom",
    globals: true,
    setupFiles: ["web/src/test/setup.ts"],
    css: false,
    // No tocar los tests node-runner de L1 ni node_modules.
    exclude: ["node_modules/**", "dist/**", "web/src/**/*.test.ts"],
  },
});
