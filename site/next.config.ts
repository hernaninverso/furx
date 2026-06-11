import type { NextConfig } from "next";
import { resolve } from "path";

const nextConfig: NextConfig = {
  output: "export",
  images: { unoptimized: true },
  trailingSlash: true,
  turbopack: { root: resolve(__dirname) },
  env: {
    NEXT_PUBLIC_SITE_URL: process.env.NEXT_PUBLIC_SITE_URL || "https://furx.cloud",
    NEXT_PUBLIC_APP_URL: process.env.NEXT_PUBLIC_APP_URL || "https://app.furx.cloud",
    NEXT_PUBLIC_API_URL: process.env.NEXT_PUBLIC_API_URL || "https://api.furx.cloud",
    NEXT_PUBLIC_GH_REPO: process.env.NEXT_PUBLIC_GH_REPO || "https://github.com/hernaninverso/furx",
    NEXT_PUBLIC_RELEASE_BASE:
      process.env.NEXT_PUBLIC_RELEASE_BASE ||
      "https://github.com/hernaninverso/furx/releases/latest/download",
  },
};

export default nextConfig;
