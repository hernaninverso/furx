import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// `base: "./"` REQUIRED for Tauri 2 production — under `tauri://localhost/` absolute paths fail silently.
export default defineConfig({
  root: "web",
  base: "./",
  publicDir: "public",
  plugins: [react()],
  build: { outDir: "../dist", emptyOutDir: true, target: "esnext" },
  server: { port: 1421, strictPort: true, host: "127.0.0.1" },
  clearScreen: false,
  envPrefix: ["VITE_", "TAURI_"],
});
