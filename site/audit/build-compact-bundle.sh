#!/usr/bin/env bash
# Build COMPACT audit bundle — only summary + 4 critical files for fast triple audit.
set -euo pipefail
OUT="${1:-/tmp/furx-site-audit-compact.md}"
cd "$(dirname "$0")/../.."

{
  echo "# (Furx) web audit — compact bundle"
  echo
  echo "## Project summary"
  echo "Two Next.js 16 + React 19 + Tailwind 3.4 static-export sites:"
  echo "- site/ — public marketing: landing, pricing, download, 8 docs pages, changelog, community, security, sign-in, 10 legal pages, 404. 30 routes total."
  echo "- app-dashboard/ — passwordless dashboard: sign-in (magic link), account, downloads, seats, audit replay, compliance, settings, 404. 10 routes."
  echo
  echo "Operator: INVERSO HUB S.R.L. (Argentina). Billing: Paddle MoR (UK/US). Hosting: Cloudflare Pages."
  echo "Brand: coral #FF5C35 F-mark. Geist + Inter + JetBrains Mono variable fonts (self-hosted via fontsource)."
  echo
  echo "## Routes manifest (site/)"
  ls -1 site/src/app/*/page.tsx site/src/app/*/*/page.tsx 2>/dev/null | sort
  echo
  echo "## Routes manifest (app-dashboard/)"
  ls -1 app-dashboard/src/app/*/page.tsx app-dashboard/src/app/*/*/page.tsx 2>/dev/null | sort
  echo
  echo "## Top 4 critical files"
  echo
  for f in site/src/app/layout.tsx site/public/_headers site/src/app/page.tsx app-dashboard/src/app/page.tsx ; do
    echo "### $f"
    echo '```'
    cat "$f"
    echo '```'
    echo
  done
  echo
  echo "## What to audit · be specific with file:line refs · return JSON only"
  echo
  echo "1. CORRECTNESS bugs (broken imports, dead links, missing tags)."
  echo "2. SECURITY: CSP has unsafe-inline for script-src — fix or accept? Magic-link flow uses credentials:'include' on cross-origin POST — CORS allowlist + CSRF? Secrets in client code? OWASP top 10."
  echo "3. LEGAL gaps: TOS/Privacy/DPA for BYOK + Argentina + Paddle MoR + GDPR Art. 28."
  echo "4. BRAND consistency: coral #FF5C35 everywhere, F motif, font cascade."
  echo "5. SEO/perf/a11y: missing meta, missing schema, missing hreflang, broken sitemap, missing skip-link."
  echo
  echo "## Response format (strict)"
  echo
  echo 'Return ONE JSON object only, no markdown fence: {"high":[{"file":"path","issue":"...","fix":"..."}],"medium":[...],"low":[...],"summary":"one sentence"}'
} > "$OUT"

wc -c "$OUT"
echo "compact bundle ready → $OUT"
