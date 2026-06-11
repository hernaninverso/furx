// 017 T045/T042 — AppEvent client: monotonic seq order + replay drop.
// Run: `node events.test.mjs`.
import { applyEvent, onAppEvent, lastSeenSeq, resetSeq, __reset } from "./events.js";

let pass = 0, fail = 0;
const ok = (c, n) => (c ? pass++ : (fail++, console.log(`FAIL ${n}`)));

__reset();
const seen = [];
const un = onAppEvent((ev) => seen.push(ev));

// 1) first event applies, cursor advances.
ok(applyEvent({ event: { tag: "TaskChanged", data: { id: "a", state: "running" } }, seq: 5 }) === true, "seq 5 applies");
ok(lastSeenSeq() === 5, "cursor == 5");

// 2) a stale/lower seq is dropped (old never overwrites newer — FR-010).
ok(applyEvent({ event: { tag: "TaskChanged", data: { id: "b", state: "idle" } }, seq: 3 }) === false, "seq 3 (stale) dropped");
ok(lastSeenSeq() === 5, "cursor still 5 after stale");

// 3) exact replay (same seq) dropped.
ok(applyEvent({ event: { tag: "X", data: {} }, seq: 5 }) === false, "seq 5 replay dropped");

// 4) higher seq applies.
ok(applyEvent({ event: { tag: "CommandExecuted", data: { command_id: "z" } }, seq: 6 }) === true, "seq 6 applies");
ok(lastSeenSeq() === 6, "cursor == 6");

// 5) only the applied events reached the handler (5 and 6).
ok(seen.length === 2, `handler saw 2 events (got ${seen.length})`);
ok(seen[0].data.id === "a" && seen[1].data.command_id === "z", "handler got the right events in order");

// 6) reset cursor (reconnect) → a fresh low seq applies again (FR-011 resync).
resetSeq();
ok(lastSeenSeq() === 0, "cursor reset to 0");
ok(applyEvent({ event: { tag: "TaskChanged", data: { id: "c", state: "running" } }, seq: 1 }) === true, "post-reset seq 1 applies");

// 7) non-finite seq is rejected.
ok(applyEvent({ event: {}, seq: "nope" }) === false, "non-finite seq rejected");

un();
console.log(`events: ${pass} passed, ${fail} failed`);
process.exit(fail === 0 ? 0 : 1);
