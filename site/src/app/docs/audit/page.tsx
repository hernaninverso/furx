import type { Metadata } from "next";
import Link from "next/link";
import PageShell, { Crumbs } from "@/components/PageShell";

export const metadata: Metadata = {
  title: "Audit log",
  description: "Furx audit log schema (SQLite, append-only, triggered UPDATE/DELETE blocks), retention, export to JSON / CSV / .furxreplay bundle.",
  alternates: { canonical: "https://furx.cloud/docs/audit/" },
};

export default function AuditPage() {
  return (
    <PageShell wide>
      <Crumbs items={[{ label: "Docs", href: "/docs/" }, { label: "Audit log" }]} />
      <article className="prose-furx">
        <h1>Audit log</h1>
        <p>
          Every action in Furx writes a row to <code>~/.furx/furx.db</code> — a SQLite file with WAL
          journaling. The <code>events</code> table is append-only: SQLite triggers block <code>UPDATE</code>{" "}
          and <code>DELETE</code> at the DDL layer.
        </p>

        <h2>Why append-only</h2>
        <p>
          Two reasons:
        </p>
        <ul>
          <li><strong>Forensic integrity.</strong> If Furx crashes mid-write, the SQLite WAL gives you crash-recovery semantics — no torn writes.</li>
          <li><strong>Compliance.</strong> SOC2 + ISO 27001 controls require immutable audit trails. The triggers make Furx&apos;s log compliance-ready out of the box.</li>
        </ul>

        <h2>Schema</h2>
        <pre>{`CREATE TABLE events (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  ts          TEXT NOT NULL,        -- ISO-8601 UTC
  kind        TEXT NOT NULL,        -- 'pty', 'council', 'keychain', 'mcp', 'crash', ...
  pane_id     TEXT,                 -- the pane where it happened
  project     TEXT,                 -- detected project (git repo root)
  actor       TEXT,                 -- 'user', 'claude', 'codex', 'gemini', 'aider', 'system'
  payload     TEXT NOT NULL,        -- JSON, schema varies per kind
  payload_sha TEXT NOT NULL         -- SHA256 of payload (tamper detection)
);

CREATE TRIGGER block_update BEFORE UPDATE ON events
  BEGIN SELECT RAISE(ABORT, 'events.append-only'); END;
CREATE TRIGGER block_delete BEFORE DELETE ON events
  BEGIN SELECT RAISE(ABORT, 'events.append-only'); END;`}</pre>

        <h2>What gets logged</h2>
        <ul>
          <li>Pane open / close.</li>
          <li>Command executed in PTY.</li>
          <li>Council Mode dispatches (provider, model, latency, token-counts, cost-estimate).</li>
          <li>Keychain reads / writes / deletes (alias only, never secret).</li>
          <li>MCP server connections + tool calls.</li>
          <li>Crashes (PII-scrubbed).</li>
          <li>Auto-update events.</li>
        </ul>

        <h2>What is NOT logged</h2>
        <ul>
          <li>Provider secret keys (only aliases).</li>
          <li>Prompt / response body of Council dispatch (only token-counts + hash). Opt-in &quot;deep audit&quot; logs prompts but stays local.</li>
          <li>Clipboard contents.</li>
          <li>Filesystem reads (only command-line invocations).</li>
        </ul>

        <h2>Retention</h2>
        <p>
          Local: <strong>forever by default</strong>. Configurable in Settings → Audit → Retention.
          Recommended: 90 days for Free, indefinite for Pro+ (you have the disk).
        </p>
        <p>
          Cloud sync (Pro+): 30 days. Compliance Pack: 3 years escrowed encrypted backup.
        </p>

        <h2>Export</h2>
        <ul>
          <li><strong>JSON</strong> (one event per line): <code>furx audit export --format json &gt; audit.jsonl</code></li>
          <li><strong>CSV</strong>: <code>furx audit export --format csv &gt; audit.csv</code></li>
          <li><strong>.furxreplay bundle</strong> (audit + FS snapshot, share with team): <em>Settings → Audit → Export bundle</em>.</li>
        </ul>

        <h2>Replay scrubber (Pro+)</h2>
        <p>
          The desktop app and the dashboard both render a timeline scrubber over your audit. Slide
          through to replay any session — see which prompts ran, which voices won, which commands
          executed.
        </p>

        <h2>Cloud sync (opt-in, Pro+)</h2>
        <p>
          When opted-in, Furx pushes <em>event metadata</em> (timestamps, types, model names — never
          prompt/response bodies) to <code>app.furx.cloud</code> over TLS 1.3. Server-side it&apos;s
          stored encrypted in PostgreSQL with row-level encryption per-tenant.
        </p>
        <p>
          To opt-out: Settings → Account → Cloud sync → OFF. The local log is unaffected.
        </p>

        <h2>Deleting your audit log</h2>
        <p>
          The triggers prevent row-level delete, but you can <strong>drop the whole file</strong>:
        </p>
        <pre>{`rm ~/.furx/furx.db ~/.furx/furx.db-wal ~/.furx/furx.db-shm
# Furx recreates on next launch with empty schema`}</pre>

        <p>For cloud-synced data: Settings → Account → Delete sync data (irreversible).</p>

        <h2>Next</h2>
        <ul>
          <li><Link href="/docs/keychain/">Keychain reference</Link>.</li>
          <li><Link href="/privacy/">Privacy policy</Link> for the legal version of all this.</li>
          <li><Link href="/security/">Security policy</Link> for the threat model.</li>
        </ul>
      </article>
    </PageShell>
  );
}
