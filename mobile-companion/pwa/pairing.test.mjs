// 065 — tests de la lógica pura de pairing.js (parseo del URI + validación de host). Run: `node pairing.test.mjs`.
import { parsePairingUri, isValidPairingHost } from "./pairing.js";

let pass = 0;
let fail = 0;
function ok(cond, name) {
  if (cond) pass++;
  else {
    fail++;
    console.error(`FAIL ${name}`);
  }
}
function eq(a, b, name) {
  ok(JSON.stringify(a) === JSON.stringify(b), `${name} (got ${JSON.stringify(a)} want ${JSON.stringify(b)})`);
}

const now = Math.floor(Date.now() / 1000);
const validUri = `furx://pair?v=1&t=${"a".repeat(64)}&h=192.168.1.5,10.0.0.2&p=43118&exp=${now + 100}&n=Mac%20de%20L%C3%A9a&ts=100.101.1.1`;

// parse — URI válido
const p = parsePairingUri(validUri);
ok(!p.error, "parse_valid_uri sin error");
eq(p.token, "a".repeat(64), "parse_valid_uri token");
eq(p.hosts, ["192.168.1.5", "10.0.0.2"], "parse_valid_uri hosts rfc1918");
eq(p.tsIp, "100.101.1.1", "parse_valid_uri tailscale");
eq(p.port, 43118, "parse_valid_uri port");
eq(p.name, "Mac de Léa", "parse_valid_uri name percent-decoded");

// parse — protocolo inválido
eq(parsePairingUri("http://pair?v=1&t=x").error, "uri_invalid", "parse_invalid_protocol");
// parse — versión desconocida
eq(parsePairingUri(`furx://pair?v=2&t=${"a".repeat(64)}`).error, "unsupported_version:2", "parse_unknown_version");
// parse — token corto
eq(parsePairingUri(`furx://pair?v=1&t=${"a".repeat(32)}`).error, "token_invalid", "parse_invalid_token_length");
// parse — expirado (más allá de la gracia de 45s)
eq(
  parsePairingUri(`furx://pair?v=1&t=${"a".repeat(64)}&exp=${now - 100}`).error,
  "token_expired",
  "parse_expired"
);
// parse — host público se descarta (no aparece en hosts)
eq(parsePairingUri(`furx://pair?v=1&t=${"a".repeat(64)}&h=8.8.8.8&exp=${now + 100}`).hosts, [], "parse_drops_public_host");

// isValidPairingHost
ok(isValidPairingHost("192.168.1.1"), "valid_host_192168");
ok(isValidPairingHost("10.0.0.5"), "valid_host_10");
ok(isValidPairingHost("172.16.3.4"), "valid_host_172");
ok(isValidPairingHost("100.100.1.1"), "valid_host_tailscale");
ok(!isValidPairingHost("127.0.0.1"), "invalid_host_loopback");
ok(!isValidPairingHost("8.8.8.8"), "invalid_host_public");
ok(!isValidPairingHost("100.200.1.1"), "invalid_host_100_outside_cgnat");
ok(!isValidPairingHost("notanip"), "invalid_host_garbage");

console.log(`pairing: ${pass} passed, ${fail} failed`);
process.exit(fail === 0 ? 0 : 1);
