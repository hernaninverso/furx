import type { Metadata } from "next";
import Link from "next/link";

import Footer from "@/components/Footer";
import Navbar from "@/components/Navbar";

export const metadata: Metadata = {
  title: "Providers — bring the models you already use",
  description:
    "Furx supports 15 BYOK providers: OpenRouter, Anthropic, OpenAI, Gemini API, Google Gemini AI Studio, Groq, Cerebras, Mistral, SambaNova, Ollama, LM Studio, llama.cpp, vLLM, LiteLLM, and custom OpenAI-compatible endpoints. Keys live only in your OS keychain — no proxy.",
  alternates: { canonical: "https://furx.cloud/providers/" },
};

const GROUPS = [
  {
    title: "One key, many models",
    badge: "fastest start",
    items: ["OpenRouter"],
    body:
      "A $10 OpenRouter deposit unlocks 300+ models — the fastest way to run Council Mode with six distinct model families.",
  },
  {
    title: "Free tiers",
    badge: "$0",
    items: ["Cerebras", "Groq", "Mistral", "SambaNova", "Google Gemini AI Studio"],
    body:
      "Separate free accounts, one key each. Daily quotas vary per provider; Furx orchestrates whatever you connect.",
  },
  {
    title: "Paid APIs, direct",
    badge: "your spend",
    items: ["Anthropic", "OpenAI", "Gemini API"],
    body:
      "Your billing, your rate limits, your usage caps. Calls go straight from your machine to the provider — Furx never proxies and never marks up.",
  },
  {
    title: "Local inference",
    badge: "$0 · private",
    items: ["Ollama", "LM Studio", "llama.cpp", "vLLM"],
    body:
      "Auto-detected on their default ports. With a local-only council, model inference never leaves your machine.",
  },
  {
    title: "Gateways & custom",
    badge: "org governance",
    items: ["LiteLLM", "Custom OpenAI-compatible"],
    body:
      "Point Furx at your own gateway. Spend tracking, allow-listing, audit — any endpoint that speaks OpenAI-compatible JSON works.",
  },
];

export default function ProvidersPage() {
  return (
    <>
      <Navbar />
      <main id="main">
        {/* HERO */}
        <section className="max-w-wide mx-auto px-6 pt-16 md:pt-20 pb-12">
          <div className="flex flex-wrap gap-2 mb-8">
            <span className="pill"><span className="dot" /> 15 providers</span>
            <span className="pill">BYOK · keys in the OS keychain</span>
          </div>
          <h1 className="font-display leading-[0.98] tracking-[-0.035em] text-[40px] md:text-[60px] text-ink text-balance max-w-[16ch]">
            Bring the models you <span className="font-italic-serif text-accent">already</span> use.
          </h1>
          <p className="text-lg text-ink-2 mt-7 mb-8 max-w-[52ch] leading-relaxed">
            Fifteen providers, side by side. Your API keys are stored only in the OS keychain —
            never written to disk, never sent to a Furx server. Every call goes straight from
            your machine to the provider. Furx is never in the request path.
          </p>
          <div className="flex flex-wrap gap-3">
            <Link href="/download/" className="btn-primary">Download Furx</Link>
            <Link href="/docs/byok/" className="btn-secondary">BYOK guide</Link>
          </div>
        </section>

        {/* GROUPS */}
        <section className="max-w-wide mx-auto px-6 mt-16">
          <div className="flex justify-between items-baseline border-b border-rule pb-4 mb-10">
            <h2 className="text-3xl md:text-4xl font-semibold text-ink">Five ways in.</h2>
            <span className="font-mono text-xs text-ink-3 uppercase tracking-wider">— mix freely</span>
          </div>
          <div className="grid md:grid-cols-2 lg:grid-cols-3 gap-5">
            {GROUPS.map((g) => (
              <article key={g.title} className="bg-panel border border-rule rounded-lg p-5 hover:border-accent transition-colors">
                <div className="flex items-baseline justify-between mb-2 gap-3">
                  <h3 className="text-base font-semibold text-ink">{g.title}</h3>
                  <span className="pill text-[10px] px-2 py-0.5 shrink-0">{g.badge}</span>
                </div>
                <p className="text-sm text-ink-2 leading-relaxed mb-3">{g.body}</p>
                <div className="flex flex-wrap gap-1.5">
                  {g.items.map((i) => (
                    <span key={i} className="font-mono text-[11px] text-ink-3 bg-bg-soft border border-rule rounded px-1.5 py-0.5">{i}</span>
                  ))}
                </div>
              </article>
            ))}
          </div>
          <p className="text-xs text-ink-3 mt-6 font-mono max-w-[70ch]">
            Add or rotate keys in Settings → Providers, or run the Furx Connect wizard on first
            launch. Free-tier quotas change over time — check each provider&apos;s current terms.
          </p>
        </section>

        {/* TRUST */}
        <section className="max-w-wide mx-auto px-6 mt-24">
          <div className="flex justify-between items-baseline border-b border-rule pb-4 mb-10">
            <h2 className="text-3xl md:text-4xl font-semibold text-ink">
              No proxy. <span className="font-italic-serif text-accent">Ever</span>.
            </h2>
          </div>
          <div className="grid md:grid-cols-3 gap-8">
            <article>
              <h3 className="text-base font-semibold text-ink mb-2">Keychain, not disk</h3>
              <p className="text-sm text-ink-2 leading-relaxed">
                Keys live in the OS keychain — macOS Keychain, Windows Credential Manager,
                libsecret on Linux. Never plaintext on disk, never in telemetry.
              </p>
            </article>
            <article>
              <h3 className="text-base font-semibold text-ink mb-2">Direct calls</h3>
              <p className="text-sm text-ink-2 leading-relaxed">
                Every request goes from your machine to the provider you chose. There is no
                Furx-operated backend in the middle, so there is nothing to mark up.
              </p>
            </article>
            <article>
              <h3 className="text-base font-semibold text-ink mb-2">Verify it</h3>
              <p className="text-sm text-ink-2 leading-relaxed">
                Every dispatch lands in a local append-only SQLite log — an audit trail the
                agent can&apos;t rewrite. The core is Apache-2.0 — read the source.{" "}
                <Link href="/security/" className="text-accent hover:underline">Security page →</Link>
              </p>
            </article>
          </div>
        </section>

        {/* CTA */}
        <section className="max-w-wide mx-auto px-6 mt-24 mb-16 text-center">
          <h2 className="text-3xl md:text-4xl font-semibold mb-4 text-ink">
            Connect one provider. <span className="font-italic-serif text-accent">Council</span> them all.
          </h2>
          <div className="flex flex-wrap gap-3 justify-center">
            <Link href="/download/" className="btn-primary">Download Furx</Link>
            <Link href="/council-mode/" className="btn-ghost">See Council Mode →</Link>
          </div>
        </section>
      </main>
      <Footer />
    </>
  );
}
