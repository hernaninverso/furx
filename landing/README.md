# Furx landing — copy listo para Cloudflare Pages

> Sube `index.html` a `furx.cloud` (o `furx.cloud`). El hero, pricing y CTAs ya están preparados.

## Deploy

```bash
cd ~/furx/landing
wrangler pages deploy . --project-name=furx-site --branch=main
# o desde GitHub Actions con CLOUDFLARE_API_TOKEN + CLOUDFLARE_ACCOUNT_ID
```

## Estructura

- `index.html` — hero + pricing + features + footer (Inverso brand, ícono coral F).
- `download.html` — links a GitHub Releases (macOS DMG, Linux deb/rpm/AppImage, Windows MSI).
- `assets/` — logo SVG, screenshots placeholder.

## Deploy

Static HTML — deploy via Cloudflare Pages (see `site/` for the current production site).
