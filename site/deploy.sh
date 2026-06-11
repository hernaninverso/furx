#!/usr/bin/env bash
# Deploy (Furx) public site to Cloudflare Pages.
# Project: furx-site (already exists per HUMAN_ACTIONS.md, since 2026-05-27).
#
# Pre-req:
#   - CLOUDFLARE_API_TOKEN env var with Pages:Edit on your Cloudflare account
#   - CLOUDFLARE_ACCOUNT_ID env var set to your Cloudflare account ID
#
# Usage:
#   ./site/deploy.sh

set -euo pipefail
cd "$(dirname "$0")"

if [ -z "${CLOUDFLARE_API_TOKEN:-}" ]; then
  echo "ERROR: CLOUDFLARE_API_TOKEN not set"
  echo "Hint: export it from your Keychain entry:"
  echo "  export CLOUDFLARE_API_TOKEN=\$(security find-generic-password -a \"\$USER\" -s <entry-name> -w)"
  exit 1
fi

if [ -z "${CLOUDFLARE_ACCOUNT_ID:-}" ]; then
  echo "ERROR: CLOUDFLARE_ACCOUNT_ID not set"
  exit 1
fi

echo "==> Building site (Next.js static export)…"
npm run build

if [ ! -d out ]; then
  echo "ERROR: out/ not created"
  exit 1
fi

echo "==> Deploying to furx-site (production main branch)…"
wrangler pages deploy out --project-name=furx-site --branch=main --commit-message="Furx full web rewrite $(date +%Y-%m-%d)"

echo
echo "==> Done. Live at:"
echo "    https://furx-site.pages.dev/"
echo
echo "Once DNS is in place: furx.cloud CNAME → furx-site.pages.dev"
