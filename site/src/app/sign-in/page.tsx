import type { Metadata } from "next";
import Link from "next/link";

import Footer from "@/components/Footer";
import Navbar from "@/components/Navbar";

const APP_URL = process.env.NEXT_PUBLIC_APP_URL || "https://app.furx.cloud";

export const metadata: Metadata = {
  title: "Sign in",
  description: "Sign in to your Furx dashboard. Passwordless magic-link via Paddle email.",
  robots: { index: false, follow: false },
};

export default function SignInPage() {
  return (
    <>
      <Navbar />
      <main id="main" className="max-w-md mx-auto px-6 py-24 text-center">
        <div className="brand-mark mx-auto mb-6 text-[26px]" aria-hidden="true" />
        <h1 className="text-3xl font-extrabold mb-3">Sign in to Furx</h1>
        <p className="text-ink-2 mb-8">
          The dashboard lives at <code className="text-accent">app.furx.cloud</code>. Sign in is
          passwordless — we send a magic link to the email you used to subscribe via Paddle.
        </p>
        <a href={APP_URL} className="btn-primary mb-4">
          Go to dashboard
        </a>
        <p className="text-ink-3 text-sm mt-6">
          Don&apos;t have a subscription yet? <Link href="/download/" className="text-accent hover:underline">Start the free trial</Link>{" "}
          (14 days Pro, no card).
        </p>
      </main>
      <Footer />
    </>
  );
}
