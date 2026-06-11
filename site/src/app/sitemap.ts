import type { MetadataRoute } from "next";

export const dynamic = "force-static";

const SITE_URL = process.env.NEXT_PUBLIC_SITE_URL || "https://furx.cloud";

const PATHS: { url: string; priority?: number; changeFrequency?: MetadataRoute.Sitemap[number]["changeFrequency"] }[] = [
  { url: "/", priority: 1.0, changeFrequency: "weekly" },
  { url: "/council-mode/", priority: 0.9, changeFrequency: "monthly" },
  { url: "/providers/", priority: 0.9, changeFrequency: "monthly" },
  { url: "/pricing/", priority: 0.9, changeFrequency: "weekly" },
  { url: "/download/", priority: 0.9, changeFrequency: "weekly" },
  { url: "/docs/", priority: 0.8, changeFrequency: "weekly" },
  { url: "/docs/quickstart/", priority: 0.7, changeFrequency: "monthly" },
  { url: "/docs/byok/", priority: 0.7, changeFrequency: "monthly" },
  { url: "/docs/council/", priority: 0.7, changeFrequency: "monthly" },
  { url: "/docs/providers/", priority: 0.7, changeFrequency: "monthly" },
  { url: "/docs/integrations/", priority: 0.7, changeFrequency: "monthly" },
  { url: "/docs/keychain/", priority: 0.7, changeFrequency: "monthly" },
  { url: "/docs/audit/", priority: 0.7, changeFrequency: "monthly" },
  { url: "/docs/troubleshooting/", priority: 0.6, changeFrequency: "monthly" },
  { url: "/changelog/", priority: 0.8, changeFrequency: "weekly" },
  { url: "/community/", priority: 0.6, changeFrequency: "monthly" },
  { url: "/sign-in/", priority: 0.4, changeFrequency: "yearly" },
  { url: "/security/", priority: 0.5, changeFrequency: "monthly" },
  { url: "/terms/", priority: 0.4, changeFrequency: "yearly" },
  { url: "/privacy/", priority: 0.4, changeFrequency: "yearly" },
  { url: "/dpa/", priority: 0.4, changeFrequency: "yearly" },
  { url: "/sla/", priority: 0.4, changeFrequency: "yearly" },
  { url: "/subprocessors/", priority: 0.4, changeFrequency: "monthly" },
  { url: "/aup/", priority: 0.4, changeFrequency: "yearly" },
  { url: "/refund/", priority: 0.4, changeFrequency: "yearly" },
  { url: "/cookies/", priority: 0.4, changeFrequency: "yearly" },
  { url: "/imprint/", priority: 0.4, changeFrequency: "yearly" },
  { url: "/escrow/", priority: 0.3, changeFrequency: "yearly" },
];

export default function sitemap(): MetadataRoute.Sitemap {
  const now = new Date();
  return PATHS.map((p) => ({
    url: `${SITE_URL}${p.url}`,
    lastModified: now,
    changeFrequency: p.changeFrequency,
    priority: p.priority,
  }));
}
