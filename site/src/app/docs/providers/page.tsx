import type { Metadata } from "next";
import Link from "next/link";
import PageShell, { Crumbs } from "@/components/PageShell";

export const metadata: Metadata = {
  title: "Providers",
  description: "All LLM providers supported by Furx: cloud (Anthropic, OpenAI, Gemini, Cerebras, Groq, …), local (Ollama, LM Studio, llama.cpp, vLLM), proxy (OpenRouter, LiteLLM).",
  alternates: { canonical: "https://furx.cloud/docs/providers/" },
};

const PROVIDERS = {
  "Cloud · paid": [
    { name: "Anthropic", endpoint: "api.anthropic.com", models: "Claude Opus 4.7, Sonnet 4.6, Haiku 4.5", notes: "Best for code reasoning. ZDR available." },
    { name: "OpenAI", endpoint: "api.openai.com", models: "GPT-5, GPT-5-mini, o4-mini", notes: "GPT-5 + reasoning models. Functions OK." },
    { name: "Google Gemini", endpoint: "generativelanguage.googleapis.com", models: "Gemini 2.5 Pro / Flash / Flash-Lite", notes: "1M-token context. Vertex AI variant too." },
  ],
  "Cloud · free tier (BYOK to provider)": [
    { name: "Cerebras", endpoint: "api.cerebras.ai", models: "gpt-oss-120b, Qwen-3-235B, Llama-4 Maverick", notes: "1M tok/day free. Fastest inference." },
    { name: "Groq", endpoint: "api.groq.com", models: "Llama-3.3-70b, Llama-4, qwen-coder", notes: "14.4k req/day free. Very fast." },
    { name: "Mistral", endpoint: "api.mistral.ai", models: "Mistral Large 2 / Codestral", notes: "1M tok/day free on EU servers." },
    { name: "SambaNova", endpoint: "api.sambanova.ai", models: "Llama-3.3-70b, DeepSeek-V3.1", notes: "Free tier, EU residency option." },
    { name: "Gemini AI Studio (free)", endpoint: "generativelanguage.googleapis.com", models: "Gemini 2.5 Flash", notes: "Generous free quota, US-only data path." },
    { name: "NVIDIA NIM", endpoint: "integrate.api.nvidia.com", models: "Llama-3.3-70b, Mixtral, DeepSeek", notes: "Free tier with throttling." },
  ],
  "Cloud · proxy / catalog": [
    { name: "OpenRouter", endpoint: "openrouter.ai", models: "300+ from all major providers", notes: "Recommended quick-start. $10 deposit, top-up as needed." },
    { name: "LiteLLM", endpoint: "your-proxy:4000", models: "any backend you wire up", notes: "Self-host for org governance + spend caps." },
    { name: "OpenAI-compatible (custom)", endpoint: "your endpoint", models: "depends on your gateway", notes: "Any URL speaking OAI JSON works." },
  ],
  "Local · auto-detected": [
    { name: "Ollama", endpoint: "127.0.0.1:11434", models: "qwen2.5-coder, deepseek-r1, llama-3.3, gemma3, phi-4, mistral-small", notes: "Pulled via <code>ollama pull</code>. Furx lists what&apos;s installed." },
    { name: "LM Studio", endpoint: "127.0.0.1:1234", models: "depends on local models", notes: "GUI for downloading models. OAI-compatible server." },
    { name: "llama.cpp", endpoint: "127.0.0.1:8080", models: "any GGUF you load", notes: "Raw llama-server. Manual model load." },
    { name: "vLLM", endpoint: "127.0.0.1:8000 (or custom)", models: "any HF model you serve", notes: "For heavier local hosting on a homelab GPU." },
    { name: "MLX (Apple Silicon)", endpoint: "via Ollama/LM Studio bridge", models: "Llama-3.3, Qwen, DeepSeek", notes: "Native Apple Silicon, beat Ollama on speed for some models." },
  ],
};

export default function ProvidersPage() {
  return (
    <PageShell wide>
      <Crumbs items={[{ label: "Docs", href: "/docs/" }, { label: "Providers" }]} />
      <article className="prose-furx">
        <h1>Providers</h1>
        <p>
          Every provider on this list works as-is in Furx via the wizard. Add a new one via{" "}
          <em>Settings → Connect → Proxy</em> if it speaks OpenAI-compatible JSON.
        </p>

        {Object.entries(PROVIDERS).map(([group, items]) => (
          <section key={group}>
            <h2>{group}</h2>
            <div className="not-prose overflow-x-auto border border-rule rounded-lg my-4">
              <table className="w-full text-sm">
                <thead className="bg-bg-soft">
                  <tr>
                    <th className="text-left px-3 py-2 font-sans text-ink">Provider</th>
                    <th className="text-left px-3 py-2 font-sans text-ink">Endpoint</th>
                    <th className="text-left px-3 py-2 font-sans text-ink">Models</th>
                    <th className="text-left px-3 py-2 font-sans text-ink">Notes</th>
                  </tr>
                </thead>
                <tbody>
                  {items.map((p) => (
                    <tr key={p.name} className="border-t border-rule align-top">
                      <td className="px-3 py-2 font-bold text-accent">{p.name}</td>
                      <td className="px-3 py-2 font-mono text-xs text-ink-2">{p.endpoint}</td>
                      <td className="px-3 py-2 text-xs text-ink-2">{p.models}</td>
                      <td className="px-3 py-2 text-xs text-ink-2" dangerouslySetInnerHTML={{ __html: p.notes }} />
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </section>
        ))}

        <h2>BYOK reminder</h2>
        <p>
          For every cloud provider above, you bring your own key. Furx stores it in your OS Keychain.
          Nothing goes through our servers.
          See <Link href="/docs/byok/">BYOK guide</Link>.
        </p>

        <h2>Missing a provider?</h2>
        <p>
          If it speaks OpenAI-compatible JSON, add it via the Proxy tab. For non-compatible APIs,
          open an issue at{" "}
          <a href="https://github.com/hernaninverso/furx/issues" target="_blank" rel="noopener noreferrer">
            github.com/hernaninverso/furx/issues
          </a>.
        </p>
      </article>
    </PageShell>
  );
}
