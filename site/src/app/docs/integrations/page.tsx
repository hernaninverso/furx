import type { Metadata } from "next";
import Link from "next/link";
import PageShell, { Crumbs } from "@/components/PageShell";

export const metadata: Metadata = {
  title: "Integrations",
  description: "Integrations Furx supports: MCP servers, Claude Code Skills, gh CLI, claude-as-* wrappers, custom proxy / LiteLLM, Slack/Discord/Telegram webhooks.",
  alternates: { canonical: "https://furx.cloud/docs/integrations/" },
};

export default function IntegrationsPage() {
  return (
    <PageShell wide>
      <Crumbs items={[{ label: "Docs", href: "/docs/" }, { label: "Integrations" }]} />
      <article className="prose-furx">
        <h1>Integrations</h1>
        <p>
          Furx is a thin orchestrator — most extension happens through standards (MCP, Skills, CLIs)
          rather than a proprietary plugin API.
        </p>

        <h2>MCP servers</h2>
        <p>
          Furx reads <code>~/.furx/.mcp.json</code> on startup and connects to every server listed.
          Each server&apos;s <code>tools/list</code> handshake is shown in <em>Settings → MCP</em> with a health
          badge.
        </p>
        <pre>{`{
  "mcpServers": {
    "filesystem": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "/Users/you/code"]
    },
    "github": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-github"],
      "env": { "GITHUB_TOKEN": "ref:keychain:gh-token" }
    }
  }
}`}</pre>
        <p>
          Use <code>ref:keychain:&lt;alias&gt;</code> to pull secrets from the OS Keychain (instead of
          plaintext env vars).
        </p>

        <h2>Claude Code Skills</h2>
        <p>
          Furx registers both <code>~/.claude/skills/</code> and <code>~/.furx/skills/</code> as skill
          sources. Run any skill from <kbd>⌘K</kbd> → &quot;skill: …&quot;.
        </p>
        <p>
          Pro/Team subscribers get cloud sync of skills across machines (opt-in,{" "}
          <Link href="/docs/byok/">audit-logged</Link>).
        </p>

        <h2>gh CLI</h2>
        <p>
          Furx detects <code>gh</code> on PATH. From any pane, <code>/pr</code> opens the current
          branch&apos;s PR in browser; <code>/issues</code> lists assigned; <code>/run</code> opens latest
          workflow runs.
        </p>

        <h2>claude-as-* wrappers</h2>
        <p>
          If you have multiple Claude Max accounts, Furx creates per-account wrappers (claude-as-A,
          claude-as-B). Each wrapper sets <code>ANTHROPIC_API_KEY</code> from a distinct Keychain
          entry. Open per-pane via <em>Pane → CLI → claude-as-A</em>.
        </p>

        <h2>Custom proxy / LiteLLM</h2>
        <p>
          Wizard → Proxy tab. Any URL speaking OpenAI JSON works. Common targets:
        </p>
        <ul>
          <li><a href="https://litellm.ai/" target="_blank" rel="noopener noreferrer">LiteLLM</a> for org governance.</li>
          <li><a href="https://openrouter.ai/" target="_blank" rel="noopener noreferrer">OpenRouter</a> as a catalog proxy.</li>
          <li>Your own gateway (FastAPI, Cloudflare Workers, …).</li>
        </ul>

        <h2>Slack / Discord / Telegram webhooks</h2>
        <p>
          Send notifications from Furx on events (Council Mode disagreement, cost threshold, crash):
        </p>
        <pre>{`# ~/.furx/notifications.yml
webhooks:
  - name: "council disagreement"
    on: council.disagreement
    url: ${`$\\{ env.SLACK_WEBHOOK \\}`}
    template: "{{voice_winner}} disagrees with {{majority}} on '{{prompt_excerpt}}'"`}</pre>
        <p>
          We do <strong>not</strong> ship a Slack/Discord/Matrix native client — webhooks are deliberate.
        </p>

        <h2>GitHub Actions</h2>
        <p>
          The Furx team ships a GitHub Action that runs Council Mode against a prompt in CI (e.g.,
          PR review by a Council of LLMs):
        </p>
        <pre>{`uses: hernaninverso/furx-council-action@v1
with:
  preset: frontier
  prompt: "Review the diff for security issues."
  fail_if_any_voice: HIGH`}</pre>

        <h2>Audit replay sharing</h2>
        <p>
          Export a <code>.furxreplay</code> bundle (audit log slice + FS snapshot) from{" "}
          <em>Settings → Audit → Export</em>. Open in another Furx install with{" "}
          <em>File → Open Replay</em>.
        </p>

        <h2>What we don&apos;t integrate</h2>
        <p>
          Slack/Discord/Matrix as a chat client (vanity feature, 0/6 council). Notion or Linear
          (use MCP servers instead). Email digest (read your audit replay link instead).
        </p>
      </article>
    </PageShell>
  );
}
