// Versioned legal texts. When you edit one, BUMP `version` so the UI asks
// the user to re-accept (recorded in settings.opt_in.eula_accepted_at +
// eula_accepted_version).
//
// English is the binding language for all Furx legal documents.
// These texts are a reasonable starting point for a BYOK app that never
// touches the user's keys. Before charging at scale, have counsel review
// the substantive terms (the Compliance Pack includes a DPA template).
//
// 2026-06-09 brand wave 5: all in-app legal moved to English (single
// binding version), entity INVERSO HUB S.R.L., Apache-2.0 core, zero
// personal references. Tier fact fixed: Council Mode is FREE — only Pro
// features degrade when the trial ends.

export const LEGAL_VERSION = "2026-06-09";

export const EULA = `End User License Agreement — Furx
Version ${LEGAL_VERSION}

1. LICENSE
Furx is distributed under the Apache License 2.0 (full text in the
"Licenses" section). This EULA supplements the Apache-2.0 with
product-specific terms of use.

2. OWNERSHIP
Furx is a trademark of INVERSO HUB S.R.L. The source code is open under
Apache-2.0. The distributed binaries (DMG / DEB / RPM / AppImage / MSI)
may contain signatures and certificates that are not distributed with
the source.

3. ACCEPTABLE USE
You may use Furx for any lawful purpose. You may not use Furx to:
  • Generate unlawful or harmful content, or content that violates the
    terms of the LLM providers you connect (Anthropic, OpenAI, etc.).
  • Circumvent technical or licensing restrictions of the providers you
    connect.
  • Distribute modified builds without the attribution the Apache-2.0
    license requires (§4: keep copyright, license, and NOTICE notices).

4. YOUR DATA AND YOUR KEYS (BYOK)
Furx uses the "Bring Your Own Keys" model: you connect your own API keys
for the LLM providers you choose. Furx does NOT proxy, does NOT store
anything on a server, and does NOT spend free-tier quotas of its own.
Calls go directly from your machine to the provider. See the Privacy
Policy for details.

5. NO WARRANTY
FURX IS PROVIDED "AS IS" WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED. INVERSO HUB S.R.L. IS NOT LIABLE FOR DAMAGES ARISING FROM THE
USE OR INABILITY TO USE THE SOFTWARE, INCLUDING DATA LOSS, COSTS OF
THIRD-PARTY API CALLS, OR THIRD-PARTY TERMS VIOLATIONS ARISING FROM
YOUR USE.

6. PRO SUBSCRIPTION
Pro features (cloud sync, session replay scrubber, Cost-Router and cost
meter, encrypted backups, premium themes) require an active subscription
processed by Paddle.com as Merchant of Record. Council Mode, the panes,
the audit log, the Memory Hub, and BYOK are part of the free core and do
not require a subscription. See the Terms of Service for billing and
cancellation details.

7. GOVERNING LAW
This agreement is governed by the laws of the Argentine Republic.
Disputes are resolved before the ordinary courts of the Autonomous City
of Buenos Aires.

8. CHANGES
Any material change to this EULA is announced in a subsequent Furx
release and requires re-acceptance.
`;

export const PRIVACY_POLICY = `Privacy Policy — Furx
Version ${LEGAL_VERSION}

# Short summary
Furx has no server of its own for your keys or the content of your
prompts. Everything lives on your machine, in local SQLite databases
under ~/.furx/. LLM calls go DIRECTLY from your machine to the provider
whose key you connected.

# Data Furx stores LOCALLY
  • Cards / incidents / monitors: append-only SQLite in ~/.furx/furx.db.
  • Pane layouts, settings, UI snapshots.
  • Cross-CLI memory (Memory Hub): SQLite FTS5 in ~/.furx/memory.db (opt-in).
  • Skills installed from ~/.claude/skills and ~/.furx/skills.
  • Crash reports in ~/Library/Application Support/furx/crashes/,
    rotated with a cap of 50 files / 10 MB total.
  • Provider API keys: in the OS keychain (NEVER in a plain file).

# Data that LEAVES your machine
  • Calls to the LLM providers you connected, with the matching key.
  • Anonymous telemetry (OPT-IN, disabled by default): feature usage
    counts and latencies — no prompt content, no keys.
  • Crash reports (OPT-IN): stacktrace + version + OS, without prompt
    content and with automatic redaction of Bearer tokens / API keys.
  • Pro license verification: contacts your configured endpoint
    (default: the licensing endpoint operated by INVERSO HUB S.R.L.;
    configurable under Services).

# Data that NEVER leaves
  • The content of your prompts and the LLM responses (except the direct
    calls to the provider, which do NOT pass through Furx).
  • Your API keys (they always reside in the OS keychain).
  • Your local files, code, or any path you index via Search /
    Embeddings (indexing is 100% local).

# Mobile companion and Cloud Sync (Pro)
If you enable Cloud Sync (a Pro feature), a copy of your audit log
metadata (timestamps, event types, model names — never prompt content),
your skills, and your .mcp.json is uploaded over TLS to the sync
endpoint you configure. See the subprocessors list at
furx.cloud/subprocessors/ for where that data is hosted. Pairing between
the mobile companion and the desktop uses an HMAC with a shared secret
that NEVER leaves the keychains on either side.

# Subprocessors
  • Paddle.com (Pro subscription): Merchant of Record. Receives your
    billing data when you buy Pro. Own policy: paddle.com/legal.
  • Cloudflare Pages: hosts the public site. Does NOT see your local app.
  • GitHub: distributes the binaries (Releases) and the auto-updater feed.
  The complete, current list (including the optional Pro sync and
  opt-in crash-report hosts) is published at furx.cloud/subprocessors/.

# Legal basis for processing (GDPR art. 6)
When Furx processes personal data, it does so under one of these bases:
  • **Contract performance** (art. 6.1.b): Pro billing via Paddle,
    support contact, binary delivery.
  • **Legitimate interest** (art. 6.1.f): anonymized crash reports to
    diagnose failures (opt-in; you can object by disabling it in Settings).
  • **Consent** (art. 6.1.a): opt-in anonymous telemetry.

# Your rights
Under GDPR / CCPA and analogous laws you have the right to:
  • **Access** the data we hold about you (typically none outside your
    Paddle account).
  • **Rectification** of incorrect data.
  • **Erasure** (right to be forgotten): Settings → Reset (soft/hard/full)
    deletes everything local; for Paddle, paddle.com/legal/privacy.
  • **Portability**: export your .furxexport from Settings → Data.
  • **Objection** to processing based on legitimate interest.
  • **Restriction** of processing while we evaluate a request.
  • **Do not sell my personal information** (CCPA): Furx does not sell
    personal data to third parties. If that ever changed, this would be
    the opt-out: mailto:dpo@furx.cloud?subject=Do%20Not%20Sell.

# Retention
| Category | Retention |
|---|---|
| Pro billing data (Paddle) | Until cancellation + 7 years (AR tax requirement) |
| Local audit logs (~/.furx/furx.db) | Until you delete them |
| UI snapshots | Until you delete them |
| Opt-in crash reports | 90 days at the endpoint you configure |
| Opt-in anonymous telemetry | 90 days, aggregated |
| Memory Hub sessions (FTS5) | Until you delete them |

# International data transfers
If you are in the EU/EEA and your billing data is processed in the US
(Paddle, GitHub, Cloudflare), the **Standard Contractual Clauses**
approved by the European Commission (Decision (EU) 2021/914) apply.
Paddle is our Data Processor for billing and operates under its own DPA
with SCCs.

# Deleting your data
You can delete everything local with Settings → Reset (soft/hard/full).
For data held by Paddle (Pro billing): paddle.com/legal/privacy.

# Contact
dpo@furx.cloud — data protection contact. Response within 30 days for
GDPR/CCPA requests. EU residents may also contact their local
supervisory authority.
`;

export const TERMS_OF_SERVICE = `Terms of Service — Furx
Version ${LEGAL_VERSION}

1. ACCEPTANCE
By using Furx you accept these terms, the EULA, and the Privacy Policy.

2. PRO SUBSCRIPTION
2.1 Processor: Paddle.com acts as Merchant of Record. Charges appear on
your statement as "Paddle * Furx".
2.2 Period: monthly ($12 USD/mo) or annual ($99 USD/yr). Auto-renews
until you cancel.
2.3 Trial: 14 days free, no card, on first install. When the trial
expires, Pro features degrade (Cloud Sync becomes read-only, session
replay and Cost-Router are disabled). The free core — including Council
Mode, panes, audit log, Memory Hub, and BYOK — keeps working in full.
2.4 Cancellation: from the Paddle portal. Cancellation takes effect at
the end of the current billing period; no proration.
2.5 Refunds: within 14 days of the initial purchase, by contacting
support@furx.cloud. After 14 days, no refund, but you can cancel to
avoid the next renewal.

3. PRICING
Prices may change with 30 days' notice. The new price applies from the
next renewal period.

4. ENTERPRISE TIER
$49/seat/mo or $2,500 USD perpetual self-host. Includes a notarized
build, data-residency options, source-code escrow, and white-label.
Scope and onboarding are agreed directly via support@furx.cloud.

5. SLA AND SUPPORT
Free: best-effort via GitHub Issues.
Pro: response within 5 business days via support@furx.cloud.
Team: 24-business-hour SLA + direct channel.
Enterprise: contractually negotiated SLA.

6. LIMITATION OF LIABILITY
The total liability of INVERSO HUB S.R.L. for damages arising from the
use of Furx is limited to the amount actually paid for the subscription
in the 12 months preceding the incident.

7. TERMINATION
INVERSO HUB S.R.L. may terminate the Pro service with 30 days' notice
in case of EULA violation, payment fraud, or use that harms the
infrastructure.

8. CHANGES
Material changes are announced by email (Paddle account) and in-app at
least 14 days before taking effect.

9. GOVERNING LAW
Laws of Argentina, jurisdiction of the Autonomous City of Buenos Aires.
For EU users: your GDPR rights are not affected.

Last updated: ${LEGAL_VERSION}
`;

export const DPA_TEMPLATE = `Data Processing Agreement (DPA) — Template
Version ${LEGAL_VERSION}

> This template is included in the "Compliance Pack" ($199 one-time).
> It is not a valid DPA on its own — complete the [BRACKETED] sections
> and review with your own counsel.

1. PARTIES
1.1 Controller: [YOUR COMPANY], domiciled at [ADDRESS].
1.2 Processor: INVERSO HUB S.R.L. (Furx), Autonomous City of Buenos
Aires, Argentina.

2. SUBJECT MATTER AND NATURE OF PROCESSING
Furx runs on the Controller's infrastructure (its employees' laptops/
desktops). The Processor does NOT receive or store the content of the
Controller's prompts, responses, or API keys.

3. DATA CATEGORIES
3.1 Personal data processed: [PRO BILLING EMAIL].
3.2 Data NOT processed by Furx: prompt content, LLM responses, API
keys, local files, the Controller's source code.

4. SUB-PROCESSORS
4.1 Paddle.com (Pro billing): Merchant of Record, authorized
sub-processor. Own DPA: paddle.com/legal/dpa.
4.2 Cloudflare Pages (auto-updater, public site): static hosting, does
NOT receive Controller data.
4.3 GitHub (Releases): distribution of signed binaries, does NOT
receive Controller data.
4.4 The complete, current sub-processor list is published at
furx.cloud/subprocessors/ (30 days' notice before additions).

5. SECURITY MEASURES (Art. 32 GDPR)
5.1 Encryption at rest: API keys in the OS keychain (not in files).
5.2 Encryption in transit: TLS 1.2+ for all outbound communication.
5.3 Append-only audit log: SQLite triggers block UPDATE/DELETE.
5.4 Auto-update signed with Ed25519/minisign.

6. INTERNATIONAL SUB-PROCESSING
Paddle/Cloudflare/GitHub servers may be outside the EEA. For EU
customers, Paddle offers Standard Contractual Clauses (SCCs).

7. DATA SUBJECT REQUESTS
The Controller is responsible for handling GDPR requests from its end
users. The Processor cooperates without undue delay to retrieve any
data the Processor might hold (typically: none).

8. INCIDENT NOTICE
The Processor notifies the Controller of any material security incident
within 72 hours of becoming aware of it.

9. AUDIT
The Controller is entitled to an annual audit (self-assessment + Q&A)
of the Processor's compliance, with 30 days' prior notice.

10. TERM AND TERMINATION
This DPA remains in force for the duration of the Pro/Team/Enterprise
subscription contract. Upon termination, the Processor retains no
Controller data (it did not store any during the term).

Controller signature:    _________________________   Date: __________
Processor signature:     INVERSO HUB S.R.L.          Date: ${LEGAL_VERSION}
`;

export const APACHE_LICENSE = `Apache License, Version 2.0

Copyright 2026 INVERSO HUB S.R.L.

Licensed under the Apache License, Version 2.0 (the "License"); you may not
use this software except in compliance with the License. You may obtain a
copy of the License at:

    http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS, WITHOUT
WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied. See the
License for the specific language governing permissions and limitations
under the License.

The complete license text ships with the source in the LICENSE file and is
also available at the URL above. A NOTICE file with attribution accompanies
the distribution; third-party components and their licenses are listed under
"Open source components".
`;

export const OSS_NOTICES = `Third-party components included in Furx:

• Tauri 2 (Apache-2.0 / MIT) — https://tauri.app
• React 19 (MIT) — https://react.dev
• Vite 6 (MIT) — https://vitejs.dev
• xterm.js 5.5 (MIT) — https://xtermjs.org
• portable-pty (MIT) — https://github.com/wez/wezterm
• rusqlite (MIT) — https://github.com/rusqlite/rusqlite
• rusqlite_migration (MIT)
• reqwest (MIT/Apache-2.0)
• tokio (MIT)
• serde (MIT/Apache-2.0)
• axum (MIT)
• mdns-sd (MIT)
• hmac, sha2, subtle, hex (MIT/Apache-2.0)
• once_cell, parking_lot (MIT/Apache-2.0)
• regex (MIT/Apache-2.0)
• keyring 3 (MIT/Apache-2.0)
• blake3 (CC0/Apache-2.0)
• mdns-sd 0.13 (MIT) — https://github.com/keepsimple1/mdns-sd
• tracing, tracing-subscriber (MIT)
• whisper.cpp (MIT, downloaded on demand)
• sox (LGPL, external executable)

The full text of each license is available at
https://github.com/hernaninverso/furx/tree/main/THIRD_PARTY_NOTICES
`;
