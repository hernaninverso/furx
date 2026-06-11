import Link from "next/link";

/**
 * Shared DPIA (Data Protection Impact Assessment) template content — spec 001 T4.4 +
 * spec 003 F5. Covers both Furx data flows (cloud traces + persona packs). The `focus`
 * prop only changes the intro emphasis; the assessment body is shared.
 *
 * This is a TEMPLATE, not legal advice (disclaimer at the bottom).
 */
export default function DpiaContent({ focus }: { focus: "traces" | "persona-pack" }) {
  const isTraces = focus === "traces";
  return (
    <article className="prose-furx">
      <h1>Data Protection Impact Assessment (DPIA) — template</h1>
      <p>
        This template helps a Furx <strong>Pro / Team / Enterprise</strong> customer document a DPIA
        for the optional features that move data to the Furx cloud:{" "}
        {isTraces ? (
          <><strong>cloud traces</strong> (and, by extension, persona packs derived from them)</>
        ) : (
          <><strong>persona packs</strong> (distilled from your approved cloud traces)</>
        )}
        . Cloud features are <strong>opt-in per project and OFF by default</strong>; if you never enable
        them, no prompt/response content leaves your machine and this DPIA is not required.
      </p>

      <h2>1 · Processing overview</h2>
      <table>
        <tbody>
          <tr><td><strong>Controller</strong></td><td>You (the customer / your organization).</td></tr>
          <tr><td><strong>Processor</strong></td><td>INVERSO HUB S.R.L. (Furx), using Cloudflare as sub-processor.</td></tr>
          <tr><td><strong>Purpose</strong></td><td>Observability of your own LLM calls (traces) and distillation of a reusable system prompt from interactions you explicitly approved (persona packs).</td></tr>
          <tr><td><strong>Lawful basis</strong></td><td>Legitimate interest / contract performance. You decide what to upload; consent is captured per project via the opt-in toggle.</td></tr>
          <tr><td><strong>Data subjects</strong></td><td>You and anyone whose data appears in the prompts/responses you choose to upload.</td></tr>
        </tbody>
      </table>

      <h2>2 · Data flows</h2>
      <ul>
        <li><strong>BYOK is preserved.</strong> Provider API keys never leave your device and are never sent to Furx — they go straight to the LLM provider. See <Link href="/docs/byok/">BYOK</Link>.</li>
        <li><strong>Traces.</strong> When <code>cloud_traces_enabled</code> is on for a project, each LLM call&apos;s metadata (model, provider, tokens, latency, status) and an optional sanitized payload (prompt + response) are uploaded to Furx.</li>
        <li><strong>Two-pass PII sanitizer.</strong> The desktop client redacts known secret/PII patterns (API keys, bearer tokens, emails, IBAN, ARNs, GUIDs) before upload; the Cloudflare Worker re-runs the same pass server-side. Redaction hits are logged as evidence.</li>
        <li><strong>Persona packs.</strong> Distillation reads only traces you marked <em>approved</em>, and a frontier model produces a short system prompt + a literal subset of your approved examples. Nothing else is read.</li>
      </ul>

      <h2>3 · Storage, retention &amp; location</h2>
      <ul>
        <li>Metadata is stored in Cloudflare D1; payloads/pack blobs in Cloudflare R2 (encrypted at rest).</li>
        <li><strong>Retention is automatic by tier</strong> via R2 Object Lifecycle: Free 7 days, Pro 30, Team 90, Enterprise 365.</li>
        <li>Processing runs on Cloudflare&apos;s global edge. Choose a regional jurisdiction with Cloudflare data-localization controls if required.</li>
        <li>You can delete a project at any time; deletion cascades to its traces, packs and replay sets.</li>
      </ul>

      <h2>4 · Sub-processors</h2>
      <p>See the live list at <Link href="/subprocessors/">furx.cloud/subprocessors</Link>. Primary: <strong>Cloudflare, Inc.</strong> (Workers, D1, R2, Workers AI). Server-side distillation/eval uses Cloudflare Workers AI (no third-party LLM vendor receives your data for these jobs).</p>

      <h2>5 · Data-subject rights</h2>
      <ul>
        <li><strong>Access / portability</strong>: Pro+ can export traces as NDJSON; packs export as signed JSON (Enterprise).</li>
        <li><strong>Erasure</strong>: delete the project (cascade) or disable cloud traces; retention windows bound residual copies.</li>
        <li><strong>Audit</strong>: an append-only audit log records every cloud action (a DPO viewer is available for Team/Enterprise).</li>
      </ul>

      <h2>6 · Risks &amp; mitigations</h2>
      <table>
        <thead><tr><th>Risk</th><th>Mitigation</th></tr></thead>
        <tbody>
          <tr><td>PII leaks into uploaded payloads</td><td>Two-pass sanitizer (client + Worker); opt-in OFF by default; only approved traces feed packs.</td></tr>
          <tr><td>Provider key exposure</td><td>BYOK — keys never reach Furx; stored in OS Keychain only.</td></tr>
          <tr><td>Over-retention</td><td>Automatic tier-based R2 lifecycle deletion.</td></tr>
          <tr><td>Unauthorized access</td><td>Per-user row-level scoping on every endpoint; session tokens; HMAC-signed exports.</td></tr>
          <tr><td>Re-identification via distilled prompt</td><td>Packs cite only approved examples; you review a side-by-side preview before applying.</td></tr>
        </tbody>
      </table>

      <h2>7 · Conclusion</h2>
      <p>With opt-in defaults, two-pass sanitization, automatic retention and per-user scoping, the residual risk for the {isTraces ? "cloud traces" : "persona pack"} feature is assessed as <strong>low</strong> for a customer who keeps PII out of uploaded payloads. Re-run this DPIA if you enable cloud features for projects handling special-category data.</p>

      <hr />
      <p className="text-sm opacity-70">
        <strong>Disclaimer:</strong> this is a template to accelerate your own assessment — it is not legal advice.
        Adapt it with your DPO / counsel for your jurisdiction and data. Related: <Link href="/docs/audit/">audit log</Link>,{" "}
        <Link href="/privacy/">privacy</Link>, <Link href="/dpa/">DPA</Link>.
      </p>
    </article>
  );
}
