// commands.js — 017 T041 · mobile command list/search + signed execution.
//
// Renders the CommandCatalog (already filtered server-side by visibility +
// deny-list) with client-side search + pagination (don't render 228 at once,
// edge case). Executing a command sends a SIGNED `execute_command` frame; the
// UI reflects pending-approval for risky commands (the gate is enforced
// server-side — this is just the surface). ZERO hardcoded commands: the list is
// 100% the catalog from the bridge (FR-013). XSS-safe: text nodes only (T068).

import { APPROVAL_RISKS } from "./protocol.js";

const PAGE = 30; // render at most this many rows at once

let catalog = [];   // [{id,label,category,risk}]
let filter = "";
let shown = PAGE;
let execFn = null;  // (commandId) => Promise<void> | void

/** Replace the whole catalog (from a CommandCatalog frame). */
export function setCatalog(list) {
  catalog = Array.isArray(list) ? list : [];
}

export function catalogSize() {
  return catalog.length;
}

/** Pure filter+slice — exported for tests (no DOM). */
export function visibleCommands(all, q, limit) {
  const needle = (q || "").trim().toLowerCase();
  const matched = !needle
    ? all
    : all.filter(
        (c) =>
          (c.label || "").toLowerCase().includes(needle) ||
          (c.id || "").toLowerCase().includes(needle) ||
          (c.category || "").toLowerCase().includes(needle),
      );
  return { total: matched.length, rows: matched.slice(0, limit) };
}

/** True if executing this command will require approval (server enforces it). */
export function needsApproval(cmd) {
  return APPROVAL_RISKS.includes(cmd.risk);
}

/**
 * Render the command surface into `listEl`. `searchEl` is the <input> bound to
 * the filter. `onExec(commandId)` sends the signed execute frame.
 */
export function renderCommands(listEl, searchEl, onExec) {
  execFn = onExec || null;
  if (searchEl && !searchEl._bound) {
    searchEl._bound = true;
    searchEl.addEventListener("input", () => {
      filter = searchEl.value;
      shown = PAGE;
      paint(listEl);
    });
  }
  paint(listEl);
}

function paint(listEl) {
  listEl.replaceChildren();
  const { total, rows } = visibleCommands(catalog, filter, shown);
  if (!total) {
    const e = document.createElement("div");
    e.className = "hint";
    e.textContent = catalog.length ? "No commands match." : "No commands available.";
    listEl.appendChild(e);
    return;
  }
  for (const c of rows) {
    const row = document.createElement("button");
    row.type = "button";
    row.className = "cmdrow";
    const main = document.createElement("div");
    main.className = "cmdrow-main";
    const lbl = document.createElement("span");
    lbl.className = "cmdrow-lbl";
    lbl.textContent = c.label || c.id; // text node — XSS-safe
    const meta = document.createElement("span");
    meta.className = "cmdrow-meta";
    meta.textContent = c.category || "";
    main.append(lbl, meta);
    row.appendChild(main);
    if (needsApproval(c)) {
      const badge = document.createElement("span");
      badge.className = "cmdrow-badge";
      badge.textContent = c.risk; // "destructive" | "credential"
      row.appendChild(badge);
    }
    row.onclick = () => {
      if (execFn) execFn(c.id, c);
    };
    listEl.appendChild(row);
  }
  if (rows.length < total) {
    const more = document.createElement("button");
    more.type = "button";
    more.className = "cmdmore secondary";
    more.textContent = `Show more (${total - rows.length})`;
    more.onclick = () => {
      shown += PAGE;
      paint(listEl);
    };
    listEl.appendChild(more);
  }
}

// Reset (tests only).
export function __reset() {
  catalog = [];
  filter = "";
  shown = PAGE;
  execFn = null;
}
