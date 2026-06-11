import type { Metadata } from "next";
import Link from "next/link";
import PageShell, { Crumbs } from "@/components/PageShell";

export const metadata: Metadata = {
  title: "Quickstart",
  description: "Install Furx, run the Furx Connect wizard, dispatch your first Council Mode prompt in 5 minutes.",
  alternates: { canonical: "https://furx.cloud/docs/quickstart/" },
};

export default function QuickstartPage() {
  return (
    <PageShell wide>
      <Crumbs items={[{ label: "Docs", href: "/docs/" }, { label: "Quickstart" }]} />
      <article className="prose-furx">
        <h1>Quickstart</h1>
        <p>Five minutes from zero to your first Council Mode dispatch.</p>

        <h2>1. Install</h2>
        <p>Pick your OS on the <Link href="/download/">download page</Link> or:</p>
        <pre>{`# macOS
curl -L https://github.com/hernaninverso/furx/releases/latest/download/Furx_0.2.0_aarch64.dmg -o ~/Downloads/Furx.dmg
open ~/Downloads/Furx.dmg

# Linux
sudo apt install ./furx_0.2.0_amd64.deb

# Windows — double-click the .msi`}</pre>

        <h2>2. First launch</h2>
        <p>
          Furx opens directly into the <strong>Furx Connect</strong> wizard. You don&apos;t need to make any
          choices yet — every path is mix-and-match.
        </p>
        <p>The wizard has six tabs:</p>
        <ul>
          <li><strong>OpenRouter</strong> (recommended for 1-key setup, $10 deposit, 300+ models).</li>
          <li><strong>Free tiers</strong> (Cerebras, Groq, Mistral, SambaNova, Gemini AI Studio, OpenRouter free).</li>
          <li><strong>Paid APIs</strong> (Anthropic, OpenAI, Gemini, any OAI-compatible).</li>
          <li><strong>Local</strong> (auto-detect Ollama, LM Studio, llama.cpp, vLLM).</li>
          <li><strong>Proxy</strong> (LiteLLM / your org gateway).</li>
          <li><strong>Mix</strong> (any combination).</li>
        </ul>

        <h2>3. Add your first key</h2>
        <p>Easiest path: <strong>OpenRouter Quick Start</strong>.</p>
        <ol>
          <li>Sign up at <a href="https://openrouter.ai" target="_blank" rel="noopener noreferrer">openrouter.ai</a>, deposit $10.</li>
          <li>Copy your key (<code>sk-or-v1-…</code>).</li>
          <li>Paste in the wizard. Furx runs a 142ms health check, writes the key to your OS Keychain, and you&apos;re in.</li>
        </ol>
        <p className="text-ink-3">
          Your key never leaves your machine. Furx talks directly to openrouter.ai over HTTPS;
          we never proxy.
        </p>

        <h2>4. Open your first pane</h2>
        <p>
          The main window is a 2×2 grid by default. Right-click a pane → pick a CLI: zsh, claude,
          codex, gemini, aider. Or hit <kbd>⌘K</kbd> → &quot;new pane&quot; → pick.
        </p>

        <h2>5. Dispatch Council Mode</h2>
        <ol>
          <li>Hit <kbd>⌘J</kbd>.</li>
          <li>Type your prompt. Example: <em>&quot;Write a Python function that validates an EU VAT number, including the EE country-specific checksum.&quot;</em></li>
          <li>Pick a preset: <strong>Quick</strong> (3 cheap LLMs), <strong>Frontier</strong> (Claude Opus + GPT-5 + Gemini 2.5), <strong>Cheapo</strong> (Cerebras + Groq + free tiers), <strong>Local</strong> (Ollama models only), or <strong>Mix</strong>.</li>
          <li>Watch the diff appear as voices return.</li>
          <li>Pick a winner with <kbd>1</kbd>–<kbd>6</kbd>, or merge with <kbd>M</kbd>.</li>
        </ol>

        <h2>6. Set up audit + sync (optional, Pro)</h2>
        <p>
          If you want session replay across machines or cloud sync of <code>.mcp.json</code>:
          Settings → Account → sign in with the same email used in Paddle. Trial 14 days, no card.
        </p>

        <h2>Next</h2>
        <ul>
          <li><Link href="/docs/byok/">BYOK deep-dive</Link> — how your keys are protected.</li>
          <li><Link href="/docs/council/">Council Mode presets</Link> — when to use which.</li>
          <li><Link href="/docs/providers/">All providers</Link> — full list with notes.</li>
          <li><Link href="/docs/troubleshooting/">Troubleshooting</Link> — Ollama not detected, Gatekeeper, etc.</li>
        </ul>
      </article>
    </PageShell>
  );
}
