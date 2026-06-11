#!/usr/bin/env bash
# Build audit bundle for triple audit (Codex + Gemini + DeepSeek via AIE).
set -euo pipefail

OUT="${1:-/tmp/furx-site-audit-bundle.md}"
cd "$(dirname "$0")/../.."

{
  echo "# (Furx) web audit bundle (2026-05-27)"
  echo
  echo "## Scope"
  echo
  echo "Two Next.js 15/16 sites: \`site/\` (public marketing+docs+legal, 30 static pages) and \`app-dashboard/\` (passwordless dashboard, 10 pages). Stack: Next 16, React 19, Tailwind 3.4, static export to Cloudflare Pages."
  echo
  echo "## What to audit"
  echo
  echo "1. **Correctness bugs**: broken imports, dead links, typos, missing aria, unreachable routes."
  echo "2. **Security**: CSP coverage, magic-link flow safety, fetch credentials handling, secrets in code (env vars vs hardcoded), CSP unsafe-inline, OWASP top 10 in the SPA shell."
  echo "3. **Legal gaps**: TOS/Privacy/DPA missing required clauses for BYOK + Argentina entity + Paddle MoR + GDPR."
  echo "4. **Brand consistency**: coral #FF5C35 everywhere, F-mark pattern, Geist+Inter+JetBrains Mono."
  echo "5. **Performance/SEO**: missing meta, missing OG, missing schema.org, missing hreflang, broken sitemap."
  echo "6. **A11y**: focus traps, skip-link, ARIA on nav/dialogs, contrast."
  echo
  echo "## File manifest"
  echo
  echo '```'
  find site app-dashboard -type f \( -name "*.tsx" -o -name "*.ts" -o -name "*.css" -o -name "_headers" -o -name "security.txt" -o -name "*.json" \) -not -path "*/node_modules/*" -not -path "*/.next/*" -not -path "*/out/*" -not -path "*/council-out/*" -not -path "*/legal-source/*" -not -name "package-lock.json" | sort
  echo '```'
  echo
  echo "## Critical source samples"
  echo

  for f in \
    site/src/app/layout.tsx \
    site/src/app/page.tsx \
    site/src/app/pricing/page.tsx \
    site/src/app/download/page.tsx \
    site/src/app/security/page.tsx \
    site/src/app/terms/page.tsx \
    site/src/app/privacy/page.tsx \
    site/src/app/dpa/page.tsx \
    site/src/app/aup/page.tsx \
    site/src/app/refund/page.tsx \
    site/src/app/sitemap.ts \
    site/src/app/robots.ts \
    site/src/components/Navbar.tsx \
    site/src/components/Footer.tsx \
    site/src/components/LegalLayout.tsx \
    site/src/components/CookieConsent.tsx \
    site/public/_headers \
    site/public/.well-known/security.txt \
    site/tailwind.config.ts \
    site/next.config.ts \
    app-dashboard/src/app/page.tsx \
    app-dashboard/src/app/auth/callback/page.tsx \
    app-dashboard/src/app/account/page.tsx \
    app-dashboard/src/components/Shell.tsx
  do
    echo "### \`$f\`"
    echo '```'
    cat "$f"
    echo '```'
    echo
  done
} > "$OUT"

wc -c "$OUT"
echo "bundle ready → $OUT"
