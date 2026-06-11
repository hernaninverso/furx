import type { Metadata } from "next";
import Link from "next/link";

import Footer from "@/components/Footer";
import Navbar from "@/components/Navbar";

export const metadata: Metadata = {
  title: "Council Mode — one prompt, up to six models, one synthesis",
  description:
    "Council Mode (⌘J) sends one prompt to up to six models in parallel and synthesizes a consensus you can inspect. Five presets, custom voices, a live cost estimator, and full history. Free and Apache-2.0.",
  alternates: { canonical: "https://furx.cloud/council-mode/" },
};

const STEPS = [
  {
    n: "01",
    title: "Pick your voices",
    body:
      "Five presets — quick, cheapo, frontier, local, mix — or build your own from 15 providers. Templates pick voices by workflow phase: planning, implementation, review, debug, refactor.",
  },
  {
    n: "02",
    title: "Dispatch",
    body:
      "⌘J sends one prompt to every voice in parallel. Watch them work side by side, with a live cost estimate before you commit. Graceful degradation if one voice rate-limits.",
  },
  {
    n: "03",
    title: "Read the verdict",
    body:
      "Furx collects every answer and synthesizes a consensus you can inspect, diff, and act on. Vote with a number key, merge, or re-dispatch. Every run is kept in history.",
  },
];

const WHY = [
  {
    title: "Competing answers, not a first guess",
    body:
      "One model's first answer is a sample, not a signal. Six models on the same problem expose the disagreements that matter — before you bet a refactor on them.",
  },
  {
    title: "Real agents do the work",
    body:
      "Comparison tools show you chat. In Furx the council runs next to real coding agents — Claude Code, Codex, Gemini CLI, Aider — in terminals that read your repo, edit files, and run tests.",
  },
  {
    title: "Cost before dispatch",
    body:
      "The estimator prices a council run before you fire it. Free-tier and local presets run at $0; paid keys show your real spend. Voice count is never a paid feature.",
  },
];

export default function CouncilModePage() {
  return (
    <>
      <Navbar />
      <main id="main">
        {/* HERO */}
        <section className="max-w-wide mx-auto px-6 pt-16 md:pt-20 pb-12">
          <div className="flex flex-wrap gap-2 mb-8">
            <span className="pill"><span className="dot" /> Free · Apache-2.0</span>
            <span className="pill">⌘J in any pane</span>
          </div>
          <h1 className="font-display leading-[0.98] tracking-[-0.035em] text-[40px] md:text-[60px] text-ink text-balance max-w-[18ch]">
            One prompt. Up to <span className="font-italic-serif text-accent">six</span> models. One synthesis.
          </h1>
          <p className="text-lg text-ink-2 mt-7 mb-8 max-w-[52ch] leading-relaxed">
            Council Mode dispatches the same prompt to up to six models at once, collects every
            answer, and synthesizes a consensus you can inspect — so you stop copy-pasting
            between tabs and stop betting your code on one model&apos;s first answer.
          </p>
          <div className="flex flex-wrap gap-3">
            <Link href="/download/" className="btn-primary">Download Furx</Link>
            <Link href="/docs/council/" className="btn-secondary">Read the docs</Link>
          </div>
        </section>

        {/* HOW */}
        <section className="max-w-wide mx-auto px-6 mt-16">
          <div className="flex justify-between items-baseline border-b border-rule pb-4 mb-10">
            <h2 className="text-3xl md:text-4xl font-semibold text-ink">How it works.</h2>
            <span className="font-mono text-xs text-ink-3 uppercase tracking-wider">— 3 steps</span>
          </div>
          <div className="grid md:grid-cols-3 gap-8">
            {STEPS.map((s) => (
              <article key={s.n}>
                <div className="font-italic-serif text-5xl text-accent mb-3">{s.n}</div>
                <h3 className="text-xl font-semibold mb-2 text-ink">{s.title}</h3>
                <p className="text-ink-2 text-sm leading-relaxed">{s.body}</p>
              </article>
            ))}
          </div>
        </section>

        {/* WHY */}
        <section className="max-w-wide mx-auto px-6 mt-24">
          <div className="flex justify-between items-baseline border-b border-rule pb-4 mb-10">
            <h2 className="text-3xl md:text-4xl font-semibold text-ink">
              Why a <span className="font-italic-serif text-accent">council</span>.
            </h2>
          </div>
          <div className="grid md:grid-cols-3 gap-5">
            {WHY.map((w) => (
              <article key={w.title} className="bg-panel border border-rule rounded-lg p-5">
                <h3 className="text-base font-semibold text-ink mb-2">{w.title}</h3>
                <p className="text-sm text-ink-2 leading-relaxed">{w.body}</p>
              </article>
            ))}
          </div>
        </section>

        {/* TRUST */}
        <section className="max-w-wide mx-auto px-6 mt-24">
          <blockquote className="font-italic-serif text-2xl md:text-3xl text-ink leading-snug max-w-[36ch]">
            &quot;Dispatch many. Watch them <span className="text-accent">disagree</span>. Ship the winner.&quot;
          </blockquote>
          <p className="text-ink-2 mt-6 max-w-[60ch] leading-relaxed text-sm">
            Every council call goes straight from your machine to the providers you connected.
            Your keys never leave your machine. No proxy. The core is Apache-2.0 — read the
            source. Details on the{" "}
            <Link href="/security/" className="text-accent hover:underline">security page</Link>.
          </p>
        </section>

        {/* CTA */}
        <section className="max-w-wide mx-auto px-6 mt-24 mb-16 text-center">
          <h2 className="text-3xl md:text-4xl font-semibold mb-4 text-ink">
            Run your first council in <span className="font-italic-serif text-accent">five minutes</span>.
          </h2>
          <p className="text-ink-2 mb-8 max-w-[56ch] mx-auto leading-relaxed">
            Install, connect a provider — a free tier is enough — open a pane, hit ⌘J.
            Council Mode ships in the free core — up to six voices per dispatch, and the
            voice count is never a paid feature. Your keys never leave your machine. No proxy.
          </p>
          <div className="flex flex-wrap gap-3 justify-center">
            <Link href="/download/" className="btn-primary">Download Furx</Link>
            <Link href="/providers/" className="btn-ghost">See all 15 providers →</Link>
          </div>
        </section>
      </main>
      <Footer />
    </>
  );
}
