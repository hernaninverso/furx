import Link from "next/link";

import Footer from "@/components/Footer";
import Navbar from "@/components/Navbar";
import HeroCarousel from "@/components/HeroCarousel";
import LeadCapture from "@/components/LeadCapture";

const APP_URL = process.env.NEXT_PUBLIC_APP_URL || "https://app.furx.cloud";
const GH_REPO = process.env.NEXT_PUBLIC_GH_REPO || "https://github.com/hernaninverso/furx";

const PATHS = [
  {
    id: "openrouter",
    title: "OpenRouter Quick Start",
    badge: "1 key · 30s",
    body:
      "One key, hundreds of models. Council Mode picks distinct providers from the OpenRouter catalog — Claude, GPT, Gemini, Qwen, DeepSeek, Llama-4 — and dispatches to as many as you wire up.",
  },
  {
    id: "free",
    title: "Free tiers",
    badge: "$0 / forever",
    body:
      "Separate free accounts: Cerebras, Groq, Mistral, SambaNova, Gemini AI Studio, OpenRouter free. Daily quotas vary per provider; Furx orchestrates whatever you connect.",
  },
  {
    id: "paid",
    title: "Paid APIs direct",
    badge: "your spend",
    body:
      "Anthropic, OpenAI, Gemini paid, Cerebras paid, or any OpenAI-compatible endpoint. Your billing, your rate limits, your usage caps. Furx never proxies.",
  },
  {
    id: "local",
    title: "Local inference",
    badge: "$0 · privacy",
    body:
      "Auto-detects Ollama (127.0.0.1:11434), LM Studio, llama.cpp, vLLM. Council with local-only models: inference never leaves your machine.",
  },
  {
    id: "proxy",
    title: "Proxy / LiteLLM",
    badge: "org governance",
    body:
      "Paste your org proxy URL. Spend tracking, allow-listing, audit — Furx doesn't care which gateway you use as long as it speaks OpenAI-compatible JSON.",
  },
  {
    id: "mix",
    title: "Mix-and-match",
    badge: "Council preset",
    body:
      "Combine all five. Council dispatches across whichever providers are healthy. Graceful degradation if one voice rate-limits, hard timeout 40s.",
  },
];

const TEMPLATES = [
  { name: "Planning", models: "Opus · GPT-5 · Qwen-235B · DeepSeek-V3.1", when: "Spec, architecture, high-level decisions. Heavy reasoning, time-tolerant." },
  { name: "Implementation", models: "Sonnet · Codex · Gemini Flash · Aider", when: "Diff & code generation. Speed and code focus." },
  { name: "Review", models: "Opus · GPT-5 · Gemini Pro · Llama-4", when: "PR review, audit. Diverse families for cross-family blind spots." },
  { name: "Debug", models: "Sonnet · Codex · Aider · local qwen-coder", when: "Stack-trace aware. Cloud + local mix to keep velocity." },
  { name: "Refactor", models: "Gemini 2.5 Pro · Opus · GPT-5", when: "Large codebases. Models with >200K-token context window." },
];

const NEVER_CHARGE = [
  "Provider keys (OS Keychain only — Furx can't read them after you store)",
  "Prompts you send (machine → provider, never via us)",
  "Responses you receive (same)",
  "Number of voices in Council Mode (one or all six — same price: $0)",
  "Number of panes (open as many as your screen fits)",
  "Local audit log in ~/.furx/furx.db",
  "Local Memory Hub (SQLite + FTS5 + Knowledge Graph)",
  "Skills you write in ~/.furx/skills or ~/.claude/skills",
  "Voice transcription (whisper.cpp local, never uploaded)",
  "Mobile bridge (point-to-point WS on your LAN or Tailscale)",
];

const PRICING_TEASER = [
  { tier: "Free", price: "$0", sub: "Apache-2.0 · forever", highlight: false },
  { tier: "Pro", price: "$12", sub: "/mo · 14-day trial", highlight: true },
  { tier: "Team", price: "$30", sub: "/seat/mo · 5+ seats", highlight: false },
  { tier: "Enterprise", price: "$49", sub: "/seat or $2.5k perpetual", highlight: false },
];

const FAQ = [
  {
    q: "Do you take a cut of my LLM costs?",
    a: "Never. Pass-through only. Furx never sees your provider keys — they live in your OS Keychain, sent directly from your machine to the provider. There is no proxy and nothing to mark up.",
  },
  {
    q: "What does Pro pay for if not voices?",
    a: "Cloud sync of your skills, .mcp.json, and Memory Hub backups. Session replay (30 days). Cost Meter Pro with alerts and CSV export. Latency heatmap trends. Encrypted daily backups. Council Mode itself is identical on Free and Pro.",
  },
  {
    q: "How many models can the Council dispatch to at once?",
    a: "Up to six voices per dispatch. The five built-in presets fill all six with OpenRouter Quick Start, and you can swap any voice or save your own preset. Free, Pro, Team — all the same.",
  },
  {
    q: "What's the trial?",
    a: "14 days Pro on first install, no credit card. Auto-reverts to Free if you don't convert. The trial unlocks cloud sync and replay; everything you can do offline keeps working.",
  },
  {
    q: "Can I self-host Pro/Team?",
    a: "Enterprise tier ($49/seat or $2.5k perpetual). Self-hosted notarized build, data residency Argentina or EU, source-code escrow via NCC Group.",
  },
  {
    q: "Refund policy?",
    a: "14-day no-questions refund via Paddle MoR. Paddle handles EU VAT, AR Argentina, US sales tax automatically.",
  },
];

export default function HomePage() {
  const faqJsonLd = {
    "@context": "https://schema.org",
    "@type": "FAQPage",
    mainEntity: FAQ.map((f) => ({
      "@type": "Question",
      name: f.q,
      acceptedAnswer: { "@type": "Answer", text: f.a },
    })),
  };

  return (
    <>
      <Navbar />
      <main id="main">
        {/* HERO */}
        <section className="max-w-wide mx-auto px-6 pt-16 md:pt-20 pb-12">
          <div className="grid lg:grid-cols-[1.05fr_1fr] gap-12 lg:gap-14 items-center">
            <div>
              <div className="flex flex-wrap gap-2 mb-8">
                <span className="pill"><span className="dot" /> v0.2 · macOS · Linux · Windows</span>
                <span className="pill">Apache-2.0 core · BYOK</span>
              </div>
              <h1 className="font-display leading-[0.96] tracking-[-0.035em] text-[44px] md:text-[64px] lg:text-[72px] text-ink text-balance">
                <span className="block">One layer under</span>
                <span className="block"><span className="font-italic-serif text-accent">every</span> coding agent.</span>
              </h1>
              <p className="text-lg text-ink-2 mt-7 mb-3 max-w-[48ch] leading-relaxed">
                One layer that works across Claude Code, Codex, Gemini and Aider — and gives them
                what none has alone: <strong className="text-ink">unified memory</strong> of every
                session, a signed <strong className="text-ink">plugin layer</strong>, a
                <strong className="text-ink"> mobile companion</strong>, and an
                <strong className="text-ink"> audit trail</strong> no agent can rewrite.
              </p>
              <p className="text-sm text-ink-3 mb-8 max-w-[46ch]">
                Runs your agents side-by-side in a terminal grid — keys in your OS keychain, no proxy,
                Apache-2.0 core.{" "}
                <Link href="/privacy/" className="text-accent hover:underline">What we never collect →</Link>
              </p>
              <div className="flex flex-wrap gap-3">
                <Link href="/download/" className="btn-primary">
                  Download for macOS <span className="kbd">⌘D</span>
                </Link>
                <a href={GH_REPO} target="_blank" rel="noopener noreferrer" className="btn-secondary">
                  <svg width="16" height="16" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
                    <path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2 .37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0016 8c0-4.42-3.58-8-8-8z" />
                  </svg>
                  GitHub
                </a>
                <Link href="/council-mode/" className="btn-ghost">See Council Mode →</Link>
              </div>
            </div>
            <HeroCarousel />
          </div>
        </section>

        {/* Quote band */}
        <section className="max-w-wide mx-auto px-6 mt-16 mb-8 md:mt-24">
          <blockquote className="font-italic-serif text-2xl md:text-3xl text-ink leading-snug max-w-[40ch]">
            "Every agent forgets when the session ends. Furx is the <span className="text-accent">memory</span>, the rules, and the remote — for all of them."
          </blockquote>
          <div className="mt-3 font-mono text-xs text-ink-3 uppercase tracking-widest">— the shared substrate, not another terminal</div>
        </section>

        {/* THE FOUR PILLARS — what's unified across every agent */}
        <section className="max-w-wide mx-auto px-6 mt-24" id="how">
          <div className="flex justify-between items-baseline border-b border-rule pb-4 mb-12">
            <h2 className="text-3xl md:text-4xl font-semibold text-ink">What every agent is <span className="font-italic-serif text-accent">missing</span>.</h2>
            <span className="font-mono text-xs text-ink-3 uppercase tracking-wider">— the four pillars</span>
          </div>
          <div className="grid md:grid-cols-2 gap-px bg-rule border border-rule rounded-lg overflow-hidden">
            <article className="bg-bg p-7 md:p-9">
              <div className="lbl mb-4 font-mono text-[10px] uppercase tracking-[0.18em] text-ink-3">i · across any CLI</div>
              <h3 className="text-2xl font-display text-ink mb-3">Unified <span className="font-italic-serif text-accent">memory</span></h3>
              <p className="text-ink-2 text-sm leading-relaxed">
                Every session your agents run — searched, recalled, deduped — in one local index.
                Claude Code, Codex or Gemini; the memory is shared. &ldquo;Don&apos;t repeat what already failed&rdquo;
                becomes a query, not a hope.
              </p>
            </article>
            <article className="bg-bg p-7 md:p-9">
              <div className="lbl mb-4 font-mono text-[10px] uppercase tracking-[0.18em] text-ink-3">ii · write once</div>
              <h3 className="text-2xl font-display text-ink mb-3">Unified <span className="font-italic-serif text-accent">plugins</span></h3>
              <p className="text-ink-2 text-sm leading-relaxed">
                One signed plugin layer (Ed25519) that runs in any agent — not a fork per CLI.
                Skills, tools and gotcha packs you author once and trust everywhere, with a verifiable
                signature, not a copy-pasted script.
              </p>
            </article>
            <article className="bg-bg p-7 md:p-9">
              <div className="lbl mb-4 font-mono text-[10px] uppercase tracking-[0.18em] text-ink-3">iii · off the desk</div>
              <h3 className="text-2xl font-display text-ink mb-3">Mobile <span className="font-italic-serif text-accent">companion</span></h3>
              <p className="text-ink-2 text-sm leading-relaxed">
                Pair your phone with a QR code and drive live sessions from anywhere on your LAN or
                Tailscale — read a pane, approve a step, fire a prompt. The agents keep running; you
                just step away from the desk.
              </p>
            </article>
            <article className="bg-bg p-7 md:p-9">
              <div className="lbl mb-4 font-mono text-[10px] uppercase tracking-[0.18em] text-ink-3">iv · the agent can&apos;t rewrite it</div>
              <h3 className="text-2xl font-display text-ink mb-3"><span className="font-italic-serif text-accent">Governance</span></h3>
              <p className="text-ink-2 text-sm leading-relaxed">
                An append-only audit log (local SQLite, enforced by DDL trigger) of every action, plus
                policy and roles. History the agent — or you — cannot quietly edit. The accountability
                a room full of autonomous agents actually needs.
              </p>
            </article>
          </div>
          <p className="text-xs text-ink-3 mt-5 font-mono leading-relaxed max-w-[64ch]">
            Underneath it all: your agents run side-by-side in a terminal grid, and{" "}
            <Link href="/council-mode/" className="text-accent hover:underline">Council Mode (⌘J)</Link>{" "}
            fans one prompt across up to six models when you want a second opinion. The pillars are what you can&apos;t get anywhere else.
          </p>
        </section>

        {/* COUNCIL TEMPLATES */}
        <section className="max-w-wide mx-auto px-6 mt-24" id="templates">
          <div className="flex justify-between items-baseline border-b border-rule pb-4 mb-10">
            <h2 className="text-3xl md:text-4xl font-semibold text-ink">Council <span className="font-italic-serif text-accent">Templates</span> by phase.</h2>
            <span className="font-mono text-xs text-ink-3 uppercase tracking-wider">— Free + Pro</span>
          </div>
          <p className="text-ink-2 max-w-[62ch] mb-8 text-base leading-relaxed">
            The five built-in presets pick voices by <em>provider type</em> (OpenRouter Quick / Free Tiers / Paid / Local / Mix). Templates pick voices by <em>workflow phase</em> — so the Council you fire matches what you&apos;re actually doing.
          </p>
          <div className="overflow-x-auto border border-rule rounded-lg bg-panel">
            <table className="w-full text-sm">
              <thead className="bg-bg-soft border-b border-rule">
                <tr>
                  <th className="text-left px-5 py-3 font-mono text-xs uppercase tracking-wider text-ink-3 font-medium">Template</th>
                  <th className="text-left px-5 py-3 font-mono text-xs uppercase tracking-wider text-ink-3 font-medium">Default voices</th>
                  <th className="text-left px-5 py-3 font-mono text-xs uppercase tracking-wider text-ink-3 font-medium">Best for</th>
                </tr>
              </thead>
              <tbody>
                {TEMPLATES.map((t) => (
                  <tr key={t.name} className="border-b border-rule last:border-0 hover:bg-bg-2">
                    <td className="px-5 py-4 font-medium text-ink">{t.name}</td>
                    <td className="px-5 py-4 text-ink-2 font-mono text-xs">{t.models}</td>
                    <td className="px-5 py-4 text-ink-2">{t.when}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <p className="text-xs text-ink-3 mt-3 font-mono">All templates ship Free. Override the voices, save your own template, or write a YAML config.</p>
        </section>

        {/* PROVIDER PATHS */}
        <section className="max-w-wide mx-auto px-6 mt-24" id="paths">
          <div className="flex justify-between items-baseline border-b border-rule pb-4 mb-10">
            <h2 className="text-3xl md:text-4xl font-semibold text-ink">Connect <span className="font-italic-serif text-accent">whatever</span> you already have.</h2>
            <span className="font-mono text-xs text-ink-3 uppercase tracking-wider">— 5 paths</span>
          </div>
          <p className="text-ink-2 max-w-[60ch] mb-8 leading-relaxed">
            The Furx Connect wizard offers five paths — mix-and-match freely. Furx doesn&apos;t manage other
            people&apos;s keys; every call goes straight from your machine to the provider.
          </p>
          <div className="grid md:grid-cols-2 lg:grid-cols-3 gap-5">
            {PATHS.map((p) => (
              <article key={p.id} className="bg-panel border border-rule rounded-lg p-5 hover:border-accent transition-colors">
                <div className="flex items-baseline justify-between mb-2 gap-3">
                  <h3 className="text-base font-semibold text-ink">{p.title}</h3>
                  <span className="pill text-[10px] px-2 py-0.5 shrink-0">{p.badge}</span>
                </div>
                <p className="text-sm text-ink-2 leading-relaxed">{p.body}</p>
              </article>
            ))}
          </div>
        </section>

        {/* WHAT WE NEVER CHARGE FOR */}
        <section className="max-w-wide mx-auto px-6 mt-24" id="never">
          <div className="flex justify-between items-baseline border-b border-rule pb-4 mb-10">
            <h2 className="text-3xl md:text-4xl font-semibold text-ink">What we <span className="font-italic-serif text-accent">never</span> charge for.</h2>
            <span className="font-mono text-xs text-ink-3 uppercase tracking-wider">— in stone</span>
          </div>
          <p className="text-ink-2 mb-8 max-w-[60ch] leading-relaxed">
            BYOK means your provider spend is yours. Pro pays for cloud sync, session replay, encrypted backups and cost analytics — services we have to host. Everything below is Free, forever, Apache-2.0.
          </p>
          <ul className="grid md:grid-cols-2 gap-x-10 gap-y-3">
            {NEVER_CHARGE.map((item) => (
              <li key={item} className="flex items-start gap-3 text-sm text-ink">
                <span className="text-accent font-mono mt-1 shrink-0">✓</span>
                <span>{item}</span>
              </li>
            ))}
          </ul>
        </section>

        {/* PRICING TEASER */}
        <section className="max-w-wide mx-auto px-6 mt-24" id="pricing">
          <div className="flex justify-between items-baseline border-b border-rule pb-4 mb-10">
            <h2 className="text-3xl md:text-4xl font-semibold text-ink">Pricing, plain.</h2>
            <Link href="/pricing/" className="text-accent text-sm hover:underline font-mono uppercase tracking-wider">See detail →</Link>
          </div>
          <p className="text-ink-2 mb-8 max-w-[60ch] leading-relaxed">
            Pass-through means our cost isn&apos;t tied to your LLM spend — we charge for the orchestration layer that we host on Cloudflare.
          </p>
          <div className="grid md:grid-cols-4 gap-4">
            {PRICING_TEASER.map((t) => (
              <div key={t.tier} className={`bg-panel border rounded-lg p-5 ${t.highlight ? "border-accent ring-1 ring-accent" : "border-rule"}`}>
                <div className="font-mono text-xs uppercase tracking-wider text-ink-3 mb-2">{t.tier}</div>
                <div className="font-mono text-4xl font-semibold text-accent">{t.price}</div>
                <div className="text-xs text-ink-3 mt-1">{t.sub}</div>
              </div>
            ))}
          </div>
        </section>

        {/* FAQ */}
        <section className="max-w-wide mx-auto px-6 mt-24" id="faq">
          <div className="flex justify-between items-baseline border-b border-rule pb-4 mb-10">
            <h2 className="text-3xl md:text-4xl font-semibold text-ink">FAQ</h2>
            <Link href="/pricing/#faq" className="text-accent text-sm hover:underline font-mono uppercase tracking-wider">More →</Link>
          </div>
          <div className="grid md:grid-cols-2 gap-5">
            {FAQ.map((f) => (
              <details key={f.q} className="bg-panel border border-rule rounded-lg p-5 group">
                <summary className="cursor-pointer text-ink font-semibold list-none flex items-start gap-3">
                  <span className="text-accent font-mono mt-0.5">›</span>
                  <span className="flex-1">{f.q}</span>
                </summary>
                <p className="text-ink-2 text-sm mt-3 pl-6 leading-relaxed">{f.a}</p>
              </details>
            ))}
          </div>
        </section>

        {/* FINAL CTA — pre-launch lead capture */}
        <section className="max-w-wide mx-auto px-6 mt-24 mb-16 text-center">
          <div className="brand-mark mx-auto mb-6 text-[22px]">F</div>
          <h2 className="text-3xl md:text-4xl font-semibold mb-4 text-balance text-ink">
            Be there when Furx <span className="font-italic-serif text-accent">goes public</span>.
          </h2>
          <p className="text-ink-2 mb-6 max-w-[58ch] mx-auto leading-relaxed">
            The shared layer for your coding agents — unified memory, signed plugins, mobile companion,
            governance — is launching open-source, Apache-2.0. Leave your email and we&apos;ll tell you the
            moment the public build is live. No spam, just the launch.
          </p>
          <LeadCapture source="landing-final-cta" />
          <div className="flex flex-wrap gap-3 justify-center mt-7">
            <a href={GH_REPO} target="_blank" rel="noopener noreferrer" className="btn-secondary">Star on GitHub</a>
            <a href={APP_URL} className="btn-ghost">Sign in to dashboard</a>
          </div>
        </section>

        <script type="application/ld+json" dangerouslySetInnerHTML={{ __html: JSON.stringify(faqJsonLd) }} />
      </main>
      <Footer />
    </>
  );
}
