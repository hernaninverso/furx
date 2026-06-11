import type { Metadata } from "next";
import Link from "next/link";

import Footer from "@/components/Footer";
import Navbar from "@/components/Navbar";

export const metadata: Metadata = {
  title: "Download",
  description: "Download Furx for macOS (Apple Silicon DMG, Developer ID signed + notarized). Linux and Windows builds land via CI — or build from source today (Apache-2.0).",
  alternates: { canonical: "https://furx.cloud/download/" },
};

const RELEASE_BASE = process.env.NEXT_PUBLIC_RELEASE_BASE || "https://github.com/hernaninverso/furx/releases/latest/download";
const RELEASES_URL = "https://github.com/hernaninverso/furx/releases";
const VERSION = "0.2.0";

type Artifact = { label: string; file: string };
const PLATFORMS: {
  os: string;
  badge: string;
  primary: Artifact | null;
  notes: string;
  secondary: Artifact[];
  detect: string[];
}[] = [
  {
    os: "macOS",
    badge: "Apple Silicon · notarized",
    primary: { label: `Download .dmg (${VERSION})`, file: `Furx_${VERSION}_aarch64.dmg` },
    notes: "Signed with an Apple Developer ID and notarized. Gatekeeper-friendly. macOS 12 (Monterey) or newer, Apple Silicon. Intel: build from source below.",
    secondary: [],
    detect: ["mac"],
  },
  {
    os: "Linux",
    badge: "coming via CI",
    primary: null,
    notes: "deb / rpm / AppImage builds land through GitHub Actions shortly after launch — watch Releases. Building from source works today (the codebase cross-compiles cleanly).",
    secondary: [],
    detect: ["linux", "x11", "wayland"],
  },
  {
    os: "Windows",
    badge: "coming via CI",
    primary: null,
    notes: "The Windows installer lands through GitHub Actions — watch Releases. The code is verified to compile for Windows; building from source works today.",
    secondary: [],
    detect: ["win"],
  },
];

const CHECKSUMS = [
  { file: `Furx_${VERSION}_aarch64.dmg`, sha256: "2cab75c8e2a5647da11c5e708ff56060647f71e4256186d96c756e638f3e0351" },
];

export default function DownloadPage() {
  return (
    <>
      <Navbar />
      <main id="main" className="max-w-wide mx-auto px-6 pt-16 pb-24">
        <header className="mb-12">
          <div className="brand-mark mb-6 text-[30px]" aria-hidden="true" />
          <h1 className="text-4xl md:text-5xl font-extrabold mb-3 text-balance">Download Furx {VERSION}</h1>
          <p className="text-ink-2 text-lg max-w-3xl">
            All builds are signed and reproducible from{" "}
            <a href="https://github.com/hernaninverso/furx" className="text-accent hover:underline" target="_blank" rel="noopener noreferrer">source on GitHub</a>.
            14-day Pro trial activates on first launch, no credit card.
          </p>
        </header>

        <div className="grid md:grid-cols-3 gap-5 mb-16">
          {PLATFORMS.map((p) => (
            <article key={p.os} className="bg-panel border border-rule rounded-lg p-6 flex flex-col">
              <div className="flex items-baseline justify-between mb-3">
                <h2 className="text-2xl font-display font-medium">{p.os}</h2>
                <span className="pill text-[10px]">{p.badge}</span>
              </div>
              {p.primary ? (
                <a href={`${RELEASE_BASE}/${p.primary.file}`} className="btn-primary text-sm justify-center mb-3">
                  {p.primary.label}
                </a>
              ) : (
                <a href={RELEASES_URL} target="_blank" rel="noopener noreferrer" className="btn-secondary text-sm justify-center mb-3">
                  Watch GitHub Releases
                </a>
              )}
              <p className="text-xs text-ink-3 leading-relaxed mb-3">{p.notes}</p>
              {p.secondary.length > 0 && (
                <details className="text-sm text-ink-2">
                  <summary className="cursor-pointer hover:text-ink font-mono text-xs">other formats</summary>
                  <ul className="mt-2 space-y-1.5 pl-3">
                    {p.secondary.map((s) => (
                      <li key={s.file}>
                        <a href={`${RELEASE_BASE}/${s.file}`} className="text-accent hover:underline text-xs font-mono">
                          {s.label}
                        </a>
                      </li>
                    ))}
                  </ul>
                </details>
              )}
            </article>
          ))}
        </div>

        {/* Install via package manager */}
        <section className="mb-16">
          <h2 className="text-2xl font-bold mb-4">Install via terminal</h2>
          <div className="grid md:grid-cols-2 gap-5">
            <div>
              <div className="text-xs text-ink-3 font-mono mb-2 uppercase tracking-wider">macOS</div>
              <pre className="code-block text-xs">{`# Direct DMG
curl -L ${RELEASE_BASE}/Furx_${VERSION}_aarch64.dmg \\
  -o ~/Downloads/Furx.dmg
open ~/Downloads/Furx.dmg`}</pre>
            </div>
            <div>
              <div className="text-xs text-ink-3 font-mono mb-2 uppercase tracking-wider">Linux / Windows (from source)</div>
              <pre className="code-block text-xs">{`git clone https://github.com/hernaninverso/furx && cd furx
npm install
npx tauri build
# prebuilt packages land on GitHub Releases via CI`}</pre>
            </div>
          </div>
        </section>

        {/* Checksums */}
        <section className="mb-16">
          <h2 className="text-2xl font-bold mb-2">Checksums &amp; signatures</h2>
          <p className="text-ink-2 text-sm mb-5">
            Verify with <code className="text-accent">shasum -a 256 &lt;file&gt;</code> and compare against the
            table below (also published on each{" "}
            <a href={RELEASES_URL} target="_blank" rel="noopener noreferrer" className="text-accent hover:underline">GitHub Release</a>).
          </p>
          <div className="overflow-x-auto border border-rule rounded-lg">
            <table className="w-full text-xs font-mono">
              <thead className="bg-bg-soft">
                <tr>
                  <th className="text-left px-4 py-2 text-ink">File</th>
                  <th className="text-left px-4 py-2 text-ink">SHA256</th>
                </tr>
              </thead>
              <tbody>
                {CHECKSUMS.map((c) => (
                  <tr key={c.file}>
                    <td className="px-4 py-2 text-ink-2">{c.file}</td>
                    <td className="px-4 py-2 text-ink-3 truncate max-w-md">{c.sha256}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <p className="text-xs text-ink-3 mt-3">
            Checksums for every artifact are listed on the corresponding{" "}
            <a href={RELEASES_URL} target="_blank" rel="noopener noreferrer" className="text-accent hover:underline">GitHub Release</a> page.
          </p>
        </section>

        {/* From source */}
        <section className="mb-16">
          <h2 className="text-2xl font-bold mb-4">Build from source</h2>
          <p className="text-ink-2 text-sm mb-4">
            Requires Rust 1.85+, Node 20+, and platform Tauri prerequisites
            (<a href="https://v2.tauri.app/start/prerequisites/" className="text-accent hover:underline" target="_blank" rel="noopener noreferrer">see Tauri docs</a>).
          </p>
          <pre className="code-block text-xs md:text-sm">{`git clone https://github.com/hernaninverso/furx && cd furx
npm install
(cd src-tauri && cargo build --release)
npx tauri build --bundles app

# Install as ~/Applications/Furx.app (macOS, dev cert)
SRC=src-tauri/target/release/bundle/macos/Furx.app
DEST=/Applications/Furx.app
rm -rf "$DEST" && ditto "$SRC" "$DEST" && xattr -cr "$DEST"
open "$DEST"`}</pre>
        </section>

        <section className="text-center text-ink-3 text-sm">
          Older versions on{" "}
          <a href="https://github.com/hernaninverso/furx/releases" target="_blank" rel="noopener noreferrer" className="text-accent hover:underline">GitHub Releases</a>.
          See the <Link href="/changelog/" className="text-accent hover:underline">changelog</Link> for what&apos;s new in {VERSION}.
        </section>
      </main>
      <Footer />
    </>
  );
}
