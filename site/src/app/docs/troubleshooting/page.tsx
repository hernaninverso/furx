import type { Metadata } from "next";
import PageShell, { Crumbs } from "@/components/PageShell";

export const metadata: Metadata = {
  title: "Troubleshooting",
  description: "Common Furx issues and fixes: Ollama not detected, Apple Gatekeeper, Linux Wayland, Windows SmartScreen, MCP handshake, license trial.",
  alternates: { canonical: "https://furx.cloud/docs/troubleshooting/" },
};

const ISSUES = [
  {
    sym: "Ollama not detected",
    cause: "Ollama isn't listening on the default 127.0.0.1:11434, or you renamed the binary.",
    fix: `Check with: curl -s http://127.0.0.1:11434/api/version
If empty, run: ollama serve &
If on a non-default port: set FURX_OLLAMA_URL=http://127.0.0.1:NNNN in the environment.`,
  },
  {
    sym: "macOS: 'Furx can't be opened because Apple cannot check it'",
    cause: "Gatekeeper has not yet validated the notarization (first launch on a fresh download).",
    fix: `Right-click Furx.app → Open → confirm. macOS will remember the choice.
If stapling is broken: spctl --assess --type execute --verbose Furx.app
If unsigned (you built from source): xattr -cr Furx.app`,
  },
  {
    sym: "Linux: tray icon missing on GNOME 40+",
    cause: "GNOME removed system tray support. Tauri uses libappindicator as fallback.",
    fix: `sudo apt install libappindicator3-1 gir1.2-appindicator3-0.1
# Or use the AppImage which bundles it
chmod +x furx_0.2.0_amd64.AppImage && ./furx_0.2.0_amd64.AppImage`,
  },
  {
    sym: "Linux Wayland: window doesn't render or NVIDIA driver issues",
    cause: "Tauri/wry has known issues with NVIDIA + Wayland.",
    fix: `Set WEBKIT_DISABLE_DMABUF_RENDERER=1 before launching:
WEBKIT_DISABLE_DMABUF_RENDERER=1 furx
# Or add to ~/.profile`,
  },
  {
    sym: "Windows: SmartScreen 'PC protected'",
    cause: "Fresh release hasn't accumulated reputation yet (will fade after ~1000 installs).",
    fix: `Click 'More info' → 'Run anyway'. We sign with Azure Trusted Signing — the cert is valid,
SmartScreen just doesn't recognize it yet because it's new.`,
  },
  {
    sym: "MCP server connection fails",
    cause: "MCP server binary not on PATH, or wrong env / args.",
    fix: `Check: Furx → Settings → MCP → click the failing server → see stderr.
Common: env vars not exported. Use ref:keychain:<alias> instead of plaintext env.`,
  },
  {
    sym: "Council Mode: 'All voices failed'",
    cause: "Every provider in the preset is rate-limited, errored, or timed out.",
    fix: `Switch to a different preset (Local works if you have Ollama).
Check provider status in Settings → Connect → see which is yellow/red.
Wait for rate-limit window to clear (Cerebras 1M tok/day resets at UTC midnight).`,
  },
  {
    sym: "Trial says 'expired' on day 5",
    cause: "Clock skew between your machine and our license API.",
    fix: `Sync your system clock (sudo sntp -sS time.apple.com on macOS).
Then in Furx: Settings → Account → Refresh license.`,
  },
  {
    sym: "Auto-update fails with 'minisign signature mismatch'",
    cause: "Corrupted download or the wrong updater key (we rotated 2026-05-27).",
    fix: `Re-download from /download/ manually and reinstall.
The bundled pubkey is in tauri.conf.json; if you have a v0.1.x build, auto-update is intentionally broken — manual install required for v0.2.0.`,
  },
  {
    sym: "Audit log file growing too large",
    cause: "Council dispatches generate ~1 KB per voice; heavy use can hit 100 MB / month.",
    fix: `Settings → Audit → Retention → set 90 days.
Manual: furx audit prune --before "30 days ago".
Cloud sync (Pro+) only keeps the last 30 days server-side.`,
  },
];

export default function TroubleshootingPage() {
  return (
    <PageShell wide>
      <Crumbs items={[{ label: "Docs", href: "/docs/" }, { label: "Troubleshooting" }]} />
      <article className="prose-furx">
        <h1>Troubleshooting</h1>
        <p>
          The most common issues people hit. If yours isn&apos;t here, run{" "}
          <code>furx doctor</code> (writes a redacted report) and open a Discussion on{" "}
          <a href="https://github.com/hernaninverso/furx/discussions" target="_blank" rel="noopener noreferrer">GitHub</a>.
        </p>

        {ISSUES.map((i) => (
          <section key={i.sym} className="not-prose mb-8 bg-panel border border-rule rounded-lg p-5">
            <h2 className="text-lg font-display font-medium text-accent mb-2">{i.sym}</h2>
            <p className="text-ink-2 text-sm mb-3"><strong className="text-ink">Cause:</strong> {i.cause}</p>
            <div>
              <strong className="text-ink text-sm">Fix:</strong>
              <pre className="code-block text-xs mt-2 whitespace-pre-wrap">{i.fix}</pre>
            </div>
          </section>
        ))}

        <h2>furx doctor</h2>
        <p>
          Run this first if anything weird happens. It checks: Tauri version, signing cert, audit
          DB integrity, Keychain access, provider connectivity, Ollama presence, MCP servers reachable.
        </p>
        <pre>{`$ furx doctor

✓ Tauri 2.7.1 OK
✓ Audit DB ~/.furx/furx.db OK (52,401 events, 12 MB)
✓ Keychain readable (3 entries: furx-provider-openrouter, furx-provider-anthropic, furx-license)
✗ Ollama: 127.0.0.1:11434 not responding (start with: ollama serve)
✓ MCP filesystem OK (5 tools)
✓ License API reachable (last-sync 2 min ago)

Report saved to /tmp/furx-doctor-2026-05-27.txt`}</pre>
      </article>
    </PageShell>
  );
}
