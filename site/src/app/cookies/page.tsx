import type { Metadata } from "next";
import LegalLayout from "@/components/LegalLayout";

export const metadata: Metadata = {
  title: "Cookie Policy",
  description: "Furx Cookie Policy — only strictly necessary cookies by default. Plausible analytics is opt-in. No third-party tracking.",
  alternates: { canonical: "https://furx.cloud/cookies/" },
};

export default function CookiesPage() {
  return (
    <LegalLayout title="Cookie Policy · Furx">
      <p>
        This policy explains which cookies and local storage technologies are used by the websites
        operated by INVERSO HUB S.R.L. in connection with Furx (<code>furx.cloud</code>,{" "}
        <code>app.furx.cloud</code>) and how you can manage them.
      </p>

      <h2>1. Quick summary</h2>
      <ul>
        <li>By default: only strictly necessary cookies. No third-party tracking.</li>
        <li>Measurement cookies (self-hosted Plausible) are <strong>opt-in</strong> — the initial banner asks first.</li>
        <li>We do not use advertising cookies, Google Analytics, or Facebook Pixel.</li>
        <li>We do not sell or share data with third-party adtech.</li>
      </ul>

      <h2>2. Strictly necessary cookies</h2>
      <table>
        <thead><tr><th>Name</th><th>Purpose</th><th>Duration</th><th>Type</th></tr></thead>
        <tbody>
          <tr><td><code>furx_consent_v1</code></td><td>Stores your consent preference for the banner</td><td>1 year</td><td>localStorage</td></tr>
          <tr><td><code>furx_session</code></td><td>Authenticated dashboard session (HttpOnly, Secure, SameSite=Lax)</td><td>30 days</td><td>HTTP cookie</td></tr>
          <tr><td><code>furx_csrf</code></td><td>Anti-CSRF token for authenticated forms</td><td>Session</td><td>HTTP cookie</td></tr>
        </tbody>
      </table>
      <p>
        These cookies are <strong>essential</strong> for the dashboard to function. They do not require
        consent (Recital 32, GDPR; Art. 22(4) ePrivacy).
      </p>

      <h2>3. Opt-in cookies / technologies (measurement)</h2>
      <table>
        <thead><tr><th>System</th><th>Purpose</th><th>Duration</th><th>Privacy</th></tr></thead>
        <tbody>
          <tr>
            <td><strong>Self-hosted Plausible</strong></td>
            <td>Site usage metrics (page views, referrer, screen size)</td>
            <td>Cookie-less</td>
            <td>Anonymous, no fingerprinting, daily IP hash that is discarded</td>
          </tr>
        </tbody>
      </table>
      <p>
        If you decline, Plausible does not load. If you accept, it collects only aggregate metrics (not individual ones).
        Plausible is self-hosted on infrastructure operated by INVERSO HUB S.R.L. — the data is not shared with any third party.
      </p>

      <h2>4. Technologies we do NOT use</h2>
      <ul>
        <li>Google Analytics, Google Tag Manager.</li>
        <li>Facebook Pixel, Twitter Pixel, LinkedIn Insight.</li>
        <li>Hotjar, FullStory, Mixpanel, Amplitude, or other intrusive analytics.</li>
        <li>Third-party advertising cookies.</li>
        <li>Beacons, canvas/audio/WebGL fingerprinting.</li>
      </ul>

      <h2>5. How to manage your consent</h2>
      <ul>
        <li>Initial banner: the first time you visit the site, you can choose &quot;Essentials only&quot; or &quot;Allow measurement&quot;.</li>
        <li>Change later: <em>Footer → Cookie preferences</em>.</li>
        <li>Delete manually: clear <code>localStorage</code> and cookies from your browser.</li>
      </ul>

      <h2>6. Browsers with tracking protection</h2>
      <p>
        We respect the <strong>Do Not Track</strong> (DNT) and <strong>Global Privacy Control</strong>{" "}
        (GPC, IAB) signals. If your browser sends DNT=1 or GPC=1, we do not load Plausible even if you previously accepted.
      </p>

      <h2>7. Third-party cookies that may appear (link-out)</h2>
      <p>
        When you click outbound links (GitHub, Paddle checkout, LLM providers), you enter the
        third party&apos;s domain, which operates under its own policies. We have no control over the cookies
        those sites set.
      </p>

      <h2>8. Changes</h2>
      <p>
        Material changes are announced with a persistent banner for 30 days. Minor changes
        (internal renames, date updates) are published without notice.
      </p>

      <h2>9. Contact</h2>
      <p>
        Questions about cookies: <a href="mailto:dpo@furx.cloud">dpo@furx.cloud</a>
      </p>
    </LegalLayout>
  );
}
