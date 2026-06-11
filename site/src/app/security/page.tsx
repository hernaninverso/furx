import type { Metadata } from "next";
import LegalLayout from "@/components/LegalLayout";

export const metadata: Metadata = {
  title: "Security policy",
  description: "Security disclosure policy and vulnerability reporting for Furx.",
  alternates: { canonical: "https://furx.cloud/security/" },
};

export default function SecurityPage() {
  return (
    <LegalLayout title="Security policy">
      <h2>Reporting a vulnerability</h2>
      <p>
        If you believe you have found a security vulnerability in Furx (desktop app, the license
        API, or the public site), please report it to{" "}
        <a href="mailto:security@furx.cloud">security@furx.cloud</a>. We aim to acknowledge
        reports within <strong>2 business days</strong> and to provide a remediation timeline within{" "}
        <strong>5 business days</strong>.
      </p>
      <p>
        Encrypt sensitive reports with our PGP key (fingerprint published in{" "}
        <a href="/.well-known/security.txt">/.well-known/security.txt</a>).
      </p>

      <h2>What to include</h2>
      <ul>
        <li>A description of the vulnerability and its potential impact.</li>
        <li>Steps to reproduce, including a minimal proof-of-concept where possible.</li>
        <li>The affected component (desktop, license API, site) and version.</li>
        <li>Whether the issue requires user interaction or specific OS/CLI configuration.</li>
        <li>Your name + contact details for follow-up (optional but appreciated, hall-of-fame credit).</li>
      </ul>

      <h2>Coordinated disclosure</h2>
      <p>
        We follow a coordinated-disclosure model with a 90-day embargo from acknowledgement. We
        commit to:
      </p>
      <ul>
        <li>Working in good faith to investigate and fix valid reports.</li>
        <li>Providing credit in our hall of fame (with your permission).</li>
        <li>Not pursuing legal action against researchers who comply with this policy.</li>
        <li>Public disclosure with CVE assignment when applicable.</li>
      </ul>

      <h2>Reward programme</h2>
      <p>
        Furx is bootstrapped, so we don&apos;t have a cash bounty programme. We offer:
      </p>
      <ul>
        <li><strong>Pro lifetime</strong> for the first valid HIGH/CRITICAL report.</li>
        <li><strong>One year Pro</strong> for valid MEDIUM reports.</li>
        <li><strong>Public credit</strong> in the hall of fame for all valid reports.</li>
      </ul>

      <h2>Out of scope</h2>
      <ul>
        <li>Denial-of-service or volumetric attacks against the public site or license API.</li>
        <li>Social-engineering attacks against Furx staff.</li>
        <li>Vulnerabilities in third-party dependencies that have not yet been disclosed by the upstream maintainer.</li>
        <li>Theoretical vulnerabilities without practical impact.</li>
        <li>Issues affecting unsupported versions (older than 6 months).</li>
        <li>BYOK key compromises caused by user mistakes (e.g., committing keys to public repo) — Furx never stores or transmits user keys.</li>
      </ul>

      <h2>Security controls in place</h2>
      <ul>
        <li>OS Keychain (macOS), Secret Service (Linux), Credential Manager (Windows) for all secrets.</li>
        <li>Append-only audit log (SQLite triggers block <code>UPDATE</code>/<code>DELETE</code>).</li>
        <li>Apple Developer ID notarization (macOS).</li>
        <li>Azure Trusted Signing (Windows).</li>
        <li>Tauri auto-update signed with minisign (Ed25519); public key in <code>tauri.conf.json</code>.</li>
        <li>SHA256 manifest + minisign signature on every release artifact.</li>
        <li>License API: HTTPS only, HMAC-SHA256 webhook validation (Paddle), short-lived JWTs.</li>
        <li>Public site: HSTS preload, CSP with no <code>unsafe-inline</code>, Subresource Integrity on third-party scripts.</li>
        <li>Magic-link sign-in rate-limited (3/min per IP via Cloudflare).</li>
        <li>Crash capture with PII scrub (no paths, no env vars, no clipboard).</li>
        <li>Renovate weekly + manual <code>pip-audit</code> / <code>npm audit</code> / <code>osv-scanner</code> in CI.</li>
        <li>Gitleaks pre-commit + GitHub Actions.</li>
      </ul>

      <h2>Trust signals</h2>
      <ul>
        <li><a href="/.well-known/security.txt">/.well-known/security.txt</a> (RFC 9116) — contact + PGP + canary.</li>
        <li><a href="https://github.com/hernaninverso/furx/security/advisories">GitHub Security Advisories</a> — public CVE list.</li>
        <li><a href="https://github.com/hernaninverso/furx/blob/main/SECURITY.md">SECURITY.md</a> — version support matrix.</li>
        <li><a href="/legal/dpa/">DPA</a> for Team/Enterprise customers with audit-evidence clause.</li>
      </ul>

      <h2>Known accepted risks (transparency)</h2>
      <ul>
        <li>
          <strong>CSP <code>script-src &#39;unsafe-inline&#39;</code></strong> on the marketing site: we use
          inline <code>application/ld+json</code> blocks for Schema.org. Static export precludes
          nonce-based CSP, and hash-based CSP would re-issue per JSON-LD change. We accept this and
          mitigate by minimizing inline scripts to data-only (no executable inline). Tracking a future
          move to nonce CSP via edge Worker.
        </li>
        <li>
          <strong>SmartScreen on Windows</strong> first-launch warning is expected on fresh releases
          (reputation accrues over installs); the binary is signed via Azure Trusted Signing.
        </li>
      </ul>

      <h2>Hall of fame</h2>
      <p id="hall-of-fame">
        Open. Be the first.
      </p>
    </LegalLayout>
  );
}
