import type { Metadata } from "next";
import Link from "next/link";
import PageShell, { Crumbs } from "@/components/PageShell";

export const metadata: Metadata = {
  title: "BYOK guide",
  description: "How Furx's Bring-Your-Own-Key model works: keys stored in OS Keychain, pass-through to providers, never proxied, never on our servers.",
  alternates: { canonical: "https://furx.cloud/docs/byok/" },
};

export default function ByokPage() {
  return (
    <PageShell wide>
      <Crumbs items={[{ label: "Docs", href: "/docs/" }, { label: "BYOK" }]} />
      <article className="prose-furx">
        <h1>BYOK — Bring Your Own Keys</h1>
        <p>
          Furx is a <strong>pass-through</strong> orchestrator. Your provider keys go from your
          machine directly to the provider over HTTPS. We don&apos;t run a proxy, don&apos;t see your prompts,
          don&apos;t see your responses, and don&apos;t bill your provider quotas.
        </p>

        <h2>Where keys live</h2>
        <ul>
          <li><strong>macOS</strong>: <a href="https://developer.apple.com/documentation/security/keychain_services" target="_blank" rel="noopener noreferrer">Keychain</a> via the native <code>security</code> framework. Service name <code>furx-provider-&lt;alias&gt;</code>.</li>
          <li><strong>Linux</strong>: <a href="https://specifications.freedesktop.org/secret-service/latest/" target="_blank" rel="noopener noreferrer">Secret Service</a> (GNOME Keyring on GNOME, KWallet on KDE, libsecret as fallback).</li>
          <li><strong>Windows</strong>: <a href="https://docs.microsoft.com/en-us/windows/win32/api/wincred/" target="_blank" rel="noopener noreferrer">Credential Manager</a> via <code>CredWrite</code> / <code>CredRead</code>.</li>
        </ul>
        <p>
          All three are user-scoped, OS-encrypted at rest, and require user login session to read.
          Furx does <strong>not</strong> have a fallback to plaintext file storage — if Keychain is unavailable,
          the wizard fails closed.
        </p>

        <h2>What we never store</h2>
        <ul>
          <li>Provider keys (in any form, even hashed).</li>
          <li>Prompts you send.</li>
          <li>Responses you receive.</li>
          <li>The contents of your audit log (it stays on your machine).</li>
          <li>Your code, git history, or filesystem snapshots.</li>
        </ul>
        <p>
          See <Link href="/privacy/">Privacy Policy</Link> for the full list of what we do/don&apos;t collect.
        </p>

        <h2>What does cross the network</h2>
        <p>Once you opt-in (Pro only):</p>
        <ul>
          <li><code>.mcp.json</code> contents (your MCP server config — but not the secrets they reference, which live in Keychain).</li>
          <li>Audit log <strong>metadata</strong> (timestamps, event types, model names) — never the prompt/response bodies, unless you explicitly enable &quot;deep sync&quot;.</li>
          <li>Crash telemetry (PII-scrubbed, opt-in, default OFF). <Link href="/privacy/#telemetry">Details</Link>.</li>
        </ul>

        <h2>Provider-side: where your data goes</h2>
        <p>
          When you dispatch a prompt, Furx opens an HTTPS connection from your machine straight to
          the provider you picked. Examples:
        </p>
        <ul>
          <li>OpenRouter: <code>api.openrouter.ai</code> (US/EU edges).</li>
          <li>Anthropic: <code>api.anthropic.com</code>.</li>
          <li>OpenAI: <code>api.openai.com</code>.</li>
          <li>Cerebras: <code>api.cerebras.ai</code>.</li>
          <li>Ollama: <code>127.0.0.1:11434</code> (local, no egress).</li>
        </ul>
        <p>
          You are responsible for the provider&apos;s TOS, data-retention policy, and rate limits. Use
          a local model (Ollama) if your data can&apos;t leave your machine.
        </p>

        <h2>Council Mode and BYOK</h2>
        <p>
          Council Mode dispatches in parallel to <em>multiple</em> providers. Each request uses its own
          key. We never multiplex multiple users&apos; keys, never share connections, never warm-pool credentials.
        </p>
        <p>
          You can build a Council preset that&apos;s 100% local (Ollama models only) — that&apos;s the
          privacy moat for legal / regtech / finance teams.
        </p>

        <h2>Rotation &amp; revocation</h2>
        <p>
          Update a key via <em>Furx Connect</em> (Settings → Connect → pick provider → Update key). The
          old key is overwritten in Keychain; nothing is cached in memory longer than the request
          that&apos;s in flight.
        </p>
        <p>
          To fully revoke: delete via wizard, then revoke at the provider side (you control that).
          Audit log entries reference key aliases, never the secret itself.
        </p>

        <h2>Multi-tenant / Team mode</h2>
        <p>
          On Team subscription, each seat&apos;s Keychain is independent. Centralized policy (allow-listing,
          spend caps) is enforced via the proxy URL — not by Furx seeing the keys. Use{" "}
          <a href="https://litellm.ai/" target="_blank" rel="noopener noreferrer">LiteLLM</a> or your own gateway as the proxy
          target.
        </p>

        <h2>Next</h2>
        <ul>
          <li><Link href="/docs/keychain/">Keychain reference</Link> — commands to inspect/export.</li>
          <li><Link href="/docs/audit/">Audit log</Link> — schema + retention.</li>
          <li><Link href="/security/">Security policy</Link> — how to report a vulnerability.</li>
        </ul>
      </article>
    </PageShell>
  );
}
