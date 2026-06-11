// web/src/lib/whatsNew.ts — 016 US3 (T031 + T076) · lógica de What's New consciente de versión.
//
// Compara la versión instalada (Tauri `health.version`, ya disponible en Shell) contra
// `localStorage('furx.whatsnew.lastSeen')` y expone SÓLO las release notes posteriores. Reglas:
//   - parser semver REAL (no lexicográfico): "1.10.0" > "1.9.0". Normaliza pre-releases
//     ("1.2.0-beta.1" < "1.2.0"). Council T076.
//   - instalación FRESCA (sin lastSeen) → NO spamea el historial: marca la versión actual como vista
//     y no muestra nada. FR-013, Edge "fresh install".
//   - upgrade (lastSeen < actual) → muestra las entradas (lastSeen, actual]. FR-012.
//   - marcar visto → persiste lastSeen = versión actual (con guard localStorage). FR-013.

import { RELEASE_NOTES, type ReleaseNote } from "../data/releaseNotes.ts";

const STORAGE_KEY = "furx.whatsnew.lastSeen";

/* ── Parser semver real ──────────────────────────────────────────────────────────────────────── */

interface SemVer {
  major: number;
  minor: number;
  patch: number;
  /// identificadores de pre-release (vacío = release estable). Para precedencia SemVer §11.
  pre: (string | number)[];
}

/// Parsea "MAJOR.MINOR.PATCH[-pre][+build]". Tolerante a prefijo "v". `null` si no parsea.
export function parseSemver(input: string): SemVer | null {
  if (typeof input !== "string") return null;
  const s = input.trim().replace(/^v/i, "");
  // separar build metadata (ignorado en precedencia, SemVer §10).
  const [core] = s.split("+");
  const m = /^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?$/.exec(core);
  if (!m) return null;
  const pre = m[4]
    ? m[4].split(".").map((id) => (/^\d+$/.test(id) ? Number(id) : id))
    : [];
  return { major: Number(m[1]), minor: Number(m[2]), patch: Number(m[3]), pre };
}

/// Compara dos SemVer. <0 si a<b, 0 si igual, >0 si a>b. Implementa precedencia SemVer §11
/// (incl. pre-release: una versión con pre tiene MENOR precedencia que la misma sin pre).
export function compareSemver(a: SemVer, b: SemVer): number {
  if (a.major !== b.major) return a.major - b.major;
  if (a.minor !== b.minor) return a.minor - b.minor;
  if (a.patch !== b.patch) return a.patch - b.patch;
  // mismo core: comparar pre-release.
  if (a.pre.length === 0 && b.pre.length === 0) return 0;
  if (a.pre.length === 0) return 1;  // estable > pre
  if (b.pre.length === 0) return -1; // pre < estable
  const len = Math.min(a.pre.length, b.pre.length);
  for (let i = 0; i < len; i++) {
    const x = a.pre[i], y = b.pre[i];
    const xn = typeof x === "number", yn = typeof y === "number";
    if (xn && yn) { if (x !== y) return (x as number) - (y as number); continue; }
    if (xn !== yn) return xn ? -1 : 1; // numéricos < alfanuméricos (SemVer §11.4.3)
    if (x !== y) return (x as string) < (y as string) ? -1 : 1;
  }
  return a.pre.length - b.pre.length;
}

/// Comparación por strings de versión (convenience). Versiones no parseables se tratan como "0.0.0"
/// para que NUNCA rompan (fail-soft): una versión basura no muestra todo el historial.
export function compareVersionStrings(a: string, b: string): number {
  const pa = parseSemver(a) ?? { major: 0, minor: 0, patch: 0, pre: [] };
  const pb = parseSemver(b) ?? { major: 0, minor: 0, patch: 0, pre: [] };
  return compareSemver(pa, pb);
}

/* ── Estado de lastSeen (con guards de localStorage, T071) ───────────────────────────────────── */

export function getLastSeen(): string | null {
  try {
    if (typeof localStorage === "undefined") return null;
    return localStorage.getItem(STORAGE_KEY);
  } catch {
    return null;
  }
}

export function setLastSeen(version: string): void {
  try {
    if (typeof localStorage !== "undefined") localStorage.setItem(STORAGE_KEY, version);
  } catch {
    /* storage no disponible — no es fatal; en el peor caso reaparece al próximo boot */
  }
}

/* ── API de What's New ───────────────────────────────────────────────────────────────────────── */

export type InstallKind = "fresh" | "upgrade" | "current";

export interface WhatsNewState {
  /// "fresh" (sin lastSeen) | "upgrade" (lastSeen < actual) | "current" (al día / downgrade).
  kind: InstallKind;
  /// entradas a mostrar (vacío si fresh o current). Orden: más nueva primero.
  entries: ReleaseNote[];
}

/**
 * Resuelve el estado de What's New. PURO sobre (currentVersion, lastSeen, notes).
 *   - fresh: lastSeen == null → kind "fresh", SIN entradas (no spamear). El caller debe marcar visto.
 *   - upgrade: lastSeen < current → entradas en (lastSeen, current]. kind "upgrade".
 *   - current/downgrade: lastSeen >= current → kind "current", sin entradas.
 */
export function resolveWhatsNew(
  currentVersion: string,
  lastSeen: string | null = getLastSeen(),
  notes: ReleaseNote[] = RELEASE_NOTES,
): WhatsNewState {
  if (lastSeen == null) return { kind: "fresh", entries: [] };
  const cmp = compareVersionStrings(lastSeen, currentVersion);
  if (cmp >= 0) return { kind: "current", entries: [] };
  // upgrade: notas con (lastSeen < version <= current), más nueva primero.
  const entries = notes
    .filter(
      (n) =>
        compareVersionStrings(n.version, lastSeen) > 0 &&
        compareVersionStrings(n.version, currentVersion) <= 0,
    )
    .sort((a, b) => compareVersionStrings(b.version, a.version));
  return { kind: "upgrade", entries };
}
