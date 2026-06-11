import type { Metadata } from "next";
import Link from "next/link";
import PageShell, { Crumbs } from "@/components/PageShell";

export const metadata: Metadata = {
  title: "Council Mode",
  description: "Council Mode dispatches one prompt to as many LLMs as you wire up, in parallel. Presets, voting, cost-aware routing, graceful degradation, role-based Templates.",
  alternates: { canonical: "https://furx.cloud/docs/council/" },
};

const PRESETS = [
  {
    name: "Quick",
    desc: "3 fast, cheap models. Good for routine code review, rephrasing, doc-gen.",
    voices: "Cerebras gpt-oss-120b · Groq Llama-3.3-70b · Mistral-Large",
    cost: "$0 (all free tiers)",
    latency: "~2–3 s for 3 returns",
  },
  {
    name: "Frontier",
    desc: "Top-shelf reasoning models. For architecture decisions, novel code, gnarly bugs.",
    voices: "Claude Opus 4.7 · GPT-5 · Gemini 2.5 Pro · Cerebras Qwen-3-235B · DeepSeek-V3.1 · Llama-4 Maverick",
    cost: "depends on your paid keys",
    latency: "~5–12 s for 6 returns",
  },
  {
    name: "Cheapo",
    desc: "Maximize free-tier coverage. Burst-friendly.",
    voices: "Cerebras + Groq + Mistral + SambaNova + Gemini AI Studio + OpenRouter free",
    cost: "$0",
    latency: "~3–6 s",
  },
  {
    name: "Local",
    desc: "Zero bytes leave your machine. For sensitive code (legal, regtech, fin).",
    voices: "Ollama qwen2.5-coder · deepseek-r1 · llama-3.3:70b · gemma3 · phi-4 · mistral-small",
    cost: "$0",
    latency: "depends on your GPU",
  },
  {
    name: "Mix",
    desc: "Hand-picked combination. Save your own preset.",
    voices: "any of the above",
    cost: "varies",
    latency: "varies",
  },
];

export default function CouncilPage() {
  return (
    <PageShell wide>
      <Crumbs items={[{ label: "Docs", href: "/docs/" }, { label: "Council Mode" }]} />
      <article className="prose-furx">
        <h1>Council Mode</h1>
        <p>
          <kbd>⌘J</kbd> dispatches one prompt to <strong>as many LLMs as you wire up, in parallel</strong>, then shows you
          the returns side-by-side with a diff. Vote with <kbd>1</kbd>–<kbd>6</kbd>, merge with <kbd>M</kbd>,
          re-dispatch with <kbd>R</kbd>.
        </p>

        <h2>Why dispatch to multiple LLMs?</h2>
        <ul>
          <li><strong>Disagreement signals risk.</strong> If 5 of 6 models agree but 1 disagrees with a sharp reason, that&apos;s a bug worth investigating.</li>
          <li><strong>Cost-aware routing.</strong> The cheap preset handles routine work; the frontier preset earns its cost on hard problems.</li>
          <li><strong>Cross-family tie-break.</strong> Claude/GPT/Gemini have different blind spots — diversity beats any single model on average.</li>
          <li><strong>Free-tier amortization.</strong> The Cheapo preset is $0 forever — you literally cannot pay too much.</li>
        </ul>

        <h2>Presets</h2>
        <p>Pre-installed; you can edit them or create your own.</p>

        <div className="not-prose">
          <div className="overflow-x-auto border border-rule rounded-lg my-6">
            <table className="w-full text-sm">
              <thead className="bg-bg-soft">
                <tr>
                  <th className="text-left px-4 py-3 font-sans text-ink">Preset</th>
                  <th className="text-left px-4 py-3 font-sans text-ink">Voices</th>
                  <th className="text-left px-4 py-3 font-sans text-ink">Cost</th>
                  <th className="text-left px-4 py-3 font-sans text-ink">Latency</th>
                </tr>
              </thead>
              <tbody>
                {PRESETS.map((p) => (
                  <tr key={p.name} className="border-t border-rule">
                    <td className="px-4 py-3 align-top">
                      <div className="font-bold text-accent">{p.name}</div>
                      <div className="text-xs text-ink-3 mt-1">{p.desc}</div>
                    </td>
                    <td className="px-4 py-3 align-top text-ink-2 text-xs font-mono">{p.voices}</td>
                    <td className="px-4 py-3 align-top text-ink-2 text-xs">{p.cost}</td>
                    <td className="px-4 py-3 align-top text-ink-2 text-xs">{p.latency}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>

        <h2>Graceful degradation</h2>
        <p>
          If a voice rate-limits, errors, or times out (40s hard cap), the rest continue. You see a status
          per voice — green (returned), yellow (in-flight), red (failed). Failed voices don&apos;t block the modal.
        </p>

        <h2>Voting</h2>
        <p>
          Each voice gets a number 1–6 in the order it returns. Press the number to pick that response;
          press <kbd>M</kbd> to merge multiple winners; press <kbd>D</kbd> to see a fine diff between two
          you can&apos;t decide on.
        </p>

        <h2>Cost estimator</h2>
        <p>
          Before dispatch, the Council modal shows an estimated cost based on prompt length × provider
          pricing. Local models always show $0. Free-tier models show $0 with the daily quota meter.
        </p>

        <h2>Custom dispatch policy</h2>
        <p>
          Team+ subscriptions get a config option <code>~/.furx/dispatch.yml</code> to define your own
          presets, with rules like:
        </p>
        <pre>{`presets:
  arch-review:
    voices:
      - { provider: anthropic, model: claude-opus-4-7 }
      - { provider: openai, model: gpt-5 }
      - { provider: gemini, model: gemini-2.5-pro }
    require_min_responses: 2
    timeout_ms: 30000
    fallback_to_local: true`}</pre>

        <h2>Audit</h2>
        <p>
          Every Council dispatch writes one row per voice to <code>~/.furx/furx.db</code> with timestamps,
          provider, model, latency, token-counts, cost-estimate, and a hash of the prompt. The prompt
          and response bodies stay local (unless you opt-in to cloud sync).
        </p>

        <h2>Next</h2>
        <ul>
          <li><Link href="/docs/providers/">Provider list</Link> — which models work where.</li>
          <li><Link href="/docs/byok/">BYOK guide</Link> — keys never leave your machine.</li>
          <li><Link href="/docs/audit/">Audit log</Link> — replay your sessions.</li>
        </ul>
      </article>
    </PageShell>
  );
}
