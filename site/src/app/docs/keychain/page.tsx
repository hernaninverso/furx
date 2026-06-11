import type { Metadata } from "next";
import Link from "next/link";
import PageShell, { Crumbs } from "@/components/PageShell";

export const metadata: Metadata = {
  title: "Keychain & secrets",
  description: "How Furx stores secrets in OS Keychain (macOS), Secret Service (Linux), Credential Manager (Windows). Rotation, revocation, export.",
  alternates: { canonical: "https://furx.cloud/docs/keychain/" },
};

export default function KeychainPage() {
  return (
    <PageShell wide>
      <Crumbs items={[{ label: "Docs", href: "/docs/" }, { label: "Keychain" }]} />
      <article className="prose-furx">
        <h1>Keychain &amp; secrets</h1>
        <p>
          Furx never writes secrets to disk in plaintext. All provider keys, MCP server tokens, and
          license tokens live in your OS&apos;s native credential store.
        </p>

        <h2>macOS</h2>
        <p>
          Apple Keychain via <code>security</code> CLI. Service names use the prefix{" "}
          <code>furx-</code>.
        </p>
        <pre>{`# List all Furx entries
security dump-keychain | grep -A1 "furx-"

# Read a specific provider key
security find-generic-password -a "$USER" -s furx-provider-openrouter -w

# Delete (Furx wizard does this for you)
security delete-generic-password -a "$USER" -s furx-provider-openrouter`}</pre>

        <h2>Linux</h2>
        <p>
          Secret Service via libsecret. On GNOME (Keyring) or KDE (KWallet).
        </p>
        <pre>{`# List
secret-tool search service furx-provider-openrouter

# Read
secret-tool lookup service furx-provider-openrouter

# Delete
secret-tool clear service furx-provider-openrouter`}</pre>
        <p>
          If you&apos;re on a headless server without a keyring service, install{" "}
          <code>gnome-keyring</code> and run <code>dbus-launch</code> first. For Docker / CI,
          use the env-var fallback (see below).
        </p>

        <h2>Windows</h2>
        <p>
          Credential Manager via <code>CredRead</code> / <code>CredWrite</code>. Target name is{" "}
          <code>furx:provider:&lt;alias&gt;</code>.
        </p>
        <pre>{`# PowerShell
Get-StoredCredential -Target furx:provider:openrouter

# Or via cmd.exe
cmdkey /list:furx:*`}</pre>

        <h2>Env-var fallback (Docker / CI only)</h2>
        <p>
          For CI/headless contexts where no keyring is available, Furx reads from env vars prefixed
          with <code>FURX_KEY_</code>. This path is <strong>disabled by default</strong> on user installs
          and requires <code>FURX_ALLOW_ENV_KEYS=1</code>.
        </p>
        <pre>{`export FURX_ALLOW_ENV_KEYS=1
export FURX_KEY_OPENROUTER=sk-or-v1-...
export FURX_KEY_ANTHROPIC=sk-ant-...
furx council --preset frontier --prompt "review this diff"`}</pre>
        <p className="text-warn">
          Never set this in a normal shell session — env vars leak into child processes, command
          history, and crash dumps. Only use in scoped CI/Docker.
        </p>

        <h2>Rotation</h2>
        <p>
          From the running app: Settings → Connect → click a provider → &quot;Update key&quot;. The new key
          overwrites the old in Keychain; nothing remains in memory beyond in-flight requests.
        </p>

        <h2>Revocation</h2>
        <ol>
          <li>Settings → Connect → Delete (removes from Keychain locally).</li>
          <li>Go to the provider&apos;s dashboard (openrouter.ai, console.anthropic.com, etc.) and revoke the key there.</li>
          <li>Audit log entries continue to reference the alias (e.g., <code>provider:openrouter</code>), never the secret itself.</li>
        </ol>

        <h2>Export &amp; backup</h2>
        <p>
          Furx does <strong>not</strong> provide a built-in export of secrets — by design, you should use
          your OS&apos;s Keychain export tool if you need a backup. For team-wide sharing, use{" "}
          <Link href="/docs/integrations/">a proxy/gateway</Link> with org-managed credentials, not shared user keys.
        </p>

        <h2>Audit</h2>
        <p>
          Every Keychain read/write writes a row to <code>~/.furx/furx.db</code>:
        </p>
        <pre>{`SELECT ts, op, alias, caller_pid
FROM events
WHERE kind = 'keychain'
ORDER BY ts DESC
LIMIT 20;`}</pre>
        <p>
          Op = <code>read</code> / <code>write</code> / <code>delete</code>. Caller PID lets you trace which
          pane/CLI requested it.
        </p>

        <h2>Reset Furx&apos;s Keychain entries</h2>
        <p>
          <code>furx doctor --reset-keychain</code> deletes all <code>furx-*</code> entries (prompts twice).
          Useful before transferring the machine.
        </p>
      </article>
    </PageShell>
  );
}
