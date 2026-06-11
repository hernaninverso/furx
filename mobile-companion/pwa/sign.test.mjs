// Cross-language test: the JS signing module MUST produce the same HMAC hex as
// the Rust `canonical_bytes`+`sign` for a pinned vector (see the Rust test
// `cross_lang_hmac_vector` in mobile_bridge.rs). Run: `node sign.test.mjs`.
import { canonicalBytes, hmacHex } from "./furx-sign.js";

const SECRET = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const RUST_PINNED = "423eadd24f252979c0d7fdc41b87f23e64f7fab9213c7ad49610b6b5883d3e22";

const canonical = canonicalBytes("PtyWrite", "n-vec", 1700000000, "p7", "manual", "echo hi");
const sig = await hmacHex(SECRET, canonical);

if (sig !== RUST_PINNED) {
  console.error(`FAIL: JS sig mismatch\n  js:   ${sig}\n  rust: ${RUST_PINNED}`);
  process.exit(1);
}
console.log("PASS: JS HMAC matches Rust canonical encoding");
console.log(`  sig = ${sig}`);
