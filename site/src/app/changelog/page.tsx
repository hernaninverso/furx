import type { Metadata } from "next";
import Link from "next/link";

import Footer from "@/components/Footer";
import Navbar from "@/components/Navbar";

export const metadata: Metadata = {
  title: "Changelog",
  description: "Furx release notes — versions, features, fixes, breaking changes. RSS + JSON Feed.",
  alternates: {
    canonical: "https://furx.cloud/changelog/",
    types: {
      "application/rss+xml": [{ url: "/changelog/rss.xml", title: "Furx changelog" }],
      "application/feed+json": [{ url: "/changelog/feed.json", title: "Furx changelog (JSON)" }],
    },
  },
};

const RELEASES = [
  {
    version: "0.2.0",
    date: "2026-05-27",
    type: "minor",
    headline: "BYOK universal · Furx Connect wizard · Council Mode multi-provider",
    items: {
      added: [
        "Furx Connect wizard (6 paths: OpenRouter / free tiers / paid / local / proxy / mix).",
        "Council Mode dispatch across distinct provider families (⌘J).",
        "Resilience layer in Rust (rate-limit / quota / circuit per provider).",
        "Cost estimator inline in CouncilModal.",
        "Connect status panel in Settings with health badges.",
        "Linux DEB / RPM / AppImage builds.",
        "Windows MSI signed via Azure Trusted Signing.",
      ],
      changed: [
        "No more bundled PyApp AIE sidecar — dispatcher is fully native Rust now (−60 MB).",
        "Trial logic moved server-side (license API) — no more local-clock spoofing.",
      ],
      fixed: [
        "Memory daemon UMP handshake on cold start.",
        "macOS notarization stapling on aarch64 DMG.",
      ],
    },
  },
  {
    version: "0.1.0",
    date: "2026-05-12",
    type: "stable-baseline",
    headline: "First public stable — terminal orchestrator + audit log + MCP",
    items: {
      added: [
        "Multi-pane PTY grid (any number of panes) with shared cards rail.",
        "Append-only audit log (SQLite WAL + DDL triggers).",
        "⌘P search (code + memories + git), ⌘K palette, ⌘B broadcast.",
        "MCP server health probes + tools/list handshake.",
        "Voice → text via whisper.cpp.",
        "Auto-update (Tauri minisign Ed25519).",
        "Crash capture with PII scrub.",
      ],
    },
  },
];

export default function ChangelogPage() {
  return (
    <>
      <Navbar />
      <main id="main" className="max-w-base mx-auto px-6 pt-16 pb-24">
        <header className="mb-12">
          <h1 className="text-4xl font-extrabold mb-3">Changelog</h1>
          <p className="text-ink-2 text-lg max-w-2xl mb-3">
            Versions follow SemVer. Subscribe via{" "}
            <a href="/changelog/rss.xml" className="text-accent hover:underline">RSS</a> or{" "}
            <a href="/changelog/feed.json" className="text-accent hover:underline">JSON Feed</a>.
          </p>
          <p className="text-ink-3 text-sm">
            Releases also live on{" "}
            <a href="https://github.com/hernaninverso/furx/releases" className="text-accent hover:underline" target="_blank" rel="noopener noreferrer">
              GitHub Releases
            </a>{" "}
            with attached binaries + SHA256 + minisign signatures.
          </p>
        </header>

        <div className="space-y-12">
          {RELEASES.map((r) => (
            <article key={r.version} className="border-l-2 border-accent-pale pl-6 pb-4">
              <header className="mb-4">
                <div className="flex flex-wrap items-baseline gap-3">
                  <h2 className="text-2xl font-mono font-bold text-accent">v{r.version}</h2>
                  <span className="text-ink-3 text-sm font-mono">{r.date}</span>
                  <span className="pill text-[10px]">{r.type}</span>
                </div>
                <p className="text-ink-2 mt-2 text-lg">{r.headline}</p>
              </header>
              {r.items.added && (
                <section className="mb-4">
                  <h3 className="text-sm uppercase tracking-wider text-ok font-mono mb-2">Added</h3>
                  <ul className="space-y-1.5 text-ink-2 text-sm list-disc pl-5">
                    {r.items.added.map((i) => <li key={i}>{i}</li>)}
                  </ul>
                </section>
              )}
              {r.items.changed && (
                <section className="mb-4">
                  <h3 className="text-sm uppercase tracking-wider text-warn font-mono mb-2">Changed</h3>
                  <ul className="space-y-1.5 text-ink-2 text-sm list-disc pl-5">
                    {r.items.changed.map((i) => <li key={i}>{i}</li>)}
                  </ul>
                </section>
              )}
              {r.items.fixed && (
                <section className="mb-4">
                  <h3 className="text-sm uppercase tracking-wider text-info font-mono mb-2">Fixed</h3>
                  <ul className="space-y-1.5 text-ink-2 text-sm list-disc pl-5">
                    {r.items.fixed.map((i) => <li key={i}>{i}</li>)}
                  </ul>
                </section>
              )}
            </article>
          ))}
        </div>

        <div className="mt-16 text-center text-ink-3 text-sm">
          <Link href="/" className="text-accent hover:underline">← Home</Link>{" "}
          · <Link href="/download/" className="text-accent hover:underline">Download latest</Link>{" "}
          · <Link href="/community/" className="text-accent hover:underline">Community</Link>
        </div>
      </main>
      <Footer />
    </>
  );
}
