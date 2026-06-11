// 059 — tests del hotkey configurable de push-to-talk. Runnable: `node --test src/lib/__tests__/pttHotkey.test.ts`.
import {
  DEFAULT_PTT_HOTKEY,
  parsePttHotkey,
  matchesPttHotkey,
  pttModifierKeyNames,
  eventToHotkeyString,
  formatPttHotkey,
} from "../pttHotkey.ts";

let pass = 0, fail = 0;
function eq(actual: unknown, expected: unknown, name: string) {
  const a = JSON.stringify(actual), e = JSON.stringify(expected);
  if (a === e) pass++;
  else { fail++; console.log(`FAIL ${name}: got ${a} want ${e}`); }
}

// parse — default ⌥Space
eq(parsePttHotkey("Alt+Space"), { altKey: true, ctrlKey: false, metaKey: false, shiftKey: false, code: "Space" }, "parse Alt+Space");
eq(parsePttHotkey(DEFAULT_PTT_HOTKEY).code, "Space", "default code Space");
eq(parsePttHotkey("Control+KeyT"), { altKey: false, ctrlKey: true, metaKey: false, shiftKey: false, code: "KeyT" }, "parse Control+KeyT");
eq(parsePttHotkey("Meta+Shift+KeyV"), { altKey: false, ctrlKey: false, metaKey: true, shiftKey: true, code: "KeyV" }, "parse Meta+Shift+KeyV");
// inválidos → default
eq(parsePttHotkey(""), parsePttHotkey("Alt+Space"), "empty → default");
eq(parsePttHotkey(null), parsePttHotkey("Alt+Space"), "null → default");
eq(parsePttHotkey("Alt"), parsePttHotkey("Alt+Space"), "sólo-modificador → default (necesita tecla base)");
eq(parsePttHotkey("Alt+Control+Meta+Shift"), parsePttHotkey("Alt+Space"), "todos-modificadores sin base → default");

// match — requiere mods configurados presentes + code; ignora extras (held-key robusto)
const altSpace = parsePttHotkey("Alt+Space");
eq(matchesPttHotkey({ altKey: true, ctrlKey: false, metaKey: false, shiftKey: false, code: "Space" }, altSpace), true, "match Alt+Space exacto");
eq(matchesPttHotkey({ altKey: true, ctrlKey: false, metaKey: false, shiftKey: true, code: "Space" }, altSpace), false, "NO-match con Shift extra (match exacto, anti-colisión)");
eq(matchesPttHotkey({ altKey: true, ctrlKey: false, metaKey: true, shiftKey: false, code: "Space" }, altSpace), false, "NO-match con Meta extra");
eq(matchesPttHotkey({ altKey: false, ctrlKey: false, metaKey: false, shiftKey: false, code: "Space" }, altSpace), false, "no-match sin Alt");
eq(matchesPttHotkey({ altKey: true, ctrlKey: false, metaKey: false, shiftKey: false, code: "KeyA" }, altSpace), false, "no-match code distinto");
// anti-colisión: un hotkey sin modificador (ej "KeyT") NO debe dispararse con ⌘T
const keyT = parsePttHotkey("KeyT");
eq(matchesPttHotkey({ altKey: false, ctrlKey: false, metaKey: false, shiftKey: false, code: "KeyT" }, keyT), true, "match KeyT pelado");
eq(matchesPttHotkey({ altKey: false, ctrlKey: false, metaKey: true, shiftKey: false, code: "KeyT" }, keyT), false, "NO-match ⌘T sobre hotkey KeyT (anti-colisión)");
const ctrlT = parsePttHotkey("Control+KeyT");
eq(matchesPttHotkey({ altKey: false, ctrlKey: true, metaKey: false, shiftKey: false, code: "KeyT" }, ctrlT), true, "match Control+KeyT");

// modifier key names (para el release)
eq(pttModifierKeyNames(altSpace), ["Alt"], "modKeyNames Alt+Space");
eq(pttModifierKeyNames(parsePttHotkey("Meta+Shift+KeyV")), ["Meta", "Shift"], "modKeyNames Meta+Shift");

// eventToHotkeyString (grabación)
eq(eventToHotkeyString({ altKey: true, ctrlKey: false, metaKey: false, shiftKey: false, code: "Space", key: " " }), "Alt+Space", "record Alt+Space");
eq(eventToHotkeyString({ altKey: false, ctrlKey: true, metaKey: false, shiftKey: false, code: "KeyT", key: "t" }), "Control+KeyT", "record Control+KeyT");
eq(eventToHotkeyString({ altKey: false, ctrlKey: false, metaKey: false, shiftKey: false, code: "AltLeft", key: "Alt" }), null, "record sólo-modificador → null (espera tecla base)");

// format (display)
eq(formatPttHotkey(parsePttHotkey("Alt+Space")), "⌥Space", "format ⌥Space");
eq(formatPttHotkey(parsePttHotkey("Control+KeyT")), "⌃T", "format ⌃T");
eq(formatPttHotkey(parsePttHotkey("Meta+Shift+KeyV")), "⌘⇧V", "format ⌘⇧V");
eq(formatPttHotkey(parsePttHotkey("Control+Digit1")), "⌃1", "format ⌃1");

console.log(`pttHotkey: ${pass} passed, ${fail} failed`);
process.exit(fail === 0 ? 0 : 1);
