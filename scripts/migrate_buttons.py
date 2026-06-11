#!/usr/bin/env python3
"""022 US9 — migra `<button className="ghost|primary|danger|mini…">` ad-hoc a <Button variant size>.
Preserva TODOS los demás atributos (onClick/disabled/type/style/aria/title/key) y el contenido.
Empareja el </button> correcto con depth-tracking. NO toca Shell.tsx.
"""
import re
import sys
from pathlib import Path

LEGACY = {
    "primary": ("primary", None),
    "ghost": ("ghost", None),
    "danger": ("danger", None),
    "secondary": ("secondary", None),
    "success": ("success", None),
    "mini": ("secondary", "sm"),
    "mini primary": ("primary", "sm"),
    "mini danger": ("danger", "sm"),
}

OPEN_RE = re.compile(r'<button\s+className="(ghost|primary|danger|secondary|success|mini[^"]*)"')


def find_tag_end(s, start):
    """Devuelve el índice del '>' que cierra el tag de apertura desde start."""
    depth = 0
    in_str = None
    in_brace = 0
    i = start
    while i < len(s):
        c = s[i]
        if in_str:
            if c == in_str:
                in_str = None
        elif c in "\"'":
            in_str = c
        elif c == "{":
            in_brace += 1
        elif c == "}":
            in_brace -= 1
        elif c == ">" and in_brace == 0:
            return i
        i += 1
    return -1


def find_close(s, after):
    """Empareja el </button> correcto a partir de 'after' (depth-track de <button)."""
    depth = 1
    i = after
    while i < len(s):
        nxt_open = s.find("<button", i)
        nxt_close = s.find("</button>", i)
        if nxt_close == -1:
            return -1
        if nxt_open != -1 and nxt_open < nxt_close:
            depth += 1
            i = nxt_open + 7
        else:
            depth -= 1
            if depth == 0:
                return nxt_close
            i = nxt_close + 9
    return -1


def migrate(text):
    count = 0
    out = []
    i = 0
    while True:
        m = OPEN_RE.search(text, i)
        if not m:
            out.append(text[i:])
            break
        out.append(text[i:m.start()])
        cls = m.group(1).strip()
        variant, size = LEGACY.get(cls, ("secondary", None))
        tag_end = find_tag_end(text, m.start())
        if tag_end == -1:
            out.append(text[m.start():])
            break
        # atributos entre el className y el '>' de apertura.
        rest_attrs = text[m.end():tag_end]
        close = find_close(text, tag_end + 1)
        if close == -1:
            out.append(text[m.start():])
            break
        inner = text[tag_end + 1:close]
        new_open = f'<Button variant="{variant}"'
        if size:
            new_open += f' size="{size}"'
        new_open += rest_attrs + ">"
        out.append(new_open + inner + "</Button>")
        count += 1
        i = close + 9
    return "".join(out), count


def add_import(text, rel_to_components):
    if re.search(r'\bimport\s*\{[^}]*\bButton\b[^}]*\}\s*from\s*["\'][^"\']*Button["\']', text):
        return text
    imp = f'import {{ Button }} from "{rel_to_components}";'
    # insertar tras la última línea de import del bloque inicial.
    lines = text.split("\n")
    last_import = -1
    for idx, ln in enumerate(lines):
        if ln.startswith("import "):
            last_import = idx
        elif last_import != -1 and ln.strip() and not ln.startswith("import ") and not ln.startswith("//"):
            break
    if last_import == -1:
        return imp + "\n" + text
    lines.insert(last_import + 1, imp)
    return "\n".join(lines)


def rel_button_path(file_path: Path, src_root: Path) -> str:
    comp = src_root / "components" / "Button"
    rel = Path(__import__("os").path.relpath(comp, file_path.parent)).as_posix()
    if not rel.startswith("."):
        rel = "./" + rel
    return rel


def main():
    src_root = Path("web/src")
    total = 0
    touched = []
    for f in sorted(src_root.rglob("*.tsx")):
        if f.name in ("Shell.tsx", "Button.tsx"):
            continue
        text = f.read_text()
        if not OPEN_RE.search(text):
            continue
        new, n = migrate(text)
        if n == 0:
            continue
        new = add_import(new, rel_button_path(f, src_root))
        f.write_text(new)
        total += n
        touched.append(f"{f}: {n}")
    print(f"MIGRATED {total} buttons in {len(touched)} files")
    for t in touched:
        print("  " + t)


if __name__ == "__main__":
    main()
