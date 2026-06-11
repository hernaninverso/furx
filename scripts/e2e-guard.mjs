// 022 US13 · L4 — guard de plataforma para `npm run test:e2e`.
// El e2e con tauri-driver SÓLO corre en Linux (Apple no provee WebDriver de escritorio; tauri-driver
// soporta WebKitWebDriver/Linux). En macOS/Windows sale con un mensaje claro SIN ejecutar nada (no
// rompe la build local). En Linux, delega a WebdriverIO con e2e/wdio.conf.ts.
import { platform } from "node:os";
import { spawnSync } from "node:child_process";

if (platform() !== "linux") {
  console.log(
    "[L4 e2e] tauri-driver sólo corre en CI Linux (no en " + platform() + ").\n" +
    "         Apple no provee WebDriver de escritorio. Ver e2e/README.md.\n" +
    "         Este comando es un no-op fuera de Linux (exit 0).",
  );
  process.exit(0);
}

// En Linux: corre WebdriverIO con la config de tauri-driver.
// (Requiere: webkit2gtk-driver + `cargo install tauri-driver` + `npm run tauri build`. Ver README.)
const r = spawnSync("npx", ["wdio", "run", "e2e/wdio.conf.ts"], { stdio: "inherit" });
process.exit(r.status ?? 1);
