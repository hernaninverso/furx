// 017 T045/T063 — verifyOutbound round-trips the server-signed frames, and the
// JS canonical for the new tags matches what the Rust `sign_outbound` produces
// (cross-lang: same length-prefixed encoding). Run: `node outbound-sign.test.mjs`.
import { canonicalBytes, hmacHex, verifyOutbound, OUTBOUND_TAGS, nowSecs } from "./furx-sign.js";

const SECRET = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
let pass = 0, fail = 0;
const ok = (c, n) => (c ? pass++ : (fail++, console.log(`FAIL ${n}`)));

// Tag table mirrors mobile_bridge.rs.
ok(OUTBOUND_TAGS.nav_spec === "NavSpec_", "nav_spec tag");
ok(OUTBOUND_TAGS.command_catalog === "CmdCatlg", "command_catalog tag");
ok(OUTBOUND_TAGS.app_event === "AppEvnt_", "app_event tag");

// Build a signed NavSpec frame the way the server does, then verify it.
// 017 [MF-2]: verifyOutbound now enforces ts-skew + nonce dedup, so frames must
// carry a CURRENT ts and a UNIQUE nonce (the server's `sign_outbound` does both).
let nonceSeq = 0;
async function makeFrame(kind, body, ts = nowSecs(), nonce = null) {
  if (nonce === null) nonce = `s-${nonceSeq++}`; // unique per frame
  const tag = OUTBOUND_TAGS[kind];
  const sig = await hmacHex(SECRET, canonicalBytes(tag, nonce, ts, body, "", ""));
  return { nonce, ts, sig };
}

const navBody = JSON.stringify({ version: 1, domains: [] });
const navFrame = await makeFrame("nav_spec", navBody);
ok(await verifyOutbound(SECRET, "nav_spec", navFrame, navBody), "valid nav_spec verifies");

// Tampered body → reject.
ok(!(await verifyOutbound(SECRET, "nav_spec", navFrame, JSON.stringify({ version: 9, domains: [] }))), "tampered nav_spec body rejected");

// Wrong secret → reject.
ok(!(await verifyOutbound("ff".repeat(32), "nav_spec", navFrame, navBody)), "wrong secret rejected");

// Wrong tag (sign as nav_spec, verify as command_catalog) → reject (no cross-frame replay).
ok(!(await verifyOutbound(SECRET, "command_catalog", navFrame, navBody)), "cross-frame tag replay rejected");

// app_event body convention is "seq|json".
const evBody = `42|${JSON.stringify({ tag: "TaskChanged", data: { id: "a", state: "running" } })}`;
const evFrame = await makeFrame("app_event", evBody);
ok(await verifyOutbound(SECRET, "app_event", evFrame, evBody), "valid app_event verifies");
// Re-numbered seq (different body) → reject (seq is authenticated).
ok(!(await verifyOutbound(SECRET, "app_event", evFrame, `99|${JSON.stringify({ tag: "TaskChanged", data: { id: "a", state: "running" } })}`)), "re-numbered seq rejected");

// 017 [MF-2] anti-replay: a genuine, already-seen frame is rejected on the second
// verify (nonce dedup) — defeats a MITM that recaptures a signed frame and replays
// it after a reconnect resets the seq cursor.
const replayBody = JSON.stringify({ version: 1, domains: [] });
const replayFrame = await makeFrame("nav_spec", replayBody);
ok(await verifyOutbound(SECRET, "nav_spec", replayFrame, replayBody), "first verify of fresh frame passes");
ok(!(await verifyOutbound(SECRET, "nav_spec", replayFrame, replayBody)), "replay of same frame rejected (nonce dedup)");

// 017 [MF-2] skew: a correctly-signed but stale frame (ts outside the window) is rejected.
const staleFrame = await makeFrame("nav_spec", replayBody, nowSecs() - 3600, "s-stale");
ok(!(await verifyOutbound(SECRET, "nav_spec", staleFrame, replayBody)), "stale ts rejected (skew)");

// Cross-lang pin: this exact hex must equal the Rust sign_outbound for the same
// inputs. (The Rust test t063 asserts Rust == canonical HMAC; here we pin JS so
// drift on either side fails.)
const pinSig = await hmacHex(SECRET, canonicalBytes("NavSpec_", "n-out", 1700000000, '{"version":1,"domains":[]}', "", ""));
const RUST_PIN = "e45ba44aac4cab09319da86fbb8788ccb67056198d371fea39889dfd47b6c51e";
ok(pinSig === RUST_PIN, `navspec pin matches Rust (got ${pinSig})`);

console.log(`outbound-sign: ${pass} passed, ${fail} failed`);
process.exit(fail === 0 ? 0 : 1);
