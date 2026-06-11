// nav.js — 017 T040/T068 · data-driven bottom-nav for the Furx companion.
//
// Renders the bottom-nav + section switching from a NavSpec received over the
// bridge (SSOT: navGroups → buildNavSpec → bridge → here). ZERO hardcoded
// domain ids/labels — everything comes from the spec. XSS-safe: every label/icon
// is inserted as a text node (never innerHTML), so a forged/MITM'd label can't
// inject markup (T068). Tabs are ≥44px touch targets with iOS safe-area padding.

// Escape-by-construction: we only ever set `.textContent`, so there's nothing to
// escape — but expose a helper for any caller that builds attribute strings.
export function safeText(s) {
  return typeof s === "string" ? s : String(s ?? "");
}

// Active-tab state lives here (persists during the session, per FR-003).
let activeDomain = null;
let onSelectCb = null;

/**
 * Render the bottom-nav into `navEl` and the section bodies into `sectionsEl`.
 * @param {object} spec  MobileNavSpec { version, domains:[{domainId,label,items}] }
 * @param {(domainId:string)=>void} onSelect  called when a tab is tapped.
 * @returns {string[]} the domain ids rendered (for tests/debug).
 */
export function renderNav(navEl, sectionsEl, spec, onSelect) {
  onSelectCb = onSelect || null;
  navEl.replaceChildren();
  sectionsEl.replaceChildren();
  const domains = (spec && Array.isArray(spec.domains)) ? spec.domains : [];
  if (!domains.length) return [];

  // Preserve the active tab across re-renders if it still exists; else first.
  if (!domains.some((d) => d.domainId === activeDomain)) {
    activeDomain = domains[0].domainId;
  }

  const ids = [];
  for (const d of domains) {
    ids.push(d.domainId);
    // ── bottom-nav tab ──
    const tab = document.createElement("button");
    tab.className = "navtab" + (d.domainId === activeDomain ? " active" : "");
    tab.type = "button";
    tab.dataset.domain = d.domainId;
    tab.setAttribute("aria-label", safeText(d.label));
    const ico = document.createElement("span");
    ico.className = "navtab-ico";
    ico.textContent = firstIcon(d); // text node — XSS-safe
    const lbl = document.createElement("span");
    lbl.className = "navtab-lbl";
    lbl.textContent = safeText(d.label); // text node — XSS-safe
    tab.append(ico, lbl);
    tab.onclick = () => selectDomain(d.domainId);
    navEl.appendChild(tab);

    // ── section body (list of items) ──
    const sec = document.createElement("div");
    sec.className = "navsection" + (d.domainId === activeDomain ? "" : " hidden");
    sec.dataset.domain = d.domainId;
    const items = Array.isArray(d.items) ? d.items : [];
    for (const it of items) {
      const row = document.createElement("div");
      row.className = "navitem";
      const i2 = document.createElement("span");
      i2.className = "navitem-ico";
      i2.textContent = safeText(it.icon);
      const t2 = document.createElement("span");
      t2.className = "navitem-lbl";
      t2.textContent = safeText(it.label);
      row.append(i2, t2);
      sec.appendChild(row);
    }
    if (!items.length) {
      const empty = document.createElement("div");
      empty.className = "hint";
      empty.textContent = "No items.";
      sec.appendChild(empty);
    }
    sectionsEl.appendChild(sec);
  }
  return ids;
}

function firstIcon(d) {
  // Use the domain's first item icon as the tab glyph (the spec has no per-domain
  // icon; this keeps the nav purely data-driven without inventing metadata).
  if (Array.isArray(d.items) && d.items.length && typeof d.items[0].icon === "string") {
    return d.items[0].icon;
  }
  return "•";
}

export function selectDomain(domainId) {
  activeDomain = domainId;
  document.querySelectorAll(".navtab").forEach((t) => {
    t.classList.toggle("active", t.dataset.domain === domainId);
  });
  document.querySelectorAll(".navsection").forEach((s) => {
    s.classList.toggle("hidden", s.dataset.domain !== domainId);
  });
  if (onSelectCb) onSelectCb(domainId);
}

export function activeDomainId() {
  return activeDomain;
}

// Reset (tests only).
export function __reset() {
  activeDomain = null;
  onSelectCb = null;
}
