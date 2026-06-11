import type { Metadata } from "next";
import Link from "next/link";

import Footer from "@/components/Footer";
import Navbar from "@/components/Navbar";

export const metadata: Metadata = {
  title: "Documentation",
  description:
    "Furx docs: quickstart, BYOK guide, Council Mode, providers, integrations (MCP + Skills), Keychain, audit log, troubleshooting, CLI reference.",
  alternates: { canonical: "https://furx.cloud/docs/" },
};

const SECTIONS = [
  {
    title: "Get started",
    pages: [
      { href: "/docs/quickstart/", title: "Quickstart", desc: "Install + first launch + Furx Connect wizard in 5 minutes." },
      { href: "/docs/byok/", title: "BYOK guide", desc: "How Furx passes keys through to providers without ever touching our database." },
      { href: "/docs/council/", title: "Council Mode", desc: "Dispatch one prompt to as many LLMs as you wire up — presets, role Templates, voting, cost." },
    ],
  },
  {
    title: "Providers & integrations",
    pages: [
      { href: "/docs/providers/", title: "Provider list", desc: "Cerebras, Groq, OpenRouter, Anthropic, OpenAI, Gemini, Ollama, LM Studio, llama.cpp, vLLM." },
      { href: "/docs/integrations/", title: "Integrations", desc: "MCP servers, Claude Code Skills, gh CLI, claude-as-* wrappers, custom proxy / LiteLLM." },
    ],
  },
  {
    title: "Security & data",
    pages: [
      { href: "/docs/keychain/", title: "Keychain & secrets", desc: "Where keys live on macOS, Linux, Windows. Rotation, revocation, export." },
      { href: "/docs/audit/", title: "Audit log", desc: "SQLite schema, triggers, retention, export to JSON / CSV / .furxreplay." },
      { href: "/docs/dpia-traces/", title: "DPIA — cloud traces", desc: "Data Protection Impact Assessment template for the optional cloud traces feature." },
      { href: "/docs/dpia-persona-pack/", title: "DPIA — persona packs", desc: "DPIA template for persona packs distilled from your approved traces." },
    ],
  },
  {
    title: "Operations",
    pages: [
      { href: "/docs/troubleshooting/", title: "Troubleshooting", desc: "Common issues: Ollama not detected, Apple Gatekeeper, Linux Wayland, Windows SmartScreen." },
    ],
  },
];

export default function DocsHubPage() {
  return (
    <>
      <Navbar />
      <main id="main" className="max-w-wide mx-auto px-6 pt-16 pb-24">
        <header className="mb-12">
          <div className="brand-mark mb-6 text-[26px]" aria-hidden="true" />
          <h1 className="text-4xl md:text-5xl font-extrabold mb-3 text-balance">Documentation</h1>
          <p className="text-ink-2 text-lg max-w-3xl">
            How to install, configure, and use Furx. Everything you need to ship in 5 minutes.
            For deeper architecture, see the{" "}
            <a href="https://github.com/hernaninverso/furx" className="text-accent hover:underline" target="_blank" rel="noopener noreferrer">repo README</a>.
          </p>
        </header>

        {SECTIONS.map((section) => (
          <section key={section.title} className="mb-12">
            <h2 className="text-xl font-sans font-bold mb-4 text-ink uppercase tracking-wider text-sm font-mono">
              {section.title}
            </h2>
            <div className="grid md:grid-cols-2 lg:grid-cols-3 gap-4">
              {section.pages.map((p) => (
                <Link
                  key={p.href}
                  href={p.href}
                  className="bg-panel border border-rule rounded-lg p-5 hover:border-accent-dim transition-colors block"
                >
                  <h3 className="text-lg font-display font-medium text-accent mb-2">{p.title}</h3>
                  <p className="text-ink-2 text-sm leading-relaxed">{p.desc}</p>
                </Link>
              ))}
            </div>
          </section>
        ))}

        <section className="mt-16 bg-panel border border-rule rounded-lg p-6">
          <h2 className="text-lg font-display font-medium mb-3">Need something not here?</h2>
          <p className="text-ink-2 text-sm mb-4">
            We don&apos;t have a hosted search index yet (Pagefind ships in the next site release).
            For now: <code className="text-accent">⌘F</code> on this page, GitHub Discussions, or Discord.
          </p>
          <div className="flex flex-wrap gap-3">
            <a href="https://github.com/hernaninverso/furx/discussions" className="btn-secondary text-sm" target="_blank" rel="noopener noreferrer">
              GitHub Discussions
            </a>
            <Link href="/community/" className="btn-secondary text-sm">All channels</Link>
          </div>
        </section>
      </main>
      <Footer />
    </>
  );
}
