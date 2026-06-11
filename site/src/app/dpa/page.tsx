import type { Metadata } from "next";
import LegalLayout from "@/components/LegalLayout";

export const metadata: Metadata = {
  title: "Data Processing Agreement (DPA)",
  description: "Data Processing Agreement for Team/Enterprise customers — GDPR Art. 28, SCC 2021/914 Module 2.",
  alternates: { canonical: "https://furx.cloud/dpa/" },
};

export default function DpaPage() {
  return (
    <LegalLayout title="Data Processing Agreement (DPA)">
      <p>
        This Data Processing Agreement (hereinafter, the <strong>&quot;DPA&quot;</strong>) forms an
        integral part of the Furx subscription agreement (Team / Enterprise / Compliance Pack) entered
        into between the <strong>Customer</strong> (acting as <em>Controller</em>) and{" "}
        <strong>INVERSO HUB S.R.L.</strong> (acting as <em>Processor</em>), pursuant to Art. 28 of
        Regulation (EU) 2016/679 (GDPR).
      </p>

      <p>
        No DPA applies to Free and individual Pro plans — Furx does not process personal data of the
        User on behalf of a Customer (the User is the data subject of their own account data). This DPA
        applies where the Customer (an organization) subscribes to Team plans or above.
      </p>

      <h2>1. Definitions</h2>
      <ul>
        <li><strong>Personal Data</strong>: as defined in GDPR Art. 4(1).</li>
        <li><strong>Processing</strong>: as defined in GDPR Art. 4(2).</li>
        <li><strong>Sub-processor</strong>: a third party engaged by the Processor to process the Customer&apos;s Personal Data.</li>
        <li><strong>Breach</strong>: a security incident affecting the confidentiality, integrity, or availability of Personal Data.</li>
      </ul>

      <h2>2. Nature, purpose, and duration of processing</h2>
      <ul>
        <li><strong>Purpose</strong>: providing the Furx service (local orchestration, opt-in audit log sync, Team seat management, customer dashboard).</li>
        <li><strong>Nature</strong>: storage, transmission, logical access.</li>
        <li><strong>Categories of data subjects</strong>: employees / contractors of the Customer who use Furx.</li>
        <li><strong>Categories of data</strong>: email, name, access IP, audit log metadata (timestamps, event types, LLM model names — never prompt/response bodies), usage data (logins, linked devices).</li>
        <li><strong>Duration</strong>: term of the agreement + a 30-day grace period + legal retention pursuant to policy.</li>
      </ul>

      <h2>3. Processor obligations</h2>
      <p>INVERSO HUB undertakes to:</p>
      <ul>
        <li>Process Personal Data <strong>only</strong> on documented instructions from the Controller (Art. 28(3)(a)).</li>
        <li>Ensure the confidentiality of authorized personnel (Art. 28(3)(b)).</li>
        <li>Implement appropriate technical and organizational measures (Art. 32) — detailed in Section 6.</li>
        <li>Assist the Controller in fulfilling data subjects&apos; rights (Art. 28(3)(e)).</li>
        <li>Assist the Controller with breach notifications and DPIAs where applicable (Art. 28(3)(f), Art. 33/34/35).</li>
        <li>Return or delete all Personal Data upon termination of the agreement (Art. 28(3)(g)), except where legal retention applies.</li>
        <li>Make available the information necessary to demonstrate compliance and allow for audits (Art. 28(3)(h)).</li>
      </ul>

      <h2>4. Sub-processors</h2>
      <p>
        The Customer authorizes the use of sub-processors as listed at{" "}
        <a href="/subprocessors/">/subprocessors</a>. INVERSO HUB will notify the Customer by email +
        RSS at least <strong>30 days in advance</strong> before adding or replacing a sub-processor,
        allowing the Customer to object on reasonable grounds.
      </p>

      <h2>5. International transfers</h2>
      <p>
        INVERSO HUB applies the <strong>Standard Contractual Clauses (SCC) 2021/914 Module 2</strong>{" "}
        for transfers from the EEA to its sub-processors. The Customer accedes to those clauses as
        data exporter, and INVERSO HUB executes them with each sub-processor as data importer.
      </p>

      <h2>6. Technical and organizational measures (SCC Annex II)</h2>
      <h3>Pseudonymization / encryption</h3>
      <ul>
        <li>TLS 1.3 for all communications in transit.</li>
        <li>AES-256 encryption at rest (PostgreSQL, backups).</li>
        <li>HMAC-SHA256 on Paddle webhooks.</li>
        <li>License tokens and magic links: JWTs signed with Ed25519, short expiration.</li>
      </ul>
      <h3>Confidentiality, integrity, availability, resilience</h3>
      <ul>
        <li>Append-only audit log with DDL triggers blocking UPDATE/DELETE.</li>
        <li>Daily encrypted backups to Cloudflare R2 (Frankfurt), retained for 30 days.</li>
        <li>PostgreSQL streaming replication to a Hetzner EU standby.</li>
        <li>24/7 monitoring (Better Stack), alerts to on-call.</li>
      </ul>
      <h3>Restoration after an incident</h3>
      <ul>
        <li>RPO: 1 hour (backups + WAL streaming).</li>
        <li>RTO: 4 hours (manual failover to standby).</li>
        <li>Monthly restore test.</li>
      </ul>
      <h3>Regular testing and evaluation</h3>
      <ul>
        <li>SAST with Bandit + Ruff + Semgrep + Gitleaks on every commit.</li>
        <li>Weekly DAST with OWASP ZAP.</li>
        <li>Annual external pen-test.</li>
        <li>Weekly Renovate + osv-scanner in CI.</li>
      </ul>
      <h3>Identification and authentication</h3>
      <ul>
        <li>Passwordless magic link (default).</li>
        <li>TOTP 2FA opt-in (Pro+), mandatory for Team+ admins.</li>
        <li>Single-use tokens, 10-minute expiry.</li>
        <li>Rate limit of 3/min/IP.</li>
      </ul>
      <h3>Access control</h3>
      <ul>
        <li>Least-privilege principle across Inverso infrastructure (operational admins ≤ 2 people).</li>
        <li>Append-only, tamper-evident audit log of all admin actions.</li>
      </ul>
      <h3>Vendor management</h3>
      <ul>
        <li>GDPR due diligence before onboarding (CF, Paddle, GitHub, Sentry).</li>
        <li>DPA / SCCs executed with each vendor.</li>
        <li>Annual review.</li>
      </ul>

      <h2>7. Breach notification</h2>
      <p>
        INVERSO HUB will notify the Customer of any security breach affecting their Personal Data
        within <strong>72 hours</strong> of detection, including:
      </p>
      <ul>
        <li>Nature of the breach + categories and approximate volume of data subjects / records affected.</li>
        <li>Likely consequences.</li>
        <li>Measures taken and proposed.</li>
        <li>DPO contact details.</li>
      </ul>

      <h2>8. Audit</h2>
      <p>
        The Customer may request a documentation audit (review of the SOC2 evidence pack, most recent
        pen-test report, sub-processor list). On-site audits are subject to 60 days&apos; notice,
        costs borne by the Customer, and a mutual NDA.
      </p>

      <h2>9. Termination and return</h2>
      <p>
        Upon termination of the agreement, INVERSO HUB:
      </p>
      <ul>
        <li>Exports the Customer&apos;s Personal Data in structured JSON format.</li>
        <li>Deletes active copies within 30 days, except where legal retention applies.</li>
        <li>Encrypted backups are automatically purged 30 days after generation.</li>
      </ul>

      <h2>10. Liability</h2>
      <p>
        The limitations of liability in the main agreement apply to this DPA, without prejudice to
        non-derogable obligations under the GDPR.
      </p>

      <h2>11. Signed PDF download</h2>
      <p>
        For Team/Enterprise: a signed PDF is available upon request to{" "}
        <a href="mailto:legal@furx.cloud?subject=DPA%20PDF">legal@furx.cloud</a> (delivered
        within 48 business hours).
      </p>

      <h2>12. DPO contact</h2>
      <p>
        <a href="mailto:dpo@furx.cloud">dpo@furx.cloud</a>
      </p>
    </LegalLayout>
  );
}
